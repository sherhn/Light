//! Git behaviour of the application: background status refresh, gutter bases, operations.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use ide_core::diff;
use ide_core::git::{self, ChangeKind};
use ide_core::localvcs::{Base, LocalVcs};

const MAX_CHANGES: usize = 5000;

use crate::app::App;
use crate::editor_view::Sev;
use crate::i18n::{self, tr};
use crate::state::*;

impl App {
    /// (Re)discover the repository and the local commit store of the current project.
    pub fn git_attach(&mut self) {
        self.git = GitState::default();
        self.git.available = git::git_available();
        if let Some(root) = self.project_root().map(Path::to_path_buf) {
            self.git.local = Some(LocalVcs::open(&root));
            if self.git.available {
                self.git.repo = git::find_repo(&root).filter(|r| r.is_valid());
            }
        }
        self.git_refresh();
    }

    /// Is a local commit (checkpoint) the base of diffs right now?
    pub fn base_is_local(&self) -> bool {
        matches!(self.git.local.as_ref().map(|l| l.base()), Some(Base::Local(_)))
    }

    /// Human readable name of the current diff base.
    pub fn base_label(&self) -> String {
        match self.git.local.as_ref().map(|l| l.base().clone()) {
            Some(Base::Local(id)) => {
                let msg = self.git.local.as_ref().and_then(|l| l.commit(id)).map(|c| c.message.clone()).unwrap_or_default();
                i18n::fmt("local #{0}: {1}", &[&id, &msg])
            }
            _ => "HEAD".to_string(),
        }
    }

    /// Root used for relative paths: the git repository root, else the project root.
    pub fn vcs_root(&self) -> Option<PathBuf> {
        self.git.repo.as_ref().map(|r| r.root.clone()).or_else(|| self.project_root().map(Path::to_path_buf))
    }

    pub fn git_refresh(&mut self) {
        if self.git.rx.is_some() {
            return;
        }
        // do not compete with the initial project indexing for the disk; the periodic refresh retries
        if self.project.as_ref().is_some_and(|p| p.index_rx.is_some()) {
            return;
        }
        let repo = self.git.repo.clone();
        let local = self.git.local.clone();
        if repo.is_none() && !self.base_is_local() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.git.rx = Some(rx);
        self.git.last_refresh = self.now;
        std::thread::spawn(move || {
            let base_id = match local.as_ref().map(|l| l.base().clone()) {
                Some(Base::Local(id)) => Some(id),
                _ => None,
            };
            let mut changes = match (base_id, &local) {
                (Some(id), Some(l)) => l.changes_vs(id, repo.as_ref()),
                _ => repo.as_ref().map(|r| r.status()).unwrap_or_default(),
            };
            if let Some(r) = &repo {
                changes = ide_core::changes::without_ignored(changes, &r.root);
            }
            changes.sort_by(|a, b| a.path.cmp(&b.path));
            let truncated = changes.len() > MAX_CHANGES;
            changes.truncate(MAX_CHANGES);
            let snap = GitSnapshot {
                branch: repo.as_ref().and_then(|r| r.branch()),
                head: repo.as_ref().and_then(|r| r.head_hash()),
                changes,
                branches: repo.as_ref().map(|r| r.branches()).unwrap_or_default(),
                log: repo.as_ref().map(|r| r.log_all(200)).unwrap_or_default(),
                truncated,
            };
            let _ = tx.send(snap);
        });
    }

    pub fn git_poll(&mut self) {
        if let Some(rx) = &self.git.rx
            && let Ok(snap) = rx.try_recv()
        {
            self.git.rx = None;
            let head_changed = snap.head != self.git.head;
            self.git.branch = snap.branch;
            self.git.head = snap.head;
            self.git.branches = snap.branches;
            self.git.graph = ide_core::graph::layout(&snap.log);
            self.git.log = snap.log;
            self.git.changes_truncated = snap.truncated;
            // keep the user's check marks for files that are still changed; check new tracked ones by default
            // (untracked files stay unchecked, as in PyCharm: adding a file to git is a decision)
            let old: HashSet<String> = self.git.changes.iter().map(|c| c.path.clone()).collect();
            let now: HashSet<String> = snap.changes.iter().map(|c| c.path.clone()).collect();
            self.git.checked.retain(|p| now.contains(p));
            for c in &snap.changes {
                if !old.contains(&c.path) && c.kind != ChangeKind::Untracked {
                    self.git.checked.insert(c.path.clone());
                }
            }
            self.git.changes = snap.changes;
            self.git_rebuild_kinds();
            if head_changed {
                for d in self.docs.iter_mut() {
                    d.git_base = None;
                }
            }
            // the file system may have changed (git operations, external tools): redraw the tree
            if let Some(p) = self.project.as_mut() {
                p.tree_ver += 1;
            }
        }
        if let Some(rx) = &self.git.op_rx
            && let Ok((what, res)) = rx.try_recv()
        {
            self.git.op_rx = None;
            self.git.busy = None;
            match res {
                Ok(msg) => {
                    let text = if msg.is_empty() { i18n::fmt("{}: done", &[&i18n::tr_dyn(&what)]) } else { format!("{}: {}", i18n::tr_dyn(&what), msg.lines().last().unwrap_or("")) };
                    self.toast(text, Sev::Weak);
                }
                Err(e) => self.toast(i18n::fmt("{0} failed: {1}", &[&i18n::tr_dyn(&what), &e.lines().next().unwrap_or("")]), Sev::Error),
            }
            self.git_refresh();
            self.refresh_project();
            for d in self.docs.iter_mut() {
                d.sync_with_disk();
            }
        }
        // periodic refresh while idle
        if (self.git.repo.is_some() || self.base_is_local()) && self.git.rx.is_none() && self.now - self.git.last_refresh > 4.0 {
            self.git_refresh();
        }
    }

