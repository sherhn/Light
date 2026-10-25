//! The diff tab: the changes of one file against the current diff base, shown as a tab of the editor area
//! (side by side or unified). One tab is reused for whichever file the user looks at, like a preview tab.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ide_core::diff::{self, DiffLine, SideBySide};
use ide_core::git::ChangeKind;

use crate::app::App;
use crate::doc::Doc;
use crate::editor_view::Sev;
use crate::i18n::tr;

/// How often the files behind an open diff tab are checked for changes.
const RECHECK_SECS: f64 = 0.4;

pub struct DiffTab {
    /// Path relative to the version-control root, `/` separated.
    pub rel: String,
    pub abs: PathBuf,
    pub kind: ChangeKind,
    pub base_label: String,
    pub old: String,
    pub new: String,
    /// The working file is not text: there is nothing to compare.
    pub binary: bool,
    pub sbs: SideBySide,
    pub lines: Vec<DiffLine>,
    pub unified: bool,
    /// Index into `sbs.hunks` of the difference the user is looking at.
    pub cur: Option<usize>,
    /// Display row to bring to the top region of the view on the next frame.
    pub scroll_to: Option<usize>,
    /// What `new` was read from: (revision of the open document, or 0) and the file's mtime.
    new_stamp: (u64, Option<SystemTime>),
    /// What `old` was read for: HEAD hash and base label.
    old_key: String,
    checked_at: f64,
}

impl DiffTab {
    pub fn empty() -> DiffTab {
        DiffTab {
            rel: String::new(),
            abs: PathBuf::new(),
            kind: ChangeKind::Modified,
            base_label: String::new(),
            old: String::new(),
            new: String::new(),
            binary: false,
            sbs: SideBySide::default(),
            lines: vec![],
            unified: true,
            cur: None,
            scroll_to: None,
            new_stamp: (0, None),
            old_key: String::new(),
            checked_at: -100.0,
        }
    }

    /// File name shown on the tab.
    pub fn title(&self) -> String {
        match self.rel.rsplit_once('/') {
            Some((_, n)) => n.to_string(),
            None if self.rel.is_empty() => tr("Diff").to_string(),
            None => self.rel.clone(),
        }
    }

    /// Number of differences (change blocks).
    pub fn hunk_count(&self) -> usize {
        self.sbs.hunks.len()
    }

