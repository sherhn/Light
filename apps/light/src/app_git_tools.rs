//! More of Git: who changed each line (blame, next to the line numbers), the history of a file, the stashes,
//! cherry-picking and reverting a commit, comparing the current branch with another, and the interactive rebase.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Sense, Vec2};
use ide_core::git::{BlameLine, ChangeKind, Commit, RebaseAction, RebaseStep, Stash};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::*;
use crate::widgets::{self, ROW_H};

/// Something to do to the app once a menu is closed.
pub type AppAction = Box<dyn FnOnce(&mut App)>;

/// The current branch next to another.
#[derive(Clone, Debug, Default)]
pub struct Comparison {
    pub other: String,
    /// The commits the other branch has and this one has not, and the other way round.
    pub theirs: Vec<Commit>,
    pub ours: Vec<Commit>,
    pub files: Vec<(String, ChangeKind)>,
}

/// A commit of the interactive rebase being prepared.
#[derive(Clone, Debug)]
pub struct RebaseRow {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub action: RebaseAction,
    pub message: String,
}

/// The interactive rebase being prepared: onto `base`, the commits after it in the order they will be.
#[derive(Clone, Debug)]
pub struct RebaseState {
    pub base: String,
    pub base_label: String,
    pub rows: Vec<RebaseRow>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct GitTools {
    /// Blame of the files it is shown for: path -> the lines.
    pub blame: HashMap<PathBuf, Vec<BlameLine>>,
    /// The history of one file: its path relative to the repository, and the commits.
    pub history: Option<(String, Vec<Commit>)>,
    pub stashes: Vec<Stash>,
    pub comparison: Option<Comparison>,
    /// Where the rows of the Git window's views were drawn (the UI tests click them): key -> rectangle.
    pub rows: Vec<(String, Rect)>,
}

/// `2026-11-03` of seconds since the epoch.
fn date_of(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{month:02}-{day:02}", yoe + era * 400 + i64::from(month <= 2))
}

/// The label of a blamed line: when and who (`Not committed` for a new line).
pub fn blame_label(b: &BlameLine) -> String {
    if b.uncommitted() { tr("Not committed").to_string() } else { format!("{}  {}", date_of(b.time), b.author) }
}

impl App {
    fn active_rel(&self) -> Option<(PathBuf, String)> {
        let path = self.doc().filter(|d| d.diff.is_none()).and_then(|d| d.path.clone())?;
        let rel = self.git.repo.as_ref()?.rel(&path)?;
        Some((path, rel))
    }

    // ------------------------------------------------------------------ blame

    /// Show who changed each line of the active file next to its numbers, or stop showing it.
    pub fn toggle_blame(&mut self) {
        let Some((path, rel)) = self.active_rel() else {
            self.toast(tr("Open a file of the Git repository first"), Sev::Weak);
            return;
        };
        if self.git_tools.blame.remove(&path).is_some() {
            return;
        }
        let Some(repo) = self.git.repo.clone() else { return };
        match repo.blame(&rel) {
            Ok(lines) => {
                self.git_tools.blame.insert(path, lines);
            }
            Err(e) => self.toast(i18n::fmt("Cannot annotate: {}", &[&e.lines().next().unwrap_or_default()]), Sev::Error),
        }
    }

    /// Blame again the files it is shown for (after a save or a commit).
    pub fn refresh_blame(&mut self) {
        let Some(repo) = self.git.repo.clone() else { return };
        let paths: Vec<PathBuf> = self.git_tools.blame.keys().cloned().collect();
        for p in paths {
            if let Some(rel) = repo.rel(&p)
                && let Ok(lines) = repo.blame(&rel)
            {
                self.git_tools.blame.insert(p, lines);
            }
        }
    }