    fn git_rebuild_kinds(&mut self) {
        let Some(root) = self.vcs_root() else { return };
        let mut kinds: HashMap<PathBuf, ChangeKind> = HashMap::new();
        let mut dirs: HashSet<PathBuf> = HashSet::new();
        for c in &self.git.changes {
            let abs = root.join(c.path.trim_end_matches('/'));
            let mut p = abs.parent();
            while let Some(d) = p {
                if !dirs.insert(d.to_path_buf()) {
                    break;
                }
                if d == root {
                    break;
                }
                p = d.parent();
            }
            kinds.insert(abs, c.kind);
        }
        self.git.kinds = kinds;
        self.git.dirty_dirs = dirs;
    }

    /// Text of `abs` in the current diff base. `None`: no base (markers off); `Some(None)`: the file is
    /// absent from the base (every line is new); `Some(Some(text))`: base content, LF normalised.
    pub fn base_text_of(&self, abs: &Path) -> Option<Option<String>> {
        let root = self.vcs_root()?;
        let rel = abs.strip_prefix(&root).ok()?.to_string_lossy().replace('\\', "/");
        let repo = self.git.repo.as_ref();
        match self.git.local.as_ref() {
            Some(l) if matches!(l.base(), Base::Local(_)) => Some(l.base_text(&rel, repo).map(|t| t.replace("\r\n", "\n"))),
            _ => repo.map(|r| r.head_content(&rel).map(|t| t.replace("\r\n", "\n"))),
        }
    }

    /// Make sure every open document knows its base content (for gutter markers).
    pub fn git_update_bases(&mut self) {
        let todo: Vec<(usize, PathBuf)> = self.docs.iter().enumerate().filter(|(_, d)| d.git_base.is_none()).filter_map(|(i, d)| d.path.clone().map(|p| (i, p))).collect();
        for (i, p) in todo {
            if let Some(base) = self.base_text_of(&p) {
                let d = &mut self.docs[i];
                d.git_base = Some(base);
                d.git_marks_rev = u64::MAX;
            }
        }
    }

    pub fn git_change_of(&self, path: &Path) -> Option<ChangeKind> {
        if let Some(k) = self.git.kinds.get(path) {
            return Some(*k);
        }
        // files inside a wholly untracked directory (reported by git as one `dir/` entry)
        if self.git.kinds.is_empty() {
            return None;
        }
        path.ancestors().skip(1).take(40).find_map(|a| self.git.kinds.get(a).filter(|k| **k == ChangeKind::Untracked).copied())
    }

    pub fn git_run_sync(&mut self, what: &str, f: impl FnOnce(&git::Repo) -> Result<String, String>) -> bool {
        let Some(repo) = self.git.repo.clone() else {
            self.toast(tr("This project is not a Git repository"), Sev::Weak);
            return false;
        };
        match f(&repo) {
            Ok(msg) => {
                if !msg.is_empty() {
                    self.toast(format!("{}: {msg}", i18n::tr_dyn(what)), Sev::Weak);
                }
                self.git.rx = None;
                self.git_refresh();
                self.refresh_project();
                true
            }
            Err(e) => {
                self.toast(i18n::fmt("{0} failed: {1}", &[&i18n::tr_dyn(what), &e.lines().next().unwrap_or("")]), Sev::Error);
                false
            }
        }
    }

