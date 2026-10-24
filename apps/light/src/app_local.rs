//! Behaviour behind the local commit browser (view a commit's files, switch between commits, restore) and the
//! "old code" popup that opens when a change marker in the gutter is clicked.

use eframe::egui::Pos2;
use ide_core::diff;
use ide_core::localvcs::Base;

use crate::app::App;
use crate::editor_view::Sev;
use crate::i18n::{self, tr};
use crate::state::*;

fn lf(s: String) -> String {
    s.replace("\r\n", "\n")
}

impl App {
    // ------------------------------------------------------------------ local commit browser

    /// Open the browser on local commit `id`.
    pub fn open_local_view(&mut self, id: u64) {
        let repo = self.git.repo.clone();
        let Some(local) = self.git.local.as_ref() else { return };
        if local.commit(id).is_none() {
            return;
        }
        let files = local.commit_changes(id, repo.as_ref());
        self.popup = Popup::LocalView(Box::new(LocalViewState { id, files, sel: 0, mode: LocalViewMode::Changes, loaded: None, diff: vec![], code: vec![], scroll_to: None }));
    }

    /// Switch the open browser to another local commit.
    pub fn local_view_goto(&self, st: &mut LocalViewState, id: u64) {
        let repo = self.git.repo.as_ref();
        let Some(local) = self.git.local.as_ref() else { return };
        if local.commit(id).is_none() {
            return;
        }
        // keep the same file selected when the new commit also touches it
        let keep = st.files.get(st.sel).map(|c| c.path.clone());
        st.files = local.commit_changes(id, repo);
        st.id = id;
        st.sel = keep.and_then(|k| st.files.iter().position(|c| c.path == k)).unwrap_or(0);
        st.loaded = None;
    }

    /// Make sure `diff` / `code` of the browser hold the selected file of the selected commit.
    pub fn local_view_load(&self, st: &mut LocalViewState) {
        if st.loaded == Some((st.id, st.sel)) {
            return;
        }
        st.loaded = Some((st.id, st.sel));
        st.diff.clear();
        st.code.clear();
        st.scroll_to = None;
        let (Some(local), Some(ch)) = (self.git.local.as_ref(), st.files.get(st.sel)) else { return };
        let repo = self.git.repo.as_ref();
        let before = lf(local.text_before(st.id, &ch.path, repo).unwrap_or_default());
        let after = lf(local.text_in(st.id, &ch.path, repo).unwrap_or_default());
        st.diff = diff::diff_lines(&before, &after);
        st.scroll_to = st.diff.iter().position(|l| l.kind != diff::LineKind::Same);
        st.code = after.lines().map(str::to_string).collect();
    }

    /// Ask before putting one file back into its state in local commit `id`.
    pub fn local_restore_file_confirm(&mut self, id: u64, rel: String) {
        let message = i18n::fmt("Restore '{0}' to its state in local commit #{1}? Your current version of the file is overwritten.", &[&rel, &id]);
        self.popup = Popup::Confirm { kind: ConfirmKind::RestoreLocalFile(id, rel), message };
    }

    pub fn local_restore_file(&mut self, id: u64, rel: &str) {
        self.save_all();
        let repo = self.git.repo.clone();
        let Some(local) = self.git.local.clone() else { return };
        match local.restore_file(id, rel, repo.as_ref()) {
            Ok(()) => {
                for d in self.docs.iter_mut() {
                    d.sync_with_disk();
                }
                self.after_base_change();
                self.refresh_project();
                self.toast(i18n::fmt("Restored {0} from local commit #{1}", &[&rel, &id]), Sev::Weak);
            }
            Err(e) => self.toast(i18n::fmt("Restore failed: {}", &[&e]), Sev::Error),
        }
    }

    /// The commit's diff against the working tree becomes the editor's base (markers, changes list).
    pub fn local_view_use_as_base(&mut self, id: u64) {
        self.local_set_base(Base::Local(id));
        self.left_tool = Some(LeftTool::Commit);
    }

    // ------------------------------------------------------------------ old code popup

    /// A change marker on `line` was clicked: show the old code of its block (a second click closes it).
    pub fn open_hunk_popup(&mut self, line: usize, anchor: Pos2) {
        if self.hunk_popup.as_ref().is_some_and(|h| h.line == line) {
            self.hunk_popup = None;
            return;
        }
        let base_label = self.base_label();
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(path) = d.path.clone() else { return };
        let (old, is_new_file) = match &d.git_base {
            Some(Some(b)) => (b.clone(), false),
            Some(None) => (String::new(), true),
            None => return,
        };
        let new = d.ed.text();
        let hunks = diff::hunks(&old, new);
        let Some(hunk) = diff::hunk_at(&hunks, line, new.lines().count()).cloned() else { return };
        self.hunk_popup = Some(HunkPopup { line, rev: d.ed.revision(), scroll_y: d.view.scroll.y, hunk, anchor, doc_path: path, base_label, is_new_file });
    }

    /// Path of the active document relative to the version-control root (git root, or the project for local-only).
    pub fn current_vcs_rel_path(&self) -> Option<String> {
        let root = self.vcs_root()?;
        let p = self.doc()?.path.as_ref()?;
        p.strip_prefix(&root).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
    }

    /// Replace the changed block in the editor with the old code.
    pub fn hunk_revert(&mut self) {
        let Some(h) = self.hunk_popup.take() else { return };
        let now = self.now;
        let Some(d) = self.docs.get_mut(self.active) else { return };
        if d.path.as_ref() != Some(&h.doc_path) || d.ed.revision() != h.rev {
            return;
        }
        let text = d.ed.text().to_string();
        let k = &h.hunk;
        let line_count = d.ed.line_count();
        let joined = k.old_lines.join("\n");
        let (start, end, repl) = if k.new_len > 0 {
            let start = d.ed.line_start(k.new_start);
            let end_line = k.new_start + k.new_len;
            let end = if end_line < line_count { d.ed.line_start(end_line) } else { text.len() };
            let keeps_newline = text[start..end].ends_with('\n');
            let repl = if k.old_lines.is_empty() || !keeps_newline { joined } else { format!("{joined}\n") };
            (start, end, repl)
        } else if k.new_start < line_count {
            let at = d.ed.line_start(k.new_start);
            (at, at, format!("{joined}\n"))
        } else {
            // the deleted lines were at the very end of the file
            let repl = if text.is_empty() || text.ends_with('\n') { format!("{joined}\n") } else { format!("\n{joined}") };
            (text.len(), text.len(), repl)
        };
        d.ed.replace_range(start, end, &repl);
        d.view.last_input_time = now;
        d.git_marks_rev = u64::MAX;
        self.toast(tr("Change reverted"), Sev::Weak);
    }

    /// The old code of the open popup as one string (for copying).
    pub fn hunk_old_text(&self) -> String {
        self.hunk_popup.as_ref().map(|h| h.hunk.old_lines.join("\n")).unwrap_or_default()
    }
}
