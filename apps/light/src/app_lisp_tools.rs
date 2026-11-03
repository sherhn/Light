//! The tools of the Lisp engine beyond its REPL: the debugger an error of an evaluation stops in (its restarts, the
//! frames of the stack and their variables), the inspector of a value, who calls a function or uses a variable, and
//! tracing a function.

use std::path::PathBuf;

use eframe::egui::{self, Align, Layout, RichText, Sense, Vec2};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::state::*;
use crate::widgets::{self, ROW_H};

/// Where an evaluation stopped for an error.
#[derive(Clone, Debug, Default)]
pub struct LispDebugger {
    /// The evaluation (its request id).
    pub id: u64,
    pub condition: String,
    pub kind: String,
    /// How deep: an error of an evaluation made in the debugger opens one more.
    pub level: usize,
    /// (name, what it does), in the order the server numbers them.
    pub restarts: Vec<(String, String)>,
    pub frames: Vec<String>,
    pub selected: Option<usize>,
    pub locals: Vec<(String, String)>,
    /// A value for the restarts that take one (use-value, store-value).
    pub value: String,
}

impl LispDebugger {
    /// Read the payload of a `debugger` message.
    pub fn parse(id: u64, payload: &str) -> LispDebugger {
        let mut d = LispDebugger { id, ..Default::default() };
        for line in payload.lines() {
            let mut parts = line.split('\t');
            match parts.next() {
                Some("C") => d.condition = parts.next().unwrap_or_default().to_string(),
                Some("T") => d.kind = parts.next().unwrap_or_default().to_string(),
                Some("L") => d.level = parts.next().and_then(|l| l.parse().ok()).unwrap_or(1),
                Some("R") => {
                    let _n = parts.next();
                    d.restarts.push((parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string()));
                }
                Some("F") => {
                    let _n = parts.next();
                    d.frames.push(parts.next().unwrap_or_default().to_string());
                }
                _ => {}
            }
        }
        d
    }
}

/// What the inspector shows.
#[derive(Clone, Debug, Default)]
pub struct InspectorState {
    pub object: String,
    pub kind: String,
    /// How many values deep (1: what was inspected).
    pub depth: usize,
    pub parts: Vec<(String, String)>,
    pub error: Option<String>,
}

impl InspectorState {
    pub fn parse(payload: &str) -> InspectorState {
        let mut s = InspectorState::default();
        for line in payload.lines() {
            let mut parts = line.split('\t');
            match parts.next() {
                Some("O") => s.object = parts.next().unwrap_or_default().to_string(),
                Some("T") => s.kind = parts.next().unwrap_or_default().to_string(),
                Some("D") => s.depth = parts.next().and_then(|d| d.parse().ok()).unwrap_or(1),
                Some("E") => s.error = Some(parts.next().unwrap_or_default().to_string()),
                Some("P") => {
                    let _n = parts.next();
                    s.parts.push((parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string()));
                }
                _ => {}
            }
        }
        s
    }
}

/// The function a frame is a call of: `(FOO 1 2)` -> `FOO`.
fn frame_function(frame: &str) -> Option<String> {
    let inner = frame.strip_prefix('(')?;
    let name: String = inner.chars().take_while(|c| !c.is_whitespace() && *c != ')').collect();
    (!name.is_empty() && !name.starts_with('(')).then_some(name)
}

impl App {
    /// The name or the expression the Lisp tools work on: the selection, or the symbol at the caret.
    fn lisp_target(&self) -> Option<String> {
        let d = self.doc()?;
        if d.ed.has_selection() {
            let s = d.ed.selected_text().trim().to_string();
            return (!s.is_empty()).then_some(s);
        }
        self.symbol_at_caret()
    }

    /// A message of the server: an evaluation stopped for an error.
    pub fn lisp_debugger_opened(&mut self, id: u64, payload: &str) {
        let d = LispDebugger::parse(id, payload);
        self.repl.log.push(LogLine { kind: LogKind::Error, text: d.condition.clone() });
        self.repl.debuggers.push(d);
        self.bottom_tool = Some(BottomTool::Repl);
    }

