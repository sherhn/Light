//! Merge conflicts: merging a branch into the current one, the banner of a merge (or rebase, cherry-pick, revert) that
//! stopped for conflicts, the window listing the conflicted files, and in the editor the conflicts of a file, its two
//! sides tinted, each resolved with a click.

use eframe::egui::{self, Align, Layout, RichText, Vec2};
use ide_core::conflicts::{self, Choice, Conflict};
use ide_core::git::{ChangeKind, MergeKind};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::*;
use crate::widgets;

impl App {
    /// The files git says are in conflict (relative paths).
    pub fn conflicted_files(&self) -> Vec<String> {
        self.git.changes.iter().filter(|c| c.kind == ChangeKind::Conflicted).map(|c| c.path.clone()).collect()
    }

    /// Is document `idx` one of the files git says are in conflict?
    fn doc_is_conflicted(&self, idx: usize) -> bool {
        let Some(path) = self.docs.get(idx).and_then(|d| d.path.as_ref()) else { return false };
        self.git.repo.as_ref().and_then(|r| r.rel(path)).is_some_and(|rel| self.conflicted_files().contains(&rel))
    }

    /// The conflicts in the text of document `idx`.
    pub fn doc_conflicts(&self, idx: usize) -> Vec<Conflict> {
        match self.docs.get(idx) {
            Some(d) if d.diff.is_none() && d.ed.text().contains("<<<<<<<") => conflicts::find(d.ed.text()),
            _ => vec![],
        }
    }

    /// The backgrounds of the sides of the conflicts of document `idx`: ours, the base, theirs, and the marker lines.
    pub fn conflict_tints(&self, idx: usize) -> Vec<(usize, usize, egui::Color32)> {
        let pal = self.pal;
        let mut v = Vec::new();
        for c in self.doc_conflicts(idx) {
            let ours_end = c.base_marker.unwrap_or(c.middle);
            v.push((c.start, c.start, pal.accent.gamma_multiply(0.12)));
            if c.start + 1 < ours_end {
                v.push((c.start + 1, ours_end - 1, pal.git_added.gamma_multiply(0.22)));
            }
            if let Some(b) = c.base_marker {
                v.push((b, b, pal.accent.gamma_multiply(0.12)));
                if b + 1 < c.middle {
                    v.push((b + 1, c.middle - 1, pal.text_dim.gamma_multiply(0.12)));
                }
            }
            v.push((c.middle, c.middle, pal.accent.gamma_multiply(0.12)));
            if c.middle + 1 < c.end {
                v.push((c.middle + 1, c.end - 1, pal.blue.gamma_multiply(0.26)));
            }
            v.push((c.end, c.end, pal.accent.gamma_multiply(0.12)));
        }
        v
    }

    /// The conflict the caret is in, or the next one after it (the last one at the end of the file).
    fn current_conflict(&self, idx: usize, list: &[Conflict]) -> usize {
        let line = self.docs[idx].ed.line_col(self.docs[idx].ed.caret()).0;
        list.iter().position(|c| c.end >= line).unwrap_or(list.len().saturating_sub(1))
    }

    /// Resolve conflict `n` of document `idx` (one undo step).
    pub fn resolve_conflict(&mut self, idx: usize, n: usize, choice: Choice) {
        let list = self.doc_conflicts(idx);
        let Some(c) = list.get(n) else { return };
        let ed = &mut self.docs[idx].ed;
        let start = ed.line_start(c.start);
        let end = if c.end + 1 < ed.line_count() { ed.line_start(c.end + 1) } else { ed.text().len() };
        let mut kept = match choice {
            Choice::Ours => c.ours.clone(),
            Choice::Theirs => c.theirs.clone(),
            Choice::Both => c.ours.clone() + &c.theirs,
        };
        if end == ed.text().len() && !ed.text().ends_with('\n') && kept.ends_with('\n') {
            kept.pop();
        }
        ed.replace_range(start, end, &kept);
        ed.set_caret(start, false);
        self.docs[idx].view.scroll_to_caret = true;
    }