    /// The annotation of every line of document `idx` (empty: blame is not shown), the newer the brighter.
    pub fn blame_annotations(&self, idx: usize) -> Vec<(String, Color32)> {
        let Some(lines) = self.docs.get(idx).and_then(|d| d.path.as_ref()).and_then(|p| self.git_tools.blame.get(p)) else { return vec![] };
        let pal = self.pal;
        let (oldest, newest) = lines.iter().filter(|b| !b.uncommitted()).fold((i64::MAX, i64::MIN), |(a, b), l| (a.min(l.time), b.max(l.time)));
        lines.iter().map(|b| {
            if b.uncommitted() {
                return (blame_label(b), pal.text_dim.gamma_multiply(0.7));
            }
            let age = if newest > oldest { (newest - b.time) as f32 / (newest - oldest) as f32 } else { 0.0 };
            (blame_label(b), pal.text.lerp_to_gamma(pal.text_dim.gamma_multiply(0.6), age))
        }).collect()
    }

    /// A click on a line's annotation: its commit.
    pub fn show_blamed_commit(&mut self, idx: usize, line: usize) {
        let Some(b) = self.docs.get(idx).and_then(|d| d.path.as_ref()).and_then(|p| self.git_tools.blame.get(p)).and_then(|l| l.get(line)).cloned() else { return };
        if b.uncommitted() {
            self.toast(tr("This line is not committed yet"), Sev::Weak);
            return;
        }
        let Some(repo) = self.git.repo.clone() else { return };
        let text = repo.show(&b.hash).unwrap_or_else(|e| e);
        self.popup = Popup::Info { title: format!("{}  {}", &b.hash[..7.min(b.hash.len())], b.summary), text };
    }

    // ------------------------------------------------------------------ history of a file

    pub fn show_file_history(&mut self) {
        let Some((_, rel)) = self.active_rel() else {
            self.toast(tr("Open a file of the Git repository first"), Sev::Weak);
            return;
        };
        let Some(repo) = self.git.repo.clone() else { return };
        let log = repo.file_log(&rel, 300);
        if log.is_empty() {
            self.toast(tr("The file has no history in Git yet"), Sev::Weak);
            return;
        }
        self.git_tools.history = Some((rel, log));
        self.git.tab = GitTab::History;
        self.bottom_tool = Some(BottomTool::Git);
    }

    /// The change commit `c` made to the file: its version before the commit next to the one after.
    fn show_history_change(&mut self, rel: &str, c: &Commit) {
        let Some(repo) = self.git.repo.clone() else { return };
        let at = repo.path_at(&c.hash, rel);
        let new = repo.file_at(&c.hash, &at).unwrap_or_default();
        let parent = c.parents.first().cloned();
        let old = parent.as_ref().and_then(|p| repo.file_at(p, &at).or_else(|| repo.file_at(p, rel))).unwrap_or_default();
        let before = parent.map(|p| p[..7.min(p.len())].to_string()).unwrap_or_else(|| tr("nothing (the file is new)").to_string());
        self.open_fixed_diff(&at, old, new, before, format!("{}  {}", c.short, c.subject));
    }

    // ------------------------------------------------------------------ stashes

    pub fn refresh_stashes(&mut self) {
        self.git_tools.stashes = self.git.repo.as_ref().map(|r| r.stashes()).unwrap_or_default();
    }

    pub fn stash_save(&mut self, message: &str, untracked: bool) {
        self.save_all();
        let m = message.to_string();
        if self.git_run_sync("Stash", move |r| r.stash_save(&m, untracked).map(|_| String::new())) {
            self.toast(tr("The changes are put aside"), Sev::Weak);
            self.reload_from_git();
            self.refresh_stashes();
            self.git.tab = GitTab::Stashes;
            self.bottom_tool = Some(BottomTool::Git);
        }
    }

    pub fn stash_do(&mut self, verb: &'static str, name: &str) {
        if verb != "drop" {
            self.save_all();
        }
        let n = name.to_string();
        let what = match verb { "apply" => "Apply Stash", "pop" => "Pop Stash", _ => "Drop Stash" };
        // git's own words name the stash by its hash: the IDE says what happened in its own
        if self.git_run_sync(what, move |r| r.stash_do(verb, &n).map(|_| String::new())) {
            self.reload_from_git();
            self.toast(tr(match verb { "apply" => "The stash is applied", "pop" => "The stash is applied and dropped", _ => "The stash is dropped" }), Sev::Weak);
        }
        self.refresh_stashes();
    }

