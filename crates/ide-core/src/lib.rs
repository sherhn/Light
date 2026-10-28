//! UI independent core of the IDE: editor model, command registry, project model and search.

pub mod changes;
pub mod build;
pub mod bundle;
pub mod clone;
pub mod cmdhistory;
pub mod cmdscript;
pub mod commands;
pub mod defsearch;
pub mod diff;
pub mod docinfo;
pub mod editor;
pub mod exec;
pub mod git;
pub mod github;
pub mod graph;
pub mod index;
pub mod localvcs;
pub mod lsp;
pub mod paths;
pub mod modules;
pub mod search;
pub mod termcomplete;
pub mod todo;
pub mod settings;
pub mod shellenv;
pub mod workspace;
pub mod wsl;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A uniquely named temp directory, removed on drop.
    pub struct TempDir(PathBuf);

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    impl TempDir {
        pub fn new(tag: &str) -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!("light-test-{}-{}-{}", std::process::id(), tag, n));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