    fn goto_conflict(&mut self, idx: usize, c: &Conflict) {
        let d = &mut self.docs[idx];
        let o = d.ed.line_start(c.start);
        d.ed.set_caret(o, false);
        d.view.scroll_to_caret = true;
        self.editor_focus = true;
    }

    /// The file's conflicts are all resolved: it is saved and staged, so git takes it as resolved.
    pub fn mark_resolved(&mut self, idx: usize) {
        if !self.doc_conflicts(idx).is_empty() {
            self.toast(tr("The file still has conflicts"), Sev::Weak);
            return;
        }
        if !self.save_doc(idx) {
            return;
        }
        let Some(rel) = self.docs[idx].path.as_ref().and_then(|p| self.git.repo.as_ref().and_then(|r| r.rel(p))) else { return };
        if self.git_run_sync("Mark as Resolved", move |r| r.stage(&[rel]).map(|_| String::new())) {
            self.toast(tr("Marked as resolved"), Sev::Weak);
            self.git_refresh_now();
        }
    }

    /// One side for the whole file (from the Conflicts window): git writes it, the open document reads it again.
    pub fn take_conflict_side(&mut self, rel: &str, ours: bool) {
        let rel_owned = rel.to_string();
        if self.git_run_sync(if ours { "Accept Yours" } else { "Accept Theirs" }, move |r| r.take_side(&rel_owned, ours).map(|_| String::new())) {
            let path = self.git.repo.as_ref().map(|r| r.root.join(rel));
            if let Some(d) = self.docs.iter_mut().find(|d| d.path.as_ref() == path.as_ref()) {
                let _ = d.reload();
            }
            self.git_refresh_now();
        }
    }

    /// Choose a branch to merge into the current one.
    pub fn open_merge_palette(&mut self) {
        if self.git.repo.is_none() {
            self.toast(tr("This project is not a Git repository"), Sev::Weak);
            return;
        }
        let items: Vec<Item> = self.git.branches.iter().filter(|b| !b.current).map(|b| Item {
            label: b.name.clone(),
            detail: if b.remote { tr("remote").into() } else { tr("local").into() },
            badge: "B".into(),
            section: "",
            target: PTarget::Merge(b.name.clone()),
            hits: vec![],
            shortcut: String::new(),
        }).collect();
        if items.is_empty() {
            self.toast(tr("There is no other branch to merge"), Sev::Weak);
            return;
        }
        self.popup = Popup::Palette(PaletteState { mode: PaletteMode::Choose, query: String::new(), items: items.clone(), selected: 0, built_for: None, fixed: Some(items), just_opened: true });
    }

    /// Merge `branch` into the current branch; conflicts open the Conflicts window.
    pub fn git_merge(&mut self, branch: &str) {
        self.save_all();
        let Some(repo) = self.git.repo.clone() else { return };
        match repo.merge(branch) {
            Ok(msg) => {
                self.toast(i18n::fmt("Merged '{0}': {1}", &[&branch, &msg]), Sev::Weak);
            }
            Err(e) if repo.merge_state() == Some(MergeKind::Merge) => {
                let _ = e;
                self.toast(i18n::fmt("Merging '{}' stopped for conflicts", &[&branch]), Sev::Error);
                self.popup = Popup::Conflicts;
            }
            Err(e) => self.toast(i18n::fmt("{0} failed: {1}", &[&tr("Merge"), &e.lines().next().unwrap_or("")]), Sev::Error),
        }
        self.reload_clean_docs();
        self.git.rx = None;
        self.git_refresh_now();
    }

    /// Ask git again at once (a merge changes the files and the state right away).
    fn git_refresh_now(&mut self) {
        let Some(repo) = self.git.repo.clone() else { return };
        self.git.changes = ide_core::changes::without_ignored(repo.status(), &repo.root);
        self.git.merge = repo.merge_state();
        self.git_refresh();
        self.refresh_project();
    }

    /// The documents without unsaved edits read what git wrote.
    fn reload_clean_docs(&mut self) {
        for d in self.docs.iter_mut().filter(|d| d.diff.is_none() && d.path.as_ref().is_some_and(|p| p.is_file()) && !d.ed.is_modified()) {
            let _ = d.reload();
        }
    }