    /// Go on with restart `n` of the innermost debugger.
    pub fn lisp_restart(&mut self, n: usize) {
        let Some(d) = self.repl.debuggers.last() else { return };
        let payload = if d.value.trim().is_empty() { n.to_string() } else { format!("{n}\t{}", d.value.trim()) };
        self.repl_request("restart", &payload, Pending::Ignore);
        // the REPL goes on where it was: its input has the keyboard again
        self.repl.focus_input = true;
    }

    pub fn lisp_select_frame(&mut self, n: usize) {
        let Some(d) = self.repl.debuggers.last_mut() else { return };
        d.selected = Some(n);
        d.locals.clear();
        self.repl_request("locals", &n.to_string(), Pending::Locals);
    }

    pub fn lisp_inspect(&mut self, expr: Option<String>) {
        let Some(expr) = expr.or_else(|| self.lisp_target()) else {
            self.toast(tr("Put the caret on a name or select an expression to inspect"), Sev::Weak);
            return;
        };
        if !self.ensure_repl() || !self.repl.ready {
            self.toast(tr("The REPL is not ready yet"), Sev::Weak);
            return;
        }
        self.repl_request("inspect", &expr, Pending::Inspect);
    }

    /// `who-calls`, `who-references`, `who-macroexpands` of the name at the caret, to the Find window.
    pub fn lisp_xref(&mut self, kind: &'static str) {
        let Some(name) = self.symbol_at_caret() else {
            self.toast(tr("Put the caret on a name first"), Sev::Weak);
            return;
        };
        if !self.ensure_repl() || !self.repl.ready {
            self.toast(tr("The REPL is not ready yet"), Sev::Weak);
            return;
        }
        self.repl_request("xref", &format!("{kind}\t{name}"), Pending::Xref { kind, name });
    }

    pub fn lisp_toggle_trace(&mut self) {
        let Some(name) = self.symbol_at_caret() else {
            self.toast(tr("Put the caret on a function name first"), Sev::Weak);
            return;
        };
        if !self.ensure_repl() || !self.repl.ready {
            self.toast(tr("The REPL is not ready yet"), Sev::Weak);
            return;
        }
        let payload = name.clone();
        self.repl_request("trace", &payload, Pending::Trace { name });
    }