    /// The open documents without unsaved edits read what git wrote, and the state of the repository is asked again.
    fn reload_from_git(&mut self) {
        for d in self.docs.iter_mut().filter(|d| d.diff.is_none() && d.path.as_ref().is_some_and(|p| p.is_file()) && !d.ed.is_modified()) {
            let _ = d.reload();
        }
        if let Some(repo) = self.git.repo.clone() {
            self.git.changes = ide_core::changes::without_ignored(repo.status(), &repo.root);
            self.git.merge = repo.merge_state();
        }
        self.git.rx = None;
        self.git_refresh();
        self.refresh_blame();
    }

    // ------------------------------------------------------------------ cherry-pick, revert

    pub fn git_cherry_pick(&mut self, hash: &str, subject: &str) {
        self.save_all();
        let Some(repo) = self.git.repo.clone() else { return };
        match repo.cherry_pick(hash) {
            Ok(_) => self.toast(i18n::fmt("Cherry-picked '{}'", &[&subject]), Sev::Weak),
            Err(e) => {
                if repo.merge_state().is_some() {
                    self.toast(i18n::fmt("Cherry-picking '{}' stopped for conflicts", &[&subject]), Sev::Error);
                    self.popup = Popup::Conflicts;
                } else {
                    self.toast(i18n::fmt("{0} failed: {1}", &[&tr("Cherry-Pick"), &e.lines().next().unwrap_or_default()]), Sev::Error);
                }
            }
        }
        self.reload_from_git();
    }

    pub fn git_revert(&mut self, hash: &str, subject: &str) {
        self.save_all();
        let Some(repo) = self.git.repo.clone() else { return };
        match repo.revert(hash) {
            Ok(_) => self.toast(i18n::fmt("Reverted '{}'", &[&subject]), Sev::Weak),
            Err(e) => {
                if repo.merge_state().is_some() {
                    self.toast(i18n::fmt("Reverting '{}' stopped for conflicts", &[&subject]), Sev::Error);
                    self.popup = Popup::Conflicts;
                } else {
                    self.toast(i18n::fmt("{0} failed: {1}", &[&tr("Revert"), &e.lines().next().unwrap_or_default()]), Sev::Error);
                }
            }
        }
        self.reload_from_git();
    }

    // ------------------------------------------------------------------ compare branches

    pub fn open_compare_palette(&mut self) {
        if self.git.repo.is_none() {
            self.toast(tr("This project is not a Git repository"), Sev::Weak);
            return;
        }
        let items: Vec<Item> = self.git.branches.iter().filter(|b| !b.current).map(|b| Item {
            label: b.name.clone(),
            detail: if b.remote { tr("remote").into() } else { tr("local").into() },
            badge: "B".into(),
            section: "",
            target: PTarget::Compare(b.name.clone()),
            hits: vec![],
            shortcut: String::new(),
        }).collect();
        if items.is_empty() {
            self.toast(tr("There is no other branch to compare with"), Sev::Weak);
            return;
        }
        self.popup = Popup::Palette(PaletteState { mode: PaletteMode::Choose, query: String::new(), items: items.clone(), selected: 0, built_for: None, fixed: Some(items), just_opened: true });
    }

    pub fn git_compare(&mut self, other: &str) {
        let Some(repo) = self.git.repo.clone() else { return };
        let (theirs, ours) = repo.compare(other, 300);
        let files = repo.compare_files(other);
        self.git_tools.comparison = Some(Comparison { other: other.to_string(), theirs, ours, files });
        self.git.tab = GitTab::Compare;
        self.bottom_tool = Some(BottomTool::Git);
    }

    /// A file that differs between the branches: its version on the current branch next to the other's.
    fn show_compared_file(&mut self, rel: &str) {
        let Some(repo) = self.git.repo.clone() else { return };
        let Some(other) = self.git_tools.comparison.as_ref().map(|c| c.other.clone()) else { return };
        let ours = repo.file_at("HEAD", rel).unwrap_or_default();
        let theirs = repo.file_at(&other, rel).unwrap_or_default();
        let branch = self.git.branch.clone().unwrap_or_else(|| "HEAD".into());
        self.open_fixed_diff(rel, ours, theirs, branch, other);
    }

