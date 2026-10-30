//! Popups and dialogs: Search Everywhere & co, input dialogs, commit, diff, settings, completion,
//! find bar and the welcome screen.

use crate::scrollbars::FadingScroll as _;
use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Key, Margin, Pos2, Rect, RichText, Sense, Vec2};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::Icon;
use crate::state::*;
use crate::ui_panels::badge_color;
use crate::widgets::{self, TextField};

pub(crate) fn popup_frame(pal: &crate::theme::Palette) -> egui::Frame {
    widgets::glass_frame(pal)
}

fn highlighted(text: &str, hits: &[usize], font: FontId, base: Color32, hit: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut buf = String::new();
    let mut cur = false;
    for (i, ch) in text.chars().enumerate() {
        let is_hit = hits.contains(&i);
        if is_hit != cur && !buf.is_empty() {
            job.append(&buf, 0.0, egui::TextFormat::simple(font.clone(), if cur { hit } else { base }));
            buf.clear();
        }
        cur = is_hit;
        buf.push(ch);
    }
    if !buf.is_empty() {
        job.append(&buf, 0.0, egui::TextFormat::simple(font, if cur { hit } else { base }));
    }
    job
}

enum PaletteAction {
    None,
    Close,
    Run(Item),
}

impl App {
    pub fn draw_popups(&mut self, ctx: &egui::Context) {
        if self.popup.is_open() && !matches!(self.popup, Popup::Palette(_)) {
            widgets::scrim(ctx, &self.pal);
        }
        self.draw_palette(ctx);
        self.draw_clone(ctx);
        self.draw_project_browser(ctx);
        self.draw_input_dialogs(ctx);
        self.draw_scenarios(ctx);
        self.draw_local_view(ctx);
        self.draw_settings(ctx);
        self.draw_info_dialogs(ctx);
        self.draw_completion(ctx);
        self.draw_hint(ctx);
        self.draw_hover(ctx);
        self.draw_hunk_popup(ctx);
    }

    // ------------------------------------------------------------------ palette

