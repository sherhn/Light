//! Keyboard handling and command execution. Every key goes through the keymap -> command registry.

use eframe::egui::{self, Event, Key, Modifiers};
use ide_core::commands::{self, Shortcut};
use ide_core::editor::LineEnding;
use ide_core::exec::{self, ExecCtx};
use ide_core::search::TextQuery;

use crate::app::{App, RunConfig};
use crate::editor_view::Sev;
use crate::i18n::{self, tr};
use crate::preview::PreviewMode;
use crate::state::*;

pub fn key_name(key: Key) -> Option<&'static str> {
    use Key::*;
    Some(match key {
        A => "A", B => "B", C => "C", D => "D", E => "E", F => "F", G => "G", H => "H", I => "I", J => "J", K => "K", L => "L",
        M => "M", N => "N", O => "O", P => "P", Q => "Q", R => "R", S => "S", T => "T", U => "U", V => "V", W => "W", X => "X",
        Y => "Y", Z => "Z", Num0 => "0", Num1 => "1", Num2 => "2", Num3 => "3", Num4 => "4", Num5 => "5", Num6 => "6", Num7 => "7",
        Num8 => "8", Num9 => "9", F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6", F7 => "F7", F8 => "F8",
        F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12", Enter => "Enter", Tab => "Tab", Backspace => "Backspace",
        Delete => "Delete", Insert => "Insert", Escape => "Escape", Space => "Space", ArrowLeft => "Left", ArrowRight => "Right",
        ArrowUp => "Up", ArrowDown => "Down", Home => "Home", End => "End", PageUp => "PageUp", PageDown => "PageDown",
        Equals | Plus => "Equals", Backtick => "Backtick", Minus => "Minus", Slash => "Slash", Comma => "Comma", Period => "Period", Semicolon => "Semicolon",
        _ => return None,
    })
}

pub fn shortcut_of(key: Key, m: Modifiers) -> Option<Shortcut> {
    let name = key_name(key)?;
    Some(Shortcut { ctrl: m.command, shift: m.shift, alt: m.alt, key: name })
}

/// Parse a shortcut string like `Ctrl+Shift+A` into an egui key + modifiers (for scripts and tests).
pub fn parse_key_combo(s: &str) -> Option<(Key, Modifiers)> {
    let sc = Shortcut::parse(s)?;
    let key = all_keys().into_iter().find(|k| key_name(*k) == Some(sc.key))?;
    Some((key, Modifiers { alt: sc.alt, ctrl: sc.ctrl, shift: sc.shift, mac_cmd: false, command: sc.ctrl }))
}

fn all_keys() -> Vec<Key> {
    use Key::*;
    vec![
        A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z, Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7,
        Num8, Num9, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, Enter, Tab, Backspace, Delete, Insert, Escape, Space,
        ArrowLeft, ArrowRight, ArrowUp, ArrowDown, Home, End, PageUp, PageDown, Equals, Minus, Slash, Comma, Period, Semicolon, Backtick,
    ]
}

/// Commands that only make sense while the code editor has the keyboard focus.
fn needs_editor(id: &str) -> bool {
    ["caret.", "select.", "edit.", "lisp.", "code.", "repl.eval", "nav.matching_bracket", "nav.declaration", "refactor.", "scroll."]
        .iter()
        .any(|p| id.starts_with(p))
        && !matches!(id, "edit.undo_global")
}

fn is_symbol_char(c: char) -> bool {
    c.is_alphanumeric() || "-_*+/<>=!?:%&$.@^~#".contains(c)
}

impl App {
    /// Is the code editor receiving typing right now?
    pub fn editor_active(&self, ctx: &egui::Context) -> bool {
        self.editor_focus && !self.popup.is_open() && !ctx.text_edit_focused() && self.doc().is_some_and(|d| d.diff.is_none()) && !self.editor_hidden()
    }