    // ------------------------------------------------------------------ interactive rebase

    /// Prepare an interactive rebase of the commits after `base` (a commit of the log).
    pub fn open_rebase(&mut self, base: &str, label: &str) {
        let Some(repo) = self.git.repo.clone() else { return };
        if !self.git.changes.iter().all(|c| c.kind == ChangeKind::Untracked) {
            self.toast(tr("Commit or stash the changes first: a rebase rewrites the files"), Sev::Weak);
            return;
        }
        let rows: Vec<RebaseRow> = repo.commits_since(base).into_iter().map(|c| RebaseRow { hash: c.hash, short: c.short, message: c.subject.clone(), subject: c.subject, action: RebaseAction::Pick }).collect();
        if rows.is_empty() {
            self.toast(tr("There are no commits after this one to rebase"), Sev::Weak);
            return;
        }
        self.popup = Popup::Rebase(Box::new(RebaseState { base: base.to_string(), base_label: label.to_string(), rows, error: None }));
    }

    fn run_rebase(&mut self, st: RebaseState) {
        let steps: Vec<RebaseStep> = st.rows.iter().map(|r| RebaseStep { hash: r.hash.clone(), action: r.action, message: if r.action == RebaseAction::Reword { format!("{}\n", r.message.trim()) } else { String::new() } }).collect();
        let Some(repo) = self.git.repo.clone() else { return };
        match repo.rebase_interactive(&st.base, &steps) {
            Ok(_) => {
                self.popup = Popup::None;
                self.toast(tr("Rebased"), Sev::Weak);
            }
            Err(e) if repo.merge_state().is_some() => {
                self.popup = Popup::Conflicts;
                self.toast(i18n::fmt("The rebase stopped: {}", &[&e.lines().next().unwrap_or_default()]), Sev::Error);
            }
            Err(e) => self.popup = Popup::Rebase(Box::new(RebaseState { error: Some(e), ..st })),
        }
        self.reload_from_git();
    }

