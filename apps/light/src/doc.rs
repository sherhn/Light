//! An open file: the editor model plus UI side state.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use ide_core::editor::Editor;
use lisp_syntax::{Dialect, Severity};

use crate::diff_tab::DiffTab;
use crate::editor_view::{Marker, Sev, ViewState};
use crate::i18n::tr;

static NEXT_DOC_ID: AtomicU64 = AtomicU64::new(1);

/// A stable identity of a document: tab indices shift when tabs are closed, ids do not.
fn next_id() -> u64 {
    NEXT_DOC_ID.fetch_add(1, Ordering::Relaxed)
}

/// A virtual tab that is a page of the IDE, not a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Modules,
}

impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Page::Modules => "Modules",
        }
    }
}

pub struct Doc {
    pub id: u64,
    /// Set for the diff tab: a virtual document that shows changes instead of text.
    pub diff: Option<Box<DiffTab>>,
    /// Set for the tabs that are pages (they also carry an empty `diff`, which keeps the editor commands off them).
    pub page: Option<Page>,
    pub path: Option<PathBuf>,
    pub ed: Editor,
    pub view: ViewState,
    pub disk_mtime: Option<SystemTime>,
    /// The file changed on disk while there were unsaved edits.
    pub conflict: bool,
    /// Revision last seen by the autosave timer and when it was first seen (UI clock, seconds).
    pub autosave_rev: u64,
    pub autosave_at: f64,
    pub untitled_name: String,
    /// Compiler problems from the last compile (byte ranges valid for `compiled_rev`).
    pub compiler_markers: Vec<Marker>,
    pub compiled_rev: u64,
    /// What the check command of the file's language found in the saved text (valid while `check_rev` is the revision).
    pub check_markers: Vec<Marker>,
    pub check_rev: u64,
    /// What the language server found in the text (valid while `lsp_rev` is the revision).
    pub lsp_markers: Vec<Marker>,
    pub lsp_rev: u64,
    /// HEAD content of the file: None = not loaded, Some(None) = untracked / not in HEAD.
    pub git_base: Option<Option<String>>,
    pub git_marks: Vec<ide_core::diff::Mark>,
    pub git_marks_rev: u64,
    /// Folded forms, identified by (text of the form's first line, occurrence index).
    pub folds: std::collections::HashSet<(String, usize)>,
    pub outline_cache: Option<(u64, Vec<lisp_syntax::outline::Definition>)>,
    /// The rendered preview of a Markdown or reStructuredText document: its mode, scroll and caches.
    pub preview: crate::preview::PreviewState,
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Doc {
    pub fn open(path: &Path) -> io::Result<Doc> {
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file is not valid UTF-8 text"))?;
        let dialect = Dialect::from_path(path).unwrap_or_default();
        let mut ed = Editor::new(&text, dialect);
        // until the app says what language it is: Lisp is Lisp, anything else plain text
        if !lisp_syntax::dialect::is_lisp_file(path) {
            ed.set_language(Some(std::sync::Arc::new(light_lang::Language::plain())));
        }
        Ok(Doc {
            id: next_id(),
            diff: None,
            page: None,
            path: Some(path.to_path_buf()),
            ed,
            view: ViewState::default(),
            disk_mtime: mtime(path),
            conflict: false,
            autosave_rev: 0,
            autosave_at: 0.0,
            untitled_name: String::new(),
            compiler_markers: vec![],
            compiled_rev: 0,
            check_markers: vec![],
            check_rev: u64::MAX,
            lsp_markers: vec![],
            lsp_rev: u64::MAX,
            git_base: None,
            git_marks: vec![],
            git_marks_rev: u64::MAX,
            folds: Default::default(),
            outline_cache: None,
            preview: Default::default(),
        })
    }

    pub fn untitled(n: usize) -> Doc {
        let mut ed = Editor::new("", Dialect::CommonLisp);
        ed.set_language(Some(std::sync::Arc::new(light_lang::Language::plain())));
        Doc {
            id: next_id(),
            diff: None,
            page: None,
            path: None,
            ed,
            view: ViewState::default(),
            disk_mtime: None,
            conflict: false,
            autosave_rev: 0,
            autosave_at: 0.0,
            untitled_name: format!("untitled-{n}.txt"),
            compiler_markers: vec![],
            compiled_rev: 0,
            check_markers: vec![],
            check_rev: u64::MAX,
            lsp_markers: vec![],
            lsp_rev: u64::MAX,
            git_base: None,
            git_marks: vec![],
            git_marks_rev: u64::MAX,
            folds: Default::default(),
            outline_cache: None,
            preview: Default::default(),
        }
    }

    /// The Modules tab.
    pub fn modules_tab() -> Doc {
        let mut d = Doc::diff_tab();
        d.page = Some(Page::Modules);
        d
    }

    /// The diff tab: a document without text or file whose view shows a file's changes.
    pub fn diff_tab() -> Doc {
        let mut d = Doc::untitled(0);
        d.diff = Some(Box::new(DiffTab::empty()));
        d
    }

    /// Top-level definitions, cached per revision (parsing a big file every frame would freeze the UI).
    pub fn defs(&mut self) -> &[lisp_syntax::outline::Definition] {
        let rev = self.ed.revision();
        if self.outline_cache.as_ref().is_none_or(|(r, _)| *r != rev) {
            // the outline is made of Lisp definitions; other files have none
            let defs = match self.ed.language() {
                None => lisp_syntax::outline::outline_parsed(self.ed.text(), &self.ed.parsed()),
                Some(l) => l.outline(self.ed.text()),
            };
            self.outline_cache = Some((rev, defs));
        }
        &self.outline_cache.as_ref().expect("just filled").1
    }

    pub fn name(&self) -> String {
        if let Some(p) = self.page {
            return tr(p.title()).to_string();
        }
        if let Some(t) = &self.diff {
            return format!("{} ({})", t.title(), tr("diff"));
        }
        match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => self.untitled_name.clone(),
        }
    }