    /// Double-tap of the Shift key opens Search Everywhere.
    fn detect_double_shift(&mut self, ctx: &egui::Context) {
        let (shift, other_mods, used_key) = ctx.input(|i| {
            (
                i.modifiers.shift,
                i.modifiers.command || i.modifiers.alt,
                i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. } | Event::Text(_) | Event::PointerButton { pressed: true, .. })),
            )
        });
        let (down, dirty, last_tap) = self.shift_tap;
        let mut dirty = dirty || used_key || other_mods;
        match (down, shift) {
            (false, true) => dirty = used_key || other_mods,
            (true, false) => {
                if !dirty {
                    if self.now - last_tap < 0.5 {
                        self.shift_tap = (false, false, -10.0);
                        if !self.popup.is_open() {
                            self.run_command(ctx, "nav.search_everywhere");
                        }
                        return;
                    }
                    self.shift_tap = (false, false, self.now);
                } else {
                    self.shift_tap = (false, false, -10.0);
                }
                return;
            }
            _ => {}
        }
        self.shift_tap = (shift, dirty, last_tap);
    }

    pub fn handle_input(&mut self, ctx: &egui::Context) {
        self.detect_double_shift(ctx);
        // Tab / arrows may have moved egui's keyboard focus onto a button; while the editor owns the
        // keyboard that would make Enter/Space press that button, so drop non-text focus.
        if self.editor_focus && !self.popup.is_open()
            && let Some(id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        let events: Vec<Event> = ctx.input(|i| i.events.clone());
        let mut consume: Vec<(Modifiers, Key)> = Vec::new();
        let mut typed_symbol = false;
        let mut moved = false;
        for ev in events {
            match ev {
                Event::Key { key, pressed: true, modifiers, .. } => {
                    let Some(sc) = shortcut_of(key, modifiers) else { continue };
                    let editor_on = self.editor_active(ctx);

                    // Completion popup keys take priority.
                    if editor_on && self.completion.active {
                        let handled = match (key, modifiers.any()) {
                            (Key::ArrowDown, false) => {
                                self.completion.selected = (self.completion.selected + 1) % self.completion.items.len().max(1);
                                true
                            }
                            (Key::ArrowUp, false) => {
                                let n = self.completion.items.len().max(1);
                                self.completion.selected = (self.completion.selected + n - 1) % n;
                                true
                            }
                            (Key::Enter, false) | (Key::Tab, false) => {
                                self.accept_completion();
                                true
                            }
                            (Key::Escape, false) => {
                                self.completion.active = false;
                                true
                            }
                            _ => false,
                        };
                        if handled {
                            consume.push((modifiers, key));
                            continue;
                        }
                    }

                    let Some(cmd) = self.keymap.lookup(&sc) else { continue };
                    if cmd.id == "app.escape" {
                        if self.popup.is_open() {
                            // popups close themselves; keep Escape for them
                            continue;
                        }
                        self.completion.active = false;
                        self.hint = None;
                        if self.find.open && !ctx.text_edit_focused() {
                            self.find.open = false;
                        }
                        if !self.editor_focus && self.doc().is_some() {
                            self.editor_focus = true;
                        }
                        consume.push((modifiers, key));
                        continue;
                    }
                    if needs_editor(cmd.id) && !editor_on {
                        continue;
                    }
                    if self.popup.is_open() && !matches!(cmd.id, "nav.search_everywhere" | "nav.find_action" | "nav.goto_file" | "nav.goto_symbol" | "nav.recent_files" | "app.settings" | "app.modules" | "file.save" | "app.exit") {
                        continue;
                    }
                    // Plain printable keys with only shift are text, never commands, unless bound (e.g. Shift+F10).
                    self.run_command(ctx, cmd.id);
                    if cmd.id.starts_with("caret.") || cmd.id.starts_with("select.") {
                        moved = true;
                    }
                    consume.push((modifiers, key));
                }
                Event::Text(t) => {
                    if !self.editor_active(ctx) {
                        continue;
                    }
                    let t: String = t.chars().filter(|c| !c.is_control()).collect();
                    if t.is_empty() {
                        continue;
                    }
                    let now = self.now;
                    if let Some(d) = self.docs.get_mut(self.active) {
                        d.ed.type_text(&t);
                        d.view.scroll_to_caret = true;
                        d.view.last_input_time = now;
                        // Lisp names hold `-` and `*`; the names of other languages are letters, digits and `_`
                        typed_symbol = if d.ed.is_lisp() { t.chars().all(is_symbol_char) } else { t.chars().all(|c| c.is_alphanumeric() || c == '_') };
                    }
                    if !typed_symbol {
                        self.completion.active = false;
                    }
                }
                Event::Copy => {
                    if self.editor_active(ctx) {
                        self.run_command(ctx, "edit.copy");
                    }
                }
                Event::Cut => {
                    if self.editor_active(ctx) {
                        self.run_command(ctx, "edit.cut");
                    }
                }
                Event::Paste(text)
                    if self.editor_active(ctx) => {
                        let now = self.now;
                        if let Some(d) = self.docs.get_mut(self.active) {
                            d.ed.paste(&text);
                            d.view.scroll_to_caret = true;
                            d.view.last_input_time = now;
                        }
                        self.completion.active = false;
                    }
                _ => {}
            }
        }
        if !consume.is_empty() {
            ctx.input_mut(|i| {
                for (m, k) in &consume {
                    i.consume_key(*m, *k);
                }
            });
        }
        if typed_symbol && self.settings.auto_popup {
            self.refresh_completion(false);
        } else if self.completion.active && moved {
            self.completion.active = false;
        } else if self.completion.active {
            // backspace etc. keep the list in sync
            let still = self.doc().is_some_and(|d| {
                let c = d.ed.caret();
                let (s, _) = if d.ed.is_lisp() { d.ed.symbol_range_at(c) } else { d.ed.identifier_range_at(c) };
                s == self.completion.start && c >= s
            });
            if still {
                let rev_changed = self.doc().is_some_and(|d| d.ed.text()[self.completion.start..d.ed.caret()] != self.completion.prefix);
                if rev_changed {
                    self.refresh_completion(true);
                }
            } else {
                self.completion.active = false;
            }
        }
    }

    fn with_editor(&mut self, ctx: &egui::Context, id: &str) {
        let page = self.doc().map_or(30, |d| d.view.last_viewport_lines);
        let now = self.now;
        if let Some(d) = self.docs.get_mut(self.active) {
            let rev = d.ed.revision();
            let before = d.ed.caret();
            if exec::apply_with(&mut d.ed, id, ExecCtx { page_lines: page }) {
                d.view.scroll_to_caret = true;
                d.view.last_input_time = now;
                if d.ed.revision() != rev
                    && let Some(p) = &d.path
                {
                    self.last_edit = Some((p.clone(), d.ed.caret()));
                }
                let _ = before;
            }
        }
        ctx.request_repaint();
    }

    pub fn run_command(&mut self, ctx: &egui::Context, id: &str) {
        if commands::is_lisp_command(id) && !self.lisp_enabled() {
            self.toast(tr("This is part of the Lisp module - install and switch it on in Modules"), Sev::Weak);
            return;
        }
        let editor_on = self.doc().is_some_and(|d| d.diff.is_none()) && !self.editor_hidden();
        match id {
            // ---- file
            "file.new" => {
                let dir = self.project.as_ref().map(|p| p.selected.clone().filter(|s| s.is_dir()).or_else(|| p.selected.as_ref().and_then(|s| s.parent().map(|x| x.to_path_buf()))).unwrap_or_else(|| p.root.clone()));
                match dir {
                    Some(dir) => self.popup = Popup::NewFile { dir, input: String::new(), is_dir: false, just_opened: true },
                    None => self.new_untitled(),
                }
            }
            "file.new_project" => {
                let parent = dirs_home().to_string_lossy().into_owned();
                // the modules that are on are ticked; the dialog asks for the registry's list
                let modules = self.mods.installed.iter().filter(|m| m.enabled).map(|m| m.manifest.id.clone()).collect();
                if self.mods.index.is_none() {
                    self.modules_fetch_index();
                }
                self.popup = Popup::NewProject { parent, name: "my-project".into(), just_opened: true, modules, template: "@auto".into(), creating: false };
            }
            "file.clone" => self.open_clone_dialog(),
            "file.open_project" => {
                self.open_project_browser();
            }
            "file.open_file" => {
                let mut dlg = rfd::FileDialog::new().set_title(tr("Open File"));
                if let Some(r) = self.project_root() {
                    dlg = dlg.set_directory(r);
                }
                if let Some(f) = dlg.pick_file() {
                    self.open_file(&f);
                }
            }
            "file.reopen_last" => {
                if let Some(p) = self.settings.recent_projects.first().cloned() {
                    self.request_open_project(std::path::Path::new(&p));
                }
            }
            "file.save" | "file.save_all" => self.save_all(),
            "view.split_right" => self.split_editor(true),
            "view.split_down" => self.split_editor(false),
            "view.unsplit" => self.unsplit(),
            "view.switch_pane" => self.focus_other_pane(),
            "diff.next" => self.diff_step_change(1),
            "diff.prev" => self.diff_step_change(-1),
            "diff.next_file" => self.diff_step_file(1),
            "diff.prev_file" => self.diff_step_file(-1),
            "file.reload" => {
                let mut err = None;
                if let Some(d) = self.doc_mut()
                    && let Err(e) = d.reload()
                {
                    err = Some(e.to_string());
                }
                if let Some(e) = err {
                    self.toast(i18n::fmt("Reload failed: {}", &[&e]), Sev::Error);
                }
            }
            "file.close_tab" => self.request_close_doc(self.active),
            "file.close_all" => {
                self.flush_saves();
                if self.has_unsaved() {
                    self.popup = Popup::Confirm { kind: ConfirmKind::CloseAll, message: tr("Some files have unsaved changes. Close all without saving?").into() };
                } else {
                    self.docs.clear();
                    self.active = 0;
                }
            }
            "file.close_others" => {
                self.flush_saves();
                let keep = self.active;
                if let Some(d) = self.docs.get(keep) {
                    let path = d.path.clone();
                    let mut i = 0;
                    self.docs.retain(|x| {
                        i += 1;
                        i - 1 == keep || (x.ed.is_modified() && x.path != path)
                    });
                    self.active = 0;
                    if let Some(p) = path
                        && let Some(pos) = self.docs.iter().position(|d| d.path.as_ref() == Some(&p))
                    {
                        self.active = pos;
                    }
                }
            }
            "file.next_tab" => {
                if !self.docs.is_empty() {
                    self.active = (self.active + 1) % self.docs.len();
                }
            }
            "file.prev_tab" => {
                if !self.docs.is_empty() {
                    self.active = (self.active + self.docs.len() - 1) % self.docs.len();
                }
            }
            "file.reveal" => {
                if let Some(p) = self.doc().and_then(|d| d.path.clone()) {
                    reveal_in_file_manager(&p);
                }
            }
            "file.copy_path" => {
                if let Some(p) = self.doc().and_then(|d| d.path.clone()) {
                    ctx.copy_text(p.to_string_lossy().into_owned());
                    self.toast(tr("Path copied"), Sev::Weak);
                }
            }
            "app.modules" => self.open_modules_tab(),
            "app.settings" => {
                self.popup = Popup::Settings { lisp_path: self.settings.lisp_path.clone(), detected: self.lisp.as_ref().map(|l| l.describe()).unwrap_or_default(), filter: String::new(), page: 0 };
            }
            "app.exit" => {
                self.flush_saves();
                if self.has_unsaved() {
                    self.popup = Popup::Confirm { kind: ConfirmKind::Exit, message: tr("You have unsaved changes. Exit without saving?").into() };
                } else {
                    self.exit_clean();
                }
            }
            // ---- edit (clipboard)
            "edit.copy" => {
                if let Some(d) = self.doc() {
                    ctx.copy_text(d.ed.copy_text());
                }
            }
            "edit.cut" => {
                let now = self.now;
                if let Some(d) = self.docs.get_mut(self.active) {
                    let t = d.ed.cut();
                    d.view.last_input_time = now;
                    ctx.copy_text(t);
                }
            }
            "edit.paste" => {
                // Paste arrives as Event::Paste (Ctrl+V); the menu cannot read the clipboard.
                self.toast(tr("Use Ctrl+V to paste"), Sev::Weak);
            }
            // ---- find
            "find.find" => self.open_find(false),
            "find.replace" => self.open_find(true),
            "find.next" => self.find_step(true),
            "find.prev" => self.find_step(false),
            "find.in_files" => {
                let q = self.doc().filter(|d| d.ed.has_selection()).map(|d| d.ed.selected_text().to_string()).unwrap_or_default();
                self.popup = Popup::FindInFiles { query: q, replace: String::new(), case_sensitive: false, whole_word: false, just_opened: true, replace_mode: false };
            }
            "find.replace_in_files" => {
                let q = self.doc().filter(|d| d.ed.has_selection()).map(|d| d.ed.selected_text().to_string()).unwrap_or_default();
                self.popup = Popup::FindInFiles { query: q, replace: String::new(), case_sensitive: false, whole_word: false, just_opened: true, replace_mode: true };
            }
            "find.usages" => self.find_usages(),
            // ---- navigate
            "nav.search_everywhere" => self.open_palette(PaletteMode::Everywhere),
            "nav.find_action" => self.open_palette(PaletteMode::Actions),
            "nav.goto_file" => self.open_palette(PaletteMode::Files),
            "nav.goto_symbol" => self.open_palette(PaletteMode::Symbols),
            "nav.recent_files" => self.open_palette(PaletteMode::Recent),
            "nav.file_structure" => self.open_structure_palette(),
            "nav.goto_line" => {
                if editor_on {
                    self.popup = Popup::GotoLine { input: String::new(), just_opened: true };
                }
            }
            "nav.declaration" => self.goto_declaration(None),
            "nav.back" => self.navigate_back(),
            "nav.forward" => self.navigate_forward(),
            "nav.next_problem" | "nav.prev_problem" => {
                let fwd = id == "nav.next_problem";
                if let Some(d) = self.doc() {
                    let mut starts: Vec<usize> = d.markers().iter().filter(|m| m.sev == Sev::Error || m.sev == Sev::Warning).map(|m| m.start).collect();
                    starts.sort_unstable();
                    starts.dedup();
                    let caret = d.ed.caret();
                    let target = if fwd { starts.iter().find(|&&s| s > caret).or(starts.first()) } else { starts.iter().rev().find(|&&s| s < caret).or(starts.last()) };
                    if let Some(&t) = target {
                        let path = d.path.clone();
                        if let Some(d) = self.doc_mut() {
                            d.ed.set_caret(t, false);
                            d.view.scroll_to_caret = true;
                        }
                        let _ = path;
                    }
                }
            }
            "nav.last_edit" => {
                if let Some((p, o)) = self.last_edit.clone() {
                    self.goto(&p, o, true);
                }
            }
            // ---- code
            "code.complete" => self.refresh_completion(true),
            "code.quick_doc" => self.show_quick_doc(),
            "code.param_info" => self.show_param_info(),
            "code.expand_macro" => self.macroexpand_form(),
            "refactor.rename" => {
                if let Some(name) = self.symbol_at_caret() {
                    self.popup = Popup::Rename { old: name.clone(), input: name, just_opened: true };
                }
            }
            // ---- run / REPL
            "run.run_file" => self.run_selected(),
            "run.load_system" => self.repl_load_system(),
            "run.test_system" => {
                let sys = match &self.run_config {
                    RunConfig::Load(s) | RunConfig::Test(s) => Some(s.clone()),
                    RunConfig::CurrentFile | RunConfig::Make(_) | RunConfig::Scenario(_) => self.asd_systems().into_iter().next(),
                };
                match sys {
                    Some(s) => self.run_asdf_system(&s, true),
                    None => self.toast(tr("No .asd system found in this project"), Sev::Weak),
                }
            }
            "run.run_config" => {}
            "run.scenarios" => self.open_scenarios_dialog(),
            "run.compile_file" => self.compile_current_file(),
            "run.stop" => {
                self.stop_run();
                if !self.repl.busy.is_empty() {
                    self.repl_restart();
                    self.toast(tr("REPL interrupted (restarted)"), Sev::Weak);
                }
            }
            "repl.eval" | "repl.eval_last" => {
                let range = self.doc().and_then(|d| if id == "repl.eval_last" && !d.ed.has_selection() { d.ed.sexp_before_caret() } else { d.ed.form_to_eval() });
                if let (Some((s, e)), Some(d)) = (range, self.doc()) {
                    let code = d.ed.text()[s..e].to_string();
                    self.bottom_tool = Some(BottomTool::Repl);
                    self.repl_eval(&code, true);
                    // keep editor focus: it is the editor's command
                } else if self.doc().is_some() {
                    self.toast(tr("Nothing to evaluate here"), Sev::Weak);
                }
            }
            "repl.load_file" => {
                if let Some(a) = self.doc().map(|_| self.active) {
                    if self.docs[a].path.is_none() || self.docs[a].ed.is_modified() {
                        self.save_doc(a);
                    }
                    if let Some(p) = self.docs[a].path.clone() {
                        let code = format!("(load {})", lisp_runtime::runner::lisp_string(&self.lisp_path_of(&p)));
                        self.bottom_tool = Some(BottomTool::Repl);
                        self.repl_eval(&code, true);
                    }
                }
            }
            "repl.restart" => {
                self.bottom_tool = Some(BottomTool::Repl);
                self.repl_restart();
            }
            "repl.clear" => self.repl.log.clear(),
            "repl.focus" => {
                self.bottom_tool = Some(BottomTool::Repl);
                self.repl.focus_input = true;
                self.editor_focus = false;
                if self.repl.session.is_none() {
                    self.ensure_repl();
                }
            }
            // ---- view
            "view.project" => self.toggle_left(LeftTool::Project),
            "view.structure" => self.toggle_left(LeftTool::Structure),
            "view.repl" => self.toggle_bottom(BottomTool::Repl),
            "view.run" => self.toggle_bottom(BottomTool::Run),
            "view.problems" => self.toggle_bottom(BottomTool::Problems),
            "view.terminal" => self.toggle_bottom(BottomTool::Terminal),
            "view.git" => self.toggle_bottom(BottomTool::Git),
            "view.hide_all" => {
                if self.left_tool.is_some() || self.bottom_tool.is_some() {
                    self.left_tool = None;
                    self.bottom_tool = None;
                } else {
                    self.left_tool = Some(LeftTool::Project);
                }
            }
            "view.zoom_in" => self.settings.zoom(1.0),
            "view.zoom_out" => self.settings.zoom(-1.0),
            "view.zoom_reset" => self.settings.font_size = 14.0,
            "view.preview_code" | "view.preview_split" | "view.preview_only" => {
                let mode = match id {
                    "view.preview_code" => PreviewMode::Code,
                    "view.preview_split" => PreviewMode::Split,
                    _ => PreviewMode::Preview,
                };
                if self.doc_markup(self.active).is_some() {
                    self.set_preview_mode(self.active, mode);
                } else {
                    self.toast(tr("Only Markdown and reStructuredText files have a preview"), Sev::Weak);
                }
            }
            "view.toggle_theme" => {
                self.settings.theme = self.settings.toggled_theme();
                self.theme_changed();
                self.pal.apply(ctx);
            }
            "view.line_numbers" => self.settings.show_line_numbers = !self.settings.show_line_numbers,
            "view.whitespace" => self.settings.show_whitespace = !self.settings.show_whitespace,
            "view.rainbow" => self.settings.rainbow_brackets = !self.settings.rainbow_brackets,
            "help.about" => self.popup = Popup::About,
            "help.keymap" => {
                self.popup = Popup::Settings { lisp_path: self.settings.lisp_path.clone(), detected: self.lisp.as_ref().map(|l| l.describe()).unwrap_or_default(), filter: String::new(), page: 0 };
            }
            // ---- git
            "git.commit" | "view.commit" => {
                if id == "view.commit" && self.left_tool == Some(LeftTool::Commit) {
                    self.left_tool = None;
                } else {
                    self.left_tool = Some(LeftTool::Commit);
                    self.git_refresh();
                    if id == "git.commit" {
                        self.git.focus_msg = true;
                        self.editor_focus = false;
                    }
                }
            }
            "git.local_commit" => {
                if self.project.is_some() {
                    self.popup = Popup::LocalCommit { input: String::new(), just_opened: true };
                } else {
                    self.toast(tr("Open a project first"), Sev::Weak);
                }
            }
            "git.reset_base" => self.local_set_base(ide_core::localvcs::Base::Head),
            "git.push" => {
                self.save_all();
                self.git_run_async("Push", |r| r.push());
            }
            "git.pull" => {
                self.save_all();
                self.git_run_async("Update", |r| r.pull());
            }
            "git.branches" => self.open_branch_palette(),
            "git.new_branch" => self.popup = Popup::NewBranch { input: String::new(), just_opened: true },
            "git.diff" => {
                if let Some(rel) = self.current_rel_path() {
                    self.git_show_diff(&rel);
                }
            }
            "git.rollback" => {
                if let Some(rel) = self.current_rel_path() {
                    if self.git.changes.iter().any(|c| c.path == rel) {
                        self.popup = Popup::Confirm { kind: ConfirmKind::Rollback(rel.clone()), message: i18n::fmt("Discard all local changes to '{}'? This cannot be undone.", &[&rel]) };
                    } else {
                        self.toast(tr("No changes in this file"), Sev::Weak);
                    }
                }
            }
            "git.refresh" => {
                self.git.rx = None;
                self.git_refresh();
                self.refresh_project();
            }
            "git.init" => {
                if let Some(root) = self.project_root().map(|p| p.to_path_buf()) {
                    match ide_core::git::init(&root) {
                        Ok(_) => {
                            self.git_attach();
                            self.toast(tr("Git repository created"), Sev::Weak);
                        }
                        Err(e) => self.toast(i18n::fmt("git init failed: {}", &[&e]), Sev::Error),
                    }
                }
            }
            "git.stage_file" => {
                if let Some(rel) = self.current_rel_path() {
                    self.git_run_sync("Add", move |r| r.stage(&[rel]).map(|_| String::new()));
                }
            }
            "edit.eol_lf" | "edit.eol_crlf" => {
                if editor_on {
                    let le = if id == "edit.eol_crlf" { LineEnding::CrLf } else { LineEnding::Lf };
                    if let Some(d) = self.doc_mut() {
                        d.ed.set_line_ending(le);
                    }
                }
            }
            // ---- folding
            "edit.fold" | "edit.unfold" | "edit.fold_all" | "edit.unfold_all" => self.fold_command(id),
            // ---- everything else is an editor command
            other => {
                if editor_on {
                    self.with_editor(ctx, other);
                    if matches!(other, "edit.undo" | "edit.redo" | "edit.newline" | "edit.backspace" | "edit.delete" | "edit.indent" | "edit.unindent") {
                        self.completion.active = false;
                    }
                }
            }
        }
    }

    /// Open the settings dialog on `page`.
    pub fn open_settings_page(&mut self, page: usize) {
        self.popup = Popup::Settings { lisp_path: self.settings.lisp_path.clone(), detected: self.lisp.as_ref().map(|l| l.describe()).unwrap_or_default(), filter: String::new(), page };
    }

    pub fn toggle_left(&mut self, t: LeftTool) {
        self.left_tool = if self.left_tool == Some(t) { None } else { Some(t) };
    }

    pub fn toggle_bottom(&mut self, t: BottomTool) {
        self.bottom_tool = if self.bottom_tool == Some(t) { None } else { Some(t) };
        if self.bottom_tool == Some(BottomTool::Repl) && self.repl.session.is_none() {
            self.ensure_repl();
        }
        if self.bottom_tool == Some(BottomTool::Terminal) {
            self.terminal_start_dir();
            self.term.focus_input = true;
            self.editor_focus = false;
        }
    }

    pub fn run_find_dialog(&mut self, query: &str, replace: Option<&str>, case_sensitive: bool, whole_word: bool) {
        let opts = TextQuery { case_sensitive, whole_word };
        let Some(rep) = replace else {
            self.run_find_in_files(query, opts);
            return;
        };
        {
            let Some(root) = self.project_root().map(std::path::Path::to_path_buf) else {
                self.toast(tr("Open a project first"), Sev::Weak);
                return;
            };
            self.save_all();
            let hits = ide_core::search::find_in_files(&root, query, opts, 20_000);
            let mut files: Vec<std::path::PathBuf> = hits.iter().map(|h| h.path.clone()).collect();
            files.sort();
            files.dedup();
            let mut total = 0;
            for f in files {
                if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(f.as_path())) {
                    let ranges = ide_core::search::find_in_text(self.docs[i].ed.text(), query, opts);
                    total += ranges.len();
                    self.docs[i].ed.replace_ranges(&ranges, rep);
                } else if let Ok(text) = std::fs::read_to_string(&f) {
                    let ranges = ide_core::search::find_in_text(&text, query, opts);
                    let mut out = text.clone();
                    for &(s, e) in ranges.iter().rev() {
                        out.replace_range(s..e, rep);
                    }
                    if std::fs::write(&f, out).is_ok() {
                        total += ranges.len();
                    }
                }
            }
            self.toast(i18n::fmt("Replaced {} occurrence(s)", &[&total]), Sev::Weak);
            self.run_find_in_files(rep, opts);
        }
    }
}

