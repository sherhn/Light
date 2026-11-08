//! The Debug window: the buttons that drive the program, its call stack, the variables of the frame shown and the
//! console (its output, and expressions evaluated where it stopped).

use eframe::egui::{self, Align, Key, Layout, Pos2, Rect, RichText, Sense, Vec2};

use crate::app::App;
use crate::app_debug::{DebugStatus, DebugTab};
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::widgets::{self, ROW_H};

/// Adapter values are arbitrary text, not a single display line (Node strings
/// can contain entire HTML documents). Keep rows compact without changing data.
fn value_preview(value: &str) -> String {
    let mut preview=String::new();
    for (index,ch) in value.chars().enumerate() {
        if index==2048 {preview.push('…');break}
        match ch {
            '\n'|'\u{2028}'|'\u{2029}'=>preview.push_str("\\n"),
            '\r'=>preview.push_str("\\r"), '\t'=>preview.push_str("\\t"),
            ch if ch.is_control()=>preview.push('�'),
            ch=>preview.push(ch),
        }
    }
    preview
}

fn row_text(ui: &egui::Ui, text: &str, font: egui::FontId, color: egui::Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job=egui::text::LayoutJob::simple(value_preview(text),font,color,width.max(0.0));
    job.break_on_newline=false;
    job.wrap.max_rows=1;job.wrap.break_anywhere=true;job.wrap.overflow_character=Some('…');
    ui.painter().layout_job(job)
}

fn variable_row(ui: &mut egui::Ui, pal: &crate::theme::Palette, depth: usize, name: &str, value: &str, branch: Option<bool>) -> egui::Response {
    let (rect,response)=ui.allocate_exact_size(Vec2::new(ui.available_width(),ROW_H-6.0),Sense::click());
    let painter=ui.painter_at(rect.intersect(ui.clip_rect()));
    widgets::row_background(ui,pal,rect,false,response.hovered());
    let x=rect.min.x+8.0+depth as f32*14.0;
    if let Some(open)=branch {
        icons::paint(&painter,Rect::from_min_size(Pos2::new(x,rect.center().y-5.0),Vec2::splat(10.0)),if open {Icon::ChevronDown}else{Icon::ChevronRight},pal.text_dim);
    }
    let name_start=x+14.0;
    let available=(rect.max.x-name_start-8.0).max(0.0);
    let name_width=if value.is_empty(){available}else{available*0.45};
    let name=row_text(ui,name,if depth==0{fonts::semibold(12.5)}else{egui::FontId::monospace(12.5)},if depth==0{pal.text_dim}else{pal.special_var},name_width);
    let end=name_start+name.size().x;
    painter.galley(Pos2::new(name_start,rect.center().y-name.size().y/2.0),name,pal.text);
    if !value.is_empty() {
        let value=row_text(ui,&format!("= {}",value_preview(value)),egui::FontId::monospace(12.5),pal.text,rect.max.x-end-14.0);
        painter.galley(Pos2::new(end+6.0,rect.center().y-value.size().y/2.0),value,pal.text);
    }
    response
}

fn variable_tooltip(response: &egui::Response, name: &str, value: &str, controls_bottom: f32) {
    let screen = response.ctx.content_rect();
    let above = response.rect.top() - controls_bottom;
    let below = screen.bottom() - response.rect.bottom();
    let (align, room) = if above >= below {
        (egui::emath::RectAlign::TOP_END, above)
    } else {
        (egui::emath::RectAlign::BOTTOM_END, below)
    };
    let width = (screen.width() - 32.0).min(580.0);
    if room < 100.0 {
        // Leave the controls clickable even when the Debug pane is very short.
        egui::Tooltip::for_enabled(response).width(width).show(|ui| {
            ui.add(egui::Label::new(RichText::new(value_preview(value)).monospace()).truncate());
        });
        return;
    }
    let mut tooltip = egui::Tooltip::for_enabled(response).width(width);
    tooltip.popup = tooltip.popup.align(align).align_alternatives(&[]);
    tooltip.show(|ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            if ui.button(tr("Copy value")).clicked() {
                ui.ctx().copy_text(value.to_owned());
            }
            ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
        });
        egui::ScrollArea::vertical().max_height((room - 68.0).min(300.0)).show(ui, |ui| {
            let mut text = value;
            ui.add(egui::TextEdit::multiline(&mut text).font(egui::FontId::monospace(12.5)).desired_width(ui.available_width()));
        });
    });
}


/// `0x...` of an address.
fn hex_address(a: u64) -> String {
    format!("0x{a:016x}")
}