    pub fn save(&mut self) -> io::Result<()> {
        let Some(p) = self.path.clone() else {
            return Err(io::Error::other("file has no path"));
        };
        std::fs::write(&p, self.ed.text_for_disk())?;
        self.ed.mark_saved();
        self.disk_mtime = mtime(&p);
        self.conflict = false;
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path) -> io::Result<()> {
        std::fs::write(path, self.ed.text_for_disk())?;
        self.path = Some(path.to_path_buf());
        self.ed.set_dialect(Dialect::from_path(path).unwrap_or_default());
        self.ed.mark_saved();
        self.disk_mtime = mtime(path);
        self.conflict = false;
        Ok(())
    }

    /// Read the text in `lang` (`None`: Lisp).
    pub fn set_language(&mut self, lang: Option<std::sync::Arc<light_lang::Language>>) {
        self.ed.set_language(lang);
        self.outline_cache = None;
    }

    pub fn reload(&mut self) -> io::Result<()> {
        let Some(p) = self.path.clone() else { return Ok(()) };
        let text = String::from_utf8(std::fs::read(&p)?).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file is not valid UTF-8 text"))?;
        self.ed.set_text(&text);
        self.disk_mtime = mtime(&p);
        self.conflict = false;
        self.compiler_markers.clear();
        Ok(())
    }

    /// Check the file on disk; reloads silently when there are no local edits.
    /// Returns true if the content was reloaded.
    pub fn sync_with_disk(&mut self) -> bool {
        let Some(p) = self.path.clone() else { return false };
        let now = mtime(&p);
        if now == self.disk_mtime {
            return false;
        }
        if !self.ed.is_modified() {
            return self.reload().is_ok();
        }
        self.disk_mtime = now;
        self.conflict = true;
        false
    }

