//! Window chrome: menu bar, toolbar, tool strip, editor tabs, breadcrumbs, status bar, toasts.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use ide_core::commands;
use ide_core::editor::LineEnding;

use crate::app::{App, RunConfig};
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::*;
use crate::widgets::{self, Pill, CRUMB_H, STATUS_H, TAB_H};

impl App {
    fn menu_item(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: &str) {
        let Some(c) = commands::by_id(id) else { return };
        // Lisp's commands are there while the Lisp module is on
        if commands::is_lisp_command(id) && !self.lisp_enabled() {
            return;
        }
        let sc = c.shortcuts.first().map(|s| commands::display_shortcut(s)).unwrap_or_default();
        let mut btn = egui::Button::new(tr(c.title));
        if !sc.is_empty() {
            btn = btn.shortcut_text(RichText::new(sc).color(self.pal.text_dim));
        }
        if ui.add(btn).clicked() {
            ui.close();
            self.run_command(ctx, id);
        }
    }

    fn menu_items(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, ids: &[&str]) {
        for id in ids {
            if *id == "-" {
                ui.separator();
            } else {
                self.menu_item(ui, ctx, id);
            }
        }
    }

    pub fn draw_menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.set_min_height(widgets::MENU_H - 6.0);
            let v = ui.visuals_mut();
            v.override_text_color = Some(pal.text);
            for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.fg_stroke = Stroke::new(1.0, pal.text);
            }
            ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.menu_button(tr("File"), |ui| {
                self.menu_items(ui, ctx, &["file.new", "file.new_project", "file.open_project", "file.clone", "file.open_file", "file.reopen_last"]);
                let recent = self.settings.recent_projects.clone();
                ui.menu_button(tr("Recent Projects"), |ui| {
                    if recent.is_empty() {
                        ui.label(RichText::new(tr("No recent projects")).color(pal.text_dim));
                    }
                    for p in recent {
                        if ui.button(&p).clicked() {
                            ui.close();
                            self.request_open_project(std::path::Path::new(&p));
                        }
                    }
                });
                self.menu_items(ui, ctx, &["-", "file.reload", "file.close_tab", "file.close_all", "-", "file.copy_path", "file.reveal", "-", "app.modules", "app.settings", "app.exit"]);
            });
            ui.menu_button(tr("Edit"), |ui| {
                self.menu_items(
                    ui,
                    ctx,
                    &["edit.undo", "edit.redo", "-", "edit.cut", "edit.copy", "edit.paste", "edit.select_all", "-", "edit.extend_selection", "edit.shrink_selection", "edit.duplicate", "edit.delete_line", "code.comment_line", "edit.join_lines", "edit.toggle_case", "-", "edit.fold", "edit.unfold", "edit.fold_all", "edit.unfold_all", "-", "find.find", "find.replace", "find.next", "find.prev", "find.in_files", "find.replace_in_files", "find.usages"],
                );
            });
            ui.menu_button(tr("View"), |ui| {
                self.menu_items(
                    ui,
                    ctx,
                    &["view.project", "view.commit", "view.structure", "view.git", "view.repl", "view.run", "view.problems", "view.terminal", "-", "view.split_right", "view.split_down", "view.unsplit", "view.switch_pane", "-", "view.zoom_in", "view.zoom_out", "view.zoom_reset", "-", "view.preview_code", "view.preview_split", "view.preview_only", "-", "view.toggle_theme", "view.line_numbers", "view.whitespace", "view.rainbow", "view.hide_all"],
                );
            });
            ui.menu_button(tr("Navigate"), |ui| {
                self.menu_items(
                    ui,
                    ctx,
                    &["nav.search_everywhere", "nav.goto_file", "nav.goto_symbol", "nav.goto_line", "nav.recent_files", "nav.file_structure", "-", "nav.declaration", "nav.back", "nav.forward", "nav.matching_bracket", "nav.next_problem", "nav.prev_problem", "nav.last_edit"],
                );
            });
            ui.menu_button(tr("Code"), |ui| {
                self.menu_items(ui, ctx, &["code.complete", "code.quick_doc", "code.param_info", "code.expand_macro", "-", "code.reformat", "code.auto_indent"]);
                if self.lisp_enabled() {
                    ui.separator();
                    ui.menu_button(tr("Structural Editing"), |ui| {
                        for c in commands::COMMANDS.iter().filter(|c| c.id.starts_with("lisp.")) {
                            self.menu_item(ui, ctx, c.id);
                        }
                    });
                }
            });
            ui.menu_button(tr("Refactor"), |ui| self.menu_items(ui, ctx, &["refactor.rename"]));
            ui.menu_button(tr("Git"), |ui| {
                self.menu_items(ui, ctx, &["git.commit", "git.local_commit", "git.reset_base", "git.push", "git.pull", "-", "git.branches", "git.new_branch", "-", "git.diff", "git.rollback", "git.stage_file", "-", "git.refresh", "git.init"]);
            });
            ui.menu_button(tr("Run"), |ui| self.menu_items(ui, ctx, &["run.run_file", "run.compile_file", "run.stop", "-", "run.load_system", "run.test_system", "-", "run.scenarios"]));
            ui.menu_button(tr("Tools"), |ui| self.menu_items(ui, ctx, &["repl.eval", "repl.eval_last", "repl.load_file", "repl.restart", "repl.clear", "repl.focus", "-", "view.terminal"]));
            ui.menu_button(tr("Window"), |ui| self.menu_items(ui, ctx, &["file.next_tab", "file.prev_tab", "file.close_others", "view.hide_all"]));
            ui.menu_button(tr("Help"), |ui| self.menu_items(ui, ctx, &["nav.find_action", "help.keymap", "help.about"]));
        });
    }

    // ------------------------------------------------------------------ toolbar

    pub fn draw_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        ui.horizontal_centered(|ui| {
            ui.add_space(12.0);
            ui.spacing_mut().item_spacing.x = 6.0;

            // ---- project widget
            let name = self.project.as_ref().map_or(tr("No project").to_string(), |p| p.name.clone());
            let initials: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
            let g = ui.painter().layout_no_wrap(name.clone(), fonts::semibold(13.5), pal.text);
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 76.0, 38.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, widgets::radius(), pal.hover);
            }
            let av = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, rect.center().y - 14.0), Vec2::splat(28.0));
            ui.painter().rect_filled(av, CornerRadius::same(8), pal.accent);
            ui.painter().text(av.center(), Align2::CENTER_CENTER, if initials.is_empty() { "?".into() } else { initials }, fonts::semibold(11.5), Color32::WHITE);
            ui.painter().text(Pos2::new(av.max.x + 10.0, rect.center().y), Align2::LEFT_CENTER, &name, fonts::semibold(13.5), pal.text);
            icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(rect.max.x - 22.0, rect.center().y - 6.0), Vec2::splat(12.0)), Icon::ChevronDown, pal.text_dim);
            let recents = self.settings.recent_projects.clone();
            let mut open_other: Option<String> = None;
            let mut cmd: Option<&str> = None;
            egui::Popup::menu(&resp).width(260.0).show(|ui| {
                ui.label(RichText::new(tr("Recent projects")).color(pal.text_dim).size(11.5));
                for p in recents.iter().take(8) {
                    let n = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
                    if ui.button(n).on_hover_text(p).clicked() {
                        open_other = Some(p.clone());
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(tr("Open Project...")).clicked() {
                    cmd = Some("file.open_project");
                    ui.close();
                }
                if ui.button(tr("New Project...")).clicked() {
                    cmd = Some("file.new_project");
                    ui.close();
                }
            });
            if let Some(p) = open_other {
                self.request_open_project(std::path::Path::new(&p));
            }
            if let Some(c) = cmd {
                self.run_command(ctx, c);
            }

            // ---- local commit works for every project, git or not
            if self.project.is_some() && self.git.branch.is_none() {
                ui.add_space(4.0);
                if widgets::icon_button(ui, &pal, Icon::History, 34.0, pal.text_dim, tr("Local Commit (Ctrl+Alt+K)")).clicked() {
                    self.run_command(ctx, "git.local_commit");
                }
            }
            // ---- branch widget and VCS actions
            if let Some(branch) = self.git.branch.clone() {
                let resp = Pill::new(&branch).icon(Icon::Branch, pal.text_dim).chevron().height(34.0).show(ui, &pal);
                let branches = self.git.branches.clone();
                let mut checkout: Option<String> = None;
                let mut new_branch = false;
                egui::Popup::menu(&resp).width(280.0).show(|ui| {
                    if ui.button(tr("New Branch...")).clicked() {
                        new_branch = true;
                        ui.close();
                    }
                    ui.separator();
                    ui.label(RichText::new(tr("Local")).color(pal.text_dim).size(11.5));
                    for b in branches.iter().filter(|b| !b.remote) {
                        let label = if b.current { i18n::fmt("{}   (current)", &[&b.name]) } else { b.name.clone() };
                        if ui.add_enabled(!b.current, egui::Button::new(label)).clicked() {
                            checkout = Some(b.name.clone());
                            ui.close();
                        }
                    }
                    if branches.iter().any(|b| b.remote) {
                        ui.separator();
                        ui.label(RichText::new(tr("Remote")).color(pal.text_dim).size(11.5));
                        for b in branches.iter().filter(|b| b.remote) {
                            if ui.button(&b.name).clicked() {
                                checkout = Some(b.name.clone());
                                ui.close();
                            }
                        }
                    }
                });
                if new_branch {
                    self.popup = Popup::NewBranch { input: String::new(), just_opened: true };
                }
                if let Some(b) = checkout {
                    self.git_checkout(&b);
                }
                if self.git.repo.is_some() {
                    ui.add_space(4.0);
                    if widgets::icon_button(ui, &pal, Icon::ArrowDown, 34.0, pal.text_dim, tr("Update Project (Ctrl+T)")).clicked() {
                        self.run_command(ctx, "git.pull");
                    }
                    let n = self.git.changes.len();
                    let r = widgets::icon_button(ui, &pal, Icon::Commit, 34.0, if n > 0 { pal.accent } else { pal.text_dim }, tr("Commit (Ctrl+K)"));
                    if n > 0 {
                        ui.painter().circle_filled(r.rect.right_top() + Vec2::new(-8.0, 8.0), 3.5, pal.accent);
                    }
                    if r.clicked() {
                        self.run_command(ctx, "git.commit");
                    }
                    if widgets::icon_button(ui, &pal, Icon::History, 34.0, if self.base_is_local() { pal.accent } else { pal.text_dim }, tr("Local Commit (Ctrl+Alt+K)")).clicked() {
                        self.run_command(ctx, "git.local_commit");
                    }
                    if widgets::icon_button(ui, &pal, Icon::ArrowUp, 34.0, pal.text_dim, tr("Push (Ctrl+Shift+K)")).clicked() {
                        self.run_command(ctx, "git.push");
                    }
                    if let Some(b) = &self.git.busy {
                        ui.label(RichText::new(format!("{}...", i18n::tr_dyn(b))).color(pal.warning).size(12.0));
                    }
                }
            }

            // ---- right side
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                if widgets::icon_button(ui, &pal, Icon::Gear, 36.0, pal.text_dim, tr("Settings (Ctrl+Alt+S)")).clicked() {
                    self.run_command(ctx, "app.settings");
                }
                if widgets::icon_button(ui, &pal, Icon::Search, 36.0, pal.text_dim, tr("Search Everywhere (Double Shift)")).clicked() {
                    self.run_command(ctx, "nav.search_everywhere");
                }
                ui.add_space(10.0);
                let running = self.run.running || self.compile.is_some();
                if widgets::icon_button(ui, &pal, Icon::Stop, 36.0, if running { pal.error } else { pal.text_dim.gamma_multiply(0.5) }, tr("Stop (Ctrl+F2)")).clicked() {
                    self.run_command(ctx, "run.stop");
                }
                if widgets::icon_button(ui, &pal, Icon::Check, 36.0, pal.text_dim, tr("Compile current file (Ctrl+F9)")).clicked() {
                    self.run_command(ctx, "run.compile_file");
                }
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, widgets::radius(), pal.hover);
                }
                icons::paint(ui.painter(), r.shrink(10.0), Icon::Play, pal.ok);
                if resp.on_hover_text(tr("Run (Shift+F10)")).clicked() {
                    self.run_command(ctx, "run.run_file");
                }
                let label = self.run_config_label();
                let icon = if matches!(self.run_config, RunConfig::Make(_)) {
                    (Icon::Hammer, pal.accent)
                } else if matches!(self.run_config, RunConfig::Scenario(_)) {
                    (Icon::Script, pal.blue)
                } else if matches!(self.run_config, RunConfig::Load(_) | RunConfig::Test(_)) || self.doc().is_some_and(|d| d.ed.is_lisp()) {
                    (Icon::Lisp, pal.keyword_form)
                } else {
                    (Icon::Play, pal.ok)
                };
                let resp = Pill::new(&label).icon(icon.0, icon.1).chevron().bordered().height(34.0).show(ui, &pal);
                let systems = self.asd_systems();
                let targets = self.make_targets.clone();
                let scenarios: Vec<String> = self.scenarios().iter().map(|s| s.name.clone()).collect();
                let mut edit_scenarios = false;
                // where the project runs: the same for every language
                if resp.clicked() && self.wsl.probe.is_none() && !self.wsl.probing() && self.project.is_some() {
                    self.wsl_probe();
                }
                let linux_fs = self.project_on_linux_fs();
                let distros: Vec<String> = self.wsl.distros().into_iter().filter(|d| crate::app_env::usable_distro(d)).collect();
                let current_env = self.project_distro();
                let mut env_choice: Option<Option<String>> = None;
                let mut chosen: Option<RunConfig> = None;
                egui::Popup::menu(&resp).width(240.0).show(|ui| {
                    if ui.button(tr("Current File")).clicked() {
                        chosen = Some(RunConfig::CurrentFile);
                        ui.close();
                    }
                    if !targets.is_empty() {
                        ui.separator();
                        ui.label(RichText::new(if self.wsl_target().is_some() { tr("make (in Linux)") } else { tr("make") }).color(pal.text_dim).size(11.5));
                        for t in &targets {
                            if ui.button(format!("make {t}")).clicked() {
                                chosen = Some(RunConfig::Make(t.clone()));
                                ui.close();
                            }
                        }
                        if !systems.is_empty() {
                            ui.separator();
                        }
                    }
                    for s in &systems {
                        if ui.button(i18n::fmt("Load system '{}'", &[&s])).clicked() {
                            chosen = Some(RunConfig::Load(s.clone()));
                            ui.close();
                        }
                        if ui.button(i18n::fmt("Test system '{}'", &[&s])).clicked() {
                            chosen = Some(RunConfig::Test(s.clone()));
                            ui.close();
                        }
                    }
                    // console commands one after another
                    ui.separator();
                    ui.label(RichText::new(tr("Scenarios")).color(pal.text_dim).size(11.5));
                    for n in &scenarios {
                        if ui.button(n).clicked() {
                            chosen = Some(RunConfig::Scenario(n.clone()));
                            ui.close();
                        }
                    }
                    if ui.button(if scenarios.is_empty() { tr("Add a scenario...") } else { tr("Edit scenarios...") }).clicked() {
                        edit_scenarios = true;
                        ui.close();
                    }
                    if self.project.is_some() && !linux_fs {
                        ui.separator();
                        ui.label(RichText::new(tr("Run in")).color(pal.text_dim).size(11.5));
                        if ui.selectable_label(current_env.is_none(), tr("On Windows")).clicked() {
                            env_choice = Some(None);
                            ui.close();
                        }
                        for d in &distros {
                            if ui.selectable_label(current_env.as_deref() == Some(d.as_str()), i18n::fmt("In Linux: {}", &[d])).clicked() {
                                env_choice = Some(Some(d.clone()));
                                ui.close();
                            }
                        }
                    }
                });
                if let Some(c) = env_choice {
                    self.set_project_env(c);
                }
                if let Some(c) = chosen {
                    self.run_config = c;
                }
                if edit_scenarios {
                    self.open_scenarios_dialog();
                }
            });
        });
    }

    // ------------------------------------------------------------------ tool strip

    fn strip_button(&mut self, ui: &mut egui::Ui, icon: Icon, active: bool, tip: &str, badge: Option<Color32>) -> bool {
        let pal = self.pal;
        let (r, resp) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::click());
        if active {
            ui.painter().rect_filled(r, CornerRadius::same(14), pal.selected);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(14), pal.hover);
        }
        icons::paint(ui.painter(), r.shrink(10.0), icon, if active { pal.accent } else { pal.text_dim });
        if let Some(c) = badge {
            ui.painter().circle_filled(r.right_top() + Vec2::new(-8.0, 8.0), 4.0, c);
        }
        resp.on_hover_text(tip).clicked()
    }

    pub fn draw_tool_strip(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            let commit_badge = (!self.git.changes.is_empty()).then_some(self.pal.accent);
            for (icon, tool, tip, badge) in [
                (Icon::Project, LeftTool::Project, tr("Project (Alt+1)"), None),
                (Icon::Commit, LeftTool::Commit, tr("Commit (Alt+0)"), commit_badge),
                (Icon::List, LeftTool::Structure, tr("Structure (Alt+7)"), None),
            ] {
                if self.strip_button(ui, icon, self.left_tool == Some(tool), tip, badge) {
                    self.toggle_left(tool);
                }
            }
            // the modules: a page of its own, with a dot when the open project asks for some that are not on
            let on_page = self.doc().is_some_and(|d| d.page.is_some());
            let badge = (!self.needed_modules.is_empty()).then_some(self.pal.accent);
            if self.strip_button(ui, Icon::Puzzle, on_page, tr("Modules (Ctrl+Alt+P)"), badge) {
                self.open_modules_tab();
            }
        });
        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            let errors = self.all_problems().iter().filter(|p| p.sev == Sev::Error).count();
            let pal = self.pal;
            let tools = [
                (Icon::Search, BottomTool::Find, tr("Find (Ctrl+Shift+F)"), None),
                (Icon::Warning, BottomTool::Problems, tr("Problems (Alt+6)"), (errors > 0).then_some(pal.error)),
                (Icon::Terminal, BottomTool::Terminal, tr("Terminal (Alt+F12)"), None),
                (Icon::Play, BottomTool::Run, tr("Run (Alt+4)"), None),
                (Icon::Eye, BottomTool::Repl, tr("REPL (Alt+8)"), self.repl.ready.then_some(pal.ok)),
                (Icon::History, BottomTool::Git, tr("Git (Alt+9)"), None),
            ];
            let lisp = self.lisp_enabled();
            for (icon, tool, tip, badge) in tools.into_iter().filter(|t| lisp || t.1 != BottomTool::Repl) {
                if self.strip_button(ui, icon, self.bottom_tool == Some(tool), tip, badge) {
                    self.toggle_bottom(tool);
                }
            }
        });
    }

    // ------------------------------------------------------------------ tabs

    pub fn draw_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let (strip, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), TAB_H), Sense::hover());
        let font = fonts::ui(13.5);
        let mut x = strip.min.x + 12.0;
        let mut to_close: Option<usize> = None;
        let mut to_activate: Option<usize> = None;
        let names: Vec<(String, bool, bool)> = self.docs.iter().map(|d| (d.name(), d.ed.is_modified() && d.path.is_none() && d.diff.is_none(), d.conflict)).collect();
        let other_pane = self.split_other_index();
        let is_split = self.split.is_some();
        let mut split_request: Option<(usize, Option<bool>)> = None;
        let problems = self.all_problems();
        for (i, (name, modified, conflict)) in names.iter().enumerate() {
            let galley = ui.painter().layout_no_wrap(name.clone(), font.clone(), pal.text);
            let w = 14.0 + 18.0 + 8.0 + galley.size().x + 10.0 + 22.0 + 8.0;
            let r = Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(w, TAB_H));
            let resp = ui.interact(r, ui.scope_id().with(("tab", i)), Sense::click());
            let active = i == self.active;
            let hit = r.shrink2(Vec2::new(0.0, 8.0));
            if active {
                ui.painter().rect_filled(hit, CornerRadius::same(12), pal.tab_active_bg);
            } else if other_pane == Some(i) {
                // the document of the other pane of a split
                ui.painter().rect_filled(hit, CornerRadius::same(12), pal.tab_active_bg.gamma_multiply(0.5));
            } else if resp.hovered() {
                ui.painter().rect_filled(hit, CornerRadius::same(12), pal.hover);
            }
            let ir = Rect::from_min_size(Pos2::new(r.min.x + 14.0, r.center().y - 8.0), Vec2::splat(16.0));
            let is_diff = self.docs[i].diff.is_some();
            let is_lisp = !is_diff && self.docs[i].ed.is_lisp();
            let (tab_icon, tab_icon_color) = if self.docs[i].page.is_some() {
                (Icon::Puzzle, pal.accent)
            } else if is_diff {
                (Icon::FileDiff, pal.accent)
            } else if is_lisp {
                (Icon::Lisp, pal.keyword_form)
            } else {
                (Icon::File, pal.text_dim)
            };
            icons::paint(ui.painter(), ir, tab_icon, tab_icon_color);
            let has_err = problems.iter().any(|p| p.path == self.docs[i].path && p.path.is_some() && p.sev == Sev::Error);
            let git_color = self.docs[i].path.as_ref().and_then(|p| self.git_change_of(p)).map(|k| self.git_color(k));
            let text_color = if has_err { pal.error } else if let Some(c) = git_color { c } else if active { pal.text } else { pal.text_dim };
            ui.painter().text(Pos2::new(r.min.x + 40.0, r.center().y), Align2::LEFT_CENTER, name, font.clone(), text_color);
            let cr = Rect::from_center_size(Pos2::new(r.max.x - 20.0, r.center().y), Vec2::splat(20.0));
            let close_resp = ui.interact(cr, ui.scope_id().with(("tabclose", i)), Sense::click());
            if *modified || *conflict {
                if close_resp.hovered() {
                    ui.painter().rect_filled(cr, CornerRadius::same(5), pal.hover);
                    icons::paint(ui.painter(), cr.shrink(5.0), Icon::Close, pal.text);
                } else {
                    ui.painter().circle_filled(cr.center(), 4.0, if *conflict { pal.warning } else { pal.modified });
                }
            } else if resp.hovered() || active || close_resp.hovered() {
                if close_resp.hovered() {
                    ui.painter().rect_filled(cr, CornerRadius::same(5), pal.selected);
                }
                icons::paint(ui.painter(), cr.shrink(5.0), Icon::Close, if close_resp.hovered() { pal.text } else { pal.text_dim });
            }
            if close_resp.clicked() || resp.middle_clicked() {
                to_close = Some(i);
            } else if resp.clicked() {
                to_activate = Some(i);
            }
            resp.context_menu(|ui| {
                if ui.button(tr("Close")).clicked() {
                    to_close = Some(i);
                    ui.close();
                }
                if ui.button(tr("Close Other Tabs")).clicked() {
                    to_activate = Some(i);
                    self.pending_close_others = true;
                    ui.close();
                }
                ui.separator();
                if ui.button(tr("Split Right")).clicked() {
                    split_request = Some((i, Some(true)));
                    ui.close();
                }
                if ui.button(tr("Split Down")).clicked() {
                    split_request = Some((i, Some(false)));
                    ui.close();
                }
                if is_split && ui.button(tr("Unsplit")).clicked() {
                    split_request = Some((i, None));
                    ui.close();
                }
                ui.separator();
                if ui.button(tr("Copy Path")).clicked() {
                    if let Some(p) = &self.docs[i].path {
                        ctx.copy_text(p.to_string_lossy().into_owned());
                    }
                    ui.close();
                }
                if ui.button(tr("Reveal in File Manager")).clicked() {
                    if let Some(p) = self.docs[i].path.clone() {
                        crate::input::reveal_in_file_manager(&p);
                    }
                    ui.close();
                }
            });
            x += w + 2.0;
            if x > strip.max.x {
                break;
            }
        }
        if let Some(i) = to_activate {
            self.show_doc(i);
            self.editor_focus = true;
        }
        match split_request {
            Some((i, Some(vertical))) => {
                self.show_doc(i);
                self.split_editor(vertical);
            }
            Some((_, None)) => self.unsplit(),
            None => {}
        }
        if self.pending_close_others {
            self.pending_close_others = false;
            self.run_command(ctx, "file.close_others");
        }
        if let Some(i) = to_close {
            self.request_close_doc(i);
        }
    }

    pub fn git_color(&self, k: ide_core::git::ChangeKind) -> Color32 {
        use ide_core::git::ChangeKind::*;
        let p = &self.pal;
        match k {
            Modified | Renamed => p.git_modified,
            Added => p.git_added,
            Untracked => p.git_untracked,
            Deleted => p.text_dim,
            Conflicted => p.error,
        }
    }

    // ------------------------------------------------------------------ breadcrumbs

    pub fn draw_breadcrumbs(&mut self, ui: &mut egui::Ui, idx: usize) {
        let pal = self.pal;
        let def_label = self.docs.get_mut(idx).and_then(|d| {
            let caret = d.ed.caret();
            lisp_syntax::outline::definition_at(d.defs(), caret).map(|def| format!("{} {}", def.operator, def.name))
        });
        let Some(d) = self.docs.get(idx) else { return };
        let mut parts: Vec<String> = Vec::new();
        match (&d.path, self.project_root()) {
            (Some(p), Some(root)) if p.starts_with(root) => {
                parts.push(self.project.as_ref().map(|x| x.name.clone()).unwrap_or_default());
                parts.extend(ide_core::workspace::relative(root, p).split('/').map(str::to_string));
            }
            (Some(p), _) => parts.push(p.to_string_lossy().into_owned()),
            (None, _) => parts.push(d.name()),
        }
        if let Some(l) = def_label {
            parts.push(l);
        }
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), CRUMB_H), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::ZERO, pal.editor_bg);
        let mut x = r.min.x + 16.0;
        let font = fonts::ui(12.0);
        let n = parts.len();
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == n;
            let g = ui.painter().layout_no_wrap(part.clone(), font.clone(), if last { pal.text } else { pal.text_dim });
            ui.painter().galley(Pos2::new(x, r.center().y - g.size().y / 2.0), g.clone(), pal.text);
            x += g.size().x;
            if !last {
                icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(x + 4.0, r.center().y - 5.0), Vec2::splat(10.0)), Icon::ChevronRight, pal.text_dim);
                x += 20.0;
            }
        }
        self.draw_preview_switch(ui, r, idx);
    }

    // ------------------------------------------------------------------ status bar

    pub fn draw_status_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        ui.horizontal_centered(|ui| {
            ui.add_space(14.0);
            if let Some(b) = self.git.branch.clone() {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Branch, pal.text_dim);
                ui.label(RichText::new(b).color(pal.text_dim).font(fonts::ui(12.0)));
                ui.add_space(8.0);
            }
            if self.base_is_local() {
                ui.label(RichText::new(i18n::fmt("diff vs {}", &[&self.base_label()])).color(pal.accent).font(fonts::ui(12.0)));
                ui.add_space(8.0);
            }
            if !self.status.is_empty() {
                ui.label(RichText::new(self.status.clone()).color(pal.text_dim).font(fonts::ui(12.0)));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                ui.spacing_mut().item_spacing.x = 18.0;
                let small = |t: String| RichText::new(t).color(pal.text_dim).font(fonts::ui(12.0));
                let info = self.doc().filter(|d| d.diff.is_none()).map(|d| {
                    let (line, col) = d.ed.line_col(d.ed.caret());
                    let sel = if d.ed.has_selection() { format!(" ({} chars)", d.ed.selected_text().chars().count()) } else { String::new() };
                    (format!("{}:{}{}", line + 1, col + 1, sel), d.ed.language().map_or(d.ed.dialect().name(), |l| l.name()).to_string(), d.ed.line_ending)
                });
                if let Some((pos, dialect, eol)) = info {
                    if ui.add(egui::Label::new(small(pos)).sense(Sense::click())).on_hover_text(tr("Go to Line (Ctrl+G)")).clicked() {
                        self.run_command(ctx, "nav.goto_line");
                    }
                    ui.label(small(dialect));
                    ui.label(small(tr("2 spaces").into()));
                    ui.label(small("UTF-8".into()));
                    // the line endings the file is written with, with a menu to change them (as in VS Code)
                    let resp = ui.add(egui::Label::new(small(eol.label().into())).sense(Sense::click())).on_hover_text(tr("Line endings of the file (LF or CRLF) - click to change"));
                    let mut pick = None;
                    egui::Popup::menu(&resp).show(|ui| {
                        for (le, id) in [(LineEnding::Lf, "edit.eol_lf"), (LineEnding::CrLf, "edit.eol_crlf")] {
                            if ui.selectable_label(eol == le, le.label()).clicked() {
                                pick = Some(id);
                                ui.close();
                            }
                        }
                    });
                    if let Some(id) = pick {
                        self.run_command(ctx, id);
                    }
                }
                if self.lisp_enabled() && (self.doc().is_some_and(|d| d.ed.is_lisp()) || self.repl.session.is_some()) {
                let (txt, color) = if self.repl.starting {
                    (tr("REPL starting...").to_string(), pal.warning)
                } else if self.repl.ready {
                    let busy = if self.repl.busy.is_empty() { "" } else { tr(" (busy)") };
                    (format!("{}{}", self.repl.package.to_lowercase(), busy), pal.ok)
                } else if self.lisp.is_none() {
                    (tr("No Lisp found").to_string(), pal.error)
                } else {
                    (tr("REPL off").to_string(), pal.text_dim)
                };
                let resp = ui.add(egui::Label::new(small(txt)).sense(Sense::click())).on_hover_text(tr("Toggle REPL (Alt+8)"));
                let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, color);
                if resp.clicked() {
                    self.run_command(ctx, "view.repl");
                }
                }
                // where the project is built and run, with a menu to switch it
                self.draw_env_chip(ui);
            });
        });
    }

    // ------------------------------------------------------------------ toasts

    pub fn draw_toasts(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let screen = ctx.content_rect();
        let mut y = screen.max.y - STATUS_H - 16.0;
        for (i, t) in self.toasts.iter().enumerate().rev() {
            let (icon, color) = match t.sev {
                Sev::Error => (Icon::Error, pal.error),
                Sev::Warning => (Icon::Warning, pal.warning),
                Sev::Weak => (Icon::Info, pal.accent),
            };
            let font = fonts::ui(13.5);
            let text = elide(&t.text, 80);
            let g = ctx.fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), pal.text));
            let w = (g.size().x + 68.0).min(620.0);
            let h = 48.0;
            let rect = Rect::from_min_size(Pos2::new(screen.max.x - w - 20.0, y - h), Vec2::new(w, h));
            egui::Area::new(egui::Id::unique(("toast", i))).fixed_pos(rect.min).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(12), pal.popup_bg);
                p.rect_filled(Rect::from_min_size(rect.min + Vec2::new(0.0, 10.0), Vec2::new(3.0, h - 20.0)), CornerRadius::same(2), color);
                icons::paint(p, Rect::from_min_size(rect.min + Vec2::new(18.0, 16.0), Vec2::splat(16.0)), icon, color);
                p.text(Pos2::new(rect.min.x + 46.0, rect.center().y), Align2::LEFT_CENTER, text, font, pal.text);
                ui.allocate_rect(rect, Sense::hover());
            });
            y -= h + 10.0;
        }
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
}

pub fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}...")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elide_keeps_short_and_trims_long() {
        assert_eq!(elide("abc", 5), "abc");
        assert_eq!(elide("abcdefghij", 5), "abcd...");
        assert_eq!(elide("жукжукжук", 4), "жук...");
    }
}