    pub fn draw_rebase_dialog(&mut self, ctx: &egui::Context) {
        let Popup::Rebase(st) = &self.popup else { return };
        let mut st = (**st).clone();
        let pal = self.pal;
        let (mut start, mut cancel) = (false, ctx.input(|i| i.key_pressed(egui::Key::Escape)));
        let mut moves: Option<(usize, isize)> = None;
        let mut rects: Vec<(String, Rect)> = Vec::new();
        let actions = [(RebaseAction::Pick, tr("pick")), (RebaseAction::Reword, tr("reword")), (RebaseAction::Squash, tr("squash")), (RebaseAction::Fixup, tr("fixup")), (RebaseAction::Drop, tr("drop"))];
        self.dialog_card(ctx, "rebase", tr("Interactive Rebase"), 720.0, |ui| {
            ui.label(RichText::new(i18n::fmt("The commits after {}, from the oldest. Change the order with the arrows; squash and fixup melt a commit into the one above it.", &[&st.base_label])).color(pal.text_dim).size(12.5));
            ui.add_space(10.0);
            let n = st.rows.len();
            egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                for (i, row) in st.rows.iter_mut().enumerate() {
                    let dropped = row.action == RebaseAction::Drop;
                    egui::Frame::new().fill(if dropped { pal.error.gamma_multiply(0.06) } else { pal.panel_bg }).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(i > 0, |ui| if widgets::icon_button(ui, &pal, Icon::ArrowUp, 24.0, pal.text_dim, tr("Earlier")).clicked() { moves = Some((i, -1)); });
                            ui.add_enabled_ui(i + 1 < n, |ui| if widgets::icon_button(ui, &pal, Icon::ArrowDown, 24.0, pal.text_dim, tr("Later")).clicked() { moves = Some((i, 1)); });
                            ui.label(RichText::new(&row.short).font(egui::FontId::monospace(12.5)).color(pal.text_dim));
                            let subject = RichText::new(&row.subject).size(13.0);
                            ui.label(if dropped { subject.strikethrough().color(pal.text_dim) } else { subject });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                for (a, label) in actions.iter().rev() {
                                    let active = row.action == *a;
                                    let color = match a { RebaseAction::Drop => pal.error, RebaseAction::Reword => pal.accent, RebaseAction::Squash | RebaseAction::Fixup => pal.warning, RebaseAction::Pick => pal.ok };
                                    let g = ui.painter().layout_no_wrap(label.to_string(), fonts::ui(12.0), pal.text);
                                    let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 14.0, 22.0), Sense::click());
                                    if active {
                                        ui.painter().rect_filled(r, egui::CornerRadius::same(11), color.gamma_multiply(0.25));
                                    } else if resp.hovered() {
                                        ui.painter().rect_filled(r, egui::CornerRadius::same(11), pal.hover);
                                    }
                                    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, *label, fonts::ui(12.0), if active { color } else { pal.text_dim });
                                    rects.push((format!("{i}:{label}"), r));
                                    if resp.clicked() {
                                        row.action = *a;
                                    }
                                }
                            });
                        });
                        if row.action == RebaseAction::Reword {
                            ui.add(egui::TextEdit::multiline(&mut row.message).desired_rows(2).desired_width(f32::INFINITY).font(fonts::ui(13.0)));
                        }
                    });
                    ui.add_space(4.0);
                }
            });
            if let Some(e) = &st.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(pal.error).size(12.5));
            }
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let r = widgets::primary_button(ui, &pal, tr("Start Rebasing"));
                rects.push(("start".into(), r.rect));
                if r.clicked() {
                    start = true;
                }
                if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("rebase_rects"), rects));
        if let Some((i, d)) = moves {
            let j = (i as isize + d) as usize;
            st.rows.swap(i, j);
        }
        if cancel {
            self.popup = Popup::None;
            self.editor_focus = true;
        } else if start {
            self.run_rebase(st);
        } else {
            self.popup = Popup::Rebase(Box::new(st));
        }
    }

    /// Stash Changes: the message, and whether the new files go too.
    pub fn draw_stash_dialog(&mut self, ctx: &egui::Context) {
        let Popup::Stash { message, untracked, just_opened } = &self.popup else { return };
        let (mut message, mut untracked, just_opened) = (message.clone(), *untracked, *just_opened);
        let pal = self.pal;
        let (mut ok, mut cancel) = (false, ctx.input(|i| i.key_pressed(egui::Key::Escape)));
        self.dialog_card(ctx, "stash", tr("Stash Changes"), 440.0, |ui| {
            ui.label(RichText::new(tr("The changes are put aside and the files are as at the last commit; Apply or Pop brings them back.")).color(pal.text_dim).size(12.5));
            ui.add_space(10.0);
            let r = ui.add(egui::TextEdit::singleline(&mut message).hint_text(tr("What the changes are (optional)")).desired_width(f32::INFINITY));
            if just_opened {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            ui.add_space(8.0);
            ui.checkbox(&mut untracked, tr("Also the new files Git does not track yet"));
            let (o, c) = self.ok_cancel(ui, tr("Stash"));
            ok |= o;
            cancel |= c;
        });
        if ok {
            self.popup = Popup::None;
            self.stash_save(&message, untracked);
        } else if cancel {
            self.popup = Popup::None;
        } else {
            self.popup = Popup::Stash { message, untracked, just_opened: false };
        }
    }

    // ------------------------------------------------------------------ the views of the Git window

    /// The history of a file: its commits; a click shows the change the commit made to it.
    pub fn draw_file_history(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let Some((rel, log)) = self.git_tools.history.clone() else {
            self.empty_state(ui, Icon::History, pal.text_dim, tr("Show the history of a file: Git > Show History for File"));
            return;
        };
        let mut show = None;
        let mut rows = Vec::new();
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            icons::paint(ui.painter(), Rect::from_min_size(ui.cursor().min + Vec2::new(0.0, 2.0), Vec2::splat(14.0)), Icon::History, pal.accent);
            ui.add_space(18.0);
            ui.label(RichText::new(i18n::fmt("History of {}", &[&rel])).font(fonts::semibold(13.0)));
            ui.label(RichText::new(i18n::fmt("{} commits", &[&log.len()])).color(pal.text_dim).size(12.0));
        });
        egui::ScrollArea::vertical().id_salt("file_history").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for c in &log {
                let (r, resp) = self.commit_row(ui, c);
                rows.push((format!("history:{}", c.subject), r));
                if resp.clicked() {
                    show = Some(c.clone());
                }
            }
        });
        self.git_tools.rows = rows;
        if let Some(c) = show {
            self.show_history_change(&rel, &c);
        }
    }

    /// A commit in a list: its short hash, subject, author and when.
    fn commit_row(&self, ui: &mut egui::Ui, c: &Commit) -> (Rect, egui::Response) {
        let pal = self.pal;
        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
        widgets::row_background(ui, &pal, r, false, resp.hovered());
        ui.painter().circle_filled(Pos2::new(r.min.x + 22.0, r.center().y), 4.0, pal.accent);
        ui.painter().text(Pos2::new(r.min.x + 36.0, r.center().y), egui::Align2::LEFT_CENTER, &c.short, egui::FontId::monospace(12.5), pal.text_dim);
        let p = ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(r.min.x + 106.0, r.min.y), Pos2::new(r.max.x - 220.0, r.max.y)));
        p.text(Pos2::new(r.min.x + 106.0, r.center().y), egui::Align2::LEFT_CENTER, &c.subject, fonts::ui(13.5), pal.text);
        ui.painter().text(Pos2::new(r.max.x - 12.0, r.center().y), egui::Align2::RIGHT_CENTER, format!("{}   {}", c.author, c.when), fonts::ui(12.0), pal.text_dim);
        (r, resp)
    }

    /// The stashes, each with Apply, Pop and Drop; Stash Changes puts the changes aside.
    pub fn draw_stashes(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let mut act: Option<(&'static str, String)> = None;
        let mut rows = Vec::new();
        let mut new_stash = false;
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new(i18n::fmt("Stashes: {}", &[&self.git_tools.stashes.len()])).color(pal.text_dim).size(12.5));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                let r = widgets::secondary_button(ui, &pal, tr("Stash Changes..."));
                rows.push(("stash:new".to_string(), r.rect));
                if r.clicked() {
                    new_stash = true;
                }
            });
        });
        ui.add_space(4.0);
        if self.git_tools.stashes.is_empty() {
            self.empty_state(ui, Icon::Package, pal.text_dim, tr("No stashes: Stash Changes puts the changes of the working tree aside"));
        }
        egui::ScrollArea::vertical().id_salt("stashes").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for s in self.git_tools.stashes.clone() {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H + 2.0), Sense::hover());
                widgets::row_background(ui, &pal, r, false, resp.hovered());
                icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(r.min.x + 14.0, r.center().y - 8.0), Vec2::splat(16.0)), Icon::Package, pal.builtin);
                let end = ui.painter().text(Pos2::new(r.min.x + 40.0, r.center().y), egui::Align2::LEFT_CENTER, &s.message, fonts::ui(13.5), pal.text).max.x;
                ui.painter().text(Pos2::new(end + 10.0, r.center().y), egui::Align2::LEFT_CENTER, format!("{}  ·  {}", s.name, s.when), fonts::ui(12.0), pal.text_dim);
                let mut buttons = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(Pos2::new(r.max.x - 250.0, r.min.y), Pos2::new(r.max.x - 12.0, r.max.y))).layout(Layout::right_to_left(Align::Center)));
                for (verb, label) in [("drop", tr("Drop")), ("pop", tr("Pop")), ("apply", tr("Apply"))] {
                    let b = widgets::compact_button(&mut buttons, &pal, label, verb == "pop");
                    rows.push((format!("stash:{verb}:{}", s.message), b.rect));
                    if b.clicked() {
                        act = Some((verb, s.name.clone()));
                    }
                }
            }
        });
        self.git_tools.rows = rows;
        if new_stash {
            self.popup = Popup::Stash { message: String::new(), untracked: true, just_opened: true };
        }
        if let Some((verb, name)) = act {
            self.stash_do(verb, &name);
        }
    }

    /// The current branch next to another: the commits each has that the other has not, and the files that differ.
    pub fn draw_comparison(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let Some(cmp) = self.git_tools.comparison.clone() else {
            self.empty_state(ui, Icon::Branch, pal.text_dim, tr("Compare the current branch with another: Git > Compare with Branch"));
            return;
        };
        let current = self.git.branch.clone().unwrap_or_else(|| "HEAD".into());
        let mut file = None;
        let mut rows = Vec::new();
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            egui::ScrollArea::vertical().id_salt("compare_commits").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (title, list) in [(i18n::fmt("In {0}, not in {1}", &[&cmp.other, &current]), &cmp.theirs), (i18n::fmt("In {0}, not in {1}", &[&current, &cmp.other]), &cmp.ours)] {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("  {title}  ({})", list.len())).font(fonts::semibold(12.5)).color(pal.text_dim));
                    if list.is_empty() {
                        ui.label(RichText::new(format!("    {}", tr("nothing"))).color(pal.text_dim).size(12.0));
                    }
                    for c in list {
                        let (r, _) = self.commit_row(ui, c);
                        rows.push((format!("compare:{}", c.subject), r));
                    }
                }
            });
            let ui = &mut cols[1];
            ui.add_space(4.0);
            ui.label(RichText::new(i18n::fmt("{} files differ", &[&cmp.files.len()])).font(fonts::semibold(12.5)).color(pal.text_dim));
            egui::ScrollArea::vertical().id_salt("compare_files").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (path, kind) in &cmp.files {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                    widgets::row_background(ui, &pal, r, false, resp.hovered());
                    let color = self.git_color(*kind);
                    ui.painter().text(Pos2::new(r.min.x + 14.0, r.center().y), egui::Align2::LEFT_CENTER, kind.letter(), fonts::semibold(12.5), color);
                    ui.painter().text(Pos2::new(r.min.x + 34.0, r.center().y), egui::Align2::LEFT_CENTER, path, fonts::ui(13.5), color);
                    rows.push((format!("file:{path}"), r));
                    if resp.clicked() {
                        file = Some(path.clone());
                    }
                }
            });
        });
        self.git_tools.rows = rows;
        if let Some(f) = file {
            self.show_compared_file(&f);
        }
    }

    /// The menu of a commit of the log.
    pub fn commit_menu(&mut self, ui: &mut egui::Ui, c: &Commit) -> Option<crate::app_git_tools::AppAction> {
        let pal = self.pal;
        let mut chosen: Option<crate::app_git_tools::AppAction> = None;
        let (hash, subject, short) = (c.hash.clone(), c.subject.clone(), c.short.clone());
        if widgets::menu_row(ui, &pal, Some(Icon::Commit), tr("Cherry-Pick"), "").clicked() {
            let (h, s) = (hash.clone(), subject.clone());
            chosen = Some(Box::new(move |app| app.git_cherry_pick(&h, &s)));
            ui.close();
        }
        if widgets::menu_row(ui, &pal, Some(Icon::Undo), tr("Revert Commit"), "").clicked() {
            let (h, s) = (hash.clone(), subject.clone());
            chosen = Some(Box::new(move |app| app.git_revert(&h, &s)));
            ui.close();
        }
        if widgets::menu_row(ui, &pal, Some(Icon::Merge), tr("Interactive Rebase from Here..."), "").clicked() {
            let (h, label) = (hash.clone(), format!("{short}  {subject}"));
            chosen = Some(Box::new(move |app| app.open_rebase(&h, &label)));
            ui.close();
        }
        ui.separator();
        if widgets::menu_row(ui, &pal, Some(Icon::Copy), tr("Copy Revision Number"), "").clicked() {
            ui.ctx().copy_text(hash);
            ui.close();
        }
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_labels_of_blamed_lines() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(1_762_128_000), "2025-11-03");
        let b = BlameLine { hash: "a".repeat(40), author: "Ann".into(), time: 1_762_128_000, summary: "s".into() };
        assert_eq!(blame_label(&b), "2025-11-03  Ann");
        let new = BlameLine { hash: "0".repeat(40), ..b };
        assert!(new.uncommitted() && blame_label(&new) == "Not committed");
    }
}