    /// The answers of the server to the tools' requests.
    pub fn lisp_tool_reply(&mut self, p: Pending, payload: &str) {
        match p {
            Pending::Locals => {
                if let Some(d) = self.repl.debuggers.last_mut() {
                    d.locals = payload.lines().filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.to_string()))).collect();
                }
            }
            Pending::Inspect => {
                let st = InspectorState::parse(payload);
                if let Some(e) = &st.error {
                    self.toast(i18n::fmt("Cannot inspect: {}", &[e]), Sev::Error);
                } else {
                    self.popup = Popup::Inspector(Box::new(st));
                }
            }
            Pending::Xref { kind, name } => {
                let mut hits = Vec::new();
                for line in payload.lines() {
                    let mut parts = line.split('\t');
                    let (who, file, offset) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().and_then(|o| o.parse::<usize>().ok()).unwrap_or(0));
                    let path = self.lisp_path_to_local(file);
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    // the offset is in characters, of the top-level form
                    let byte = text.char_indices().nth(offset).map_or(0, |(b, _)| b);
                    let line_no = text[..byte.min(text.len())].matches('\n').count();
                    let line_text = text.lines().nth(line_no).unwrap_or(who).trim().to_string();
                    hits.push(ide_core::search::FileHit { path, line: line_no, col: 0, len: 0, line_text: format!("{who}:  {line_text}") });
                }
                let what = match kind { "who-calls" => "Who calls '{}'", "who-references" => "Who references '{}'", "who-sets" => "Who sets '{}'", _ => "Who macroexpands '{}'" };
                if hits.is_empty() {
                    self.toast(i18n::fmt("Nothing is known: compile or load the code that uses '{}' first", &[&name]), Sev::Weak);
                }
                self.find_results = FindResults { title: i18n::fmt(what, &[&name]), hits };
                self.bottom_tool = Some(BottomTool::Find);
            }
            Pending::Trace { name } => match payload.trim() {
                "traced" => self.toast(i18n::fmt("Tracing '{}': its calls show in the REPL", &[&name]), Sev::Weak),
                "untraced" => self.toast(i18n::fmt("No longer tracing '{}'", &[&name]), Sev::Weak),
                other => self.toast(i18n::fmt("Cannot trace '{0}': {1}", &[&name, &other.trim_start_matches("; ")]), Sev::Error),
            },
            _ => {}
        }
    }

    /// A path the Lisp gave (its Linux one for a project in WSL) as this computer sees it.
    fn lisp_path_to_local(&self, file: &str) -> PathBuf {
        match self.wsl_target() {
            Some(t) if file.starts_with('/') => lisp_runtime::wsl::to_windows(file, &t),
            _ => PathBuf::from(file),
        }
    }

    /// Above the REPL's log while an evaluation is stopped: the condition, the restarts, the stack and its variables.
    pub fn draw_lisp_debugger(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.repl.debuggers.last().cloned() else { return };
        let pal = self.pal;
        let mut restart = None;
        let mut frame = None;
        let mut source = None;
        let mut rects = Vec::new();
        egui::Frame::new().fill(pal.error.gamma_multiply(0.08)).stroke(egui::Stroke::new(1.0, pal.error.gamma_multiply(0.4))).corner_radius(egui::CornerRadius::same(9)).inner_margin(egui::Margin::symmetric(10, 8)).outer_margin(egui::Margin::symmetric(12, 4)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(i18n::fmt("Debugger, level {}", &[&d.level])).font(fonts::semibold(12.5)).color(pal.error));
                ui.label(RichText::new(&d.kind).color(pal.text_dim).size(11.5));
            });
            ui.label(RichText::new(&d.condition).font(egui::FontId::monospace(12.5)).color(pal.text));
            ui.add_space(4.0);
            ui.columns(2, |cols| {
                // the restarts (all of them: there are few, and the one wanted is often the last)
                let ui = &mut cols[0];
                ui.label(RichText::new(tr("Restarts")).font(fonts::semibold(12.0)).color(pal.text_dim));
                ui.spacing_mut().item_spacing.y = 0.0;
                for (n, (name, what)) in d.restarts.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 6.0), Sense::click());
                    widgets::row_background(ui, &pal, r, false, resp.hovered());
                    let end = ui.painter().text(egui::pos2(r.min.x + 6.0, r.center().y), egui::Align2::LEFT_CENTER, format!("{n}  {what}"), fonts::ui(12.5), pal.text).max.x;
                    ui.painter().text(egui::pos2(end + 8.0, r.center().y), egui::Align2::LEFT_CENTER, name, fonts::ui(11.5), pal.text_dim);
                    rects.push((format!("restart:{name}"), r));
                    if resp.clicked() {
                        restart = Some(n);
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Value")).color(pal.text_dim).size(11.5));
                    if let Some(dbg) = self.repl.debuggers.last_mut() {
                        ui.add(egui::TextEdit::singleline(&mut dbg.value).hint_text(tr("for use-value, store-value")).font(egui::FontId::monospace(12.0)).desired_width(ui.available_width()));
                    }
                });
                // the stack, and the variables of the frame chosen
                let ui = &mut cols[1];
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Frames")).font(fonts::semibold(12.0)).color(pal.text_dim));
                    if let Some(name) = d.selected.and_then(|n| d.frames.get(n)).and_then(|f| frame_function(f))
                        && widgets::compact_button(ui, &pal, tr("Go to Source"), false).clicked()
                    {
                        source = Some(name);
                    }
                });
                egui::ScrollArea::vertical().id_salt("lisp_frames").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (n, f) in d.frames.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 8.0), Sense::click());
                        widgets::row_background(ui, &pal, r, d.selected == Some(n), resp.hovered());
                        ui.painter().with_clip_rect(r).text(egui::pos2(r.min.x + 6.0, r.center().y), egui::Align2::LEFT_CENTER, format!("{n}  {f}"), egui::FontId::monospace(12.0), pal.text);
                        rects.push((format!("frame:{n}"), r));
                        if resp.clicked() {
                            frame = Some(n);
                        }
                        if d.selected == Some(n) {
                            for (name, value) in &d.locals {
                                ui.label(RichText::new(format!("      {name} = {value}")).font(egui::FontId::monospace(12.0)).color(pal.special_var));
                            }
                        }
                    }
                });
            });
        });
        ui.data_mut(|data| data.insert_temp(egui::Id::unique("lisp_debugger_rects"), rects));
        if let Some(n) = restart {
            self.lisp_restart(n);
        }
        if let Some(n) = frame {
            self.lisp_select_frame(n);
        }
        if let Some(name) = source {
            self.goto_declaration(Some(name.to_lowercase()));
        }
    }

    /// The inspector: the value, its type and parts; a click goes into a part, Back out of it.
    pub fn draw_inspector(&mut self, ctx: &egui::Context) {
        let Popup::Inspector(st) = &self.popup else { return };
        let st = (**st).clone();
        let pal = self.pal;
        let mut part = None;
        let (mut back, mut close) = (false, ctx.input(|i| i.key_pressed(egui::Key::Escape)));
        let mut rects = Vec::new();
        self.dialog_card(ctx, "lisp_inspector", tr("Inspector"), 620.0, |ui| {
            ui.label(RichText::new(&st.object).font(egui::FontId::monospace(13.0)));
            ui.label(RichText::new(&st.kind).color(pal.text_dim).size(12.0));
            ui.add_space(8.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if st.parts.is_empty() {
                    ui.label(RichText::new(tr("It has no parts to look into.")).color(pal.text_dim));
                }
                for (n, (label, value)) in st.parts.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 6.0), Sense::click());
                    widgets::row_background(ui, &pal, r, false, resp.hovered());
                    let end = ui.painter().text(egui::pos2(r.min.x + 8.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::monospace(12.5), pal.special_var).max.x;
                    ui.painter().text(egui::pos2(end + 10.0, r.center().y), egui::Align2::LEFT_CENTER, value, egui::FontId::monospace(12.5), pal.text);
                    rects.push((label.clone(), r));
                    if resp.clicked() {
                        part = Some(n);
                    }
                }
            });
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::secondary_button(ui, &pal, tr("Close")).clicked() {
                    close = true;
                }
                if st.depth > 1 && widgets::secondary_button(ui, &pal, tr("Back")).clicked() {
                    back = true;
                }
            });
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("inspector_rects"), rects));
        if let Some(n) = part {
            self.repl_request("inspect-part", &n.to_string(), Pending::Inspect);
        } else if back {
            self.repl_request("inspect-back", "", Pending::Inspect);
        } else if close {
            self.popup = Popup::None;
            self.editor_focus = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_of_the_debugger_and_the_inspector_are_read() {
        let d = LispDebugger::parse(7, "C\tarithmetic error DIVISION-BY-ZERO signalled\nT\tdivision-by-zero\nL\t2\nR\t0\tuse-value\tUse a value\nR\t1\tabort-to-light\tReturn to the Light REPL\nF\t0\t(SB-KERNEL::INTEGER-/-INTEGER 10 0)\nF\t1\t(OUTER 5)\n");
        assert_eq!((d.id, d.level, d.kind.as_str()), (7, 2, "division-by-zero"));
        assert_eq!(d.restarts[1], ("abort-to-light".to_string(), "Return to the Light REPL".to_string()));
        assert_eq!(d.frames, ["(SB-KERNEL::INTEGER-/-INTEGER 10 0)", "(OUTER 5)"]);
        assert_eq!(frame_function(&d.frames[1]).as_deref(), Some("OUTER"));
        assert_eq!(frame_function("((LAMBDA (X)) 1)"), None);
        let i = InspectorState::parse("O\t(1 \"two\")\nT\tcons\nD\t1\nP\t0\t0\t1\nP\t1\t1\t\"two\"\n");
        assert_eq!((i.object.as_str(), i.depth, i.parts.len(), i.parts[1].1.as_str()), ("(1 \"two\")", 1, 2, "\"two\""));
        assert!(InspectorState::parse("E\tunbound variable X\n").error.is_some());
    }
}
