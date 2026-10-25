//! The Commit tool window (docked on the left, like in PyCharm): the changed files as a checkable tree, the
//! commit message and the Commit buttons. Clicking a file shows its diff in the editor area.

use eframe::egui::{self, Align2, CornerRadius, Key, Pos2, Rect, RichText, Sense, Vec2, pos2, vec2};
use ide_core::changes::{self, Check, Group, Row};
use ide_core::git::ChangeKind;
use ide_core::localvcs::Base;

use crate::app::App;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::*;
use crate::widgets::{self, ROW_H};

/// Height reserved below the changes list: message box, buttons and spacing.
const BOTTOM_H: f32 = 210.0;
const MESSAGE_H: f32 = 86.0;

/// Everything the user did in the list during one frame; applied after drawing.
#[derive(Default)]
struct ListActions {
    toggle: Vec<Vec<String>>,
    collapse: Vec<String>,
    diff: Option<String>,
    open: Option<String>,
    rollback: Option<String>,
    stage: Option<(String, bool)>,
}

impl App {
    pub fn draw_commit_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        if self.project.is_none() {
            self.empty_state(ui, Icon::Branch, pal.text_dim, tr("Open a project to use version control"));
            return;
        }
        ui.add_space(10.0);
        self.draw_commit_header(ui, ctx);
        ui.add_space(6.0);
        if self.git.repo.is_none() && !self.base_is_local() {
            self.empty_state(ui, Icon::Branch, pal.text_dim, tr("This project is not a Git repository. Local commits work without git."));
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                if self.git.available && widgets::primary_button(ui, &pal, tr("Create Git Repository")).clicked() {
                    self.run_command(ctx, "git.init");
                }
                ui.add_space(6.0);
                if widgets::secondary_button(ui, &pal, tr("Make a Local Commit")).clicked() {
                    self.popup = Popup::LocalCommit { input: String::new(), just_opened: true };
                }
            });
            return;
        }
        let list_h = (ui.available_height() - BOTTOM_H).max(0.0);
        let actions = ui.allocate_ui(vec2(ui.available_width(), list_h), |ui| self.draw_change_tree(ui)).inner;
        self.apply_list_actions(actions);
        self.draw_commit_form(ui, ctx);
    }

    fn draw_commit_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let mut set_base: Option<Base> = None;
        let mut local = false;
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new(tr("Commit")).font(fonts::semibold(15.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                if widgets::icon_button(ui, &pal, Icon::Refresh, 28.0, pal.text_dim, tr("Refresh")).clicked() {
                    self.run_command(ctx, "git.refresh");
                }
                let (icon, tip) = if self.git.by_dir { (Icon::List, tr("Show files as a flat list")) } else { (Icon::Folder, tr("Group files by directory")) };
                if widgets::icon_button(ui, &pal, icon, 28.0, pal.text_dim, tip).clicked() {
                    self.git.by_dir = !self.git.by_dir;
                }
            });
        });
        if let Some(b) = self.git.branch.clone() {
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Branch, pal.text_dim);
                ui.add(egui::Label::new(RichText::new(&b).color(pal.text_dim).size(12.5)).truncate()).on_hover_text(b);
            });
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.add_space(12.0);
            let label = crate::ui_chrome::elide(&self.base_label(), 22);
            let resp = widgets::Pill::new(&label).icon(Icon::History, pal.text_dim).chevron().bordered().height(28.0).show(ui, &pal);
            let commits: Vec<_> = self.git.local.as_ref().map(|l| l.commits().iter().rev().map(|c| (c.id, c.message.clone())).collect()).unwrap_or_default();
            egui::Popup::menu(&resp).width(300.0).show(|ui| {
                ui.label(RichText::new(tr("Compare with")).color(pal.text_dim).size(11.5));
                if ui.button(tr("HEAD (git)")).clicked() {
                    set_base = Some(Base::Head);
                    ui.close();
                }
                if !commits.is_empty() {
                    ui.separator();
                    ui.label(RichText::new(tr("Local commits")).color(pal.text_dim).size(11.5));
                }
                for (id, msg) in commits {
                    if ui.button(format!("#{id}  {}", crate::ui_chrome::elide(&msg, 36))).clicked() {
                        set_base = Some(Base::Local(id));
                        ui.close();
                    }
                }
            });
            if widgets::secondary_button(ui, &pal, tr("Local Commit")).on_hover_text(tr("Snapshot outside of git; the diff starts from it")).clicked() {
                local = true;
            }
        });
        if local {
            self.popup = Popup::LocalCommit { input: String::new(), just_opened: true };
        }
        if let Some(b) = set_base {
            self.local_set_base(b);
        }
    }

    /// The scrolling tree of changes. Returns what the user did.
    fn draw_change_tree(&mut self, ui: &mut egui::Ui) -> ListActions {
        let pal = self.pal;
        let mut act = ListActions::default();
        if self.git.changes.is_empty() {
            let msg = if self.base_is_local() { i18n::fmt("No changes since {}", &[&self.base_label()]) } else { tr("No local changes").to_string() };
            self.empty_state(ui, Icon::Check, pal.ok, &msg);
            return act;
        }
        if self.git.changes_truncated {
            ui.label(RichText::new(format!("   {}", i18n::fmt("Showing the first {} changes only", &[&self.git.changes.len()]))).color(pal.warning).size(12.0));
        }
        let rows = changes::rows(&self.git.changes, self.git.by_dir, &self.git.collapsed);
        self.commit_rects.clear();
        let mut rects: Vec<(String, Rect, Rect)> = vec![];
        let current = self.diff_current_rel().map(str::to_string);
        let root_name = self.project.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().scroll.floating = false;
        egui::ScrollArea::vertical().id_salt("commit_changes").auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let cy = r.center().y;
                match row {
                    Row::Group { group, count, collapsed } => {
                        widgets::row_background(ui, &pal, r, false, resp.hovered());
                        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 22.0, cy), Vec2::splat(14.0)), if *collapsed { Icon::ChevronRight } else { Icon::ChevronDown }, pal.text_dim);
                        let paths = changes::paths_in(&self.git.changes, *group, None);
                        let cb = Rect::from_center_size(pos2(r.min.x + 46.0, cy), Vec2::splat(16.0));
                        widgets::checkbox_state(ui.painter(), cb, changes::check_state(&self.git.checked, &paths), &pal);
                        let title = match group {
                            Group::Changes => tr("Changes"),
                            Group::Unversioned => tr("Unversioned Files"),
                        };
                        let g = ui.painter().layout_no_wrap(title.to_string(), fonts::semibold(13.5), pal.text);
                        let gw = g.size().x;
                        ui.painter().galley(pos2(r.min.x + 66.0, cy - g.size().y / 2.0), g, pal.text);
                        ui.painter().text(pos2(r.min.x + 76.0 + gw, cy), Align2::LEFT_CENTER, count.to_string(), fonts::ui(12.5), pal.text_dim);
                        if resp.clicked() {
                            if hit_checkbox(&resp, cb) {
                                act.toggle.push(paths.iter().map(|p| p.to_string()).collect());
                            } else {
                                act.collapse.push(changes::collapse_key(*group, None));
                            }
                        }
                    }
                    Row::Dir { group, dir, count, collapsed } => {
                        widgets::row_background(ui, &pal, r, false, resp.hovered());
                        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 34.0, cy), Vec2::splat(14.0)), if *collapsed { Icon::ChevronRight } else { Icon::ChevronDown }, pal.text_dim);
                        let paths = changes::paths_in(&self.git.changes, *group, Some(dir));
                        let cb = Rect::from_center_size(pos2(r.min.x + 58.0, cy), Vec2::splat(16.0));
                        widgets::checkbox_state(ui.painter(), cb, changes::check_state(&self.git.checked, &paths), &pal);
                        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 82.0, cy), Vec2::splat(16.0)), if *collapsed { Icon::Folder } else { Icon::FolderOpen }, pal.text_dim);
                        let name = if dir.is_empty() { root_name.clone() } else { dir.clone() };
                        let g = ui.painter().layout_no_wrap(name, fonts::ui(13.5), pal.text);
                        let gw = g.size().x.min(r.width() - 150.0).max(10.0);
                        ui.painter().with_clip_rect(r).galley(pos2(r.min.x + 98.0, cy - g.size().y / 2.0), g, pal.text);
                        ui.painter().text(pos2(r.min.x + 108.0 + gw, cy), Align2::LEFT_CENTER, count.to_string(), fonts::ui(12.0), pal.text_dim);
                        if resp.clicked() {
                            if hit_checkbox(&resp, cb) {
                                act.toggle.push(paths.iter().map(|p| p.to_string()).collect());
                            } else {
                                act.collapse.push(changes::collapse_key(*group, Some(dir)));
                            }
                        }
                    }
                    Row::File { index, nested } => {
                        let c = &self.git.changes[*index];
                        let selected = current.as_deref() == Some(c.path.as_str());
                        widgets::row_background(ui, &pal, r, selected, resp.hovered());
                        let x0 = r.min.x + if *nested { 38.0 } else { 14.0 };
                        let cb = Rect::from_center_size(pos2(x0 + 30.0, cy), Vec2::splat(16.0));
                        rects.push((c.path.clone(), r, cb));
                        widgets::checkbox_state(ui.painter(), cb, if self.git.checked.contains(&c.path) { Check::On } else { Check::Off }, &pal);
                        let col = self.git_color(c.kind);
                        let br = Rect::from_center_size(pos2(x0 + 56.0, cy), Vec2::splat(18.0));
                        ui.painter().rect_filled(br, CornerRadius::same(5), col.gamma_multiply(0.2));
                        ui.painter().text(br.center(), Align2::CENTER_CENTER, c.kind.letter(), fonts::semibold(11.0), col);
                        let shown = c.path.trim_end_matches('/');
                        let (name, dir) = match shown.rsplit_once('/') {
                            Some((d, n)) => (format!("{n}{}", if c.path.ends_with('/') { "/" } else { "" }), d.to_string()),
                            None => (c.path.clone(), String::new()),
                        };
                        let g = ui.painter().layout_no_wrap(name, fonts::ui(13.5), col);
                        let gw = g.size().x;
                        let clip = ui.painter().with_clip_rect(r.shrink2(vec2(6.0, 0.0)));
                        clip.galley(pos2(x0 + 74.0, cy - g.size().y / 2.0), g, col);
                        if !*nested && !dir.is_empty() {
                            clip.text(pos2(x0 + 84.0 + gw, cy), Align2::LEFT_CENTER, dir, fonts::ui(12.0), pal.text_dim);
                        }
                        if resp.clicked() {
                            if hit_checkbox(&resp, cb) {
                                act.toggle.push(vec![c.path.clone()]);
                            } else {
                                act.diff = Some(c.path.clone());
                            }
                        }
                        if resp.double_clicked() && !hit_checkbox(&resp, cb) && c.kind != ChangeKind::Deleted {
                            act.open = Some(c.path.clone());
                        }
                        resp.context_menu(|ui| {
                            if ui.button(tr("Show Diff")).clicked() {
                                act.diff = Some(c.path.clone());
                                ui.close();
                            }
                            if c.kind != ChangeKind::Deleted && ui.button(tr("Open File")).clicked() {
                                act.open = Some(c.path.clone());
                                ui.close();
                            }
                            if self.git.repo.is_some() && !self.base_is_local() && ui.button(if c.staged { tr("Unstage") } else { tr("Stage") }).clicked() {
                                act.stage = Some((c.path.clone(), !c.staged));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(tr("Rollback...")).clicked() {
                                act.rollback = Some(c.path.clone());
                                ui.close();
                            }
                        });
                    }
                }
            }
        });
        self.commit_rects = rects;
        act
    }

    fn apply_list_actions(&mut self, act: ListActions) {
        for paths in act.toggle {
            let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
            changes::toggle_all(&mut self.git.checked, &refs);
        }
        for key in act.collapse {
            if !self.git.collapsed.remove(&key) {
                self.git.collapsed.insert(key);
            }
        }
        if let Some(rel) = act.diff {
            self.open_diff_tab(&rel);
        }
        if let Some(rel) = act.open
            && let Some(root) = self.vcs_root()
        {
            self.open_file(&root.join(&rel));
        }
        if let Some(rel) = act.rollback {
            self.popup = Popup::Confirm { kind: ConfirmKind::Rollback(rel.clone()), message: i18n::fmt("Discard all local changes to '{}'? This cannot be undone.", &[&rel]) };
        }
        if let Some((rel, on)) = act.stage {
            self.git_run_sync(if on { "Stage" } else { "Unstage" }, move |r| if on { r.stage(&[rel]).map(|_| String::new()) } else { r.unstage(&[rel]).map(|_| String::new()) });
        }
    }

    fn draw_commit_form(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let msg_id = egui::Id::unique("git_commit_msg");
        ui.add_space(6.0);
        let w = ui.available_width() - 24.0;
        let mut commit = false;
        let mut commit_push = false;
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.set_width(w);
                // Ctrl+Enter in the message box commits, as in PyCharm; swallow it before the text edit inserts a newline
                if ctx.memory(|m| m.has_focus(msg_id)) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter)) {
                    commit = true;
                }
                let mut msg = std::mem::take(&mut self.git.commit_msg);
                let r = widgets::text_area(ui, &pal, &mut msg, tr("Commit message"), MESSAGE_H, Some(msg_id));
                if self.git.focus_msg {
                    r.request_focus();
                    self.git.focus_msg = false;
                }
                if r.clicked() || r.gained_focus() {
                    self.editor_focus = false;
                }
                self.git.commit_msg = msg;
                ui.add_space(8.0);
                let sel = self.git.checked.len();
                let can = sel > 0 && !self.git.commit_msg.trim().is_empty() && self.git.repo.is_some();
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(can, |ui| {
                        if widgets::primary_button(ui, &pal, &i18n::fmt("Commit ({})", &[&sel])).clicked() {
                            commit = true;
                        }
                        if widgets::secondary_button(ui, &pal, tr("Commit and Push")).clicked() {
                            commit_push = true;
                        }
                    });
                });
                if self.git.repo.is_none() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr("Not a Git repository: use Local Commit")).color(pal.text_dim).size(11.5));
                }
            });
        });
        let can = !self.git.checked.is_empty() && !self.git.commit_msg.trim().is_empty() && self.git.repo.is_some();
        if (commit || commit_push) && can && self.git_commit_selected() && commit_push {
            self.git_run_async("Push", |r| r.push());
        }
    }
}

/// Did the click land on the checkbox (with a generous hit area) rather than on the row?
fn hit_checkbox(resp: &egui::Response, cb: Rect) -> bool {
    resp.interact_pointer_pos().is_some_and(|p: Pos2| cb.expand(7.0).contains(p))
}