    fn set_texts(&mut self, old: String, new: String) {
        self.sbs = diff::side_by_side(&old, &new);
        self.lines = diff::diff_lines(&old, &new);
        self.old = old;
        self.new = new;
        let n = self.sbs.hunks.len();
        self.cur = self.cur.filter(|c| *c < n);
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl App {
    /// Index of the diff tab, if one is open.
    pub fn diff_doc_index(&self) -> Option<usize> {
        self.docs.iter().position(|d| d.diff.is_some() && d.page.is_none())
    }

    /// The diff tab of the active document, if it is one.
    pub fn active_diff(&self) -> Option<&DiffTab> {
        self.docs.get(self.active).and_then(|d| d.diff.as_deref())
    }

    /// Path (relative to the version-control root) shown in the diff tab right now.
    pub fn diff_current_rel(&self) -> Option<&str> {
        self.diff_doc_index().and_then(|i| self.docs[i].diff.as_deref()).map(|t| t.rel.as_str()).filter(|r| !r.is_empty())
    }

    /// Show the changes of `rel` in the diff tab (created on first use) and bring it forward.
    pub fn open_diff_tab(&mut self, rel: &str) {
        if rel.ends_with('/') {
            self.toast(tr("This is an untracked folder - open the files inside to see their changes"), Sev::Weak);
            return;
        }
        let Some(root) = self.vcs_root() else { return };
        let abs = root.join(rel);
        let kind = self.git.changes.iter().find(|c| c.path == rel).map_or(ChangeKind::Modified, |c| c.kind);
        let idx = match self.diff_doc_index() {
            Some(i) => i,
            None => {
                self.docs.push(Doc::diff_tab());
                self.docs.len() - 1
            }
        };
        let label = self.base_label();
        let tab = self.docs[idx].diff.as_mut().expect("diff tab");
        if tab.rel != rel {
            tab.rel = rel.to_string();
            tab.abs = abs;
            tab.cur = None;
            tab.scroll_to = None;
            tab.new_stamp = (0, None);
            tab.old_key.clear();
        }
        tab.kind = kind;
        tab.base_label = label;
        self.refresh_diff(idx, true);
        if let Some(tab) = self.docs[idx].diff.as_mut() {
            // the first difference is what the user came for
            tab.cur = (!tab.sbs.hunks.is_empty()).then_some(0);
            tab.scroll_to = tab.sbs.hunks.first().map(|h| h.0);
        }
        self.show_doc(idx);
    }

    /// Bring the diff tab's texts up to date with the working file and the diff base.
    pub fn refresh_diff(&mut self, idx: usize, force: bool) {
        let Some(tab) = self.docs.get(idx).and_then(|d| d.diff.as_deref()) else { return };
        if tab.rel.is_empty() || (!force && self.now - tab.checked_at < RECHECK_SECS) {
            return;
        }
        let abs = tab.abs.clone();
        let base_label = self.base_label();
        let old_key = format!("{}|{base_label}", self.git.head.clone().unwrap_or_default());
        let (new_stamp, open_text) = match self.docs.iter().find(|d| d.diff.is_none() && d.path.as_deref() == Some(abs.as_path())) {
            Some(d) => ((d.ed.revision().max(1), None), Some(d.ed.text().replace("\r\n", "\n"))),
            None => ((0, mtime(&abs)), None),
        };
        let stale_new = force || tab.new_stamp != new_stamp;
        let stale_old = force || tab.old_key != old_key;
        if !stale_new && !stale_old {
            if let Some(t) = self.docs[idx].diff.as_mut() {
                t.checked_at = self.now;
            }
            return;
        }
        let (new, binary) = match open_text {
            Some(t) => (t, false),
            None => match std::fs::read(&abs) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(t) => (t.replace("\r\n", "\n"), false),
                    Err(_) => (String::new(), true),
                },
                Err(_) => (String::new(), false),
            },
        };
        let old = if stale_old { self.base_text_of(&abs).flatten().unwrap_or_default() } else { self.docs[idx].diff.as_ref().map(|t| t.old.clone()).unwrap_or_default() };
        let now = self.now;
        if let Some(t) = self.docs[idx].diff.as_mut() {
            t.base_label = base_label;
            t.binary = binary;
            t.new_stamp = new_stamp;
            t.old_key = old_key;
            t.checked_at = now;
            t.set_texts(old, new);
        }
    }

    /// Move to the next (`delta` > 0) or previous difference of the active diff tab.
    pub fn diff_step_change(&mut self, delta: i32) {
        let Some(tab) = self.docs.get_mut(self.active).and_then(|d| d.diff.as_mut()) else { return };
        let n = tab.sbs.hunks.len() as i32;
        if n == 0 {
            return;
        }
        let next = match tab.cur {
            None => if delta > 0 { 0 } else { n - 1 },
            Some(c) => (c as i32 + delta).rem_euclid(n),
        };
        tab.cur = Some(next as usize);
        tab.scroll_to = Some(tab.sbs.hunks[next as usize].0);
    }

    /// Show the next / previous changed file of the changes list in the diff tab.
    pub fn diff_step_file(&mut self, delta: i32) {
        let files: Vec<String> = self.git.changes.iter().filter(|c| !c.path.ends_with('/')).map(|c| c.path.clone()).collect();
        if files.is_empty() {
            return;
        }
        let here = self.diff_current_rel().and_then(|r| files.iter().position(|f| f == r));
        let next = match here {
            Some(i) => (i as i32 + delta).rem_euclid(files.len() as i32) as usize,
            None => 0,
        };
        self.open_diff_tab(&files[next]);
    }

    /// Open the file of the diff tab as a regular editor tab.
    pub fn diff_open_file(&mut self) {
        let Some(abs) = self.active_diff().map(|t| t.abs.clone()).filter(|p| p.is_file()) else { return };
        self.open_file(&abs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_the_file_name() {
        let mut t = DiffTab::empty();
        t.rel = "src/deep/nav.lisp".into();
        assert_eq!(t.title(), "nav.lisp");
        t.rel = "README.md".into();
        assert_eq!(t.title(), "README.md");
    }

    #[test]
    fn texts_produce_hunks_and_clamp_the_current_difference() {
        let mut t = DiffTab::empty();
        t.set_texts("a\nb\nc".into(), "a\nB\nc\nd".into());
        assert_eq!(t.hunk_count(), 2);
        t.cur = Some(1);
        t.set_texts("a\nb".into(), "a\nB".into());
        assert_eq!(t.hunk_count(), 1);
        assert_eq!(t.cur, None, "a difference that no longer exists is dropped");
    }
}