    pub fn git_run_async(&mut self, what: &str, f: impl FnOnce(git::Repo) -> Result<String, String> + Send + 'static) {
        let Some(repo) = self.git.repo.clone() else {
            self.toast(tr("This project is not a Git repository"), Sev::Weak);
            return;
        };
        if self.git.op_rx.is_some() {
            self.toast(tr("Another Git operation is still running"), Sev::Weak);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let name = what.to_string();
        self.git.busy = Some(what.to_string());
        self.git.op_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send((name, f(repo)));
        });
    }

    pub fn git_checkout(&mut self, branch: &str) {
        // unsaved edits would be lost on a file-changing checkout; save first
        self.save_all();
        let b = branch.trim_start_matches("origin/").to_string();
        let target = if self.git.branches.iter().any(|x| x.name == branch && !x.remote) { branch.to_string() } else { b };
        if self.git_run_sync("Checkout", move |r| r.checkout(&target).map(|_| String::new())) {
            for d in self.docs.iter_mut() {
                d.sync_with_disk();
                d.git_base = None;
            }
            self.toast(i18n::fmt("Switched to {}", &[&branch]), Sev::Weak);
        }
    }

    pub fn git_commit_selected(&mut self) -> bool {
        self.save_all();
        let Some(repo) = self.git.repo.clone() else {
            self.toast(tr("This project is not a Git repository - use a local commit instead"), Sev::Weak);
            return false;
        };
        let msg = self.git.commit_msg.clone();
        // with a local commit as the diff base the list may contain files git does not consider changed
        let real: HashSet<String> = repo.status().into_iter().map(|c| c.path).collect();
        let paths: Vec<String> = self.git.changes.iter().filter(|c| self.git.checked.contains(&c.path) && real.contains(&c.path)).map(|c| c.path.clone()).collect();
        let ok = self.git_run_sync("Commit", move |r| r.commit(&msg, &paths).map(|h| format!("committed {h}")));
        if ok {
            self.git.commit_msg.clear();
            for d in self.docs.iter_mut() {
                d.git_base = None;
            }
        }
        ok
    }

    /// Show the changes of `rel` (relative to the repository root) in the diff tab.
    pub fn git_show_diff(&mut self, rel: &str) {
        self.open_diff_tab(rel);
    }

    pub fn git_show_commit(&mut self, idx: usize) {
        let Some(repo) = self.git.repo.clone() else { return };
        let Some(c) = self.git.log.get(idx) else { return };
        let title = format!("{}  {}", c.short, c.subject);
        let text = repo.show(&c.hash).unwrap_or_else(|e| e);
        self.popup = Popup::Info { title, text };
    }

    /// Diff lines of an open document against HEAD (for the gutter).
    pub fn git_marks(doc: &mut crate::doc::Doc) -> &[diff::Mark] {
        if doc.git_marks_rev != doc.ed.revision() {
            doc.git_marks = match &doc.git_base {
                Some(Some(base)) => diff::gutter_marks(base, doc.ed.text()),
                Some(None) => {
                    // untracked file: every line is new
                    let n = doc.ed.line_count();
                    (0..n).map(|l| diff::Mark { line: l, kind: diff::MarkKind::Added }).collect()
                }
                None => vec![],
            };
            doc.git_marks_rev = doc.ed.revision();
        }
        &doc.git_marks
    }
}


impl App {
    /// Path of the active document relative to the repository root.
    pub fn current_rel_path(&self) -> Option<String> {
        let repo = self.git.repo.as_ref()?;
        let p = self.doc()?.path.as_ref()?;
        repo.rel(p)
    }

    pub fn open_branch_palette(&mut self) {
        if self.git.repo.is_none() {
            self.toast(tr("This project is not a Git repository"), Sev::Weak);
            return;
        }
        let items: Vec<Item> = self
            .git
            .branches
            .iter()
            .map(|b| Item {
                label: b.name.clone(),
                detail: if b.current { tr("current").into() } else if b.remote { tr("remote").into() } else { tr("local").into() },
                badge: "B".into(),
                section: "",
                target: PTarget::Branch(b.name.clone()),
                hits: vec![],
                shortcut: String::new(),
            })
            .collect();
        self.popup = Popup::Palette(PaletteState { mode: PaletteMode::Choose, query: String::new(), items: items.clone(), selected: 0, built_for: None, fixed: Some(items), just_opened: true });
    }

