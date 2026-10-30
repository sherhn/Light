//! Tool windows: Project tree, Structure, REPL, Run, Problems, Find results, Git, Terminal.

use crate::scrollbars::FadingScroll as _;
use std::path::PathBuf;

use eframe::egui::{self, Color32, CornerRadius, Key, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use lisp_syntax::outline::{SymbolKind, definition_at};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::*;
use crate::widgets::{self, PANEL_HEADER_H, ROW_H};

/// What drawing a console log found out.
#[derive(Clone, Copy, Debug, Default)]
struct LogOut {
    /// The mouse went down in the log: the keys are meant for it.
    pressed: bool,
    /// Text was copied to the clipboard.
    copied: bool,
    /// The log has the keyboard: the editor and the text fields do not (so Ctrl+C is the log's).
    keys_here: bool,
}

/// Where the rows of the console log last drawn are on screen; kept for the UI tests only (they drive the mouse by text).
#[derive(Clone, Debug, Default)]
pub struct LogGeometry {
    /// Left edge of the rows, top of the first one, their height and the width of one character.
    pub left: f32,
    pub top: f32,
    pub row_h: f32,
    pub char_w: f32,
    /// The text of the rows on screen, as shown.
    pub rows: Vec<String>,
    /// How far the log is scrolled, how far it can be, and how tall the visible part is.
    pub offset_y: f32,
    pub max_offset: f32,
    pub view_h: f32,
}

/// Id of the egui data holding the last [`LogGeometry`] and the last text a console copied.
pub const LOG_GEOMETRY_ID: &str = "log_geometry";
pub const LAST_COPIED_ID: &str = "log_last_copied";

/// Put `text` on the clipboard (and remember it for the tests).
fn copy_log_text(ctx: &egui::Context, text: String) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(LAST_COPIED_ID), text.clone()));
    ctx.copy_text(text);
}

enum LogMenu {
    Copy,
    SelectAll,
    CopyAll,
}

fn kind_color(app: &App, k: SymbolKind) -> Color32 {
    let p = &app.pal;
    match k {
        SymbolKind::Function | SymbolKind::Method | SymbolKind::Generic => p.definition,
        SymbolKind::Macro => p.keyword_form,
        SymbolKind::Variable | SymbolKind::Constant => p.special_var,
        SymbolKind::Class | SymbolKind::Struct | SymbolKind::Condition | SymbolKind::Type => p.builtin,
        SymbolKind::Package | SymbolKind::System => p.ok,
        SymbolKind::Other => p.text_dim,
    }
}

pub fn badge_color(app: &App, badge: &str) -> Color32 {
    let p = &app.pal;
    match badge {
        "f" | "M" | "g" => p.definition,
        "m" => p.keyword_form,
        "v" | "c" => p.special_var,
        "C" | "S" | "E" | "T" => p.builtin,
        "P" | "A" => p.ok,
        "F" | "B" => p.accent,
        _ => p.text_dim,
    }
}

impl App {
    // ------------------------------------------------------------------ panel header

