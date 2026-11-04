//! Notes are ordinary Markdown documents, saved by the editor alongside the project.
use std::path::{Path, PathBuf};
use crate::{app::App, editor_view::Sev};

pub fn note_path(root: &Path, file: Option<&Path>) -> PathBuf {
    let directory = root.join(".light-notes");
    match file {
        None => directory.join("project.md"),
        Some(file) => {
            if let Ok(relative) = file.strip_prefix(root)
                && relative.components().all(|c| matches!(c, std::path::Component::Normal(_)))
                && let Some(name) = relative.file_name()
            {
                let mut name = name.to_os_string(); name.push(".md");
                return directory.join("files").join(relative).with_file_name(name);
            }
            let relative = file.strip_prefix(root).unwrap_or(file).to_string_lossy().replace('\\', "/");
            directory.join("files").join(format!("{}.md", ide_core::modules::sha256::sha256_hex(relative.as_bytes())))
        }
    }
}

impl App {
    pub fn open_note(&mut self, file: Option<PathBuf>) {
        let root = self.project.as_ref().map(|p| p.root.clone()).or_else(|| file.as_ref()?.parent().map(Path::to_path_buf));
        let Some(root) = root else { self.toast(crate::i18n::tr("Open a project or file first"), Sev::Weak); return };
        let path = note_path(&root, file.as_deref());
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut output) => {
                    use std::io::Write;
                    let title = file.as_ref().map_or_else(|| root.file_name().unwrap_or(root.as_os_str()).to_string_lossy().into_owned(), |p| p.strip_prefix(&root).unwrap_or(p).display().to_string());
                    write!(output, "# {title}\n\n")?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
            Ok(())
        })();
        match result {
            Ok(()) => { self.open_file(&path); }
            Err(e) => self.toast(format!("{}: {e}", crate::i18n::tr("Cannot open notes")), Sev::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notes_are_portable_and_distinguish_files_with_the_same_name() {
        let a = Path::new("one"); let b = Path::new("two");
        assert_eq!(note_path(a, Some(&a.join("src/a.rs"))).strip_prefix(a).unwrap(), note_path(b, Some(&b.join("src/a.rs"))).strip_prefix(b).unwrap());
        assert_ne!(note_path(a, Some(&a.join("src/a.rs"))), note_path(a, Some(&a.join("test/a.rs"))));
        assert!(note_path(a, Some(Path::new("../escape"))).starts_with(a.join(".light-notes")));
    }
}