    fn draw_palette(&mut self, ctx: &egui::Context) {
        if !matches!(self.popup, Popup::Palette(_)) {
            return;
        }
        self.tick_palette();
        let Popup::Palette(mut st) = std::mem::replace(&mut self.popup, Popup::None) else { return };
        let pal = self.pal;
        let mut action = PaletteAction::None;
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.5).clamp(560.0, 780.0);
        let (esc, down, up, enter, tab) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::Enter), i.key_pressed(Key::Tab)));
        if esc {
            action = PaletteAction::Close;
        }
        let n = st.items.len();
        let mut moved = false;
        if down && n > 0 {
            st.selected = (st.selected + 1) % n;
            moved = true;
        }
        if up && n > 0 {
            st.selected = (st.selected + n - 1) % n;
            moved = true;
        }
        if tab && matches!(st.mode, PaletteMode::Everywhere | PaletteMode::Files | PaletteMode::Symbols | PaletteMode::Actions) {
            st.mode = match st.mode {
                PaletteMode::Everywhere => PaletteMode::Files,
                PaletteMode::Files => PaletteMode::Symbols,
                PaletteMode::Symbols => PaletteMode::Actions,
                _ => PaletteMode::Everywhere,
            };
        }
        if enter && let Some(it) = st.items.get(st.selected) {
            action = PaletteAction::Run(it.clone());
        }
        ctx.input_mut(|i| {
            for k in [Key::ArrowDown, Key::ArrowUp, Key::Enter, Key::Tab] {
                i.consume_key(egui::Modifiers::NONE, k);
            }
        });
        let mut outside_rect = Rect::NOTHING;
        egui::Area::new(egui::Id::unique("palette")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            popup_frame(&pal).show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    if matches!(st.mode, PaletteMode::Everywhere | PaletteMode::Files | PaletteMode::Symbols | PaletteMode::Actions) {
                        for (m, label) in [(PaletteMode::Everywhere, tr("All")), (PaletteMode::Files, tr("Files")), (PaletteMode::Symbols, tr("Symbols")), (PaletteMode::Actions, tr("Actions"))] {
                            let active = st.mode == m;
                            let g = ui.painter().layout_no_wrap(label.to_string(), fonts::ui(13.5), pal.text);
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 24.0, 28.0), Sense::click());
                            if active {
                                ui.painter().rect_filled(r, CornerRadius::same(8), pal.selected);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(r, CornerRadius::same(8), pal.hover);
                            }
                            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, if active { fonts::semibold(13.5) } else { fonts::ui(13.5) }, if active { pal.text } else { pal.text_dim });
                            if resp.clicked() {
                                st.mode = m;
                            }
                        }
                    } else {
                        ui.label(RichText::new(tr(st.mode.title())).font(fonts::semibold(15.0)));
                    }
                });
                ui.add_space(10.0);
                let hint = match st.mode {
                    PaletteMode::Everywhere => tr("Search everywhere: files, symbols, actions"),
                    PaletteMode::Actions => tr("Type an action name"),
                    PaletteMode::Files => tr("Type a file name"),
                    PaletteMode::Symbols => tr("Type a symbol name"),
                    PaletteMode::Recent => tr("Filter recent files"),
                    PaletteMode::Structure => tr("Filter definitions"),
                    PaletteMode::Choose => tr("Filter"),
                };
                let resp = TextField::new(&mut st.query).id(egui::Id::unique("palette_query")).hint(hint).icon(Icon::Search).font(fonts::ui(15.0)).height(42.0).show(ui, &pal);
                widgets::focus_once(ui, &resp, &mut st.just_opened);
                ui.add_space(8.0);
                let row_h = 38.0;
                let list_h = (n as f32 * row_h + 50.0).clamp(70.0, 420.0);
                if n == 0 {
                    ui.add_space(10.0);
                    ui.label(RichText::new(if st.query.is_empty() { tr("Start typing...") } else { tr("Nothing found") }).color(pal.text_dim));
                    ui.add_space(10.0);
                }
                egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show_fading(ui, |ui| {
                    let mut last_section = "";
                    for (i, it) in st.items.iter().enumerate() {
                        if !it.section.is_empty() && it.section != last_section {
                            last_section = it.section;
                            ui.add_space(6.0);
                            widgets::caption(ui, &pal, &tr(it.section).to_uppercase());
                            ui.add_space(2.0);
                        }
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                        if i == st.selected {
                            ui.painter().rect_filled(r, CornerRadius::same(9), pal.selected);
                            if moved {
                                resp.scroll_to_me(None);
                            }
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, CornerRadius::same(9), pal.hover);
                        }
                        let bc = badge_color(self, &it.badge);
                        let br = Rect::from_min_size(Pos2::new(r.min.x + 10.0, r.center().y - 12.0), Vec2::splat(24.0));
                        ui.painter().rect_filled(br, CornerRadius::same(7), bc.gamma_multiply(0.22));
                        ui.painter().text(br.center(), Align2::CENTER_CENTER, &it.badge, fonts::semibold(12.0), bc);
                        let job = highlighted(&it.label, &it.hits, fonts::ui(14.0), pal.text, pal.accent);
                        let g = ui.painter().layout_job(job);
                        let gw = g.size().x;
                        ui.painter().galley(Pos2::new(r.min.x + 44.0, r.center().y - g.size().y / 2.0), g, pal.text);
                        if !it.detail.is_empty() {
                            ui.painter().text(Pos2::new(r.min.x + 56.0 + gw, r.center().y), Align2::LEFT_CENTER, &it.detail, fonts::ui(12.0), pal.text_dim);
                        }
                        if !it.shortcut.is_empty() {
                            ui.painter().text(Pos2::new(r.max.x - 12.0, r.center().y), Align2::RIGHT_CENTER, &it.shortcut, fonts::ui(12.0), pal.text_dim);
                        }
                        if resp.clicked() {
                            action = PaletteAction::Run(it.clone());
                        }
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (k, t) in [("Up Down", tr("navigate")), ("Enter", tr("select")), ("Tab", tr("switch")), ("Esc", tr("close"))] {
                        widgets::chip(ui, &pal, k);
                        ui.label(RichText::new(t).color(pal.text_dim).size(11.5));
                        ui.add_space(8.0);
                    }
                });
                outside_rect = ui.min_rect().expand(16.0);
            });
        });
        let clicked_outside = ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| !outside_rect.contains(p)));
        if clicked_outside {
            action = PaletteAction::Close;
        }
        match action {
            PaletteAction::None => self.popup = Popup::Palette(st),
            PaletteAction::Close => {
                self.popup = Popup::None;
                self.editor_focus = true;
            }
            PaletteAction::Run(it) => {
                self.popup = Popup::None;
                self.editor_focus = true;
                match it.target {
                    PTarget::Command(id) => self.run_command(ctx, &id),
                    PTarget::File(p) => {
                        self.push_nav();
                        self.open_file(&p);
                    }
                    PTarget::Symbol { path, offset } => self.goto(&path, offset, true),
                    PTarget::Offset(o) => {
                        self.push_nav();
                        if let Some(d) = self.doc_mut() {
                            d.ed.set_caret(o, false);
                            d.view.scroll_to_caret = true;
                        }
                    }
                    PTarget::Branch(b) => self.git_checkout(&b),
                }
            }
        }
    }

    // ------------------------------------------------------------------ dialogs

    /// A modal dialog card. `build` returns true when the user confirmed.
    pub(crate) fn dialog_card(&self, ctx: &egui::Context, id: &str, title: &str, width: f32, build: impl FnOnce(&mut egui::Ui)) -> Rect {
        let pal = self.pal;
        let mut rect = Rect::NOTHING;
        egui::Area::new(egui::Id::unique(id)).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            popup_frame(&pal).show(ui, |ui| {
                ui.set_width(width);
                ui.label(RichText::new(title).font(fonts::semibold(16.0)));
                ui.add_space(12.0);
                build(ui);
                rect = ui.min_rect().expand(16.0);
            });
        });
        rect
    }

    /// OK / Cancel row; returns (ok, cancel).
    pub(crate) fn ok_cancel(&self, ui: &mut egui::Ui, ok_label: &str) -> (bool, bool) {
        let pal = self.pal;
        let mut res = (false, false);
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::primary_button(ui, &pal, ok_label).clicked() {
                    res.0 = true;
                }
                if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                    res.1 = true;
                }
            });
        });
        res
    }

    fn text_field(&self, ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
        TextField::new(text).hint(hint).show(ui, &self.pal)
    }

    fn draw_input_dialogs(&mut self, ctx: &egui::Context) {
        let (esc, enter) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter)));
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        match popup {
            Popup::GotoLine { mut input, mut just_opened } => {
                let (mut ok, mut cancel) = (enter, esc);
                self.dialog_card(ctx, "goto_line", tr("Go to Line:Column"), 420.0, |ui| {
                    let r = self.text_field(ui, &mut input, tr("line[:column]"));
                    widgets::focus_once(ui, &r, &mut just_opened);
                    let (o, c) = self.ok_cancel(ui, tr("Go"));
                    ok |= o;
                    cancel |= c;
                });
                if ok && !cancel {
                    let mut parts = input.trim().split(':');
                    let line: usize = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(1);
                    let col: usize = parts.next().and_then(|s| s.trim().parse().ok()).unwrap_or(1);
                    self.push_nav();
                    if let Some(d) = self.doc_mut() {
                        let l = line.saturating_sub(1).min(d.ed.line_count() - 1);
                        let o = d.ed.offset_of(l, col.saturating_sub(1));
                        d.ed.set_caret(o, false);
                        d.view.scroll_to_caret = true;
                    }
                    self.editor_focus = true;
                } else if cancel {
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::GotoLine { input, just_opened };
                }
            }
            Popup::Rename { old, mut input, mut just_opened } => {
                let (mut ok, mut cancel) = (enter, esc);
                self.dialog_card(ctx, "rename", &i18n::fmt("Rename '{}'", &[&old]), 440.0, |ui| {
                    let r = self.text_field(ui, &mut input, tr("new name"));
                    widgets::focus_once(ui, &r, &mut just_opened);
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("All code occurrences in the project and open files are renamed.")).color(self.pal.text_dim).size(12.0));
                    let (o, c) = self.ok_cancel(ui, tr("Rename"));
                    ok |= o;
                    cancel |= c;
                });
                if cancel {
                    self.editor_focus = true;
                } else if ok {
                    self.rename_symbol(&old, input.trim());
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::Rename { old, input, just_opened };
                }
            }
            Popup::NewFile { dir, mut input, mut extension, mut kind, is_dir, mut just_opened } => {
                let (mut ok, mut cancel) = (enter, esc);
                let rel = self.project_root().map(|r| ide_core::workspace::relative(r, &dir)).unwrap_or_default();
                let project_name = self.project.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| dir.display().to_string());
                let kinds = if is_dir { vec![] } else { crate::new_file::kinds(&self.mods.installed, self.lisp_enabled()) };
                kind = kind.min(kinds.len().saturating_sub(1));
                // Up and Down pick the kind while the name is typed
                if kinds.len() > 1 {
                    let (up, down) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, Key::ArrowDown)));
                    if up || down {
                        kind = if down { (kind + 1) % kinds.len() } else { (kind + kinds.len() - 1) % kinds.len() };
                        extension = kinds[kind].extensions.first().cloned().unwrap_or_default();
                    }
                }
                let filenames = kinds.get(kind).map(|k| k.filenames.clone()).unwrap_or_default();
                let pal = self.pal;
                let name_id = egui::Id::unique("new_file_name");
                self.dialog_card(ctx, "new_file", if is_dir { tr("New Directory") } else { tr("New File") }, 480.0, |ui| {
                    if kinds.len() > 1 {
                        widgets::caption(ui, &pal, &tr("What kind of file").to_uppercase());
                        ui.add_space(6.0);
                        let col_w = (ui.available_width() - 8.0) / 2.0;
                        for pair in kinds.iter().enumerate().collect::<Vec<_>>().chunks(2) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                for (i, k) in pair {
                                    let (r, resp) = ui.allocate_exact_size(Vec2::new(col_w, 44.0), Sense::click());
                                    let chosen = *i == kind;
                                    let fill = if chosen { pal.selected } else if resp.hovered() { pal.hover.gamma_multiply(1.4) } else { pal.hover.gamma_multiply(0.6) };
                                    ui.painter().rect_filled(r, CornerRadius::same(12), fill);
                                    if chosen {
                                        ui.painter().rect_stroke(r, CornerRadius::same(12), egui::Stroke::new(1.0, pal.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
                                    }
                                    let badge = Rect::from_min_size(Pos2::new(r.min.x + 10.0, r.center().y - 13.0), Vec2::new(34.0, 26.0));
                                    if k.extensions.is_empty() {
                                        crate::icons::paint(ui.painter(), badge.shrink(4.0), Icon::Blank, pal.text_dim);
                                    } else {
                                        ui.painter().rect_filled(badge, CornerRadius::same(7), pal.accent.gamma_multiply(if chosen { 0.35 } else { 0.18 }));
                                        ui.painter().text(badge.center(), Align2::CENTER_CENTER, k.badge(), fonts::semibold(10.5), pal.accent);
                                    }
                                    let title = if k.extensions.is_empty() { tr("File").to_string() } else { k.title.clone() };
                                    let detail = if k.extensions.is_empty() { tr("any name, as typed").to_string() } else { k.extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join("  ") };
                                    let text_x = badge.max.x + 10.0;
                                    let clip = ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(text_x, r.min.y), Pos2::new(r.max.x - 8.0, r.max.y)));
                                    clip.text(Pos2::new(text_x, r.center().y - 8.0), Align2::LEFT_CENTER, title, fonts::semibold(13.0), pal.text);
                                    clip.text(Pos2::new(text_x, r.center().y + 9.0), Align2::LEFT_CENTER, detail, fonts::ui(11.5), pal.text_dim);
                                    if resp.clicked() {
                                        kind = *i;
                                        extension = k.extensions.first().cloned().unwrap_or_default();
                                        ui.memory_mut(|m| m.request_focus(name_id));
                                    }
                                }
                            });
                            ui.add_space(4.0);
                        }
                        ui.add_space(10.0);
                        widgets::caption(ui, &pal, &tr("Name").to_uppercase());
                        ui.add_space(6.0);
                    }
                    let hint = if is_dir { tr("directory name") } else if extension.is_empty() { tr("file name with its extension") } else { tr("file name") };
                    let ghost = crate::new_file::ghost(&input, &extension, &filenames);
                    let r = TextField::new(&mut input).id(name_id).hint(hint).suffix(&ghost).show(ui, &pal);
                    widgets::focus_once(ui, &r, &mut just_opened);
                    // a module of several languages: which of its extensions
                    if let Some(k) = kinds.get(kind).filter(|k| k.extensions.len() > 1) {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tr("Extension")).color(pal.text_dim).size(12.0));
                            ui.add_space(4.0);
                            for e in &k.extensions {
                                let mut on = *e == extension;
                                if widgets::toggle_chip(ui, &pal, &format!(".{e}"), &mut on, "").clicked() {
                                    extension = e.clone();
                                    ui.memory_mut(|m| m.request_focus(name_id));
                                }
                            }
                        });
                    }
                    ui.add_space(8.0);
                    let shown = crate::new_file::file_name(&input, &extension, &filenames);
                    // the folder by its place in the project: the project's own name for its top
                    let place = if rel.is_empty() { project_name.clone() } else { format!("{project_name}/{rel}") };
                    let line = if input.trim().is_empty() || is_dir { i18n::fmt("in {}", &[&place]) } else { i18n::fmt("Creates {0} in {1}", &[&shown, &place]) };
                    ui.label(RichText::new(line).color(pal.text_dim).size(12.0));
                    let (o, c) = self.ok_cancel(ui, tr("Create"));
                    ok |= o;
                    cancel |= c;
                });
                if cancel {
                    self.editor_focus = true;
                } else if ok && !input.trim().is_empty() {
                    let name = crate::new_file::file_name(&input, &extension, &filenames);
                    let path = dir.join(&name);
                    let res = if is_dir { ide_core::workspace::create_dir(&path) } else { ide_core::workspace::create_file(&path, "") };
                    match res {
                        Ok(()) => {
                            self.refresh_project();
                            if let Some(p) = self.project.as_mut() {
                                p.expanded.insert(dir.clone());
                            }
                            if !is_dir {
                                self.open_file(&path);
                            }
                        }
                        Err(e) => self.toast(format!("Cannot create {name}: {e}"), Sev::Error),
                    }
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::NewFile { dir, input, extension, kind, is_dir, just_opened };
                }
            }
            Popup::RenameFile { path, mut input, mut just_opened } => {
                let (mut ok, mut cancel) = (enter, esc);
                self.dialog_card(ctx, "rename_file", tr("Rename"), 440.0, |ui| {
                    let r = self.text_field(ui, &mut input, tr("new name"));
                    widgets::focus_once(ui, &r, &mut just_opened);
                    let (o, c) = self.ok_cancel(ui, tr("Rename"));
                    ok |= o;
                    cancel |= c;
                });
                if cancel {
                } else if ok && !input.trim().is_empty() {
                    let to = path.with_file_name(input.trim());
                    match ide_core::workspace::rename(&path, &to) {
                        Ok(()) => {
                            for d in self.docs.iter_mut() {
                                if d.path.as_deref() == Some(path.as_path()) {
                                    d.path = Some(to.clone());
                                }
                            }
                            self.refresh_project();
                        }
                        Err(e) => self.toast(format!("Cannot rename: {e}"), Sev::Error),
                    }
                } else {
                    self.popup = Popup::RenameFile { path, input, just_opened };
                }
            }
            Popup::LocalCommit { mut input, mut just_opened } => {
                let (mut ok, mut cancel) = (enter, esc);
                self.dialog_card(ctx, "local_commit", tr("Local Commit"), 480.0, |ui| {
                    let r = self.text_field(ui, &mut input, tr("Message (optional)"));
                    widgets::focus_once(ui, &r, &mut just_opened);
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("Saves a snapshot of your changes outside of git (in .light/, invisible to git).\nDiffs and change markers start from it afterwards.")).color(self.pal.text_dim).size(12.0));
                    let (o, c) = self.ok_cancel(ui, tr("Create"));
                    ok |= o;
                    cancel |= c;
                });
                if cancel {
                    self.editor_focus = true;
                } else if ok {
                    self.local_commit(&input);
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::LocalCommit { input, just_opened };
                }
            }
            Popup::NewBranch { mut input, mut just_opened } => {
                let (mut ok, mut cancel) = (enter, esc);
                self.dialog_card(ctx, "new_branch", tr("New Branch"), 420.0, |ui| {
                    let r = self.text_field(ui, &mut input, tr("branch name"));
                    widgets::focus_once(ui, &r, &mut just_opened);
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("Creates the branch from the current HEAD and switches to it.")).color(self.pal.text_dim).size(12.0));
                    let (o, c) = self.ok_cancel(ui, tr("Create"));
                    ok |= o;
                    cancel |= c;
                });
                if cancel {
                    self.editor_focus = true;
                } else if ok && !input.trim().is_empty() {
                    let name = input.trim().replace(' ', "-");
                    if self.git_run_sync("New branch", move |r| r.create_branch(&name).map(|_| String::new())) {
                        self.toast("Branch created", Sev::Weak);
                    }
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::NewBranch { input, just_opened };
                }
            }
            Popup::NewProject { mut parent, mut name, mut just_opened, mut modules, template, mut creating, mut location } => {
                if let Some(root) = location.poll() {
                    let distro = ide_core::wsl::wsl_path(&root).map(|path| path.distro);
                    let cfg = crate::app_env::ProjectConfig { modules: modules.clone(), decided: distro.is_some(), linux: distro, ..Default::default() };
                    if let Err(error) = crate::app_env::save_config(&root, &cfg) {
                        self.toast(i18n::fmt("Cannot save the project settings: {}", &[&error]), Sev::Error);
                    }
                    self.request_open_project(&root);
                    return;
                }
                let working = location.job.is_some();
                let (mut ok, mut cancel) = (enter && !creating && !working, esc && !working);
                let data = self.picker_data(ctx);
                let installed_ids: Vec<String> = self.mods.installed.iter().map(|m| m.manifest.id.clone()).collect();
                let missing: Vec<String> = modules.iter().filter(|id| !installed_ids.contains(id)).cloned().collect();
                let pal = self.pal;
                self.dialog_card(ctx, "new_project", tr("New Project"), 640.0_f32.min((ctx.content_rect().width() - 64.0).max(280.0)), |ui| {
                    if working { ui.disable(); }
                    egui::ScrollArea::vertical().id_salt("new_project_fields")
                        .max_height((ctx.content_rect().height() - 160.0).max(140.0))
                        .min_scrolled_height((ctx.content_rect().height() - 180.0).clamp(100.0, 500.0))
                        .show_fading(ui, |ui| {
                    widgets::caption(ui, &pal, &tr("Name").to_uppercase());
                    ui.add_space(6.0);
                    let r = self.text_field(ui, &mut name, tr("project name"));
                    widgets::focus_once(ui, &r, &mut just_opened);
                    ui.add_space(14.0);
                    widgets::caption(ui, &pal, &tr("Location").to_uppercase());
                    ui.add_space(6.0);
                    location.draw(ui, &pal, &self.wsl.distros(), &mut parent, creating || working);
                    ui.add_space(14.0);
                    widgets::caption(ui, &pal, &tr("Modules").to_uppercase());
                    ui.add_space(6.0);
                    crate::ui_modules::module_picker(ui, &pal, &data, &mut modules);
                    ui.add_space(14.0);
                    ui.label(RichText::new(tr("The project folder starts empty. Add files with + in Project.")).color(pal.text_dim).size(12.0));
                    ui.add_space(10.0);
                    if creating {
                        ui.label(RichText::new(tr("Downloading the modules...")).color(pal.accent).size(12.5));
                    } else if !missing.is_empty() {
                        ui.label(RichText::new(i18n::fmt("{} module(s) will be downloaded first.", &[&missing.len()])).color(pal.text_dim).size(12.0));
                    }
                    });
                    let (o, c) = self.ok_cancel(ui, tr("Create"));
                    ok |= o && !creating && !working;
                    cancel |= c && !working;
                });
                'done: {
                if cancel {
                    break 'done;
                }
                if ok && !missing.is_empty() {
                    // the templates are in the modules: get them first, the project is made when they are here
                    for id in &missing {
                        self.module_install(id);
                    }
                    creating = true;
                    ok = false;
                }
                if creating {
                    if missing.iter().any(|id| self.mods.busy.contains_key(id)) {
                        // still downloading
                    } else if missing.is_empty() {
                        creating = false;
                        ok = true;
                    } else {
                        creating = false;
                        self.toast(tr("The modules could not be downloaded"), Sev::Error);
                    }
                }
                if ok {
                    location.start(&parent, &name, Default::default());
                }
                if location.job.is_some() || self.wsl.probing() { ctx.request_repaint_after(std::time::Duration::from_millis(50)); }
                self.popup = Popup::NewProject { parent, name, just_opened, modules, template, creating, location };
                }
            }
            Popup::FindInFiles { mut query, mut replace, mut case_sensitive, mut whole_word, mut just_opened, replace_mode } => {
                let (mut ok, mut cancel) = (enter, esc);
                self.dialog_card(ctx, "find_in_files", if replace_mode { tr("Replace in Files") } else { tr("Find in Files") }, 520.0, |ui| {
                    let pal = self.pal;
                    let r = TextField::new(&mut query).hint(tr("Text to find")).icon(Icon::Search).show(ui, &pal);
                    widgets::focus_once(ui, &r, &mut just_opened);
                    if replace_mode {
                        ui.add_space(8.0);
                        self.text_field(ui, &mut replace, tr("Replace with"));
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        widgets::checkbox(ui, &pal, &mut case_sensitive, tr("Match case"));
                        ui.add_space(12.0);
                        widgets::checkbox(ui, &pal, &mut whole_word, tr("Whole word"));
                    });
                    let (o, c) = self.ok_cancel(ui, if replace_mode { tr("Replace All") } else { tr("Find") });
                    ok |= o;
                    cancel |= c;
                });
                if cancel {
                    self.editor_focus = true;
                } else if ok && !query.is_empty() {
                    let rep = if replace_mode { Some(replace.clone()) } else { None };
                    self.run_find_dialog(&query, rep.as_deref(), case_sensitive, whole_word);
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::FindInFiles { query, replace, case_sensitive, whole_word, just_opened, replace_mode };
                }
            }
            other => self.popup = other,
        }
    }

    // ------------------------------------------------------------------ info / confirm / about

    fn draw_info_dialogs(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        let popup = std::mem::replace(&mut self.popup, Popup::None);
        match popup {
            Popup::Info { title, text } => {
                let mut close = esc;
                egui::Area::new(egui::Id::unique("info")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                    popup_frame(&pal).show(ui, |ui| {
                        ui.set_width(620.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&title).font(fonts::semibold(15.0)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if widgets::icon_button(ui, &pal, Icon::Close, 30.0, pal.text_dim, tr("Close (Esc)")).clicked() {
                                    close = true;
                                }
                            });
                        });
                        ui.add_space(10.0);
                        egui::Frame::new().fill(pal.editor_bg).corner_radius(widgets::radius()).inner_margin(Margin::same(14)).show(ui, |ui| {
                            egui::ScrollArea::both().max_height(380.0).auto_shrink([false, true]).show_fading(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(&text).monospace().size(13.0)).wrap_mode(egui::TextWrapMode::Extend));
                            });
                        });
                    });
                });
                if close {
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::Info { title, text };
                }
            }
            Popup::About => {
                let mut close = esc;
                egui::Area::new(egui::Id::unique("about")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                    popup_frame(&pal).show(ui, |ui| {
                        ui.set_width(400.0);
                        ui.vertical_centered(|ui| {
                            ui.add_space(10.0);
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(96.0), Sense::hover());
                            crate::brand::paint_logo(ui.painter(), r, self.logos.for_theme(self.settings.logo, pal.dark), pal.text);
                            ui.add_space(14.0);
                            ui.label(RichText::new("Light").font(fonts::semibold(24.0)));
                            ui.label(RichText::new(i18n::fmt("Version {}", &[&env!("CARGO_PKG_VERSION")])).color(pal.text_dim));
                            ui.add_space(12.0);
                            ui.label(tr("A modular IDE for any language, written in Rust."));
                            ui.add_space(4.0);
                            ui.label(RichText::new(tr("An editor, Git, terminal, search and navigation.\nLanguages and tools connect through modules.")).color(pal.text_dim).size(12.5));
                            ui.add_space(6.0);
                            ui.label(RichText::new(tr("Fonts: JetBrains Mono and Inter (SIL OFL 1.1)")).color(pal.text_dim).size(11.5));
                            ui.add_space(16.0);
                            if widgets::primary_button(ui, &pal, tr("Close")).clicked() {
                                close = true;
                            }
                        });
                    });
                });
                if close {
                    self.editor_focus = true;
                } else {
                    self.popup = Popup::About;
                }
            }
            Popup::OpenProject { path } => {
                enum Choice {
                    This,
                    New,
                    Cancel,
                }
                let mut choice = if esc { Some(Choice::Cancel) } else { None };
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned());
                egui::Area::new(egui::Id::unique("open_project_ask")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                    let r = popup_frame(&pal).show(ui, |ui| {
                        ui.set_width(440.0);
                        ui.label(RichText::new(tr("Open Project")).font(fonts::semibold(16.0)));
                        ui.add_space(8.0);
                        ui.label(RichText::new(i18n::fmt("Open {} in the current window or in a new one?", &[&name])).color(pal.text));
                        ui.label(RichText::new(path.display().to_string()).color(pal.text_dim).size(12.0));
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if widgets::primary_button(ui, &pal, tr("This Window")).clicked() {
                                    choice = Some(Choice::This);
                                }
                                if widgets::secondary_button(ui, &pal, tr("New Window")).clicked() {
                                    choice = Some(Choice::New);
                                }
                                if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                                    choice = Some(Choice::Cancel);
                                }
                            });
                        });
                    });
                    widgets::sheen(ui.painter(), r.response.rect, &pal);
                });
                if choice.is_none() && ctx.input(|i| i.key_pressed(Key::Enter)) {
                    choice = Some(Choice::This);
                }
                match choice {
                    Some(Choice::This) => self.open_project(&path),
                    Some(Choice::New) => self.open_project_in_new_window(&path),
                    Some(Choice::Cancel) => self.editor_focus = true,
                    None => self.popup = Popup::OpenProject { path },
                }
            }
            Popup::Confirm { kind, message } => {
                let mut result: Option<bool> = esc.then_some(false);
                let mut save_first = false;
                egui::Area::new(egui::Id::unique("confirm")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                    popup_frame(&pal).show(ui, |ui| {
                        ui.set_width(420.0);
                        ui.label(RichText::new(&message).size(14.0));
                        ui.add_space(18.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let main_label = match &kind {
                                ConfirmKind::CloseDoc(_) => tr("Save"),
                                ConfirmKind::CloseAll | ConfirmKind::Exit | ConfirmKind::OpenProject(_) | ConfirmKind::CloseProject => tr("Save All"),
                                ConfirmKind::Delete(_) => tr("Delete"),
                                ConfirmKind::Rollback(_) => tr("Rollback"),
                                ConfirmKind::RestoreLocal(_) | ConfirmKind::RestoreLocalFile(..) => tr("Restore"),
                                ConfirmKind::DeleteLocal(_) => tr("Delete"),
                            };
                            let danger = matches!(kind, ConfirmKind::Delete(_) | ConfirmKind::Rollback(_) | ConfirmKind::RestoreLocal(_) | ConfirmKind::RestoreLocalFile(..) | ConfirmKind::DeleteLocal(_));
                            let galley = ui.painter().layout_no_wrap(main_label.to_string(), fonts::semibold(13.5), pal.text_on_accent);
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 32.0, 34.0), Sense::click());
                            ui.data_mut(|data| data.insert_temp(egui::Id::unique("confirm_accept_button"), r));
                            let fill = if danger { pal.error } else { pal.accent };
                            ui.painter().rect_filled(r, widgets::radius(), if resp.hovered() { fill.gamma_multiply(1.15) } else { fill });
                            ui.painter().text(r.center(), Align2::CENTER_CENTER, main_label, fonts::semibold(13.5), pal.text_on_accent);
                            if resp.clicked() {
                                result = Some(true);
                                save_first = matches!(kind, ConfirmKind::CloseDoc(_) | ConfirmKind::CloseAll | ConfirmKind::Exit | ConfirmKind::OpenProject(_) | ConfirmKind::CloseProject);
                            }
                            if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                                result = Some(false);
                            }
                            if matches!(kind, ConfirmKind::CloseDoc(_) | ConfirmKind::CloseAll | ConfirmKind::Exit | ConfirmKind::OpenProject(_) | ConfirmKind::CloseProject) {
                                let discard = widgets::secondary_button(ui, &pal, tr("Don't Save"));
                                ui.data_mut(|data| data.insert_temp(egui::Id::unique("confirm_discard_button"), discard.rect));
                                if discard.clicked() { result = Some(true); }
                            }
                        });
                    });
                });
                match result {
                    None => self.popup = Popup::Confirm { kind, message },
                    Some(false) => self.editor_focus = true,
                    Some(true) => {
                        self.editor_focus = true;
                        if save_first {
                            let saved = match &kind { ConfirmKind::CloseDoc(i) => self.save_doc(*i), _ => self.save_unsaved_docs() };
                            if !saved {
                                self.popup = Popup::Confirm { kind, message };
                                return;
                            }
                        }
                        match kind {
                            ConfirmKind::CloseDoc(i) => {
                                self.close_doc(i);
                            }
                            ConfirmKind::CloseAll => {
                                self.docs.clear();
                                self.active = 0;
                            }
                            ConfirmKind::Exit => self.exit_clean(),
                            ConfirmKind::OpenProject(path) => self.finish_open_project(&path),
                            ConfirmKind::CloseProject => self.finish_close_project(),
                            ConfirmKind::Delete(p) => match ide_core::workspace::delete(&p) {
                                Ok(()) => {
                                    self.docs.retain(|d| d.path.as_ref().is_none_or(|dp| !dp.starts_with(&p)));
                                    self.active = self.active.min(self.docs.len().saturating_sub(1));
                                    self.refresh_project();
                                }
                                Err(e) => self.toast(format!("Cannot delete: {e}"), Sev::Error),
                            },
                            ConfirmKind::Rollback(rel) => self.rollback_file(&rel),
                            ConfirmKind::RestoreLocal(id) => self.local_restore(id),
                            ConfirmKind::RestoreLocalFile(id, rel) => self.local_restore_file(id, &rel),
                            ConfirmKind::DeleteLocal(id) => self.local_delete(id),
                        }
                    }
                }
            }
            other => self.popup = other,
        }
    }

    // ------------------------------------------------------------------ completion & hint

    fn draw_completion(&mut self, ctx: &egui::Context) {
        if !self.completion.active || self.completion.items.is_empty() || self.popup.is_open() {
            return;
        }
        let Some(pos) = self.caret_screen else { return };
        let pal = self.pal;
        let items = self.completion.items.clone();
        let selected = self.completion.selected;
        let mut accept: Option<usize> = None;
        let screen = ctx.content_rect();
        let mut pos = pos + Vec2::new(0.0, 4.0);
        let h = (items.len().min(10) as f32) * 28.0 + 24.0;
        if pos.y + h > screen.max.y {
            pos.y = (pos.y - h - 28.0).max(0.0);
        }
        pos.x = pos.x.min(screen.max.x - 420.0);
        egui::Area::new(egui::Id::unique("completion")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::new()
                .fill(pal.popup_bg.to_opaque())
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(380.0);
                    // the height is the rows' own (ten at most): a scroll area left to size itself keeps the height of the
                    // list before it, so a longer list showed only the first rows
                    let list_h = items.len().min(10) as f32 * 28.0;
                    egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show_fading(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, it) in items.iter().enumerate() {
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
                            if i == selected {
                                ui.painter().rect_filled(r, CornerRadius::same(7), pal.selected);
                                resp.scroll_to_me(None);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(r, CornerRadius::same(7), pal.hover);
                            }
                            let (letter, color) = match it.kind.as_str() {
                                "f" => ("f", pal.definition),
                                "m" | "o" => ("m", pal.keyword_form),
                                "v" | "c" => ("v", pal.special_var),
                                "C" => ("C", pal.builtin),
                                "k" => (":", pal.keyword_arg),
                                _ => ("s", pal.text_dim),
                            };
                            let br = Rect::from_min_size(Pos2::new(r.min.x + 6.0, r.center().y - 9.0), Vec2::splat(18.0));
                            ui.painter().rect_filled(br, CornerRadius::same(5), color.gamma_multiply(0.22));
                            ui.painter().text(br.center(), Align2::CENTER_CENTER, letter, fonts::semibold(11.0), color);
                            let plen = self.completion.prefix.chars().count();
                            let hits: Vec<usize> = (0..plen).collect();
                            let job = highlighted(&it.text, &hits, FontId::monospace(13.0), pal.text, pal.accent);
                            let g = ui.painter().layout_job(job);
                            ui.painter().galley(Pos2::new(r.min.x + 34.0, r.center().y - g.size().y / 2.0), g, pal.text);
                            if resp.clicked() {
                                accept = Some(i);
                            }
                        }
                    });
                });
        });
        if let Some(i) = accept {
            self.completion.selected = i;
            self.accept_completion();
        }
    }

    fn draw_hint(&mut self, ctx: &egui::Context) {
        let Some((text, _)) = self.hint.clone() else { return };
        let Some(pos) = self.caret_screen else { return };
        let pal = self.pal;
        let screen = ctx.content_rect();
        let mut p = Pos2::new(pos.x, pos.y - 64.0);
        p.x = p.x.min(screen.max.x - 460.0).max(8.0);
        p.y = p.y.max(8.0);
        egui::Area::new(egui::Id::unique("hint")).order(egui::Order::Tooltip).fixed_pos(p).interactable(false).show(ctx, |ui| {
            egui::Frame::new().fill(pal.popup_bg.to_opaque()).corner_radius(CornerRadius::same(9)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.label(RichText::new(text).monospace().size(12.5));
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    // ------------------------------------------------------------------ find bar

    pub fn draw_find_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if !self.find.open {
            return;
        }
        self.refresh_find();
        let pal = self.pal;
        let mut step: Option<bool> = None;
        let mut close = false;
        let mut do_replace = false;
        let mut do_replace_all = false;
        egui::Frame::new().fill(pal.panel_bg).inner_margin(Margin::symmetric(16, 10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let id = egui::Id::unique("find_query");
                let r = TextField::new(&mut self.find.query).id(id).hint(tr("Find")).icon(Icon::Search).width(300.0).height(34.0).show(ui, &pal);
                if self.find.focus {
                    r.request_focus();
                    self.find.focus = false;
                }
                if r.lost_focus() && ctx.input(|i| i.key_pressed(Key::Enter)) {
                    step = Some(!ctx.input(|i| i.modifiers.shift));
                    r.request_focus();
                }
                if r.has_focus() && ctx.input(|i| i.key_pressed(Key::Escape)) {
                    close = true;
                }
                if r.clicked() || r.gained_focus() {
                    self.editor_focus = false;
                }
                let count = if self.find.query.is_empty() {
                    String::new()
                } else if self.find.matches.is_empty() {
                    tr("No results").to_string()
                } else {
                    i18n::fmt("{0} of {1}", &[&self.find.current.map_or(0, |c| c + 1), &self.find.matches.len()])
                };
                ui.add_space(4.0);
                ui.label(RichText::new(count).color(if !self.find.query.is_empty() && self.find.matches.is_empty() { pal.error } else { pal.text_dim }).size(12.5));
                if widgets::icon_button(ui, &pal, Icon::ChevronUp, 30.0, pal.text_dim, tr("Previous (Shift+F3)")).clicked() {
                    step = Some(false);
                }
                if widgets::icon_button(ui, &pal, Icon::ChevronDown, 30.0, pal.text_dim, tr("Next (F3)")).clicked() {
                    step = Some(true);
                }
                widgets::toggle_chip(ui, &pal, "Cc", &mut self.find.case_sensitive, tr("Match case"));
                widgets::toggle_chip(ui, &pal, "W", &mut self.find.whole_word, tr("Whole word"));
                widgets::toggle_chip(ui, &pal, tr("Replace"), &mut self.find.replace_mode, "");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, &pal, Icon::Close, 30.0, pal.text_dim, tr("Close (Esc)")).clicked() {
                        close = true;
                    }
                });
            });
            if self.find.replace_mode {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let r = TextField::new(&mut self.find.replace).hint(tr("Replace")).icon(Icon::Pencil).width(300.0).height(34.0).show(ui, &pal);
                    if r.clicked() || r.gained_focus() {
                        self.editor_focus = false;
                    }
                    if r.has_focus() && ctx.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                    ui.add_space(4.0);
                    if widgets::secondary_button(ui, &pal, tr("Replace")).clicked() {
                        do_replace = true;
                    }
                    if widgets::secondary_button(ui, &pal, tr("Replace All")).clicked() {
                        do_replace_all = true;
                    }
                });
            }
        });
        widgets::divider(ui, &pal, ui.min_rect().max.y, ui.min_rect().min.x, ui.min_rect().max.x);
        if let Some(fwd) = step {
            self.find_step(fwd);
        }
        if do_replace {
            self.replace_current();
        }
        if do_replace_all {
            self.replace_all();
        }
        if close {
            self.find.open = false;
            self.find.matches.clear();
            self.editor_focus = true;
        }
    }

    // ------------------------------------------------------------------ welcome

    pub fn draw_welcome(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        // the card around the page already carries the editor colour; a square fill here would cut off its rounded corners
        let rect = ui.available_rect_before_wrap();
        let mut open: Option<PathBuf> = None;
        let col_w = 460.0f32.min(rect.width() - 48.0);
        let top = (rect.height() * 0.14).max(28.0);
        // the scroll area is the whole page, so its bar is at the window's edge; the column sits in the middle of it
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_fading(ui, |ui| {
            let pad = ((ui.available_width() - col_w) / 2.0).max(0.0);
            ui.horizontal_top(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
            ui.set_width(col_w);
            ui.add_space(top);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(72.0), Sense::hover());
                crate::brand::paint_logo(ui.painter(), r, self.logos.for_theme(self.settings.logo, pal.dark), pal.text);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Light").font(fonts::semibold(26.0)));
                    ui.label(RichText::new(tr("A shovel; the gold is yours to find")).color(pal.text_dim));
                });
            });
            ui.add_space(28.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &pal, tr("New Project")).clicked() {
                    self.run_command(ctx, "file.new_project");
                }
                if widgets::secondary_button(ui, &pal, tr("Open...")).clicked() {
                    self.run_command(ctx, "file.open_project");
                }
                if widgets::secondary_button(ui, &pal, tr("Get from VCS")).clicked() {
                    self.run_command(ctx, "file.clone");
                }
                if widgets::secondary_button(ui, &pal, tr("Modules")).clicked() {
                    self.run_command(ctx, "app.modules");
                }
            });
            ui.add_space(28.0);
            // the section is always there: getting a project from version control is its last row
            {
                widgets::caption(ui, &pal, &tr("Recent projects").to_uppercase());
                ui.add_space(8.0);
                for p in self.settings.recent_projects.iter().take(6) {
                    let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(col_w, 46.0), Sense::click());
                    widgets::row_background(ui, &pal, r.expand2(Vec2::new(6.0, 0.0)), false, resp.hovered());
                    let av = Rect::from_min_size(Pos2::new(r.min.x + 4.0, r.center().y - 15.0), Vec2::splat(30.0));
                    ui.painter().rect_filled(av, CornerRadius::same(8), pal.accent.gamma_multiply(0.85));
                    let init: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
                    ui.painter().text(av.center(), Align2::CENTER_CENTER, init, fonts::semibold(12.0), Color32::WHITE);
                    ui.painter().text(Pos2::new(r.min.x + 46.0, r.center().y - 8.0), Align2::LEFT_CENTER, &name, fonts::semibold(13.5), pal.text);
                    ui.painter().text(Pos2::new(r.min.x + 46.0, r.center().y + 9.0), Align2::LEFT_CENTER, p, fonts::ui(11.5), pal.text_dim);
                    if resp.clicked() {
                        open = Some(PathBuf::from(p));
                    }
                }
                let (r, resp) = ui.allocate_exact_size(Vec2::new(col_w, 46.0), Sense::click());
                widgets::row_background(ui, &pal, r.expand2(Vec2::new(6.0, 0.0)), false, resp.hovered());
                let av = Rect::from_min_size(Pos2::new(r.min.x + 4.0, r.center().y - 15.0), Vec2::splat(30.0));
                ui.painter().rect_filled(av, CornerRadius::same(8), pal.hover.gamma_multiply(1.6));
                crate::icons::paint(ui.painter(), av.shrink(7.0), Icon::FolderGit, pal.accent);
                ui.painter().text(Pos2::new(r.min.x + 46.0, r.center().y - 8.0), Align2::LEFT_CENTER, tr("Get from Version Control..."), fonts::semibold(13.5), pal.text);
                ui.painter().text(Pos2::new(r.min.x + 46.0, r.center().y + 9.0), Align2::LEFT_CENTER, tr("Clone a repository from GitHub or any Git URL"), fonts::ui(11.5), pal.text_dim);
                ui.data_mut(|data| data.insert_temp(egui::Id::unique("welcome_clone_row"), r));
                if resp.clicked() {
                    self.run_command(ctx, "file.clone");
                }
                ui.add_space(20.0);
            }
            widgets::caption(ui, &pal, &tr("Quick start").to_uppercase());
            ui.add_space(8.0);
            for (t, k) in [("Search Everywhere", "Double Shift"), ("Go to File", "Ctrl+Shift+N"), ("Find Action", "Ctrl+Shift+A"), ("Recent Files", "Ctrl+E"), ("Run file", "Shift+F10"), ("Commit", "Ctrl+K")] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr(t)).color(pal.text_dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| widgets::chip(ui, &pal, k));
                });
                ui.add_space(2.0);
            }
            });
            });
        });
        if let Some(p) = open {
            self.request_open_project(&p);
        }
    }
}