pub fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Open `url` (an https link) in the default browser.
pub fn open_url(url: &str) {
    if !url.starts_with("https://") || url.contains(['"', '\n', '\r', ' ']) {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `start` treats & and friends specially: the quoted raw argument keeps the link in one piece
        let _ = std::process::Command::new("cmd").args(["/C", "start", "\"\""]).raw_arg(format!("\"{url}\"")).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

pub fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg("/select,").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_core::commands;

    #[test]
    fn every_registry_shortcut_maps_to_an_egui_key() {
        for c in commands::COMMANDS {
            for s in c.shortcuts {
                if *s == "Shift Shift" {
                    continue;
                }
                assert!(parse_key_combo(s).is_some(), "shortcut {s} of {} cannot be produced by an egui key", c.id);
            }
        }
    }

    #[test]
    fn key_roundtrip() {
        let (k, m) = parse_key_combo("Ctrl+Shift+A").unwrap();
        assert_eq!(k, Key::A);
        assert!(m.command && m.shift && !m.alt);
        let sc = shortcut_of(k, m).unwrap();
        assert_eq!(sc.to_string(), "Ctrl+Shift+A");
        assert_eq!(shortcut_of(Key::Plus, Modifiers::COMMAND).unwrap().key, "Equals");
    }

    #[test]
    fn editor_context_classification() {
        assert!(needs_editor("caret.left"));
        assert!(needs_editor("edit.backspace"));
        assert!(needs_editor("lisp.raise"));
        assert!(!needs_editor("file.save"));
        assert!(!needs_editor("nav.search_everywhere"));
        assert!(!needs_editor("find.find"));
    }
}
