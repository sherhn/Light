//! The buttons next to the line numbers and the menu a click on one opens: run a test, send an HTTP request, debug.
//! Each feature says which buttons a document has ([`App::gutter_icons`]) and what its menu offers
//! ([`App::gutter_actions`]); the menu carries out the chosen [`GutterAction`].

use std::path::PathBuf;

use eframe::egui::{self, CornerRadius, Key, Margin, Pos2, Rect};

use crate::app::App;
use crate::editor_view::GutterIcon;
use crate::icons::Icon;
use crate::widgets;

/// What a row of the gutter menu does.
#[derive(Clone, Debug, PartialEq)]
pub enum GutterAction {
    /// Run a test or suite of the file (none: every test of the file), measuring coverage or not.
    RunTests(PathBuf, Option<ide_core::testing::TestItem>, bool),
    /// Send the request on a line of a `.http` file, with an environment.
    SendRequest(PathBuf, usize, Option<String>),
    RunFile(PathBuf),
    RunLanguage(PathBuf,ide_core::modules::RunDef),
    RunCode(PathBuf,ide_core::modules::CodeActionDef,String),
    RunLispFunction(PathBuf,ide_core::code_actions::LispFunction),
    RunTask(PathBuf,ide_core::modules::TaskDef,String),
}

/// The open gutter menu.
#[derive(Clone, Debug)]
pub struct GutterMenu {
    pub doc_id: u64,
    pub line: usize,
    pub anchor: Pos2,
    pub items: Vec<(Icon, String, GutterAction)>,
    /// Where the rows were drawn (the UI tests click them).
    pub rows: Vec<(String, Rect)>,
}

/// Where the gutter buttons of the focused editor were drawn last, for the UI tests.
pub const ICON_RECTS_ID: &str = "gutter_icon_rects";

impl App {
    /// The gutter buttons of document `idx`.
    pub fn gutter_icons(&mut self, idx: usize) -> Vec<GutterIcon> {
        let mut icons = self.test_icons(idx);
        icons.extend(self.http_icons(idx));
        let specific=!icons.is_empty();
        icons.extend(self.code_run_icons(idx,specific));
        icons.sort_by_key(|icon|icon.line);
        icons.dedup_by_key(|icon|icon.line);
        icons
    }

    /// What the menu of the button on `line` of document `idx` offers.
    pub fn gutter_actions(&mut self, idx: usize, line: usize) -> Vec<(Icon, String, GutterAction)> {
        let mut actions = self.test_actions(idx, line);
        actions.extend(self.http_actions(idx, line));
        actions.extend(self.code_run_actions(idx,line));
        actions
    }

    /// A gutter button was clicked: its menu opens below it (a second click closes it).
    pub fn open_gutter_menu(&mut self, idx: usize, line: usize, anchor: Pos2) {
        let doc_id = self.docs[idx].id;
        if self.gutter_menu.as_ref().is_some_and(|m| m.doc_id == doc_id && m.line == line) {
            self.gutter_menu = None;
            return;
        }
        let items = self.gutter_actions(idx, line);
        self.gutter_menu = (!items.is_empty()).then_some(GutterMenu { doc_id, line, anchor, items, rows: vec![] });
    }

    pub fn do_gutter_action(&mut self, action: GutterAction) {
        match action {
            GutterAction::RunTests(path, item, coverage) => self.run_tests_with(&path, item.as_ref(), coverage),
            GutterAction::SendRequest(path, line, env) => self.send_request(&path, line, env.as_deref()),
            GutterAction::RunFile(path)=>{if self.open_file(&path) {self.run_current_file();}},
            GutterAction::RunLanguage(path,def)=>{
                if self.mods.installed.iter().any(|module|module.enabled&&module.manifest.run.contains(&def)) {self.run_language_command(&def,&path);}
            },
            GutterAction::RunCode(path,def,symbol)=>self.run_code_action(&path,&def,&symbol),
            GutterAction::RunLispFunction(path,function)=>self.run_lisp_function(&path,&function),
            GutterAction::RunTask(path,def,task)=>{
                if self.mods.installed.iter().any(|module|module.enabled&&module.manifest.tasks.contains(&def)) {self.run_task(&def,&path,&task);}
            },
        }
    }

    pub fn draw_gutter_menu(&mut self, ctx: &egui::Context) {
        let Some(menu) = self.gutter_menu.clone() else { return };
        if self.doc().is_none_or(|d| d.id != menu.doc_id) || self.popup.is_open() || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.gutter_menu = None;
            return;
        }
        let pal = self.pal;
        let mut chosen = None;
        let mut rows = Vec::new();
        let area = egui::Area::new(egui::Id::unique("gutter_menu")).order(egui::Order::Foreground).fixed_pos(menu.anchor).show(ctx, |ui| {
            egui::Frame::new().fill(pal.popup_bg.to_opaque()).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(6)).show(ui, |ui| {
                ui.set_min_width(220.0);
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-80.0).clamp(100.0,480.0)).show(ui,|ui| {for (icon, label, action) in &menu.items {
                    let resp = widgets::menu_row(ui, &pal, Some(*icon), label, "");
                    rows.push((label.clone(), resp.rect));
                    if resp.clicked() {
                        chosen = Some(action.clone());
                    }
                }});
            });
        });
        if let Some(m) = self.gutter_menu.as_mut() {
            m.rows = rows;
        }
        // a click anywhere else closes the menu (the click that opened it is in the gutter, below the anchor's row)
        let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && (p.y < menu.anchor.y - 40.0 || p.y > menu.anchor.y || p.x > menu.anchor.x + 40.0)));
        if let Some(action) = chosen {
            self.gutter_menu = None;
            self.do_gutter_action(action);
        } else if outside {
            self.gutter_menu = None;
        }
    }
}
