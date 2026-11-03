//! The Debug window: the buttons that drive the program, its call stack, the variables of the frame shown and the
//! console (its output, and expressions evaluated where it stopped).

use eframe::egui::{self, Align, Key, Layout, Pos2, Rect, RichText, Sense, Vec2};

use crate::app::App;
use crate::app_debug::DebugStatus;
use crate::fonts;
use crate::i18n::tr;
use crate::icons::{self, Icon};
use crate::widgets::{self, ROW_H};

impl App {
    pub fn draw_debug(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let status = self.debug.status;
        // the buttons, and what the program is doing
        let mut act: Option<&str> = None;
        let mut rects = Vec::new();
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            let paused = status == DebugStatus::Paused;
            let running = status == DebugStatus::Running;
            let live = !matches!(status, DebugStatus::Idle | DebugStatus::Downloading);
            for (key, icon, tip, on, color) in [
                ("continue", Icon::Play, "Resume (F9)", paused, pal.ok),
                ("pause", Icon::Pause, "Pause", running, pal.text_dim),
                ("next", Icon::StepOver, "Step Over (F8)", paused, pal.accent),
                ("stepIn", Icon::StepInto, "Step Into (F11)", paused, pal.accent),
                ("stepOut", Icon::StepOut, "Step Out (Shift+F8)", paused, pal.accent),
                ("stop", Icon::Stop, "Stop Debugging", live, pal.error),
            ] {
                let r = ui.add_enabled_ui(on, |ui| widgets::icon_button(ui, &pal, icon, 30.0, if on { color } else { pal.text_dim.gamma_multiply(0.5) }, tr(tip))).inner;
                rects.push((key, r.rect));
                if r.clicked() {
                    act = Some(key);
                }
            }
            ui.add_space(10.0);
            let text = match status {
                DebugStatus::Idle => tr("Not debugging. Debug a file with Shift+F9; a click on a line number sets a breakpoint.").to_string(),
                DebugStatus::Downloading => tr("Downloading the debugger...").to_string(),
                DebugStatus::Starting => tr("Starting...").to_string(),
                DebugStatus::Running => tr("Running").to_string(),
                DebugStatus::Paused => self.debug.stopped.as_ref().map(|s| s.reason.clone()).unwrap_or_default(),
            };
            ui.label(RichText::new(text).color(if status == DebugStatus::Paused { pal.warning } else { pal.text_dim }).size(12.5));
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("debug_button_rects"), rects));
        match act {
            Some("stop") => self.stop_debug(),
            Some(c) => self.debug_command(c),
            None => {}
        }
        ui.add_space(4.0);
        let avail = ui.available_size();
        let col = ((avail.x - 24.0) / 3.0).max(160.0);
        let mut frame_click = None;
        let mut expand = None;
        ui.horizontal_top(|ui| {
            ui.add_space(8.0);
            // the call stack
            ui.allocate_ui_with_layout(Vec2::new(col, avail.y - 8.0), Layout::top_down(Align::Min), |ui| {
                ui.label(RichText::new(tr("Frames")).font(fonts::semibold(12.5)).color(pal.text_dim));
                egui::ScrollArea::vertical().id_salt("debug_frames").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if let Some(s) = &self.debug.stopped {
                        for (n, f) in s.frames.iter().enumerate() {
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 4.0), Sense::click());
                            widgets::row_background(ui, &pal, r, n == s.frame, resp.hovered());
                            let place = f.path.as_ref().and_then(|p| p.file_name()).map(|n| format!("{}:{}", n.to_string_lossy(), f.line)).unwrap_or_default();
                            let color = if f.path.is_some() { pal.text } else { pal.text_dim };
                            let end = ui.painter().text(Pos2::new(r.min.x + 10.0, r.center().y), egui::Align2::LEFT_CENTER, &f.name, fonts::ui(13.0), color).max.x;
                            ui.painter().text(Pos2::new(end + 8.0, r.center().y), egui::Align2::LEFT_CENTER, place, fonts::ui(11.5), pal.text_dim);
                            if resp.clicked() {
                                frame_click = Some(n);
                            }
                        }
                    }
                });
            });
            ui.separator();
            // the variables of the frame, a tree opened a level at a time
            ui.allocate_ui_with_layout(Vec2::new(col, avail.y - 8.0), Layout::top_down(Align::Min), |ui| {
                ui.label(RichText::new(tr("Variables")).font(fonts::semibold(12.5)).color(pal.text_dim));
                egui::ScrollArea::vertical().id_salt("debug_vars").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let scopes = self.debug.stopped.as_ref().map(|s| s.scopes.clone()).unwrap_or_default();
                    let mut rows: Vec<(usize, String, String, i64)> = Vec::new();
                    fn walk(app: &App, reference: i64, depth: usize, rows: &mut Vec<(usize, String, String, i64)>) {
                        if depth > 12 || !app.debug.expanded.contains(&reference) {
                            return;
                        }
                        for v in app.debug.variables.get(&reference).into_iter().flatten() {
                            rows.push((depth, v.name.clone(), v.value.clone(), v.reference));
                            if v.reference != 0 {
                                walk(app, v.reference, depth + 1, rows);
                            }
                        }
                    }
                    for s in &scopes {
                        rows.push((0, s.name.clone(), String::new(), s.reference));
                        walk(self, s.reference, 1, &mut rows);
                    }
                    for (depth, name, value, reference) in rows {
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 6.0), Sense::click());
                        widgets::row_background(ui, &pal, r, false, resp.hovered());
                        let x = r.min.x + 8.0 + depth as f32 * 14.0;
                        if reference != 0 {
                            let open = self.debug.expanded.contains(&reference);
                            icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(x, r.center().y - 5.0), Vec2::splat(10.0)), if open { Icon::ChevronDown } else { Icon::ChevronRight }, pal.text_dim);
                        }
                        let end = ui.painter().text(Pos2::new(x + 14.0, r.center().y), egui::Align2::LEFT_CENTER, &name, if depth == 0 { fonts::semibold(12.5) } else { egui::FontId::monospace(12.5) }, if depth == 0 { pal.text_dim } else { pal.special_var }).max.x;
                        if !value.is_empty() {
                            ui.painter().text(Pos2::new(end + 6.0, r.center().y), egui::Align2::LEFT_CENTER, format!("= {value}"), egui::FontId::monospace(12.5), pal.text);
                        }
                        if reference != 0 && resp.clicked() {
                            expand = Some(reference);
                        }
                    }
                });
            });
            ui.separator();
            // the console: output, and expressions evaluated where the program is
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width() - 8.0, avail.y - 8.0), Layout::top_down(Align::Min), |ui| {
                ui.label(RichText::new(tr("Console")).font(fonts::semibold(12.5)).color(pal.text_dim));
                let input_h = 34.0;
                let mut evaluate = None;
                let lines = self.debug.console.clone();
                ui.allocate_ui(Vec2::new(ui.available_width(), (ui.available_height() - input_h).max(40.0)), |ui| {
                    let _ = self.draw_log(ui, "debug_console", &lines, true, ">");
                });
                let response = ui.add_sized([ui.available_width(), 28.0], egui::TextEdit::singleline(&mut self.debug.input).hint_text(tr("Evaluate an expression (Enter)")).font(egui::FontId::monospace(13.0)));
                if response.gained_focus() {
                    self.editor_focus = false;
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    evaluate = Some(std::mem::take(&mut self.debug.input));
                    response.request_focus();
                }
                if let Some(e) = evaluate {
                    self.debug_evaluate(&e);
                }
            });
        });
        if let Some(n) = frame_click {
            self.select_frame(n);
        }
        if let Some(r) = expand {
            self.expand_variables(r);
        }
    }
}