/// Rows of a hex dump: address, 16 bytes in hex, and as text.
pub fn hex_rows(address: u64, bytes: &[u8]) -> Vec<(String, String, String)> {
    bytes.chunks(16).enumerate().map(|(n, chunk)| {
        let hex = chunk.iter().enumerate().map(|(i, b)| if i == 8 { format!(" {b:02x}") } else { format!("{b:02x}") }).collect::<Vec<_>>().join(" ");
        let text: String = chunk.iter().map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' }).collect();
        (hex_address(address + n as u64 * 16), hex, text)
    }).collect()
}

impl App {
    /// The chips over the middle of the Debug window; the disassembly and the memory only with an adapter that has them.
    fn draw_debug_tabs(&mut self, ui: &mut egui::Ui) -> DebugTab {
        let pal = self.pal;
        let mut tabs = vec![(DebugTab::Variables, tr("Variables"))];
        if self.debug_supports("supportsDisassembleRequest") {
            tabs.push((DebugTab::Disassembly, tr("Disassembly")));
        }
        if self.debug_supports("supportsReadMemoryRequest") {
            tabs.push((DebugTab::Memory, tr("Memory")));
        }
        if !tabs.iter().any(|(t, _)| *t == self.debug.tab) {
            self.debug.tab = DebugTab::Variables;
        }
        let mut chosen = None;
        let mut rects = Vec::new();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (t, label) in &tabs {
                let active = *t == self.debug.tab;
                let font = if active { fonts::semibold(12.5) } else { fonts::ui(12.5) };
                let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), pal.text);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 18.0, 22.0), Sense::click());
                if active {
                    ui.painter().rect_filled(r, egui::CornerRadius::same(11), pal.tab_active_bg);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, egui::CornerRadius::same(11), pal.hover);
                }
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, *label, font, if active { pal.text } else { pal.text_dim });
                rects.push((format!("{t:?}").to_lowercase(), r));
                if resp.clicked() {
                    chosen = Some(*t);
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(egui::Id::unique("debug_tab_rects"), rects));
        if let Some(t) = chosen
            && t != self.debug.tab
        {
            self.debug.tab = t;
            if t == DebugTab::Disassembly {
                self.request_disassembly();
            }
        }
        self.debug.tab
    }

    /// The machine code around the frame shown; the instruction it is at stands out.
    fn draw_disassembly(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let ip = self.debug.stopped.as_ref().and_then(|s| s.frames.get(s.frame)).and_then(|f| f.ip.clone());
        if !self.debug.view_note.is_empty() || self.debug.disassembly.is_empty() {
            let note = if self.debug.stopped.is_none() { tr("The program must be paused to show its machine code").to_string() } else { self.debug.view_note.clone() };
            ui.label(RichText::new(note).color(pal.text_dim).size(12.0));
            return;
        }
        let rows = self.debug.disassembly.clone();
        let mono = egui::FontId::monospace(12.0);
        egui::ScrollArea::vertical().id_salt("debug_disassembly").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut last_symbol = String::new();
            let mut last_line = None;
            for ins in &rows {
                if !ins.symbol.is_empty() && ins.symbol != last_symbol {
                    ui.label(RichText::new(format!("{}:", ins.symbol)).font(mono.clone()).color(pal.definition));
                    last_symbol = ins.symbol.clone();
                }
                let current = ip.as_deref().is_some_and(|ip| ip.eq_ignore_ascii_case(&ins.address));
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 19.0), Sense::hover());
                if current {
                    ui.painter().rect_filled(r, egui::CornerRadius::same(4), pal.warning.gamma_multiply(0.28));
                    icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(r.min.x + 2.0, r.center().y - 5.0), Vec2::splat(10.0)), Icon::ChevronRight, pal.warning);
                }
                let p = ui.painter().with_clip_rect(r);
                let end = p.text(Pos2::new(r.min.x + 16.0, r.center().y), egui::Align2::LEFT_CENTER, &ins.address, mono.clone(), pal.line_number).max.x;
                // the source line the instruction is of, while it changes
                let line = ins.line.filter(|l| Some(*l) != last_line).map(|l| format!(":{l}")).unwrap_or_default();
                last_line = ins.line;
                p.text(Pos2::new(end + 8.0, r.center().y), egui::Align2::LEFT_CENTER, &line, mono.clone(), pal.text_dim);
                p.text(Pos2::new(end + 52.0, r.center().y), egui::Align2::LEFT_CENTER, &ins.text, mono.clone(), if current { pal.text } else { pal.plain });
            }
        });
    }

    /// The memory at an address or where an expression points, as a hex dump.
    fn draw_memory(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let mut read = None;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.debug.memory_input).hint_text(tr("Address (0x...) or expression, Enter")).font(egui::FontId::monospace(12.5)).desired_width(ui.available_width()));
            if r.gained_focus() {
                self.editor_focus = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                read = Some(self.debug.memory_input.clone());
            }
        });
        if let Some(w) = read {
            self.request_memory(&w);
        }
        if !self.debug.view_note.is_empty() {
            ui.label(RichText::new(&self.debug.view_note).color(pal.text_dim).size(12.0));
        }
        let Some(view) = self.debug.memory.clone() else { return };
        let mono = egui::FontId::monospace(12.0);
        egui::ScrollArea::vertical().id_salt("debug_memory").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (address, hex, text) in hex_rows(view.address, &view.bytes) {
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
                let p = ui.painter().with_clip_rect(r);
                let end = p.text(Pos2::new(r.min.x + 4.0, r.center().y), egui::Align2::LEFT_CENTER, &address, mono.clone(), pal.line_number).max.x;
                let end = p.text(Pos2::new(end + 12.0, r.center().y), egui::Align2::LEFT_CENTER, &hex, mono.clone(), pal.number).max.x;
                p.text(Pos2::new(end + 12.0, r.center().y), egui::Align2::LEFT_CENTER, &text, mono.clone(), pal.text_dim);
            }
        });
    }

    /// The editor of a breakpoint, under its line: a condition, a hit count, a message to log.
    pub fn draw_breakpoint_editor(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.debug.editor.clone() else { return };
        if self.popup.is_open() {
            self.debug.editor = None;
            return;
        }
        let pal = self.pal;
        let mut result: Option<Option<bool>> = ctx.input(|i| i.key_pressed(Key::Escape)).then_some(None);
        let mut rects = Vec::new();
        let screen = ctx.content_rect();
        let width = 380.0f32.min(screen.width() - 24.0);
        let pos = Pos2::new(ed.anchor.x.min(screen.max.x - width - 12.0).max(8.0), ed.anchor.y.min(screen.max.y - 300.0).max(8.0));
        egui::Area::new(egui::Id::unique("breakpoint_editor")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            egui::Frame::new().fill(pal.popup_bg.to_opaque()).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(egui::CornerRadius::same(14)).inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
                ui.set_width(width - 32.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, pal.error);
                    ui.label(RichText::new(i18n::fmt("Breakpoint on line {}", &[&(ed.line + 1)])).font(fonts::semibold(14.0)));
                });
                ui.add_space(10.0);
                let mut field = |ui: &mut egui::Ui, label: &str, hint: &str, text: &mut String, first: bool| {
                    ui.label(RichText::new(label).color(pal.text_dim).size(12.0));
                    let r = ui.add(egui::TextEdit::singleline(text).hint_text(hint).font(egui::FontId::monospace(13.0)).desired_width(f32::INFINITY));
                    if first && ed.just_opened {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        result = Some(Some(true));
                    }
                    ui.add_space(8.0);
                };
                field(ui, tr("Stop only when this is true"), tr("x > 3"), &mut ed.options.condition, true);
                field(ui, tr("Hit count"), tr("3, or >= 3: stop from the third time on"), &mut ed.options.hit, false);
                field(ui, tr("Log message instead of stopping"), tr("x is {x}"), &mut ed.options.log, false);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let r = widgets::secondary_button(ui, &pal, tr("Remove"));
                    rects.push(("remove", r.rect));
                    if r.clicked() {
                        result = Some(Some(false));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let r = widgets::primary_button(ui, &pal, tr("Done"));
                        rects.push(("done", r.rect));
                        if r.clicked() {
                            result = Some(Some(true));
                        }
                        if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                            result = Some(None);
                        }
                    });
                });
            });
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("breakpoint_editor_rects"), rects));
        ed.just_opened = false;
        match result {
            Some(keep) => {
                self.debug.editor = Some(ed);
                self.close_breakpoint_editor(keep);
                if keep.is_none() {
                    self.debug.editor = None;
                }
                self.editor_focus = true;
            }
            None => self.debug.editor = Some(ed),
        }
    }

    pub fn draw_debug(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let status = self.debug.status;
        // the buttons, and what the program is doing
        let mut act: Option<&str> = None;
        let mut rects = Vec::new();
        let controls = ui.horizontal(|ui| {
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
                // a reason of one word (`step`, `breakpoint`) is said as a pause
                DebugStatus::Paused => self.debug.stopped.as_ref().map(|s| if s.reason.contains(' ') { s.reason.clone() } else { i18n::fmt("Paused ({})", &[&s.reason]) }).unwrap_or_default(),
            };
            ui.add(egui::Label::new(RichText::new(text)
                .color(if status == DebugStatus::Paused { pal.warning } else { pal.text_dim }).size(12.5)).truncate());
        });
        let controls_bottom = controls.response.rect.bottom();
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("debug_button_rects"), rects));
        match act {
            Some("stop") => self.stop_debug(),
            Some(c) => self.debug_command(c),
            None => {}
        }
        ui.add_space(4.0);
        let avail = ui.available_size();
        let body_width = (avail.x - 42.0).max(0.0);
        let body_height = (avail.y - 8.0).max(0.0);
        // the machine code and the memory need room: the middle gets more of it then
        let wide = self.debug.tab != DebugTab::Variables;
        let col = body_width * if wide { 0.27 } else { 0.33 };
        let mid = body_width * if wide { 0.45 } else { 0.33 };
        let mut frame_click = None;
        let mut expand = None;
        ui.horizontal_top(|ui| {
            ui.add_space(8.0);
            // the call stack
            ui.allocate_ui_with_layout(Vec2::new(col, body_height), Layout::top_down(Align::Min), |ui| {
                ui.label(RichText::new(tr("Frames")).font(fonts::semibold(12.5)).color(pal.text_dim));
                egui::ScrollArea::vertical().id_salt("debug_frames").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if let Some(s) = &self.debug.stopped {
                        for (n, f) in s.frames.iter().enumerate() {
                            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 4.0), Sense::click());
                            widgets::row_background(ui, &pal, r, n == s.frame, resp.hovered());
                            let place = f.path.as_ref().and_then(|p| p.file_name()).map(|n| format!("{}:{}", n.to_string_lossy(), f.line)).unwrap_or_default();
                            let color = if f.path.is_some() { pal.text } else { pal.text_dim };
                            let painter=ui.painter_at(r.intersect(ui.clip_rect()));
                            let end = painter.text(Pos2::new(r.min.x + 10.0, r.center().y), egui::Align2::LEFT_CENTER, value_preview(&f.name), fonts::ui(13.0), color).max.x;
                            painter.text(Pos2::new(end + 8.0, r.center().y), egui::Align2::LEFT_CENTER, place, fonts::ui(11.5), pal.text_dim);
                            if resp.clicked() {
                                frame_click = Some(n);
                            }
                        }
                    }
                });
            });
            ui.separator();
            // the variables of the frame, a tree opened a level at a time
            ui.allocate_ui_with_layout(Vec2::new(mid, body_height), Layout::top_down(Align::Min), |ui| {
                let tab = self.draw_debug_tabs(ui);
                if tab == DebugTab::Disassembly {
                    self.draw_disassembly(ui);
                } else if tab == DebugTab::Memory {
                    self.draw_memory(ui);
                } else {
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
                        let resp=variable_row(ui,&pal,depth,&name,&value,(reference!=0).then(||self.debug.expanded.contains(&reference)));
                        ui.ctx().data_mut(|data|data.insert_temp(egui::Id::unique(("debug_variable_row",name.as_str())),resp.rect));
                        if !value.is_empty() {
                            variable_tooltip(&resp, &name, &value, controls_bottom);
                            resp.context_menu(|ui| {if ui.button(tr("Copy value")).clicked(){ui.ctx().copy_text(value.clone());ui.close();}});
                        }
                        if reference != 0 && resp.clicked() {
                            expand = Some(reference);
                        }
                    }
                });
                }
            });
            ui.separator();
            // the console: output, and expressions evaluated where the program is
            ui.allocate_ui_with_layout(Vec2::new((ui.available_width() - 8.0).max(0.0), body_height), Layout::top_down(Align::Min), |ui| {
                ui.label(RichText::new(tr("Console")).font(fonts::semibold(12.5)).color(pal.text_dim));
                let input_h = 34.0;
                let mut evaluate = None;
                let lines = self.debug.console.clone();
                ui.allocate_ui(Vec2::new(ui.available_width(), (ui.available_height() - input_h).max(0.0)), |ui| {
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

#[cfg(test)]
mod variable_layout_tests {
    use super::*;
    #[test]
    fn multiline_values_and_long_names_stay_in_their_row_and_column() {
        let original="<table>\r\n\t<td>Пример</td>\n</table>\u{2028}end";
        assert!(!value_preview(original).contains(['\n','\r','\t','\u{2028}']));
        for width in [80.0,160.0,400.0] {
            let ctx=egui::Context::default();crate::fonts::install(&ctx);
            let mut output=ctx.run_ui(egui::RawInput::default(),|ui| {
                ui.set_width(width);
                let pal=crate::theme::Palette::dark();
                let first=variable_row(ui,&pal,1,&"long_name".repeat(100),original,None);
                let second=variable_row(ui,&pal,1,"next","42",None);
                assert!(first.rect.max.y<=second.rect.min.y);
            });
            output.textures_delta.clear();
            for shape in output.shapes {
                if let egui::Shape::Text(text)=shape.shape {
                    assert_eq!(text.galley.rows.len(),1);
                    assert!(shape.clip_rect.width()<=width+1.0);
                    assert!(text.galley.size().y<=ROW_H-6.0);
                }
            }
        }
        assert!(value_preview(&"я".repeat(100_000)).chars().count()<=2049);
    }
}
