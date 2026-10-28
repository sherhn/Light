//! Modules: the IDE is a core (editor, projects, git, terminal, Linux through WSL) and everything language-specific
//! comes as a module that can be downloaded from inside the IDE. See [`manifest`] for what a module is, [`registry`]
//! for where they come from and [`store`] for what is installed.

pub mod check;
pub mod detect;
pub mod manifest;
pub mod registry;
pub mod sha256;
pub mod store;

pub use manifest::{CheckDef, Detect, LspDef, Manifest, RunDef, Template, ToolDef, is_newer};
pub use registry::{FileEntry, Index, IndexEntry, Source, default_sources, fetch_index};
pub use store::{Installed, ModuleStore, State};