    pub fn git_merge_continue(&mut self) {
        let Some(kind) = self.git.merge else { return };
        if !self.conflicted_files().is_empty() {
            self.popup = Popup::Conflicts;
            return;
        }
        self.save_all();
        if self.git_run_sync(merge_continue_label(kind), move |r| r.merge_continue(kind)) {
            self.reload_clean_docs();
            self.git_refresh_now();
        }
    }

    pub fn git_merge_abort(&mut self) {
        let Some(kind) = self.git.merge else { return };
        if self.git_run_sync("Abort", move |r| r.merge_abort(kind).map(|_| String::new())) {
            for d in self.docs.iter_mut().filter(|d| d.diff.is_none() && d.path.as_ref().is_some_and(|p| p.is_file())) {
                let _ = d.reload();
            }
            self.git_refresh_now();
        }
    }

    /// Above the editor while an operation of git waits: what is going on, and what can be done.
    pub fn draw_merge_banner(&mut self, ui: &mut egui::Ui) {
        let Some(kind) = self.git.merge else { return };
        let pal = self.pal;
        let n = self.conflicted_files().len();
        let mut act = None;
        egui::Frame::new().fill(pal.warning.gamma_multiply(0.12)).stroke(egui::Stroke::new(1.0, pal.warning.gamma_multiply(0.5))).corner_radius(egui::CornerRadius::same(9)).inner_margin(egui::Margin::symmetric(10, 6)).outer_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                icons::paint(ui.painter(), r, Icon::Merge, pal.warning);
                let what = match kind {
                    MergeKind::Merge => tr("Merge in progress"),
                    MergeKind::Rebase => tr("Rebase in progress"),
                    MergeKind::CherryPick => tr("Cherry-pick in progress"),
                    MergeKind::Revert => tr("Revert in progress"),
                };
                ui.label(RichText::new(what).font(fonts::semibold(12.5)));
                ui.label(RichText::new(if n > 0 { i18n::fmt("{} file(s) with conflicts", &[&n]) } else { tr("all conflicts are resolved").to_string() }).color(pal.text_dim).size(12.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::compact_button(ui, &pal, tr("Abort"), false).clicked() {
                        act = Some("abort");
                    }
                    let main = if n > 0 { tr("Resolve Conflicts...") } else { tr(merge_continue_label(kind)) };
                    if widgets::compact_button(ui, &pal, main, true).clicked() {
                        act = Some(if n > 0 { "resolve" } else { "continue" });
                    }
                });
            });
        });
        match act {
            Some("abort") => self.git_merge_abort(),
            Some("resolve") => self.popup = Popup::Conflicts,
            Some("continue") => self.git_merge_continue(),
            _ => {}
        }
    }

    /// Above a file with conflicts: which one, how many, and the ways to resolve it.
    pub fn draw_conflict_bar(&mut self, ui: &mut egui::Ui, idx: usize) {
        let list = self.doc_conflicts(idx);
        let in_conflict = self.doc_is_conflicted(idx);
        if list.is_empty() && !in_conflict {
            return;
        }
        let pal = self.pal;
        let current = self.current_conflict(idx, &list);
        let mut act: Option<&str> = None;
        let mut rects = Vec::new();
        egui::Frame::new().fill(pal.panel_bg.to_opaque()).inner_margin(egui::Margin::symmetric(12, 4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if list.is_empty() {
                    ui.label(RichText::new(tr("No conflicts are left in this file")).color(pal.text_dim).size(12.5));
                    let r = widgets::compact_button(ui, &pal, tr("Mark as Resolved"), true);
                    rects.push(("Mark as Resolved", r.rect));
                    if r.clicked() {
                        act = Some("resolved");
                    }
                    return;
                }
                let c = &list[current];
                ui.label(RichText::new(i18n::fmt("Conflict {0} of {1}", &[&(current + 1), &list.len()])).font(fonts::semibold(12.5)));
                if widgets::icon_button(ui, &pal, Icon::ChevronUp, 24.0, pal.text_dim, tr("Previous conflict")).clicked() {
                    act = Some("prev");
                }
                if widgets::icon_button(ui, &pal, Icon::ChevronDown, 24.0, pal.text_dim, tr("Next conflict")).clicked() {
                    act = Some("next");
                }
                ui.add_space(8.0);
                for (key, label, tip) in [("ours", tr("Accept Yours"), c.ours_label.clone()), ("theirs", tr("Accept Theirs"), c.theirs_label.clone()), ("both", tr("Accept Both"), String::new())] {
                    let r = widgets::compact_button(ui, &pal, label, key == "ours");
                    rects.push((key, r.rect));
                    let r = if tip.is_empty() { r } else { r.on_hover_text(tip) };
                    if r.clicked() {
                        act = Some(key);
                    }
                }
                ui.label(RichText::new(format!("{}  ·  {}", c.ours_label, c.theirs_label)).color(pal.text_dim).size(11.5));
            });
        });
        ui.data_mut(|d| d.insert_temp(egui::Id::unique("conflict_bar_rects"), rects));
        match act {
            Some("ours") => self.resolve_conflict(idx, current, Choice::Ours),
            Some("theirs") => self.resolve_conflict(idx, current, Choice::Theirs),
            Some("both") => self.resolve_conflict(idx, current, Choice::Both),
            Some("prev") => {
                let c = list[current.saturating_sub(1)].clone();
                self.goto_conflict(idx, &c);
            }
            Some("next") => {
                let c = list[(current + 1).min(list.len() - 1)].clone();
                self.goto_conflict(idx, &c);
            }
            Some("resolved") => self.mark_resolved(idx),
            _ => {}
        }
    }

    /// The window of the conflicted files: one side for the whole file, or the file opened to merge it by hand.
    pub fn draw_conflicts_dialog(&mut self, ctx: &egui::Context) {
        if !matches!(self.popup, Popup::Conflicts) {
            return;
        }
        let pal = self.pal;
        let files = self.conflicted_files();
        let mut act: Option<(&str, String)> = None;
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut rects = Vec::new();
        self.dialog_card(ctx, "conflicts", tr("Conflicts"), 560.0, |ui| {
            if files.is_empty() {
                ui.label(RichText::new(tr("No file has conflicts.")).color(pal.text_dim));
            }
            for f in &files {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                    icons::paint(ui.painter(), r, Icon::File, pal.error);
                    ui.label(RichText::new(f).font(fonts::ui(13.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for (key, label) in [("merge", tr("Merge...")), ("theirs", tr("Accept Theirs")), ("ours", tr("Accept Yours"))] {
                            let r = widgets::compact_button(ui, &pal, label, key == "merge");
                            rects.push((format!("{key}:{f}"), r.rect));
                            if r.clicked() {
                                act = Some((key, f.clone()));
                            }
                        }
                    });
                });
                ui.add_space(4.0);
            }
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::secondary_button(ui, &pal, tr("Close")).clicked() {
                    close = true;
                }
            });
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("conflicts_dialog_rects"), rects));
        match act {
            Some(("merge", rel)) => {
                self.popup = Popup::None;
                if let Some(path) = self.git.repo.as_ref().map(|r| r.root.join(&rel)) {
                    self.open_file(&path);
                    let idx = self.active;
                    if let Some(c) = self.doc_conflicts(idx).first().cloned() {
                        self.goto_conflict(idx, &c);
                    }
                }
            }
            Some((side, rel)) => self.take_conflict_side(&rel, side == "ours"),
            None if close => {
                self.popup = Popup::None;
                self.editor_focus = true;
            }
            None => {}
        }
    }
}

fn merge_continue_label(kind: MergeKind) -> &'static str {
    match kind {
        MergeKind::Merge => "Commit Merge",
        MergeKind::Rebase => "Continue Rebase",
        MergeKind::CherryPick => "Continue Cherry-pick",
        MergeKind::Revert => "Continue Revert",
    }
}
