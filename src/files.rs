//! Build a crate's module tree
//!
//! This allows a (rough) building of the crate's module tree, using
//! [`Package::from_root_file`].

use std::{collections::HashMap, fmt, fs, io, path::PathBuf};

/// Error encountered while trying to build the crate's tree
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// IO error (usually caused by non-existent or non-readable files).
    #[error("Error at {0}: {1}")]
    Io(PathBuf, io::Error),
    /// [`syn`] parsing error.
    #[error("{0}")]
    Syn(#[from] syn::Error),
}

/// Handle for a [`Module`].
pub type ModuleId = u32;

/// Representation of a Rust module.
pub struct Module {
    /// File in which this module resides.
    pub file: PathBuf,
    /// Path of this module within [`file`](Self::file)'s hierarchy.
    ///
    /// # Examples
    /// - in `lib.rs`
    /// ```rust
    /// mod a {
    ///     mod b {}
    /// }
    /// ```
    /// Module `b` has internal_path `["crate", "a", "b"]`.
    /// - in `c.rs`, module `c` has internal_path `["c"]`.
    pub internal_path: Vec<String>,
    /// Visibility of this module.
    ///
    /// # Note
    /// This is the syntactic visibility modifier; in other words, in
    /// ```rust
    /// mod a {
    ///     pub mod b {}
    /// }
    /// ```
    /// `b` has visibility `pub`, even though it is not public.
    pub visibility: syn::Visibility,
    /// Submodules that appear in this module's items, either `mod a;` or
    /// `mod a { ... }`.
    ///
    /// # Note
    /// This does not contains modules nested inside other items
    /// ```rust
    /// fn f() {
    ///     mod a {}
    /// }
    /// ```
    /// Here the module `a` will be completely missed.
    pub submodules: Vec<ModuleId>,
    /// Parent module of this module.
    ///
    /// If this is the root module, it is its own parent.
    pub parent: ModuleId,
    /// Items of the module (aka functions, constants, impl blocks...)
    pub items: Vec<syn::Item>,
    /// Attributes of this module if it is a file module.
    pub attributes: Option<Vec<syn::Attribute>>,
}

/// Representation of a Rust crate's module tree.
#[derive(Debug)]
pub struct Package {
    /// Which module is the root module.
    pub root_module: ModuleId,
    /// Map from file to their main module.
    pub files_to_ids: HashMap<PathBuf, ModuleId>,
    /// Modules of this crate.
    pub modules: HashMap<ModuleId, Module>,
}

impl Package {
    /// Try to build the crate tree with the file at the given `path` as
    /// root module.
    pub fn from_root_file(path: PathBuf) -> Result<Self, Error> {
        let mut builder = PackageBuilder::default();
        let file = match fs::read_to_string(&path) {
            Ok(content) => syn::parse_file(&content)?,
            Err(io_error) => return Err(Error::Io(path, io_error)),
        };
        let internal_path = vec!["crate".to_string()];
        let root_id = builder.add_module(
            builder.next_module_id,
            path,
            internal_path,
            file.items,
            Some(file.attrs),
            syn::Visibility::Public(syn::VisPublic {
                pub_token: syn::token::Pub::default(),
            }),
        )?;

        Ok(Self {
            root_module: root_id,
            files_to_ids: builder.files_to_ids,
            modules: builder.modules,
        })
    }
}

impl fmt::Debug for Module {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Underscore;
        impl fmt::Debug for Underscore {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "_")
            }
        }
        formatter
            .debug_struct("Module")
            .field("path", &self.file)
            .field("internal_path", &self.internal_path)
            .field("visibility", &Underscore)
            .field("submodules", &self.submodules)
            .field("parent", &self.parent)
            .field("items", &Underscore)
            .field("attributes", &Underscore)
            .finish()
    }
}

/// Builder for [`Package`]
#[derive(Default)]
struct PackageBuilder {
    /// Next unallocated id
    next_module_id: ModuleId,
    files_to_ids: HashMap<PathBuf, ModuleId>,
    modules: HashMap<ModuleId, Module>,
}

impl PackageBuilder {
    fn next_id(&mut self) -> ModuleId {
        let id = self.next_module_id;
        self.next_module_id += 1;
        id
    }

    fn add_module(
        &mut self,
        parent: ModuleId,
        path: PathBuf,
        internal_path: Vec<String>,
        items: Vec<syn::Item>,
        attributes: Option<Vec<syn::Attribute>>,
        visibility: syn::Visibility,
    ) -> Result<ModuleId, Error> {
        let id = self.next_id();
        if internal_path.len() == 1 {
            self.files_to_ids.insert(path.clone(), id);
        }

        let submodules = self.explore_submodules(&items, id, &path, &internal_path)?;

        let module = Module {
            file: path,
            internal_path,
            submodules,
            parent: parent,
            items: items,
            attributes,
            visibility,
        };
        self.modules.insert(id, module);

        Ok(id)
    }

    fn explore_submodules(
        &mut self,
        items: &[syn::Item],
        parent: ModuleId,
        path: &PathBuf,
        internal_path: &[String],
    ) -> Result<Vec<ModuleId>, Error> {
        let mut submodules = Vec::new();
        for item in items {
            match item {
                syn::Item::Mod(syn::ItemMod {
                    vis,
                    ident,
                    content,
                    ..
                }) => {
                    let mut internal_path = internal_path.to_owned();
                    internal_path.push(ident.to_string());
                    let visibility = vis.clone();
                    let mut path = path.clone();
                    submodules.push(match content {
                        Some((_, items)) => self.add_module(
                            parent,
                            path,
                            internal_path,
                            items.clone(),
                            None,
                            visibility,
                        ),
                        // lib.rs
                        // a.rs
                        // a
                        //   c.rs
                        // b
                        //   mod.rs
                        //   d.rs
                        None => {
                            if let Some(last) = path.file_name() {
                                let last = last.to_str();
                                if last == Some("mod.rs") || last == Some("lib.rs") {
                                    path.pop();
                                } else {
                                    path.set_extension("");
                                }
                            } else {
                                continue;
                            }
                            for module in internal_path.iter().skip(1) {
                                path.push(module);
                            }
                            let path_mod_rs = path.join("mod.rs");
                            if path_mod_rs.exists() {
                                path = path_mod_rs;
                            } else {
                                path.set_extension("rs");
                            }
                            let file = match fs::read_to_string(&path) {
                                Ok(content) => syn::parse_file(&content)?,
                                Err(io_error) => return Err(Error::Io(path, io_error)),
                            };
                            self.add_module(
                                parent,
                                path,
                                vec![ident.to_string()], // TODO: NOPE
                                file.items,
                                Some(file.attrs),
                                visibility,
                            )
                        }
                    }?)
                }
                _ => {}
            }
        }
        Ok(submodules)
    }
}