    /// What the checks the language asks for find in the text itself: brackets that do not pair up.
    pub fn language_markers(&self) -> Vec<Marker> {
        self.ed
            .language_problems()
            .iter()
            .map(|p| {
                let (kind, c) = match p.kind {
                    light_lang::ProblemKind::Unclosed(c) => ("Unclosed '{}'", c),
                    light_lang::ProblemKind::Unmatched(c) => ("Unmatched '{}'", c),
                };
                Marker { start: p.start, end: p.end, sev: Sev::Error, msg: crate::i18n::fmt(kind, &[&c]) }
            })
            .collect()
    }

    /// Syntax problems of the current text plus the compiler's (while still fresh).
    pub fn markers(&self) -> Vec<Marker> {
        let mut v: Vec<Marker> = if !self.ed.is_lisp() {
            // the Lisp reader has nothing to say about C or a makefile: the language's own checks do, and its check command
            let mut m = self.language_markers();
            if self.check_rev == self.ed.revision() {
                m.extend(self.check_markers.iter().cloned());
            }
            if self.lsp_rev == self.ed.revision() {
                m.extend(self.lsp_markers.iter().cloned());
            }
            m
        } else {
            self.ed
            .parsed()
            .diagnostics
            .iter()
            .map(|d| Marker { start: d.start, end: d.end, sev: if d.severity == Severity::Error { Sev::Error } else { Sev::Warning }, msg: d.message.clone() })
            .collect()
        };
        if self.compiled_rev == self.ed.revision() {
            v.extend(self.compiler_markers.iter().cloned());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("light-doc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn open_edit_save_roundtrip_keeps_crlf() {
        let p = tmp("crlf.lisp", "(a)\r\n(b)\r\n");
        let mut d = Doc::open(&p).unwrap();
        assert_eq!(d.ed.line_ending.label(), "CRLF");
        d.ed.set_caret(3, false);
        d.ed.type_text("x");
        assert!(d.ed.is_modified());
        d.save().unwrap();
        assert!(!d.ed.is_modified());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "(a)x\r\n(b)\r\n");
    }

    #[test]
    fn rejects_binary_files() {
        let p = tmp("bin.lisp", "");
        std::fs::write(&p, [0xff, 0xfe, 0x00, 0x80]).unwrap();
        assert!(Doc::open(&p).is_err());
    }

    #[test]
    fn disk_sync_reloads_clean_docs_and_flags_conflicts() {
        let p = tmp("sync.lisp", "(one)");
        let mut d = Doc::open(&p).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(&p, "(two)").unwrap();
        // make sure mtime differs even on coarse filesystems
        d.disk_mtime = Some(SystemTime::UNIX_EPOCH);
        assert!(d.sync_with_disk());
        assert_eq!(d.ed.text(), "(two)");
        d.ed.set_caret(0, false);
        d.ed.type_text("z");
        d.disk_mtime = Some(SystemTime::UNIX_EPOCH);
        assert!(!d.sync_with_disk());
        assert!(d.conflict, "unsaved edits + external change = conflict");
        assert_eq!(d.ed.text(), "z(two)");
    }

    #[test]
    fn markers_report_unbalanced_brackets() {
        let mut d = Doc::untitled(1);
        d.set_language(None);
        d.ed.insert_text_raw("(defun f (");
        let m = d.markers();
        assert!(m.iter().any(|m| m.sev == Sev::Error));
    }

    #[test]
    fn untitled_names_and_save_as() {
        let mut d = Doc::untitled(3);
        assert_eq!(d.name(), "untitled-3.txt");
        assert!(!d.ed.is_lisp());
        assert!(d.save().is_err());
        let p = std::env::temp_dir().join(format!("light-saveas-{}.scm", std::process::id()));
        d.ed.insert_text_raw("(define x 1)");
        d.save_as(&p).unwrap();
        assert_eq!(d.ed.dialect(), Dialect::Scheme);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "(define x 1)");
        let _ = std::fs::remove_file(p);
    }
}