    pub fn fold_command(&mut self, id: &str) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let regions = crate::folding::regions(&d.ed);
        let line = d.ed.line_col(d.ed.caret()).0;
        match id {
            "edit.fold" => {
                if let Some(r) = crate::folding::region_at(&regions, line) {
                    let key = r.key.clone();
                    let start = r.start_line;
                    d.folds.insert(key);
                    // keep the caret visible: move it to the end of the fold's first line
                    let ls = d.ed.line_start(start);
                    let le = ls + d.ed.line_text(start).len();
                    if d.ed.caret() > le {
                        d.ed.set_caret(le, false);
                    }
                }
            }
            "edit.unfold" => {
                if let Some(r) = crate::folding::hidden_by(&regions, &d.folds, line).or_else(|| crate::folding::region_at(&regions, line)) {
                    let key = r.key.clone();
                    d.folds.remove(&key);
                }
            }
            "edit.fold_all" => {
                for r in &regions {
                    d.folds.insert(r.key.clone());
                }
                let visible = crate::folding::visible_lines(d.ed.line_count(), &regions, &d.folds);
                if !visible.contains(&line) {
                    let nl = visible.iter().rev().find(|&&l| l <= line).copied().unwrap_or(0);
                    let o = d.ed.line_start(nl);
                    d.ed.set_caret(o, false);
                }
            }
            _ => d.folds.clear(),
        }
        d.view.scroll_to_caret = true;
    }
}

// ---------------------------------------------------------------------- local commits

impl App {
    pub fn after_base_change(&mut self) {
        for d in self.docs.iter_mut() {
            d.git_base = None;
            d.git_marks_rev = u64::MAX;
        }
        self.git.rx = None;
        self.git.last_refresh = -100.0;
        self.git.checked.clear();
        self.git.changes.clear();
        self.git_refresh();
        if let Some(p) = self.project.as_mut() {
            p.tree_ver += 1;
        }
    }

    /// Create a local commit (checkpoint) of the working tree; it becomes the diff base.
    pub fn local_commit(&mut self, message: &str) -> bool {
        self.save_all();
        let repo = self.git.repo.clone();
        let Some(local) = self.git.local.as_mut() else {
            self.toast(tr("Open a project first"), Sev::Weak);
            return false;
        };
        match local.create(message, repo.as_ref()) {
            Ok(id) => {
                self.after_base_change();
                self.toast(i18n::fmt("Local commit #{} created - the diff now starts from it", &[&id]), Sev::Weak);
                true
            }
            Err(e) => {
                self.toast(i18n::fmt("Local commit failed: {}", &[&e]), Sev::Error);
                false
            }
        }
    }

    pub fn local_set_base(&mut self, base: Base) {
        let Some(local) = self.git.local.as_mut() else { return };
        match local.set_base(base) {
            Ok(()) => {
                self.after_base_change();
                let label = self.base_label();
                self.toast(i18n::fmt("Comparing with {}", &[&label]), Sev::Weak);
            }
            Err(e) => self.toast(i18n::fmt("Cannot change base: {}", &[&e]), Sev::Error),
        }
    }

    pub fn local_restore(&mut self, id: u64) {
        self.save_all();
        let repo = self.git.repo.clone();
        let Some(local) = self.git.local.clone() else { return };
        match local.restore(id, repo.as_ref()) {
            Ok(n) => {
                for d in self.docs.iter_mut() {
                    d.sync_with_disk();
                }
                self.after_base_change();
                self.refresh_project();
                self.toast(i18n::fmt("Restored local commit #{0} ({1} file(s) changed)", &[&id, &n]), Sev::Weak);
            }
            Err(e) => self.toast(i18n::fmt("Restore failed: {}", &[&e]), Sev::Error),
        }
    }

    pub fn local_delete(&mut self, id: u64) {
        let Some(local) = self.git.local.as_mut() else { return };
        match local.delete(id) {
            Ok(()) => {
                self.after_base_change();
                self.toast(i18n::fmt("Local commit #{} deleted", &[&id]), Sev::Weak);
            }
            Err(e) => self.toast(i18n::fmt("Delete failed: {}", &[&e]), Sev::Error),
        }
    }

    /// Discard local edits of one file: back to the diff base (git HEAD or the local commit).
    pub fn rollback_file(&mut self, rel: &str) {
        if self.base_is_local() {
            let Some(root) = self.vcs_root() else { return };
            let abs = root.join(rel);
            let res = match self.base_text_of(&abs).flatten() {
                Some(t) => std::fs::write(&abs, t),
                None => std::fs::remove_file(&abs),
            };
            match res {
                Ok(()) => {
                    for d in self.docs.iter_mut() {
                        d.sync_with_disk();
                    }
                    self.git.last_refresh = -100.0;
                    self.git_refresh();
                    self.refresh_project();
                }
                Err(e) => self.toast(i18n::fmt("Rollback failed: {}", &[&e]), Sev::Error),
            }
            return;
        }
        if let Some(change) = self.git.changes.iter().find(|c| c.path == rel).cloned() {
            self.git_run_sync("Rollback", move |r| r.rollback(&change).map(|_| String::new()));
            for d in self.docs.iter_mut() {
                d.git_base = None;
                d.sync_with_disk();
            }
        }
    }
}