    /// Header strip of a tool window; returns the rect so callers can add right-aligned actions.
    pub fn panel_header(&mut self, ui: &mut egui::Ui, title: &'static str) -> Rect {
        let pal = self.pal;
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), PANEL_HEADER_H), Sense::hover());
        ui.painter().text(Pos2::new(r.min.x + 16.0, r.center().y), egui::Align2::LEFT_CENTER, tr(title), fonts::semibold(13.5), pal.text);
        r
    }

    fn header_action(&mut self, ui: &mut egui::Ui, header: Rect, slot: usize, icon: Icon, tip: &'static str) -> bool {
        let pal = self.pal;
        let r = Rect::from_min_size(Pos2::new(header.max.x - 12.0 - 32.0 * (slot as f32 + 1.0), header.center().y - 14.0), Vec2::splat(28.0));
        let resp = ui.interact(r, ui.scope_id().with(("hdr", tip)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, widgets::radius(), pal.hover);
        }
        icons::paint(ui.painter(), r.shrink(7.0), icon, pal.text_dim);
        resp.on_hover_text(tr(tip)).clicked()
    }

    // ------------------------------------------------------------------ project tree

    /// Rows of the project tree. They are cached until something changes (expand/collapse, refresh, save),
    /// because listing and ignore-matching a big project every frame would freeze the UI.
    fn take_tree_rows(&mut self) -> Vec<TreeRow> {
        let Some(p) = self.project.as_mut() else { return vec![] };
        if let Some((ver, rows)) = p.tree_cache.take()
            && ver == p.tree_ver
        {
            return rows;
        }
        let root = p.root.clone();
        fn walk(app: &mut App, dir: PathBuf, depth: usize, rows: &mut Vec<TreeRow>) {
            let entries = app.dir_entries(&dir);
            for e in entries {
                let (expanded, ignored) = match app.project.as_mut() {
                    Some(p) => (e.is_dir && p.expanded.contains(&e.path), p.ignore.is_ignored(&e.path, e.is_dir)),
                    None => (false, false),
                };
                rows.push(TreeRow { path: e.path.clone(), depth, is_dir: e.is_dir, expanded, name: e.name.clone(), ignored });
                if expanded && !ignored {
                    walk(app, e.path, depth + 1, rows);
                }
            }
        }
        let mut rows = Vec::new();
        let name = self.project.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let expanded = self.project.as_ref().is_some_and(|p| p.expanded.contains(&root));
        rows.push(TreeRow { path: root.clone(), depth: 0, is_dir: true, expanded, name, ignored: false });
        if expanded {
            walk(self, root, 1, &mut rows);
        }
        rows
    }

    fn put_tree_rows(&mut self, rows: Vec<TreeRow>) {
        if let Some(p) = self.project.as_mut() {
            p.tree_cache = Some((p.tree_ver, rows));
        }
    }

    pub fn draw_project_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let header = self.panel_header(ui, "Project");
        if self.header_action(ui, header, 0, Icon::Refresh, "Refresh") {
            self.refresh_project();
        }
        if self.project.is_some() && self.header_action(ui, header, 1, Icon::Plus, "New File...") {
            self.run_command(ctx, "file.new");
        }
        if self.project.is_none() {
            ui.add_space(28.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("No project is open")).color(pal.text_dim));
                ui.add_space(12.0);
                if widgets::primary_button(ui, &pal, tr("Open Project")).clicked() {
                    self.run_command(ctx, "file.open_project");
                }
                ui.add_space(8.0);
                if widgets::secondary_button(ui, &pal, tr("New Project")).clicked() {
                    self.run_command(ctx, "file.new_project");
                }
                ui.add_space(8.0);
                if widgets::secondary_button(ui, &pal, tr("Get from VCS")).clicked() {
                    self.run_command(ctx, "file.clone");
                }
            });
            return;
        }
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let response = ui.add_sized([ui.available_width() - 12.0, 30.0], egui::TextEdit::singleline(&mut self.project_filter).hint_text(tr("Search files by name")));
            if response.gained_focus() { self.editor_focus = false; }
        });
        ui.add_space(6.0);
        let cached_rows = self.take_tree_rows();
        let query = self.project_filter.trim().to_lowercase();
        let matches: Vec<TreeRow> = self.project.as_ref().filter(|_| !query.is_empty()).into_iter().flat_map(|p| p.files.iter()).filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            (!query.is_empty() && name.to_lowercase().contains(&query)).then(|| TreeRow { path: path.clone(), depth: 0, is_dir: false, expanded: false, name, ignored: false })
        }).collect();
        let rows = if query.is_empty() { &cached_rows } else { &matches };
        let mut toggle: Option<PathBuf> = None;
        let mut open: Option<PathBuf> = None;
        let mut select: Option<PathBuf> = None;
        let mut action: Option<(String, PathBuf)> = None;
        let selected = self.project.as_ref().and_then(|p| p.selected.clone());
        let active_path = self.doc().and_then(|d| d.path.clone());
        let font = fonts::ui(13.5);
        // show_rows must see the same spacing as the rows it virtualizes.
        ui.spacing_mut().item_spacing.y = 0.0;
        crate::scrollbars::reserve_gutter(ui);
        egui::ScrollArea::both().auto_shrink([false, false]).max_height(ui.available_height()).show_rows_fading(ui, ROW_H, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                let text_w = ui.painter().layout_no_wrap(row.name.clone(), font.clone(), pal.text).size().x;
                let w = ui.available_width().max(70.0 + row.depth as f32 * 18.0 + text_w);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
                let is_sel = selected.as_ref() == Some(&row.path);
                widgets::row_background(ui, &pal, r, is_sel, resp.hovered());
                let x = r.min.x + 14.0 + row.depth as f32 * 18.0;
                if row.is_dir {
                    icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(x, r.center().y - 6.0), Vec2::splat(12.0)), if row.expanded { Icon::ChevronDown } else { Icon::ChevronRight }, pal.text_dim);
                }
                let ir = Rect::from_min_size(Pos2::new(x + 18.0, r.center().y - 8.0), Vec2::splat(16.0));
                let kind = self.git_change_of(&row.path);
                if row.is_dir {
                    icons::paint(ui.painter(), ir, if row.expanded { Icon::FolderOpen } else { Icon::Folder }, if row.ignored { pal.text_dim.gamma_multiply(0.7) } else { pal.builtin.gamma_multiply(0.85) });
                } else if lisp_syntax::dialect::is_lisp_file(&row.path) {
                    icons::paint(ui.painter(), ir, Icon::Lisp, if row.ignored { pal.text_dim.gamma_multiply(0.7) } else { pal.keyword_form });
                } else {
                    icons::paint(ui.painter(), ir, Icon::File, pal.text_dim);
                }
                let dirty_dir = row.is_dir && self.git.dirty_dirs.contains(&row.path) && row.depth > 0;
                let color = if row.ignored {
                    pal.text_dim.gamma_multiply(0.7)
                } else if let Some(k) = kind {
                    self.git_color(k)
                } else if dirty_dir {
                    pal.git_modified
                } else if active_path.as_ref() == Some(&row.path) {
                    pal.accent
                } else {
                    pal.text
                };
                ui.painter().text(Pos2::new(x + 42.0, r.center().y), egui::Align2::LEFT_CENTER, &row.name, font.clone(), color);
                let resp = if dirty_dir { resp.on_hover_text(tr("Contains files with changes")) } else { resp.on_hover_text(row.path.display().to_string()) };
                if resp.clicked() {
                    select = Some(row.path.clone());
                    if row.is_dir {
                        toggle = Some(row.path.clone());
                    }
                }
                if resp.double_clicked() && !row.is_dir {
                    open = Some(row.path.clone());
                }
                widgets::context_popup(&resp, &pal).show(|ui| {
                    let dir = if row.is_dir { row.path.clone() } else { row.path.parent().map(PathBuf::from).unwrap_or_default() };
                    if widgets::menu_row(ui, &pal, Some(Icon::FilePlus), tr("New File..."), "Alt+Insert").clicked() {
                        action = Some(("new_file".into(), dir.clone()));
                        ui.close();
                    }
                    if widgets::menu_row(ui, &pal, Some(Icon::FolderPlus), tr("New Directory..."), "").clicked() {
                        action = Some(("new_dir".into(), dir.clone()));
                        ui.close();
                    }
                    ui.separator();
                    if !row.is_dir && widgets::menu_row(ui, &pal, Some(Icon::Blank), tr("Open"), "").clicked() {
                        open = Some(row.path.clone());
                        ui.close();
                    }
                    if widgets::menu_row(ui, &pal, Some(Icon::Pencil), tr("Rename..."), "").clicked() {
                        action = Some(("rename".into(), row.path.clone()));
                        ui.close();
                    }
                    if widgets::menu_row(ui, &pal, Some(Icon::Trash), tr("Delete..."), "").clicked() {
                        action = Some(("delete".into(), row.path.clone()));
                        ui.close();
                    }
                    if row.is_dir && row.depth > 0 && widgets::menu_row(ui, &pal, Some(Icon::Eye), tr("Hide Folder"), "").on_hover_text(tr("Leave it out of the tree, search and Go to File (Settings > Project)")).clicked() {
                        action = Some(("hide".into(), row.path.clone()));
                        ui.close();
                    }
                    if !row.is_dir && self.git.repo.is_some() {
                        ui.separator();
                        if widgets::menu_row(ui, &pal, Some(Icon::FileDiff), tr("Git: Show Diff"), "").clicked() {
                            action = Some(("diff".into(), row.path.clone()));
                            ui.close();
                        }
                        if widgets::menu_row(ui, &pal, Some(Icon::Plus), tr("Git: Add"), "").clicked() {
                            action = Some(("stage".into(), row.path.clone()));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if widgets::menu_row(ui, &pal, Some(Icon::Copy), tr("Copy Path"), "").clicked() {
                        ctx.copy_text(row.path.to_string_lossy().into_owned());
                        ui.close();
                    }
                    if widgets::menu_row(ui, &pal, Some(Icon::FolderSearch), tr("Reveal in File Manager"), "").clicked() {
                        crate::input::reveal_in_file_manager(&row.path);
                        ui.close();
                    }
                    if widgets::menu_row(ui, &pal, Some(Icon::Refresh), tr("Refresh"), "").clicked() {
                        action = Some(("refresh".into(), row.path.clone()));
                        ui.close();
                    }
                });
            }
        });
        self.put_tree_rows(cached_rows);
        if let Some(p) = select
            && let Some(proj) = self.project.as_mut()
        {
            proj.selected = Some(p);
        }
        if let Some(p) = toggle
            && let Some(proj) = self.project.as_mut()
        {
            if !proj.expanded.remove(&p) {
                proj.expanded.insert(p);
            }
            proj.tree_ver += 1;
        }
        if let Some(p) = open {
            self.open_file(&p);
        }
        if let Some((a, path)) = action {
            match a.as_str() {
                "new_file" => self.popup = Popup::NewFile { dir: path, input: String::new(), extension: String::new(), kind: 0, is_dir: false, just_opened: true },
                "new_dir" => self.popup = Popup::NewFile { dir: path, input: String::new(), extension: String::new(), kind: 0, is_dir: true, just_opened: true },
                "rename" => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.popup = Popup::RenameFile { path, input: name, just_opened: true };
                }
                "delete" => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.popup = Popup::Confirm { message: i18n::fmt("Delete '{}'? This cannot be undone.", &[&name]), kind: ConfirmKind::Delete(path) };
                }
                "diff" => {
                    if let Some(rel) = self.git.repo.as_ref().and_then(|r| r.rel(&path)) {
                        self.git_show_diff(&rel);
                    }
                }
                "stage" => {
                    if let Some(rel) = self.git.repo.as_ref().and_then(|r| r.rel(&path)) {
                        self.git_run_sync("Add", move |r| r.stage(&[rel]).map(|_| String::new()));
                    }
                }
                "refresh" => self.refresh_project(),
                "hide" => self.hide_folder(&path),
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------ structure

    pub fn draw_structure_panel(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        self.panel_header(ui, "Structure");
        if self.doc().is_none() {
            ui.add_space(12.0);
            ui.label(RichText::new(format!("   {}", tr("No file is open"))).color(pal.text_dim));
            return;
        }
        // take the cached outline out of the document while drawing (no clone of a possibly huge list)
        let (defs, caret) = {
            let d = self.docs.get_mut(self.active).expect("checked above");
            d.defs();
            (d.outline_cache.take().map(|c| c.1).unwrap_or_default(), d.ed.caret())
        };
        let current = definition_at(&defs, caret).map(|x| x.start);
        let mut goto: Option<usize> = None;
        if defs.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("   {}", tr("Nothing to show"))).color(pal.text_dim));
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows_fading(ui, ROW_H, defs.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for def in &defs[range] {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                widgets::row_background(ui, &pal, r, current == Some(def.start), resp.hovered());
                let c = kind_color(self, def.kind);
                let br = Rect::from_min_size(Pos2::new(r.min.x + 16.0, r.center().y - 9.0), Vec2::splat(18.0));
                ui.painter().rect_filled(br, CornerRadius::same(5), c.gamma_multiply(0.22));
                ui.painter().text(br.center(), egui::Align2::CENTER_CENTER, def.kind.badge(), fonts::semibold(11.0), c);
                ui.painter().text(Pos2::new(r.min.x + 44.0, r.center().y), egui::Align2::LEFT_CENTER, &def.name, fonts::ui(13.5), pal.text);
                if resp.clicked() {
                    goto = Some(def.name_start);
                }
            }
        });
        if let Some(d) = self.docs.get_mut(self.active) {
            let rev = d.ed.revision();
            d.outline_cache = Some((rev, defs));
        }
        if let Some(o) = goto
            && let Some(d) = self.doc_mut()
        {
            d.ed.set_caret(o, false);
            d.view.scroll_to_caret = true;
            self.editor_focus = true;
        }
    }

    // ------------------------------------------------------------------ bottom tool windows

    pub fn draw_bottom_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let Some(tool) = self.bottom_tool else { return };
        let problems = self.all_problems();
        let err_count = problems.iter().filter(|p| p.sev == Sev::Error).count();
        let (hr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), PANEL_HEADER_H), Sense::hover());
        let mut x = hr.min.x + 12.0;
        let lisp = self.lisp_enabled();
        let tabs = [
            (BottomTool::Repl, tr("REPL").to_string()),
            (BottomTool::Run, tr("Run").to_string()),
            (BottomTool::Terminal, tr("Terminal").to_string()),
            (BottomTool::Problems, if problems.is_empty() { tr("Problems").to_string() } else { format!("{}  {}", tr("Problems"), problems.len()) }),
            (BottomTool::Git, "Git".to_string()),
            (BottomTool::Find, if self.find_results.hits.is_empty() { tr("Find").to_string() } else { format!("{}  {}", tr("Find"), self.find_results.hits.len()) }),
        ];
        let mut switch_to = None;
        for (t, label) in tabs.into_iter().filter(|(t, _)| lisp || *t != BottomTool::Repl) {
            let active = t == tool;
            let font = if active { fonts::semibold(13.5) } else { fonts::ui(13.5) };
            let g = ui.painter().layout_no_wrap(label.clone(), font.clone(), pal.text);
            let w = g.size().x + 26.0;
            let r = Rect::from_min_size(Pos2::new(x, hr.min.y), Vec2::new(w, hr.height()));
            let resp = ui.interact(r, ui.scope_id().with(("btab", label.clone())), Sense::click());
            if active {
                ui.painter().rect_filled(r.shrink2(Vec2::new(0.0, 8.0)), CornerRadius::same(12), pal.tab_active_bg);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(Vec2::new(0.0, 8.0)), CornerRadius::same(12), pal.hover);
            }
            let color = if t == BottomTool::Problems && err_count > 0 { pal.error } else if active { pal.text } else { pal.text_dim };
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, label, font, color);
            if resp.clicked() {
                switch_to = Some(t);
            }
            x += w + 2.0;
        }
        let mut close = false;
        if self.header_action(ui, hr, 0, Icon::Close, "Hide") {
            close = true;
        }
        let mut slot = 1;
        match tool {
            BottomTool::Repl => {
                if self.header_action(ui, hr, slot, Icon::Clear, "Clear output") {
                    self.repl.log.clear();
                }
                slot += 1;
                if self.header_action(ui, hr, slot, Icon::Restart, "Restart REPL (Ctrl+Shift+F5)") {
                    self.repl_restart();
                }
                slot += 1;
                if self.header_action(ui, hr, slot, Icon::Stop, "Interrupt (restarts the REPL)") {
                    self.run_command(ctx, "run.stop");
                }
            }
            BottomTool::Run => {
                if self.header_action(ui, hr, slot, Icon::Clear, "Clear output") {
                    self.run.log.clear();
                }
                slot += 1;
                if self.header_action(ui, hr, slot, Icon::Stop, "Stop (Ctrl+F2)") {
                    self.stop_run();
                }
                slot += 1;
                if self.header_action(ui, hr, slot, Icon::Play, "Rerun (Shift+F10)") {
                    self.rerun_tab();
                }
            }
            BottomTool::Terminal => {
                if self.header_action(ui, hr, slot, Icon::Clear, "Clear") {
                    self.term.log.clear();
                }
                slot += 1;
                if self.header_action(ui, hr, slot, Icon::Stop, "Stop running command") {
                    self.terminal_stop();
                }
                slot += 1;
                if self.header_action(ui, hr, slot, Icon::Script, "Run script...") {
                    self.terminal_pick_script();
                }
            }
            BottomTool::Problems => {
                if self.header_action(ui, hr, slot, Icon::Clear, "Clear compiler problems") {
                    self.compiler_problems.clear();
                    for d in self.docs.iter_mut() {
                        d.compiler_markers.clear();
                    }
                }
            }
            BottomTool::Find => {
                if self.header_action(ui, hr, slot, Icon::Clear, "Clear results") {
                    self.find_results = FindResults::default();
                }
            }
            BottomTool::Git => {
                if self.header_action(ui, hr, slot, Icon::Refresh, "Refresh") {
                    self.git.rx = None;
                    self.git_refresh();
                }
            }
        }
        if let Some(t) = switch_to {
            self.bottom_tool = Some(t);
            if t == BottomTool::Repl && self.repl.session.is_none() {
                self.ensure_repl();
            }
            if t == BottomTool::Terminal {
                self.terminal_start_dir();
                self.term.focus_input = true;
            }
        }
        if close {
            self.bottom_tool = None;
            return;
        }
        match self.bottom_tool {
            Some(BottomTool::Repl) => self.draw_repl(ui, ctx),
            Some(BottomTool::Run) => self.draw_run(ui),
            Some(BottomTool::Terminal) => self.draw_terminal(ui, ctx),
            Some(BottomTool::Problems) => self.draw_problems(ui, &problems),
            Some(BottomTool::Find) => self.draw_find_results(ui),
            Some(BottomTool::Git) => self.draw_git(ui, ctx),
            None => {}
        }
    }

    fn log_color(&self, k: LogKind) -> Color32 {
        let p = &self.pal;
        match k {
            LogKind::Input => p.text,
            LogKind::Output => p.plain,
            LogKind::Value => p.number,
            LogKind::Error => p.error,
            LogKind::Info => p.text_dim,
            LogKind::Trace => p.text_dim,
        }
    }

    /// A log as it is shown: the prompt or `=>` first, then the text. Copying uses the same text.
    fn log_row_text(l: &LogLine, prompt: &str) -> String {
        let text = l.text.trim_end_matches('\n');
        match l.kind {
            LogKind::Input => format!("{prompt} {text}"),
            LogKind::Value => format!("=> {text}"),
            LogKind::Trace => format!("  {text}"),
            _ => text.to_string(),
        }
    }

    /// A console log (REPL, terminal, run output) whose text can be selected with the mouse (drag, double click for
    /// a word, triple click for a line) and copied with Ctrl+C or the right-click menu.
    fn draw_log(&self, ui: &mut egui::Ui, id: &str, lines: &[LogLine], stick: bool, prompt: &str) -> LogOut {
        if lines.iter().any(|l| l.text.trim_end_matches('\n').contains('\n')) {
            let rows: Vec<_> = lines.iter().flat_map(|l| l.text.trim_end_matches('\n').split('\n').map(|text| LogLine { kind: l.kind, text: text.trim_end_matches('\r').to_string() })).collect();
            return self.draw_log_rows(ui, id, &rows, stick, prompt);
        }
        self.draw_log_rows(ui, id, lines, stick, prompt)
    }

    fn draw_log_rows(&self, ui: &mut egui::Ui, id: &str, lines: &[LogLine], stick: bool, prompt: &str) -> LogOut {
        use crate::log_select::{Pos, Selection, select_all, selected_text, word_range};
        let pal = self.pal;
        let font = egui::FontId::monospace((self.settings.font_size - 1.0).max(9.0));
        let row_h = (ui.fonts_mut(|f| f.row_height(&font)) + 5.0).round();
        let sel_id = ui.scope_id().with(("log_selection", id));
        let mut sel: Option<Selection> = ui.data(|d| d.get_temp::<Selection>(sel_id)).and_then(|s| s.clamped(lines.len()));
        let mut out = LogOut::default();
        let row_len = |i: usize| Self::log_row_text(&lines[i], prompt).chars().count();
        // the character of a row under a point (relative to the row's text), at most `max`
        let col_at = |g: &egui::Galley, p: Vec2, max: usize| -> usize { usize::from(g.cursor_from_pos(p).index).min(max) };
        // what the menu asked for
        let mut menu: Option<LogMenu> = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        crate::scrollbars::reserve_gutter(ui);
        let area = egui::ScrollArea::both().id_salt(id).min_scrolled_height(0.0).max_height(ui.available_height()).auto_shrink([false, false]).stick_to_bottom(stick).show_rows_fading(ui, row_h, lines.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            // the rows on screen: where they are and how their text is laid out, for finding the character under the mouse
            let mut rows: Vec<(usize, Rect, Pos2, std::sync::Arc<egui::Galley>, usize, egui::Response)> = Vec::new();
            for line in range.clone() {
                let l = &lines[line];
                let text = l.text.trim_end_matches('\n');
                let color = self.log_color(l.kind);
                let prefix = match l.kind {
                    LogKind::Input => format!("{prompt} "),
                    LogKind::Value => "=> ".to_string(),
                    LogKind::Trace => "  ".to_string(),
                    _ => String::new(),
                };
                let len = prefix.chars().count() + text.chars().count();
                let mut job = egui::text::LayoutJob::default();
                job.append("", 14.0, egui::TextFormat::simple(font.clone(), color));
                if !prefix.is_empty() {
                    job.append(&prefix, 0.0, egui::TextFormat::simple(font.clone(), if l.kind == LogKind::Input { pal.ok } else { color }));
                }
                job.append(text, 0.0, egui::TextFormat::simple(font.clone(), color));
                job.break_on_newline = false;
                job.wrap.max_width = f32::INFINITY;
                let g = ui.painter().layout_job(job);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(g.size().x + 14.0), row_h), Sense::CLICK | Sense::DRAG);
                let origin = Pos2::new(rect.min.x, rect.center().y - g.size().y / 2.0);
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                }
                if let Some((from, to, goes_on)) = sel.and_then(|s| s.span(line, len)) {
                    let x = |col: usize| origin.x + g.pos_from_cursor(egui::text::CCursor::new(col)).min.x;
                    let (x0, mut x1) = (x(from), x(to));
                    if goes_on {
                        // the line break is part of the selection
                        x1 += 7.0;
                    }
                    if x1 > x0 {
                        ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())), 0.0, pal.selection);
                    }
                }
                ui.painter().galley(origin, g.clone(), color);
                widgets::context_popup(&resp, &pal).show(|ui| {
                    if ui.add_enabled_ui(sel.is_some_and(|s| !s.is_empty()), |ui| widgets::menu_row(ui, &pal, Some(Icon::Copy), tr("Copy"), "Ctrl+C")).inner.clicked() {
                        menu = Some(LogMenu::Copy);
                        ui.close();
                    }
                    if widgets::menu_row(ui, &pal, None, tr("Select all"), "Ctrl+A").clicked() {
                        menu = Some(LogMenu::SelectAll);
                        ui.close();
                    }
                    if widgets::menu_row(ui, &pal, None, tr("Copy all"), "").clicked() {
                        menu = Some(LogMenu::CopyAll);
                        ui.close();
                    }
                });
                rows.push((line, rect, origin, g, len, resp));
            }
            // the position (line, column) under a point; above or below the rows on screen: the first or last one
            let at = |p: Pos2| -> Option<Pos> {
                let (first, last) = (rows.first()?, rows.last()?);
                if p.y < first.1.top() {
                    return Some(Pos { line: first.0, col: 0 });
                }
                if p.y >= last.1.bottom() {
                    return Some(Pos { line: last.0, col: last.4 });
                }
                let r = rows.iter().find(|r| p.y >= r.1.top() && p.y < r.1.bottom())?;
                Some(Pos { line: r.0, col: col_at(&r.3, p - r.2, r.4) })
            };
            if (self.script.is_some() || cfg!(test))
                && let Some(first) = rows.first()
            {
                let char_w = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
                let shown = range.clone().map(|i| Self::log_row_text(&lines[i], prompt)).collect();
                ui.data_mut(|d| d.insert_temp(egui::Id::unique(LOG_GEOMETRY_ID), LogGeometry { left: first.1.left(), top: first.1.top(), row_h, char_w, rows: shown, ..Default::default() }));
            }
            // A selection is dragged while the button is down - not while the row it started on is on screen: scrolling
            // takes that row away (only the rows on screen exist), and the drag must go on.
            let drag_id = sel_id.with("dragging");
            let pos_id = sel_id.with("last_pos");
            let mut dragging = ui.data(|d| d.get_temp::<bool>(drag_id)).unwrap_or(false) && ui.input(|i| i.pointer.primary_down());
            // Dragged out of the window the mouse keeps its button but egui forgets where it is: the last place it was
            // (at the edge of the window) stands in, so the scrolling goes on.
            let pointer = ui.input(|i| i.pointer.latest_pos()).or_else(|| if dragging { ui.data(|d| d.get_temp::<Pos2>(pos_id)) } else { None });
            if rows.iter().any(|r| r.5.hovered()) && ui.input(|i| i.pointer.primary_pressed()) {
                // the keys must reach the log, not a text field somewhere else
                out.pressed = true;
                ui.ctx().memory_mut(|m| {
                    if let Some(f) = m.focused() {
                        m.surrender_focus(f);
                    }
                });
                if let Some(p) = pointer.and_then(at) {
                    sel = Some(Selection::new(p, p));
                    dragging = true;
                }
            }
            if dragging {
                if let (Some(s), Some(p)) = (sel.as_mut(), pointer.and_then(at)) {
                    s.head = p;
                }
                // dragging past the top or bottom scrolls, faster the further the mouse is (points per second, so the speed
                // does not depend on the frame rate; no animation: each frame moves the view straight away)
                if let Some(p) = pointer {
                    let clip = ui.clip_rect();
                    let past = if p.y < clip.top() { clip.top() - p.y } else if p.y > clip.bottom() { clip.bottom() - p.y } else { 0.0 };
                    if past != 0.0 {
                        let dt = ui.input(|i| i.stable_dt).min(0.05);
                        let speed = (past.abs() * 8.0 + 200.0).min(2500.0);
                        ui.scroll_with_delta_animation(Vec2::new(0.0, past.signum() * speed * dt), egui::style::ScrollAnimation::none());
                    }
                }
                ui.ctx().request_repaint();
            }
            ui.data_mut(|d| {
                d.insert_temp(drag_id, dragging);
                match pointer {
                    Some(p) if dragging => {
                        d.insert_temp(pos_id, p);
                    }
                    _ => {
                        d.remove::<Pos2>(pos_id);
                    }
                }
            });
            let clicked_row = |f: fn(&egui::Response) -> bool| rows.iter().find(|r| f(&r.5));
            if let Some(r) = clicked_row(egui::Response::triple_clicked) {
                sel = Some(Selection::new(Pos { line: r.0, col: 0 }, Pos { line: r.0, col: r.4 }));
            } else if let Some(r) = clicked_row(egui::Response::double_clicked) {
                let col = pointer.map_or(0, |p| col_at(&r.3, p - r.2, r.4));
                let (from, to) = word_range(&Self::log_row_text(&lines[r.0], prompt), col);
                sel = Some(Selection::new(Pos { line: r.0, col: from }, Pos { line: r.0, col: to }));
            } else if rows.iter().any(|r| r.5.clicked()) {
                sel = None;
            }
            // keyboard: copy, select all, and Escape to let go (never while the editor or a text field has the keys)
            let keys_here = !self.editor_focus && !ui.ctx().text_edit_focused() && !self.popup.is_open();
            out.keys_here = keys_here;
            if keys_here {
                let hovered = ui.rect_contains_pointer(ui.clip_rect());
                let has_sel = sel.is_some();
                let (copy, all, esc) = ui.input_mut(|i| {
                    (
                        i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                        // Ctrl+A is the log's when it has a selection or the mouse is over it; it is consumed only then
                        (has_sel || hovered) && i.consume_key(egui::Modifiers::COMMAND, Key::A),
                        has_sel && i.consume_key(egui::Modifiers::NONE, Key::Escape),
                    )
                });
                if all {
                    sel = select_all(lines.len(), row_len);
                }
                if copy && let Some(s) = sel.filter(|s| !s.is_empty()) {
                    copy_log_text(ui.ctx(), selected_text(&s, lines.len(), |i| Self::log_row_text(&lines[i], prompt)));
                    out.copied = true;
                }
                if esc {
                    sel = None;
                }
            }
        });
        if self.script.is_some() {
            // where the log is scrolled to, for the UI tests
            ui.data_mut(|d| {
                if let Some(mut g) = d.get_temp::<LogGeometry>(egui::Id::unique(LOG_GEOMETRY_ID)) {
                    g.offset_y = area.state.clamped_offset().y;
                    g.view_h = area.inner_rect.height();
                    g.max_offset = (area.content_size.y - area.inner_rect.height()).max(0.0);
                    d.insert_temp(egui::Id::unique(LOG_GEOMETRY_ID), g);
                }
            });
        }
        match menu {
            Some(LogMenu::Copy) => {
                if let Some(s) = sel.filter(|s| !s.is_empty()) {
                    copy_log_text(ui.ctx(), selected_text(&s, lines.len(), |i| Self::log_row_text(&lines[i], prompt)));
                    out.copied = true;
                }
            }
            Some(LogMenu::SelectAll) => sel = select_all(lines.len(), row_len),
            Some(LogMenu::CopyAll) => {
                if let Some(all) = select_all(lines.len(), row_len) {
                    copy_log_text(ui.ctx(), selected_text(&all, lines.len(), |i| Self::log_row_text(&lines[i], prompt)));
                    out.copied = true;
                }
            }
            None => {}
        }
        ui.data_mut(|d| match sel {
            Some(s) => {
                d.insert_temp(sel_id, s);
            }
            None => {
                d.remove::<Selection>(sel_id);
            }
        });
        out
    }

    /// What drawing a log found out: the mouse went down in it, or its text was copied.
    fn after_log(&mut self, out: LogOut, terminal: bool) {
        if out.pressed {
            self.editor_focus = false;
        }
        if out.copied {
            // select, Ctrl+C, and carry on typing
            if terminal {
                self.term.focus_input = true;
            } else {
                self.repl.focus_input = true;
            }
        }
    }

    fn console_input_height(&self, ui: &egui::Ui, text: &str) -> f32 {
        let row = ui.fonts_mut(|f| f.row_height(&egui::FontId::monospace(self.settings.font_size)));
        let wanted = row * (text.lines().count().max(1) + usize::from(text.ends_with('\n'))).min(6) as f32 + 36.0;
        wanted.min((ui.available_height() * 0.45).max(row + 36.0))
    }

    /// Rounded single/multi-line input row shared by REPL and terminal: the prompt and the first line of text
    /// share one baseline and are centred in a field of the standard height.
    fn prompt_input(&mut self, ui: &mut egui::Ui, id: egui::Id, prompt: &str, which: u8, hint: &str, height: f32) -> egui::Response {
        let pal = self.pal;
        let font = egui::FontId::monospace(self.settings.font_size);
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let vpad = ((widgets::FIELD_H - row_h) / 2.0).max(4.0) as i8;
        let mut resp = None;
        egui::Frame::new().fill(pal.input_bg).corner_radius(widgets::radius()).inner_margin(Margin::symmetric(14, vpad)).outer_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal_top(|ui| {
                // the default minimum control height (30) would make a one-line input taller than its text row
                ui.spacing_mut().interact_size.y = row_h;
                ui.label(RichText::new(prompt).font(font.clone()).color(pal.ok));
                let buf = if which == 0 { &mut self.repl.input } else { &mut self.term.input };
                let te = egui::TextEdit::multiline(buf)
                    .id(id)
                    .font(font.clone())
                    .text_color(pal.text)
                    .desired_rows(1)
                    .desired_width(f32::INFINITY)
                    .hint_text(RichText::new(hint).font(font.clone()).color(pal.text_dim))
                    .frame(egui::Frame::NONE)
                    .margin(Margin::ZERO)
                    .lock_focus(true);
                let h = (height - 16.0 - 2.0 * vpad as f32).max(row_h);
                egui::ScrollArea::vertical().id_salt(id.with("scroll")).max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show_fading(ui, |ui| {
                    resp = Some(ui.add(te));
                });
            });
        });
        resp.unwrap()
    }

    fn draw_repl(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let (dot, st) = if self.repl.starting {
                (pal.warning, tr("starting...").to_string())
            } else if self.repl.ready {
                (pal.ok, i18n::fmt("{0}   package {1}", &[&self.repl.version, &self.repl.package]))
            } else if self.lisp.is_none() {
                (pal.error, tr("no Lisp implementation found").to_string())
            } else {
                (pal.text_dim, tr("not running").to_string())
            };
            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, dot);
            ui.label(RichText::new(st).color(pal.text_dim).font(fonts::ui(12.0)));
            if self.repl.session.is_none() && self.lisp.is_some() && widgets::secondary_button(ui, &pal, tr("Start")).clicked() {
                self.ensure_repl();
            }
        });
        let input_h = self.console_input_height(ui, &self.repl.input);
        let avail = ui.available_height() - input_h - ui.spacing().item_spacing.y;
        let mut log_out = LogOut::default();
        ui.allocate_ui(Vec2::new(ui.available_width(), avail.max(0.0)), |ui| {
            let stick = self.repl.stick_to_bottom;
            log_out = self.draw_log(ui, "repl_log", &self.repl.log, stick, ">");
        });
        self.after_log(log_out, false);
        let id = egui::Id::unique("repl_input");
        let mut submit = false;
        let mut hist: Option<isize> = None;
        if ctx.memory(|m| m.has_focus(id)) {
            let (enter, shift, up, down) = ctx.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
            let complete = {
                let p = lisp_syntax::parse(&self.repl.input, lisp_syntax::Dialect::CommonLisp);
                !p.diagnostics.iter().any(|d| d.message.starts_with("Unclosed") || d.message.starts_with("Unterminated"))
            };
            let ctrl = ctx.input(|i| i.modifiers.command);
            if enter && !shift && (complete || ctrl) && !self.repl.input.trim().is_empty() {
                ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, Key::Enter);
                    i.consume_key(egui::Modifiers::COMMAND, Key::Enter);
                });
                submit = true;
            }
            if up && !self.repl.input.contains('\n') {
                hist = Some(-1);
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
            }
            if down && !self.repl.input.contains('\n') {
                hist = Some(1);
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
            }
        }
        let resp = self.prompt_input(ui, id, ">", 0, tr("Evaluate Lisp. Enter to run, Shift+Enter for a new line"), input_h);
        if self.repl.focus_input {
            resp.request_focus();
            self.repl.focus_input = false;
        }
        if resp.clicked() || resp.gained_focus() {
            self.editor_focus = false;
        }
        if let Some(dir) = hist {
            let n = self.repl.history.len() as isize;
            if n > 0 {
                let cur = self.repl.hist_pos.map_or(n, |p| p as isize);
                let next = (cur + dir).clamp(0, n);
                self.repl.hist_pos = if next == n { None } else { Some(next as usize) };
                self.repl.input = if next == n { String::new() } else { self.repl.history[next as usize].clone() };
            }
        }
        if submit {
            let code = std::mem::take(&mut self.repl.input);
            if self.repl.history.last() != Some(&code) {
                self.repl.history.push(code.clone());
            }
            self.repl.hist_pos = None;
            self.repl_eval(&code, true);
            resp.request_focus();
        }
    }

    fn draw_terminal(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let copy_requested = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        self.terminal_start_dir();
        ui.add_space(6.0);
        let scripts = self.project_scripts();
        let (mut run, mut pick) = (None::<std::path::PathBuf>, false);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let shown = match self.wsl_target() {
                Some(t) => format!("{}:{}", t.distro, lisp_runtime::wsl::to_linux(&self.term.cwd, &t)),
                None => self.term.cwd.display().to_string(),
            };
            ui.label(RichText::new(shown).color(pal.text_dim).font(fonts::ui(12.0)));
            if let Some(r) = &self.term.script {
                ui.label(RichText::new(i18n::fmt("script {0}: {1}/{2}", &[&r.name, &r.done, &r.total])).color(pal.accent).font(fonts::ui(12.0)));
            } else if self.term.handle.is_some() {
                ui.label(RichText::new(tr("running... (Ctrl+C to stop)")).color(pal.warning).font(fonts::ui(12.0)));
            }
            widgets::right_row(ui, 30.0, |ui| {
                ui.add_space(8.0);
                let resp = widgets::Pill::new(tr("Scripts")).icon(Icon::Script, pal.text_dim).chevron().bordered().height(28.0).show(ui, &pal);
                widgets::menu_popup(&resp, &pal).width(320.0).show(|ui| {
                    if scripts.is_empty() {
                        widgets::menu_caption(ui, &pal, tr("No scripts in the project (.als, .bat, .cmd, .ps1, .sh)"));
                    }
                    for (rel, abs) in &scripts {
                        if widgets::menu_row(ui, &pal, Some(Icon::Script), rel, "").clicked() {
                            run = Some(abs.clone());
                            ui.close();
                        }
                    }
                    ui.separator();
                    if widgets::menu_row(ui, &pal, Some(Icon::FolderOpen), tr("Choose a file..."), "").clicked() {
                        pick = true;
                        ui.close();
                    }
                });
            });
        });
        if let Some(p) = run {
            self.terminal_run_script(&p);
        }
        if pick {
            self.terminal_pick_script();
        }
        let input_h = self.console_input_height(ui, &self.term.input);
        let avail = ui.available_height() - input_h - ui.spacing().item_spacing.y;
        let mut log_out = LogOut::default();
        ui.allocate_ui(Vec2::new(ui.available_width(), avail.max(0.0)), |ui| {
            log_out = self.draw_log(ui, "term_log", &self.term.log, true, "$");
        });
        self.after_log(log_out, true);
        let id = egui::Id::unique("term_input");
        let mut submit = false;
        if ctx.memory(|m| m.has_focus(id)) {
            let (enter, up, down, tab) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::Tab) && !i.modifiers.any()));
            if enter && !ctx.input(|i| i.modifiers.shift) && !self.term.input.trim().is_empty() {
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                submit = true;
            }
            // the arrows walk through the history while the line is one line (or still the entry just recalled);
            // in a longer text they move the cursor between its lines
            if (up || down) && (!self.term.input.contains('\n') || self.terminal_browsing_history()) {
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, if up { Key::ArrowUp } else { Key::ArrowDown }));
                self.terminal_history_step(ctx, id, if up { -1 } else { 1 });
            }
            if tab {
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab));
                self.terminal_complete(ctx, id);
            }
        }
        let resp = self.prompt_input(ui, id, "$", 1, tr("Type a command and press Enter"), input_h);
        // Ctrl+C copies what is selected (the log did that above); with nothing selected it stops the running command
        if !log_out.copied && self.term_busy() && copy_requested {
            let input_selected = !self.term.input.is_empty() && egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range()).is_some_and(|r| r.primary != r.secondary);
            if (ctx.memory(|m| m.has_focus(id)) || log_out.keys_here) && !input_selected {
                self.terminal_interrupt();
            }
        }
        if self.term.focus_input {
            resp.request_focus();
            self.term.focus_input = false;
        }
        if resp.clicked() || resp.gained_focus() {
            self.editor_focus = false;
        }
        if submit {
            let line = std::mem::take(&mut self.term.input);
            self.terminal_run(&line);
            resp.request_focus();
        }
    }

    fn draw_run(&mut self, ui: &mut egui::Ui) {
        let tabs = self.run_tabs();
        if tabs.is_empty() {
            self.empty_state(ui, Icon::Play, self.pal.text_dim, tr("Nothing has been run yet. Press Shift+F10 to run the selected configuration."));
            return;
        }
        ui.add_space(2.0);
        self.draw_run_tabs(ui, &tabs);
        ui.add_space(4.0);
        if self.run.log.is_empty() && self.run.handle.is_none() {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new(tr("No output. Press Shift+F10 to run it again.")).color(self.pal.text_dim).font(fonts::ui(12.0)));
            });
            return;
        }
        let out = self.draw_log(ui, "run_log", &self.run.log, true, "$");
        if out.pressed {
            self.editor_focus = false;
        }
        // Ctrl+C in the output of a running program (after a click in it) stops the program, as in a terminal
        if self.run.running && !out.copied && out.keys_here && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            self.stop_run();
        }
    }

    /// The tabs of the Run window, one per file, configuration or scenario that was run, in the order they were opened:
    /// a dot tells how its program is doing (running, done, failed); a tab closes with its cross, a middle click or its
    /// menu, which also reruns or stops it.
    fn draw_run_tabs(&mut self, ui: &mut egui::Ui, tabs: &[(String, String, crate::app_run::RunStatus)]) {
        use crate::app_run::RunStatus;
        let pal = self.pal;
        let font = fonts::ui(12.5);
        let (mut show, mut close, mut rerun, mut stop, mut close_others) = (None, None, false, false, None);
        let mut rects: Vec<(String, Rect, Rect)> = Vec::new();
        let now = ui.input(|i| i.time);
        egui::ScrollArea::horizontal().id_salt("run_tabs").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show_fading(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for (key, title, status) in tabs {
                    let active = *key == self.run_key;
                    let title = crate::ui_chrome::elide(title, 40);
                    let g = ui.painter().layout_no_wrap(title.clone(), font.clone(), pal.text);
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(12.0 + 8.0 + 8.0 + g.size().x + 6.0 + 20.0 + 6.0, 30.0), Sense::click());
                    let resp = resp.on_hover_text(match status {
                        RunStatus::Running => tr("Running"),
                        RunStatus::Succeeded => tr("Finished successfully"),
                        RunStatus::Failed => tr("Finished with an error"),
                        RunStatus::Idle => tr("Stopped"),
                    });
                    if active {
                        ui.painter().rect_filled(r, CornerRadius::same(10), pal.tab_active_bg);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(10), pal.hover);
                    }
                    let dot = Pos2::new(r.min.x + 16.0, r.center().y);
                    match status {
                        RunStatus::Running => {
                            // a pulsing dot while the program runs
                            let t = ((now * 3.0).sin() * 0.5 + 0.5) as f32;
                            ui.painter().circle_filled(dot, 3.5 + t, pal.warning.gamma_multiply(0.35 + 0.65 * t));
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                        }
                        RunStatus::Succeeded => {
                            ui.painter().circle_filled(dot, 3.5, pal.ok);
                        }
                        RunStatus::Failed => {
                            ui.painter().circle_filled(dot, 3.5, pal.error);
                        }
                        RunStatus::Idle => {
                            ui.painter().circle_stroke(dot, 3.0, Stroke::new(1.2, pal.text_dim));
                        }
                    }
                    ui.painter().text(Pos2::new(r.min.x + 28.0, r.center().y), egui::Align2::LEFT_CENTER, &title, font.clone(), if active { pal.text } else { pal.text_dim });
                    let cr = Rect::from_center_size(Pos2::new(r.max.x - 16.0, r.center().y), Vec2::splat(18.0));
                    let close_resp = ui.interact(cr, ui.scope_id().with(("run_tab_close", key)), Sense::click()).on_hover_text(tr("Close (stops the program)"));
                    if active || resp.hovered() || close_resp.hovered() {
                        if close_resp.hovered() {
                            ui.painter().rect_filled(cr, CornerRadius::same(5), pal.selected);
                        }
                        icons::paint(ui.painter(), cr.shrink(4.5), Icon::Close, if close_resp.hovered() { pal.text } else { pal.text_dim });
                    }
                    rects.push((key.clone(), r, cr));
                    if close_resp.clicked() || resp.middle_clicked() {
                        close = Some(key.clone());
                    } else if resp.clicked() && !active {
                        show = Some(key.clone());
                    }
                    widgets::context_popup(&resp, &pal).show(|ui| {
                        if widgets::menu_row(ui, &pal, Some(Icon::Play), tr("Rerun"), "Shift+F10").clicked() {
                            show = Some(key.clone());
                            rerun = true;
                            ui.close();
                        }
                        if *status == RunStatus::Running && widgets::menu_row(ui, &pal, Some(Icon::Stop), tr("Stop"), "Ctrl+F2").clicked() {
                            show = Some(key.clone());
                            stop = true;
                            ui.close();
                        }
                        ui.separator();
                        if widgets::menu_row(ui, &pal, Some(Icon::Close), tr("Close"), "").clicked() {
                            close = Some(key.clone());
                            ui.close();
                        }
                        if tabs.len() > 1 && widgets::menu_row(ui, &pal, None, tr("Close Other Tabs"), "").clicked() {
                            close_others = Some(key.clone());
                            ui.close();
                        }
                    });
                }
                ui.add_space(12.0);
            });
        });
        ui.data_mut(|d| d.insert_temp(egui::Id::unique("run_tab_rects"), rects));
        if let Some(key) = show {
            self.show_run_tab(&key);
            self.editor_focus = false;
        }
        if rerun {
            self.rerun_tab();
        }
        if stop {
            self.stop_run();
        }
        if let Some(key) = close {
            self.close_run_tab(&key);
        }
        if let Some(keep) = close_others {
            for (key, _, _) in tabs {
                if *key != keep {
                    self.close_run_tab(key);
                }
            }
        }
    }

    pub fn empty_state(&self, ui: &mut egui::Ui, icon: Icon, color: Color32, text: &str) {
        let pal = self.pal;
        ui.add_space(26.0);
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
            icons::paint(ui.painter(), r, icon, color);
            ui.add_space(8.0);
            ui.label(RichText::new(text).color(pal.text_dim));
        });
    }

    fn draw_problems(&mut self, ui: &mut egui::Ui, problems: &[ProblemRow]) {
        let pal = self.pal;
        if problems.is_empty() {
            self.empty_state(ui, Icon::Check, pal.ok, tr("No problems found in open files"));
            return;
        }
        let mut goto: Option<(Option<PathBuf>, usize, usize)> = None;
        let root = self.project_root().map(PathBuf::from);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_fading(ui, |ui| {
            for p in problems {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H + 4.0), Sense::click());
                widgets::row_background(ui, &pal, r, false, resp.hovered());
                let (icon, color) = match p.sev {
                    Sev::Error => (Icon::Error, pal.error),
                    Sev::Warning => (Icon::Warning, pal.warning),
                    Sev::Weak => (Icon::Info, pal.text_dim),
                };
                icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(r.min.x + 18.0, r.center().y - 8.0), Vec2::splat(16.0)), icon, color);
                let g = ui.painter().layout_no_wrap(p.message.clone(), fonts::ui(13.5), pal.text);
                let gw = g.size().x;
                ui.painter().galley(Pos2::new(r.min.x + 44.0, r.center().y - g.size().y / 2.0), g, pal.text);
                let loc = match (&p.path, &root) {
                    (Some(path), Some(root)) => ide_core::workspace::relative(root, path),
                    (Some(path), None) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    _ => String::new(),
                };
                let line = self.docs.iter().find(|d| d.path == p.path).map(|d| d.ed.line_col(p.start.min(d.ed.text().len())).0 + 1);
                let loc = match line {
                    Some(l) if !loc.is_empty() => format!("{loc}:{l}"),
                    _ => loc,
                };
                ui.painter().text(Pos2::new(r.min.x + 58.0 + gw, r.center().y), egui::Align2::LEFT_CENTER, format!("{loc}   {}", p.source), fonts::ui(12.0), pal.text_dim);
                if resp.clicked() {
                    goto = Some((p.path.clone(), p.start, p.end));
                }
            }
        });
        if let Some((Some(path), s, e)) = goto {
            self.goto_range(&path, s, e, true);
        }
    }

    fn draw_find_results(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.find_results.hits.is_empty() && self.find_rx.is_some() {
            self.empty_state(ui, Icon::Search, pal.accent, tr("Searching the project..."));
            return;
        }
        if self.find_results.hits.is_empty() {
            self.empty_state(ui, Icon::Search, pal.text_dim, tr("No results. Ctrl+Shift+F searches the project, Alt+F7 finds usages."));
            return;
        }
        let hits = self.find_results.hits.clone();
        let title = self.find_results.title.clone();
        let root = self.project_root().map(PathBuf::from);
        let mut goto: Option<(PathBuf, usize, usize)> = None;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new(i18n::fmt("{0}   {1} match(es)", &[&title, &hits.len()])).color(pal.text_dim).font(fonts::ui(12.0)));
        });
        let mono = egui::FontId::monospace((self.settings.font_size - 1.0).max(9.0));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_fading(ui, |ui| {
            let mut last: Option<&PathBuf> = None;
            for h in &hits {
                if last != Some(&h.path) {
                    last = Some(&h.path);
                    let rel = root.as_ref().map(|r| ide_core::workspace::relative(r, &h.path)).unwrap_or_else(|| h.path.to_string_lossy().into_owned());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        let (icon, color) = if self.lisp_enabled() && lisp_syntax::dialect::is_lisp_file(&h.path) {
                            (Icon::Lisp, pal.keyword_form)
                        } else {
                            (Icon::File, pal.text_dim)
                        };
                        icons::paint(ui.painter(), r, icon, color);
                        ui.label(RichText::new(rel).font(fonts::semibold(13.0)));
                    });
                }
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                widgets::row_background(ui, &pal, r, false, resp.hovered());
                ui.painter().text(Pos2::new(r.min.x + 40.0, r.center().y), egui::Align2::LEFT_CENTER, format!("{:>4}", h.line + 1), mono.clone(), pal.text_dim);
                let text = h.line_text.trim_start();
                let lead = h.line_text.chars().count() - text.chars().count();
                let col = h.col.saturating_sub(lead);
                let pre: String = text.chars().take(col).collect();
                let mid: String = text.chars().skip(col).take(h.len).collect();
                let post: String = text.chars().skip(col + h.len).take(200).collect();
                let mut job = egui::text::LayoutJob::default();
                job.append(&pre, 0.0, egui::TextFormat::simple(mono.clone(), pal.plain));
                job.append(&mid, 0.0, egui::TextFormat { background: pal.find_match, ..egui::TextFormat::simple(mono.clone(), pal.text) });
                job.append(&post, 0.0, egui::TextFormat::simple(mono.clone(), pal.plain));
                let g = ui.painter().layout_job(job);
                ui.painter().galley(Pos2::new(r.min.x + 84.0, r.center().y - g.size().y / 2.0), g, pal.plain);
                if resp.clicked() {
                    let off = std::fs::read_to_string(&h.path).ok().map(|t| {
                        let ls = t.split_inclusive('\n').take(h.line).map(str::len).sum::<usize>();
                        let line = t[ls..].lines().next().unwrap_or("");
                        let b = line.char_indices().nth(h.col).map_or(line.len(), |(b, _)| b);
                        let e = line.char_indices().nth(h.col + h.len).map_or(line.len(), |(b, _)| b);
                        (ls + b, ls + e)
                    });
                    if let Some((s, e)) = off {
                        goto = Some((h.path.clone(), s, e));
                    }
                }
            }
        });
        if let Some((p, s, e)) = goto {
            self.goto_range(&p, s, e, true);
        }
    }

    // ------------------------------------------------------------------ git tool window

    fn draw_git(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let pal = self.pal;
        if self.project.is_none() {
            self.empty_state(ui, Icon::Branch, pal.text_dim, tr("Open a project to use version control"));
            return;
        }
        ui.add_space(8.0);
        let mut commit_local = false;
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let local_n = self.git.local.as_ref().map_or(0, |l| l.commits().len());
            let tabs = [(GitTab::Log, tr("Log").to_string()), (GitTab::Local, i18n::fmt("Local ({})", &[&local_n]))];
            for (t, label) in tabs {
                let active = self.git.tab == t;
                let g = ui.painter().layout_no_wrap(label.clone(), fonts::ui(13.0), pal.text);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 24.0, 28.0), Sense::click());
                if active {
                    ui.painter().rect_filled(r, CornerRadius::same(7), pal.selected.gamma_multiply(0.6));
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(7), pal.hover);
                }
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, label, fonts::ui(13.0), if active { pal.text } else { pal.text_dim });
                if resp.clicked() {
                    self.git.tab = t;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(14.0);
                if widgets::secondary_button(ui, &pal, tr("Local Commit")).clicked() {
                    commit_local = true;
                }
            });
        });
        ui.add_space(6.0);
        if commit_local {
            self.popup = Popup::LocalCommit { input: String::new(), just_opened: true };
        }
        match self.git.tab {
            GitTab::Log => self.draw_git_log(ui),
            GitTab::Local => self.draw_local_commits(ui),
        }
    }

    fn draw_git_log(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.git.repo.is_none() {
            self.empty_state(ui, Icon::History, pal.text_dim, tr("No git history: this project is not a Git repository"));
            return;
        }
        if self.git.log.is_empty() {
            self.empty_state(ui, Icon::History, pal.text_dim, tr("No commits yet"));
            return;
        }
        let log = self.git.log.clone();
        let graph = std::mem::take(&mut self.git.graph);
        let cols = graph.iter().map(|g| g.width).max().unwrap_or(1).clamp(1, MAX_LANES);
        let head = self.git.head.clone();
        let mut show: Option<usize> = None;
        let row_h = ROW_H + 2.0;
        egui::ScrollArea::vertical().id_salt("git_log").auto_shrink([false, false]).show_rows_fading(ui, row_h, log.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let c = &log[i];
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                widgets::row_background(ui, &pal, r, false, resp.hovered());
                let cy = r.center().y;
                let lane_x = |l: usize| r.min.x + 22.0 + l.min(MAX_LANES - 1) as f32 * LANE_W;
                let mut tx = r.min.x + 22.0 + cols as f32 * LANE_W + 8.0;
                if let Some(g) = graph.get(i) {
                    let p = ui.painter();
                    let stroke = |color: usize| Stroke::new(1.8, lane_color(color));
                    let node = Pos2::new(lane_x(g.lane), cy);
                    for &(l, col) in &g.through {
                        p.line_segment([Pos2::new(lane_x(l), r.min.y), Pos2::new(lane_x(l), r.max.y)], stroke(col));
                    }
                    for &(l, col) in &g.incoming {
                        lane_edge(p, Pos2::new(lane_x(l), r.min.y), node, stroke(col));
                    }
                    for &(l, col) in &g.outgoing {
                        lane_edge(p, node, Pos2::new(lane_x(l), r.max.y), stroke(col));
                    }
                    let is_head = head.as_deref() == Some(c.hash.as_str());
                    if c.parents.len() > 1 {
                        p.circle_filled(node, 5.0, pal.panel_bg);
                        p.circle_stroke(node, 4.0, Stroke::new(2.0, lane_color(g.color)));
                    } else {
                        p.circle_filled(node, 4.5, lane_color(g.color));
                    }
                    if is_head {
                        p.circle_stroke(node, 7.5, Stroke::new(1.5, pal.text));
                    }
                } else {
                    ui.painter().circle_filled(Pos2::new(lane_x(0), cy), 4.0, pal.text_dim);
                }
                ui.painter().text(Pos2::new(tx, cy), egui::Align2::LEFT_CENTER, &c.short, egui::FontId::monospace(12.5), pal.text_dim);
                tx += 70.0;
                let g = ui.painter().layout_no_wrap(c.subject.clone(), fonts::ui(13.5), pal.text);
                let gw = g.size().x.min((r.max.x - tx) * 0.5).max(40.0);
                let clip = ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(tx, r.min.y), Pos2::new(tx + gw, r.max.y)));
                clip.galley(Pos2::new(tx, cy - g.size().y / 2.0), g, pal.text);
                tx += gw + 10.0;
                for name in &c.refs {
                    tx = ref_chip(ui.painter(), &pal, Pos2::new(tx, cy), name) + 6.0;
                }
                ui.painter().text(Pos2::new(r.max.x - 12.0, cy), egui::Align2::RIGHT_CENTER, format!("{}   {}", c.author, c.when), fonts::ui(12.0), pal.text_dim);
                if resp.clicked() {
                    show = Some(i);
                }
            }
        });
        self.git.graph = graph;
        if let Some(i) = show {
            self.git_show_commit(i);
        }
    }
}


const LANE_W: f32 = 16.0;
const MAX_LANES: usize = 8;

/// Branch line colours; the graph cycles through them.
fn lane_color(i: usize) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(0x5A, 0x86, 0xF5),
        Color32::from_rgb(0x4C, 0xB0, 0x7A),
        Color32::from_rgb(0xE0, 0xA0, 0x40),
        Color32::from_rgb(0xB0, 0x7C, 0xE8),
        Color32::from_rgb(0xE0, 0x6C, 0x75),
        Color32::from_rgb(0x3C, 0xB8, 0xC4),
        Color32::from_rgb(0xD6, 0x6F, 0xB0),
        Color32::from_rgb(0x8C, 0xA8, 0x4B),
    ];
    COLORS[i % COLORS.len()]
}

/// A graph edge between two points: straight when they share a lane, an S-curve otherwise.
fn lane_edge(p: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    if (from.x - to.x).abs() < 0.5 {
        p.line_segment([from, to], stroke);
    } else {
        let mid = (from.y + to.y) / 2.0;
        p.add(egui::epaint::CubicBezierShape::from_points_stroke([from, Pos2::new(from.x, mid), Pos2::new(to.x, mid), to], false, Color32::TRANSPARENT, stroke));
    }
}

/// A small label for a branch/tag/HEAD decoration; returns the x where it ends.
fn ref_chip(p: &egui::Painter, pal: &crate::theme::Palette, at: Pos2, raw: &str) -> f32 {
    let (text, color) = if let Some(cur) = raw.strip_prefix("HEAD -> ") {
        (cur.to_string(), pal.accent)
    } else if let Some(tag) = raw.strip_prefix("tag: ") {
        (tag.to_string(), pal.warning)
    } else if raw.contains('/') {
        (raw.to_string(), pal.text_dim)
    } else {
        (raw.to_string(), pal.ok)
    };
    let g = p.layout_no_wrap(text, fonts::semibold(11.5), color);
    let rect = Rect::from_min_size(Pos2::new(at.x, at.y - 9.0), Vec2::new(g.size().x + 14.0, 18.0));
    p.rect_filled(rect, CornerRadius::same(7), color.gamma_multiply(0.2));
    p.galley(Pos2::new(rect.min.x + 7.0, rect.center().y - g.size().y / 2.0), g, color);
    rect.max.x
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use ide_core::settings::Settings;

    fn project() -> Project {
        let root = PathBuf::from("C:/light-layout-project");
        let rows: Vec<_> = (0..200).map(|i| TreeRow { path: root.join(format!("file{i:03}.rs")), depth: 1, is_dir: false, expanded: false, name: format!("file{i:03}.rs"), ignored: false }).collect();
        Project { root: root.clone(), name: "layout".into(), tree_ver: 0, tree_cache: Some((0, rows)), ignore: ide_core::workspace::IgnoreMatcher::new(&root), expanded: Default::default(), selected: None, dir_cache: Default::default(), files: vec![root.join("collapsed/Navigator.rs"), root.join("other.rs")], files_rel: vec![], files_gen: 0, index: Default::default(), index_rx: None, branch: None, hidden: Default::default() }
    }

    #[test]
    fn project_uses_the_full_tall_viewport_and_searches_collapsed_files() {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let mut app = App::new(Settings::default());
        app.project = Some(project());
        for query in ["", "nAvIg", "missing", ""] {
            app.project_filter = query.into();
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 1400.0))), ..Default::default() }, |ui| {
                app.draw_project_panel(ui, &ctx);
            });
            let texts: Vec<_> = output.shapes.iter().filter_map(|s| match &s.shape { egui::Shape::Text(t) => Some((t.galley.job.text.as_str(), t.pos.y)), _ => None }).collect();
            if query.is_empty() {
                let last = texts.iter().filter(|(s, _)| s.starts_with("file")).map(|(_, y)| *y).fold(0.0, f32::max);
                assert!(last > 1300.0, "tree stopped early at {last}");
            } else {
                assert_eq!(texts.iter().filter(|(s, _)| *s == "Navigator.rs").count(), usize::from(query == "nAvIg"));
                assert!(!texts.iter().any(|(s, _)| s.starts_with("file")));
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn submitting_a_command_keeps_terminal_focus() {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        let mut app = App::new(Settings::default());
        app.term.input = "help".into();
        app.term.focus_input = true;
        for frame in 0..5 {
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 250.0))),
                events: if frame == 2 { vec![egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }] } else { vec![] },
                ..Default::default()
            }, |ui| { app.draw_terminal(ui, &ctx); });
            output.textures_delta.clear();
            assert!(ctx.memory(|m| m.has_focus(egui::Id::unique("term_input"))), "lost focus in frame {frame}: {:?}, editor={}", ctx.memory(|m| m.focused()), app.editor_focus);
        }
        assert!(app.term.input.is_empty());
    }

    #[test]
    fn multiline_console_stays_inside_the_panel() {
        for height in [120.0, 180.0, 280.0, 600.0] {
            for lines in [1, 2, 8, 100] {
                let ctx = egui::Context::default();
                fonts::install(&ctx);
                let mut app = App::new(Settings::default());
                app.term.input = "echo test\n".repeat(lines);
                app.term.log = vec![LogLine { kind: LogKind::Output, text: "first\nsecond\nthird".into() }];
                app.repl.input = app.term.input.clone();
                app.repl.log = app.term.log.clone();
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let bounds = Rect::from_min_size(ui.cursor().min, Vec2::new(640.0, height));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
                    app.draw_terminal(&mut child, &ctx);
                    assert!(child.min_rect().bottom() <= bounds.bottom() + 1.0, "{lines} lines overflow {height}: {:?}", child.min_rect());
                    let g = ctx.data(|d| d.get_temp::<LogGeometry>(egui::Id::unique(LOG_GEOMETRY_ID))).unwrap();
                    assert!(g.rows.iter().all(|r| !r.contains('\n')));
                    let mut repl_ui = ui.new_child(egui::UiBuilder::new().id_salt("repl_test").max_rect(bounds));
                    app.draw_repl(&mut repl_ui, &ctx);
                    assert!(repl_ui.min_rect().bottom() <= bounds.bottom() + 1.0, "REPL {lines} lines overflow {height}: {:?}", repl_ui.min_rect());
                });
                output.textures_delta.clear();
            }
        }
    }
}
