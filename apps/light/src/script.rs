//! Automation: drive the real UI from a JSON script (synthetic keyboard/mouse events), assert on
//! application state and take screenshots of the app window. Used for end-to-end tests.
//!
//! A script is a JSON array of steps, e.g.
//! `[{"op":"open_project","path":"..."}, {"op":"key","value":"Ctrl+Shift+A"}, {"op":"text","value":"save"},
//!   {"op":"assert_popup","value":"palette"}, {"op":"screenshot","path":"a.png"}]`

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Event, Pos2};
use serde_json::Value;

use crate::app::App;
use crate::input;
use crate::state::*;

/// Where the screenshot of a frame lands once egui has taken it.
type ShotSlot = Arc<Mutex<Option<Arc<egui::ColorImage>>>>;

pub struct Script {
    steps: VecDeque<Value>,
    pending: Vec<Event>,
    wait_frames: u32,
    wait_cond: Option<(Value, f64)>,
    shot: Option<(PathBuf, ShotSlot)>,
    pub failures: Vec<String>,
    total: usize,
    done: bool,
    started: f64,
}

pub fn load(path: &Path) -> Result<Script, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("bad script JSON: {e}"))?;
    let arr = v.as_array().ok_or("script must be a JSON array")?;
    Ok(Script { total: arr.len(), steps: arr.iter().cloned().collect(), pending: vec![], wait_frames: 3, wait_cond: None, shot: None, failures: vec![], done: false, started: 0.0 })
}

pub fn save_png(path: &Path, img: &egui::ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width() as u32, img.height() as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    w.write_image_data(&bytes).map_err(|e| e.to_string())
}

impl App {
    pub fn script_inject(&mut self, raw: &mut egui::RawInput) {
        if let Some(s) = self.script.as_mut() {
            // Scripted runs must not receive real mouse/keyboard events from the
            // desktop while the user continues working in other windows.
            raw.events.clear();
            raw.focused = true;
            let modifiers = s.pending.iter().filter_map(|event| match event {
                Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            }).next_back().unwrap_or(egui::Modifiers::NONE);
            raw.events.push(Event::ModifiersChanged(modifiers));
            raw.events.append(&mut s.pending);
        }
    }

    fn cond_met(&self, cond: &Value) -> bool {
        match cond["what"].as_str().unwrap_or("") {
            "repl_ready" => self.repl.ready,
            "idle" => self.repl.busy.is_empty() && !self.run.running && self.compile.is_none() && self.repl.pending.is_empty(),
            "index" => self.project.as_ref().is_none_or(|p| p.index_rx.is_none()),
            "run_done" => !self.run.running && self.run.handle.is_none(),
            "compile_done" => self.compile.is_none(),
            "repl_contains" => {
                let t = cond["value"].as_str().unwrap_or("");
                self.repl.log.iter().any(|l| l.text.contains(t))
            }
            "completion" => self.completion.active && !self.completion.items.is_empty(),
            "hover" => self.hover.info.is_some(),
            "checks_idle" => self.checks.idle(),
            "git_ready" => self.git.repo.is_some() && self.git.head.is_some() && self.git.rx.is_none(),
            "git_dirty" => self.git.rx.is_none() && !self.git.changes.is_empty(),
            "git_settled" => self.git.rx.is_none(),
            "git_clean" => self.git.rx.is_none() && self.git.changes.is_empty(),
            "term_idle" => !self.term_busy(),
            "script_done" => self.term.script.is_none() && self.term.handle.is_none(),
            "term_contains" => {
                let t = cond["value"].as_str().unwrap_or("");
                self.term.log.iter().any(|l| l.text.contains(t))
            }
            "diff_new_contains" => {
                let t = cond["value"].as_str().unwrap_or("");
                self.docs.iter().filter_map(|d| d.diff.as_deref()).any(|d| d.new.contains(t))
            }
            // real time has passed (`sleep`): things that go by the clock, like scrolling while dragging, need it
            "after" => self.now >= cond["until"].as_f64().unwrap_or(0.0),
            "wsl_idle" => !self.wsl.busy(),
            "new_project_idle" => matches!(&self.popup, Popup::NewProject { location, creating, .. } if location.job.is_none() && !creating),
            "clone_idle" => matches!(&self.popup, Popup::Clone(state) if state.run.is_none()),
            "project_browser_idle" => matches!(&self.popup, Popup::ProjectBrowser(browser) if browser.idle()) && !self.wsl.probing(),
            // the registry has answered (with a list or an error)
            "modules_index" => self.mods.index.is_some() || self.mods.index_error.is_some(),
            "modules_idle" => self.mods.busy.is_empty() && !self.mods.loading_index && !self.mods.loading_covers,
            // the running setup step has got at least this far (0.0..1.0)
            "wsl_progress" => {
                let min = cond["value"].as_f64().unwrap_or(0.0) as f32;
                self.wsl.task.as_ref().is_some_and(|t| t.tracker.progress().fraction.is_some_and(|f| f >= min))
            }
            "github_idle" => !self.github.busy(),
            "popup" => !matches!(self.popup, Popup::None),
            "all_saved" => self.docs.iter().all(|d| !d.ed.is_modified() || d.path.is_none()),
            "find_done" => self.find_rx.is_none(),
            "ask_open" => matches!(self.popup, Popup::OpenProject { .. }),
            "no_popup" => matches!(self.popup, Popup::None),
            _ => true,
        }
    }

    fn fail(&mut self, msg: String) {
        eprintln!("SCRIPT FAIL: {msg}");
        if let Some(s) = self.script.as_mut() {
            s.failures.push(msg);
        }
    }

    pub fn script_step(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.script.take() else { return };
        let now = ctx.input(|i| i.time);
        if s.started == 0.0 {
            s.started = now;
        }
        ctx.request_repaint();
        s.pending.clear();
        // finish a pending screenshot
        if let Some((path, slot)) = s.shot.take() {
            let img = slot.lock().unwrap().take();
            match img {
                Some(img) => {
                    if let Err(e) = save_png(&path, &img) {
                        s.failures.push(format!("screenshot {}: {e}", path.display()));
                    } else {
                        println!("screenshot saved: {}", path.display());
                    }
                }
                None => {
                    s.shot = Some((path, slot));
                    self.script = Some(s);
                    return;
                }
            }
        }
        if s.wait_frames > 0 {
            s.wait_frames -= 1;
            self.script = Some(s);
            return;
        }
        if let Some((cond, deadline)) = s.wait_cond.take() {
            if self.cond_met(&cond) {
                s.wait_frames = 2;
            } else if now > deadline {
                s.failures.push(format!("timeout waiting for {}", cond));
                eprintln!("SCRIPT FAIL: timeout waiting for {cond}");
            } else {
                s.wait_cond = Some((cond, deadline));
            }
            self.script = Some(s);
            return;
        }
        let Some(step) = s.steps.pop_front() else {
            if !s.done {
                s.done = true;
                let ok = s.failures.is_empty();
                println!("SCRIPT {}: {} steps, {} failure(s)", if ok { "OK" } else { "FAILED" }, s.total, s.failures.len());
                println!("max frame time: {:.0} ms", self.max_frame_ms);
                for f in &s.failures {
                    println!("  - {f}");
                }
                let code = if ok { 0 } else { 1 };
                std::process::exit(code);
            }
            self.script = Some(s);
            return;
        };
        let op = step["op"].as_str().unwrap_or("").to_string();
        let sval = step["value"].as_str().unwrap_or("").to_string();
        let path = step["path"].as_str().map(PathBuf::from);
        // put the script back before executing steps that need &mut self
        self.script = Some(s);
        match op.as_str() {
            "open_project" => {
                if let Some(p) = path {
                    self.open_project(&p);
                }
            }
            "open" => {
                if let Some(p) = path {
                    self.open_file(&p);
                }
            }
            "close_project" => self.close_project(),
            // define a scenario: `value` is its name, `commands` its lines, `stop` (default true) stops at a failure
            "scenario" => {
                let commands: Vec<String> = step["commands"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                let sc = crate::app_env::Scenario { name: sval.clone(), commands, stop_on_error: step["stop"].as_bool().unwrap_or(true) };
                let mut list: Vec<_> = self.scenarios().iter().filter(|s| s.name != sval).cloned().collect();
                list.push(sc);
                self.save_scenarios(list);
            }
            // choose the run configuration: `scenario:NAME`, `file`, or `make:TARGET`
            "select_run" => {
                self.run_config = match sval.split_once(':') {
                    Some(("scenario", n)) => crate::app_run::RunConfig::Scenario(n.to_string()),
                    Some(("make", t)) => crate::app_run::RunConfig::Make(t.to_string()),
                    _ => crate::app_run::RunConfig::CurrentFile,
                };
            }
            "assert_run_lacks" => {
                if self.run.log.iter().any(|l| l.text.contains(&sval)) {
                    self.fail(format!("assert_run_lacks: {sval:?} is in the run log"));
                }
            }
            "project_env" => self.set_project_env(if sval == "windows" { None } else { Some(sval.clone()) }),
            "ask_open_project" => {
                if let Some(p) = path {
                    self.request_open_project(&p);
                }
            }
            "clone_set" => {
                if let Popup::Clone(c) = &mut self.popup {
                    if let Some(u) = step["value"].as_str() {
                        c.url = u.to_string();
                    }
                    if let Some(d) = step["dir"].as_str() {
                        c.parent = d.to_string();
                        c.dir_touched = true;
                    }
                    if let Some(b) = step["branch"].as_str() {
                        c.branch = b.to_string();
                    }
                    if let Some(b) = step["build_in_linux"].as_bool() {
                        c.build_in_linux = b;
                    }
                    if let Some(b) = step["linux"].as_bool() {
                        c.linux = b;
                    }
                    if let Some(value) = step["distro"].as_str() { c.linux_distro = value.into(); }
                    if let Some(value) = step["linux_dir"].as_str() { c.linux_dir = value.into(); }
                    if let Some(v) = step["modules"].as_array() {
                        c.modules = v.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                        c.show_modules = true;
                    }
                } else {
                    self.fail("clone_set: the clone dialog is not open".to_string());
                }
            }
            "expand" => {
                if let (Some(p), Some(proj)) = (path, self.project.as_mut()) {
                    proj.expanded.insert(p);
                    proj.tree_ver += 1;
                }
            }
            "find_in_files" => self.run_find_in_files(&sval, ide_core::search::TextQuery::default()),
            "key" => {
                if let Some((key, mods)) = input::parse_key_combo(&sval) {
                    let s = self.script.as_mut().unwrap();
                    s.pending.push(Event::ModifiersChanged(mods));
                    s.pending.push(Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: mods });
                    s.pending.push(Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers: mods });
                    s.pending.push(Event::ModifiersChanged(egui::Modifiers::NONE));
                } else {
                    self.fail(format!("bad key combo '{sval}'"));
                }
            }
            "text" => {
                let s = self.script.as_mut().unwrap();
                for ch in sval.chars() {
                    s.pending.push(Event::Text(ch.to_string()));
                }
            }
            "paste" => {
                let s = self.script.as_mut().unwrap();
                s.pending.push(Event::Paste(sval.clone()));
            }
            "command" => self.run_command(ctx, &sval),
            "sleep" => {
                let until = now + step["value"].as_f64().unwrap_or(1.0);
                self.script.as_mut().unwrap().wait_cond = Some((serde_json::json!({"what": "after", "until": until}), until + 10.0));
            }
            "frames" => {
                self.script.as_mut().unwrap().wait_frames = step["value"].as_u64().unwrap_or(1) as u32;
            }
            "wait" => {
                let timeout = step["timeout"].as_f64().unwrap_or(60.0);
                self.script.as_mut().unwrap().wait_cond = Some((step.clone(), now + timeout));
            }
            "set_text" => {
                if let Some(d) = self.doc_mut() {
                    let t = sval.replace("\\n", "\n");
                    d.ed.select_all();
                    d.ed.insert_text_raw(&t);
                    d.ed.set_caret(0, false);
                }
            }
            "caret" => {
                let line = step["line"].as_u64().unwrap_or(1) as usize;
                let col = step["col"].as_u64().unwrap_or(1) as usize;
                if let Some(d) = self.doc_mut() {
                    let o = d.ed.offset_of(line.saturating_sub(1), col.saturating_sub(1));
                    d.ed.set_caret(o, false);
                    d.view.scroll_to_caret = true;
                }
                self.editor_focus = true;
            }
            "focus_editor" => self.editor_focus = true,
            "project_browser_focus" => {
                let id = if sval == "path" { "project_browser_address" } else { "project_browser_search" };
                ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique(id)));
            }
            "click" => {
                let (x, y) = (step["x"].as_f64().unwrap_or(0.0) as f32, step["y"].as_f64().unwrap_or(0.0) as f32);
                let s = self.script.as_mut().unwrap();
                let pos = Pos2::new(x, y);
                s.pending.push(Event::PointerMoved(pos));
                s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
            }
            // the mouse on a row of the console log last drawn: `line` is the text of the row as shown, `col` a character
            // column or "end"; `action`: down, move, up, or click (with `count`)
            "log_mouse" => {
                let Some(g) = ctx.data(|d| d.get_temp::<crate::ui_panels::LogGeometry>(egui::Id::unique(crate::ui_panels::LOG_GEOMETRY_ID))) else {
                    self.fail("log_mouse: no console log has been drawn".to_string());
                    return;
                };
                let want = match step["line"].as_str().unwrap_or("") {
                    // the first row may be cut off at the top: the one below it is whole
                    "@first" => g.rows.get(1).or(g.rows.first()).cloned().unwrap_or_default(),
                    "@last" => g.rows.last().cloned().unwrap_or_default(),
                    other => other.to_string(),
                };
                let want = want.as_str();
                let Some(row) = g.rows.iter().position(|r| r == want) else {
                    self.fail(format!("log_mouse: no row {want:?} on screen; rows: {:?}", g.rows));
                    return;
                };
                let len = g.rows[row].chars().count();
                let col = match &step["col"] {
                    serde_json::Value::String(s) if s == "end" => len,
                    v => v.as_u64().unwrap_or(0) as usize,
                };
                // the row's text starts 14 points in; a column boundary is where the pointer goes
                let mut pos = Pos2::new(g.left + 14.0 + col as f32 * g.char_w, g.top + (row as f32 + 0.5) * g.row_h);
                // `y`: above the top or below the bottom of the log by `px` points (dragging there scrolls)
                let px = step["px"].as_f64().unwrap_or(60.0) as f32;
                match step["y"].as_str() {
                    Some("above") => pos.y = g.top - px,
                    Some("below") => pos.y = g.top + g.view_h + px,
                    _ => {}
                }
                let button = |pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
                let s = self.script.as_mut().unwrap();
                s.pending.push(Event::PointerMoved(pos));
                match step["action"].as_str().unwrap_or("click") {
                    "down" => s.pending.push(button(true)),
                    "up" => s.pending.push(button(false)),
                    "move" => {}
                    _ => {
                        for _ in 0..step["count"].as_u64().unwrap_or(1) {
                            s.pending.push(button(true));
                            s.pending.push(button(false));
                        }
                    }
                }
            }
            "copy" => {
                let s = self.script.as_mut().unwrap();
                s.pending.push(Event::Copy);
            }
            // the mouse wheel over (x, y): `dy` points, negative scrolls down
            "wheel" => {
                let pos = Pos2::new(step["x"].as_f64().unwrap_or(0.0) as f32, step["y"].as_f64().unwrap_or(0.0) as f32);
                let s = self.script.as_mut().unwrap();
                s.pending.push(Event::PointerMoved(pos));
                s.pending.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::Vec2::new(0.0, step["dy"].as_f64().unwrap_or(0.0) as f32), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: egui::Modifiers::NONE });
            }
            // the mouse left the window (with a button down it keeps its capture, but egui forgets where it is)
            "pointer_gone" => {
                let s = self.script.as_mut().unwrap();
                s.pending.push(Event::PointerGone);
            }
            // fill the terminal log with `value` numbered lines, to have something to scroll
            "term_fill" => {
                self.terminal_start_dir();
                for i in 1..=step["value"].as_u64().unwrap_or(100) {
                    self.term.log.push(LogLine { kind: LogKind::Output, text: format!("line {i}\n") });
                }
                self.bottom_tool = Some(BottomTool::Terminal);
            }
            // a program of that name is not running any more (Windows `tasklist`)
            "assert_process_gone" => {
                let out = std::process::Command::new("tasklist").args(["/FI", &format!("IMAGENAME eq {sval}"), "/NH"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
                if out.to_lowercase().contains(&sval.to_lowercase()) {
                    self.fail(format!("assert_process_gone: {sval} is still running: {}", out.trim()));
                }
            }
            // fill in the New Project dialog: `name`, `parent`, `modules` (ids to tick) and `template` (`module/Name`)
            "new_project_set" => {
                if let Popup::NewProject { parent, name, modules, template, location, .. } = &mut self.popup {
                    if let Some(value) = step["linux"].as_bool() { location.linux = value; }
                    if let Some(value) = step["distro"].as_str() { location.distro = value.into(); }
                    if let Some(value) = step["linux_dir"].as_str() { location.parent = value.into(); }
                    if let Some(v) = step["parent"].as_str() {
                        *parent = v.to_string();
                    }
                    if let Some(v) = step["name"].as_str() {
                        *name = v.to_string();
                    }
                    if let Some(v) = step["modules"].as_array() {
                        *modules = v.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                    }
                    if let Some(v) = step["template"].as_str() {
                        *template = v.to_string();
                    }
                } else {
                    self.fail("new_project_set: the New Project dialog is not open".to_string());
                }
            }
            "assert_project_creation_error" => {
                let error = match &self.popup { Popup::NewProject { location, .. } => location.error.as_deref(), _ => None };
                if !error.is_some_and(|error| error.contains(&sval)) { self.fail(format!("expected project creation error containing {sval:?}, got {error:?}")); }
            }
            "assert_clone_error" => {
                let error = match &self.popup { Popup::Clone(state) => state.error.as_deref(), _ => None };
                if !error.is_some_and(|error| error.contains(&sval)) { self.fail(format!("expected clone error containing {sval:?}, got {error:?}")); }
            }
            "module_install" => self.module_install(&sval),
            "module_remove" => self.module_remove(&sval),
            "module_enable" => self.module_set_enabled(&sval, step["on"].as_bool().unwrap_or(true)),
            "modules_tab" => self.open_modules_tab(),
            // the language of the active document: its id (`lisp` for Lisp)
            "assert_language" => {
                let got = self.active_language_id().unwrap_or_else(|| "lisp".to_string());
                if got != sval {
                    self.fail(format!("assert_language: expected {sval}, got {got}"));
                }
            }
            // `installed`, `enabled` and `needed` list module ids (any order); `tab`: the Modules tab is open
            "assert_modules" => {
                let ids = |key: &str| -> Option<Vec<String>> { step[key].as_array().map(|a| { let mut v: Vec<String> = a.iter().filter_map(|x| x.as_str().map(String::from)).collect(); v.sort(); v }) };
                let mut errors = vec![];
                let mut got_installed: Vec<String> = self.mods.installed.iter().map(|m| m.manifest.id.clone()).collect();
                got_installed.sort();
                let mut got_enabled: Vec<String> = self.mods.installed.iter().filter(|m| m.enabled).map(|m| m.manifest.id.clone()).collect();
                got_enabled.sort();
                let mut got_needed = self.needed_modules.clone();
                got_needed.sort();
                for (key, got) in [("installed", got_installed), ("enabled", got_enabled), ("needed", got_needed)] {
                    if let Some(want) = ids(key)
                        && want != got
                    {
                        errors.push(format!("{key}: expected {want:?}, got {got:?}"));
                    }
                }
                if let Some(b) = step["lisp"].as_bool()
                    && b != self.lisp_enabled()
                {
                    errors.push(format!("lisp: expected {b}, got {}", self.lisp_enabled()));
                }
                if let Some(b) = step["tab"].as_bool() {
                    let open = self.docs.iter().any(|d| d.page.is_some());
                    if open != b {
                        errors.push(format!("tab: expected {b}, got {open}"));
                    }
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_modules: {}", errors.join("; ")));
                }
            }
            "assert_log" => {
                let Some(g) = ctx.data(|d| d.get_temp::<crate::ui_panels::LogGeometry>(egui::Id::unique(crate::ui_panels::LOG_GEOMETRY_ID))) else {
                    self.fail("assert_log: no console log has been drawn".to_string());
                    return;
                };
                let up = g.max_offset - g.offset_y;
                let mut errors = vec![];
                if let Some(b) = step["at_bottom"].as_bool()
                    && b != (up < 2.0)
                {
                    errors.push(format!("at_bottom expected {b}: scrolled {up:.0} points up from the bottom"));
                }
                if let Some(n) = step["scrolled_up_at_least"].as_f64()
                    && (up as f64) < n
                {
                    errors.push(format!("expected to be scrolled up by at least {n}, got {up:.0}"));
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_log: {}", errors.join("; ")));
                }
            }
            "assert_copied" => {
                // `value`: the whole text; or `starts` / `ends` / `contains` for the parts that do not depend on the machine
                let got = ctx.data(|d| d.get_temp::<String>(egui::Id::unique(crate::ui_panels::LAST_COPIED_ID))).unwrap_or_default();
                let mut errors = vec![];
                if step["value"].is_string() && got != sval {
                    errors.push(format!("expected {sval:?}"));
                }
                if let Some(t) = step["starts"].as_str()
                    && !got.starts_with(t)
                {
                    errors.push(format!("expected it to start with {t:?}"));
                }
                if let Some(t) = step["ends"].as_str()
                    && !got.ends_with(t)
                {
                    errors.push(format!("expected it to end with {t:?}"));
                }
                if let Some(t) = step["contains"].as_str()
                    && !got.contains(t)
                {
                    errors.push(format!("expected it to contain {t:?}"));
                }
                if let Some(n) = step["min_lines"].as_u64()
                    && (got.lines().count() as u64) < n
                {
                    errors.push(format!("expected at least {n} lines, got {}", got.lines().count()));
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_copied: {}; got {got:?}", errors.join("; ")));
                }
            }
            "screenshot" => {
                let slot: ShotSlot = Arc::new(Mutex::new(None));
                let s2 = slot.clone();
                ctx.request_screenshot(move |img| {
                    *s2.lock().unwrap() = Some(img);
                });
                if let Some(p) = path {
                    self.script.as_mut().unwrap().shot = Some((p, slot));
                }
            }
            "mods" => {
                let m = match sval.as_str() {
                    "Shift" => egui::Modifiers::SHIFT,
                    _ => egui::Modifiers::NONE,
                };
                self.script.as_mut().unwrap().pending.push(Event::ModifiersChanged(m));
            }
            "assert_find" => {
                let want = step["matches"].as_u64().unwrap_or(0) as usize;
                self.refresh_find();
                if self.find.matches.len() != want {
                    self.fail(format!("assert_find: expected {want} matches, got {}", self.find.matches.len()));
                }
            }
            "assert_clone_url" => match &self.popup {
                Popup::Clone(c) if c.url == sval => {}
                Popup::Clone(c) => self.fail(format!("assert_clone_url: expected {sval:?}, got {:?}", c.url)),
                _ => self.fail("assert_clone_url: the clone dialog is not open".to_string()),
            },
            "assert_graph" => {
                let lanes = self.git.graph.iter().map(|g| g.width).max().unwrap_or(0);
                let want_lanes = step["min_lanes"].as_u64().unwrap_or(1) as usize;
                let want_commits = step["min_commits"].as_u64().unwrap_or(1) as usize;
                if lanes < want_lanes || self.git.graph.len() < want_commits {
                    self.fail(format!("assert_graph: {} commits in {lanes} lanes, wanted at least {want_commits} in {want_lanes}", self.git.graph.len()));
                }
            }
            "assert_hint" => {
                let ok = self.hint.as_ref().is_some_and(|h| h.0.contains(&sval));
                if !ok {
                    self.fail(format!("assert_hint: {sval:?} not shown (hint={:?})", self.hint));
                }
            }
            "log" => println!("[script] {sval}"),
            "assert_text" => {
                let want = sval.replace("\\n", "\n");
                let got = self.doc().map(|d| d.ed.text().to_string()).unwrap_or_default();
                if got != want {
                    self.fail(format!("assert_text: expected {want:?}, got {got:?}"));
                }
            }
            "assert_text_contains" => {
                let got = self.doc().map(|d| d.ed.text().to_string()).unwrap_or_default();
                if !got.contains(&sval.replace("\\n", "\n")) {
                    self.fail(format!("assert_text_contains: {sval:?} not in {got:?}"));
                }
            }
            "assert_caret" => {
                let (l, c) = self.doc().map(|d| d.ed.line_col(d.ed.caret())).unwrap_or((0, 0));
                let (wl, wc) = (step["line"].as_u64().unwrap_or(1) as usize, step["col"].as_u64().unwrap_or(1) as usize);
                if (l + 1, c + 1) != (wl, wc) {
                    self.fail(format!("assert_caret: expected {wl}:{wc}, got {}:{}", l + 1, c + 1));
                }
            }
            "assert_selection" => {
                let got = self.doc().map(|d| d.ed.selected_text().to_string()).unwrap_or_default();
                if got != sval {
                    self.fail(format!("assert_selection: expected {sval:?}, got {got:?}"));
                }
            }
            "assert_repl_contains" => {
                let ok = self.repl.log.iter().any(|l| l.text.contains(&sval));
                if !ok {
                    let tail: Vec<String> = self.repl.log.iter().rev().take(8).map(|l| l.text.clone()).collect();
                    self.fail(format!("assert_repl_contains: {sval:?} not found; tail: {tail:?}"));
                }
            }
            "assert_run_contains" => {
                if !self.run.log.iter().any(|l| l.text.contains(&sval)) {
                    let tail: Vec<String> = self.run.log.iter().rev().take(8).map(|l| l.text.clone()).collect();
                    self.fail(format!("assert_run_contains: {sval:?} not found; tail: {tail:?}"));
                }
            }
            "assert_popup" => {
                let name = match &self.popup {
                    Popup::None => "none",
                    Popup::Palette(_) => "palette",
                    Popup::GotoLine { .. } => "goto_line",
                    Popup::Rename { .. } => "rename",
                    Popup::NewFile { .. } => "new_file",
                    Popup::NewProject { .. } => "new_project",
                    Popup::FindInFiles { .. } => "find_in_files",
                    Popup::Settings { .. } => "settings",
                    Popup::Clone(_) => "clone",
                    Popup::ProjectBrowser(_) => "project_browser",
                    Popup::LocalView(_) => "local_view",
                    Popup::About => "about",
                    Popup::OpenProject { .. } => "open_project",
                    Popup::Info { .. } => "info",
                    Popup::Confirm { .. } => "confirm",
                    Popup::RenameFile { .. } => "rename_file",
                    Popup::Scenarios(_) => "scenarios",
                    Popup::NewBranch { .. } => "new_branch",
                    Popup::LocalCommit { .. } => "local_commit",
                };
                if name != sval {
                    self.fail(format!("assert_popup: expected {sval}, got {name}"));
                }
            }
            "assert_completion" => {
                let has = self.completion.items.iter().any(|c| c.text == sval);
                if !(self.completion.active && has) {
                    let names: Vec<_> = self.completion.items.iter().take(8).map(|c| c.text.clone()).collect();
                    let idx = self.project.as_ref().map_or(0, |p| p.index.symbol_count());
                    self.fail(format!("assert_completion: {sval:?} not offered (active={}, prefix={:?}, index_symbols={idx}, items={names:?})", self.completion.active, self.completion.prefix));
                }
            }
            "assert_no_completion" => {
                if self.completion.active {
                    self.fail("assert_no_completion: popup is active".into());
                }
            }
            "assert_tabs" => {
                let n = step["value"].as_u64().unwrap_or(0) as usize;
                if self.docs.len() != n {
                    self.fail(format!("assert_tabs: expected {n}, got {}", self.docs.len()));
                }
            }
            "assert_active_file" => {
                let got = self.doc().map(|d| d.name()).unwrap_or_default();
                if got != sval {
                    self.fail(format!("assert_active_file: expected {sval}, got {got}"));
                }
            }
            "assert_modified" => {
                let want = step["value"].as_bool().unwrap_or(true);
                let got = self.doc().is_some_and(|d| d.ed.is_modified());
                if got != want {
                    self.fail(format!("assert_modified: expected {want}, got {got}"));
                }
            }
            "assert_eol" => {
                // value: LF or CRLF, what the status bar says; "file": true looks at the bytes on disk instead
                let got = if step["file"].as_bool().unwrap_or(false) {
                    let bytes = path.as_ref().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
                    let crlf = bytes.windows(2).filter(|w| w == b"\r\n").count();
                    let lf = bytes.iter().filter(|&&b| b == b'\n').count();
                    match (crlf, lf) {
                        (0, _) => "LF".to_string(),
                        (c, l) if c == l => "CRLF".to_string(),
                        _ => "mixed".to_string(),
                    }
                } else {
                    self.doc().map(|d| d.ed.line_ending.label().to_string()).unwrap_or_default()
                };
                if got != sval {
                    self.fail(format!("assert_eol: expected {sval}, got {got}"));
                }
            }
            "assert_env" => {
                let got = match self.wsl_target() {
                    Some(t) => format!("linux:{}", t.distro),
                    None => "windows".to_string(),
                };
                if !sval.is_empty() && got != sval {
                    self.fail(format!("assert_env: expected {sval}, got {got}"));
                }
                if let Some(b) = step["banner"].as_bool()
                    && b != self.env_banner
                {
                    self.fail(format!("assert_env: banner expected {b}, got {}", self.env_banner));
                }
                if let Some(b) = step["tools_banner"].as_bool()
                    && b != self.tools_banner_visible()
                {
                    self.fail(format!("assert_env: tools banner expected {b}, got {} (missing: {:?})", self.tools_banner_visible(), self.missing_build_tools()));
                }
                if let Some(rc) = step["run_config"].as_str() {
                    let label = self.run_config_label();
                    if label != rc {
                        self.fail(format!("assert_env: run configuration expected {rc:?}, got {label:?}"));
                    }
                }
            }
            // the "Install developer tools" button of the banner
            "banner_install_tools" => match self.missing_build_tools() {
                Some((d, missing)) if self.tools_banner_visible() => self.install_build_tools_here(&d, &missing),
                other => self.fail(format!("banner_install_tools: no banner ({other:?})")),
            },
            "assert_make_problem" => {
                let file = step["file"].as_str().unwrap_or("").replace('/', "\\");
                let line = step["line"].as_u64().unwrap_or(1) as usize;
                let found = self.compiler_problems.iter().any(|p| {
                    p.source == "make"
                        && p.path.as_ref().is_some_and(|f| f.to_string_lossy().replace('/', "\\").ends_with(&file) && f.exists())
                        && p.path.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|t| t[..p.start.min(t.len())].matches('\n').count() + 1 == line)
                });
                if !found {
                    let rows: Vec<String> = self.compiler_problems.iter().map(|p| format!("{:?} @{} {}", p.path, p.start, p.message)).collect();
                    self.fail(format!("assert_make_problem: no make problem in {file} line {line}; have {rows:?}"));
                }
            }
            "assert_max_problems" => {
                let max = step["value"].as_u64().unwrap_or(0) as usize;
                let all = self.all_problems();
                if all.len() > max {
                    let rows: Vec<String> = all.iter().map(|p| format!("{:?}: {}", p.path, p.message)).collect();
                    self.fail(format!("assert_max_problems: expected <= {max}, got {rows:?}"));
                }
            }
            // a problem in the list whose text contains `value`; with `line`, on that 1-based line of the active file
            "assert_problem" => {
                let line = step["line"].as_u64().map(|l| l as usize);
                let found = self.all_problems().iter().any(|p| {
                    p.message.contains(&sval)
                        && line.is_none_or(|l| {
                            self.docs.iter().find(|d| d.path == p.path && d.diff.is_none()).is_some_and(|d| d.ed.line_col(p.start).0 + 1 == l)
                        })
                });
                if !found {
                    let rows: Vec<String> = self.all_problems().iter().map(|p| format!("{:?}@{}: {}", p.path.as_ref().and_then(|f| f.file_name()), p.start, p.message)).collect();
                    self.fail(format!("assert_problem: {sval:?} (line {line:?}) not among {rows:?}"));
                }
            }
            "assert_problems" => {
                let min = step["min"].as_u64().unwrap_or(1) as usize;
                let n = self.all_problems().len();
                if n < min {
                    self.fail(format!("assert_problems: expected >= {min}, got {n}"));
                }
            }
            "assert_file_contains" => {
                let got = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
                if !got.contains(&sval) {
                    self.fail(format!("assert_file_contains: {sval:?} not in {:?}", path));
                }
            }
            "assert_bottom" => {
                let name = match self.bottom_tool {
                    Some(BottomTool::Repl) => "repl",
                    Some(BottomTool::Run) => "run",
                    Some(BottomTool::Problems) => "problems",
                    Some(BottomTool::Find) => "find",
                    Some(BottomTool::Git) => "git",
                    Some(BottomTool::Terminal) => "terminal",
                    None => "none",
                };
                if name != sval {
                    self.fail(format!("assert_bottom: expected {sval}, got {name}"));
                }
            }
            "assert_left" => {
                let name = match self.left_tool {
                    Some(LeftTool::Project) => "project",
                    Some(LeftTool::Structure) => "structure",
                    Some(LeftTool::Commit) => "commit",
                    None => "none",
                };
                if name != sval {
                    self.fail(format!("assert_left: expected {sval}, got {name}"));
                }
            }
            "assert_find_results" => {
                let min = step["min"].as_u64().unwrap_or(1) as usize;
                if self.find_results.hits.len() < min {
                    self.fail(format!("assert_find_results: expected >= {min}, got {}", self.find_results.hits.len()));
                }
            }
            "assert_git_changes" => {
                let want = step["value"].as_u64().unwrap_or(0) as usize;
                if self.git.changes.len() != want {
                    let names: Vec<_> = self.git.changes.iter().map(|c| c.path.clone()).collect();
                    self.fail(format!("assert_git_changes: expected {want}, got {names:?}"));
                }
            }
            "assert_base" => {
                let want_local = sval == "local";
                if self.base_is_local() != want_local {
                    let l = self.base_label();
                    self.fail(format!("assert_base: expected {sval}, got {l}"));
                }
            }
            "assert_local_commits" => {
                let want = step["value"].as_u64().unwrap_or(0) as usize;
                let n = self.git.local.as_ref().map_or(0, |l| l.commits().len());
                if n != want {
                    self.fail(format!("assert_local_commits: expected {want}, got {n}"));
                }
            }
            "assert_ignored" => {
                let want = step["ignored"].as_bool().unwrap_or(true);
                let is_dir = step["dir"].as_bool().unwrap_or(false);
                let got = match self.project.as_mut() {
                    Some(p) => {
                        let abs = p.root.join(&sval);
                        p.ignore.is_ignored(&abs, is_dir)
                    }
                    None => false,
                };
                if got != want {
                    self.fail(format!("assert_ignored: {sval} expected ignored={want}, got {got}"));
                }
            }
            "assert_project_file" => {
                let want = step["present"].as_bool().unwrap_or(true);
                let got = self.project.as_ref().is_some_and(|p| p.files_rel.contains(&sval));
                if got != want {
                    self.fail(format!("assert_project_file: {sval} expected present={want}, got {got}"));
                }
            }
            "assert_branch" => {
                let got = self.git.branch.clone().unwrap_or_default();
                if got != sval {
                    self.fail(format!("assert_branch: expected {sval:?}, got {got:?}"));
                }
            }
            "assert_git_log" => {
                if !self.git.log.iter().any(|c| c.subject.contains(&sval)) {
                    let all: Vec<_> = self.git.log.iter().map(|c| c.subject.clone()).collect();
                    self.fail(format!("assert_git_log: {sval:?} not in {all:?}"));
                }
            }
            "assert_marks" => {
                let min = step["min"].as_u64().unwrap_or(1) as usize;
                let n = self.docs.get_mut(self.active).map_or(0, |d| App::git_marks(d).len());
                if let Some(exact) = step["exact"].as_u64() {
                    if n != exact as usize {
                        self.fail(format!("assert_marks: expected exactly {exact} gutter marks, got {n}"));
                    }
                } else if n < min {
                    self.fail(format!("assert_marks: expected >= {min} gutter marks, got {n}"));
                }
            }
            "assert_folded" => {
                let want = step["value"].as_u64().unwrap_or(0) as usize;
                let n = self.doc().map_or(0, |d| d.folds.len());
                if n != want {
                    self.fail(format!("assert_folded: expected {want}, got {n}"));
                }
            }
            "assert_term_contains" => {
                if !self.term.log.iter().any(|l| l.text.contains(&sval)) {
                    let tail: Vec<String> = self.term.log.iter().rev().take(6).map(|l| l.text.clone()).collect();
                    self.fail(format!("assert_term_contains: {sval:?} not found; tail: {tail:?}"));
                }
            }
            // the text now in the terminal's command line
            "assert_term_input" => {
                if self.term.input != sval {
                    self.fail(format!("assert_term_input: expected {sval:?}, got {:?}", self.term.input));
                }
            }
            // the newest command of the terminal history
            "assert_term_history" => {
                if self.term.history.last() != Some(sval.as_str()) {
                    self.fail(format!("assert_term_history: expected {sval:?}, got {:?}", self.term.history.last()));
                }
            }
            "assert_no_term_contains" => {
                if self.term.log.iter().any(|l| l.text.contains(&sval)) {
                    self.fail(format!("assert_no_term_contains: {sval:?} was printed"));
                }
            }
            // the pointer moves to (x, y) without a button
            "move" => {
                let pos = Pos2::new(step["x"].as_f64().unwrap_or(0.0) as f32, step["y"].as_f64().unwrap_or(0.0) as f32);
                let s = self.script.as_mut().unwrap();
                s.pending.push(Event::PointerMoved(pos));
            }
            // the preview of the active tab: `mode` (code, split, preview), `blocks_min` (blocks laid out), `scrolled`
            // (it is not at the top) and `in_sync` (it shows the place the code shows)
            "assert_preview" => {
                let idx = self.active;
                let mut errors = vec![];
                let mode = if self.doc_markup(idx).is_some() { self.preview_mode(idx).name() } else { "none" };
                if let Some(m) = step["mode"].as_str()
                    && m != mode
                {
                    errors.push(format!("mode: expected {m}, got {mode}"));
                }
                if let Some(d) = self.docs.get(idx) {
                    let pv = &d.preview;
                    if let Some(n) = step["blocks_min"].as_u64()
                        && (pv.blocks_y.len() as u64) < n
                    {
                        errors.push(format!("blocks: expected at least {n}, got {}", pv.blocks_y.len()));
                    }
                    if let Some(b) = step["scrolled"].as_bool()
                        && b != (pv.scroll > 1.0)
                    {
                        errors.push(format!("scrolled: expected {b}, preview offset is {:.0}", pv.scroll));
                    }
                    if let Some(b) = step["in_sync"].as_bool() {
                        let want = pv.line_to_y(d.view.top_line_f);
                        let ok = want.is_some_and(|y| (y - pv.scroll).abs() < 40.0);
                        if ok != b {
                            errors.push(format!("in_sync: expected {b}; the code is at line {:.1} (preview y {:?}), the preview is at {:.0}", d.view.top_line_f, want, pv.scroll));
                        }
                    }
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_preview: {}", errors.join("; ")));
                }
            }
            // rest the pointer on the first occurrence of `value` in the active document (`occurrence`: which one, from 1)
            "hover_on" => {
                let nth = step["occurrence"].as_u64().unwrap_or(1).max(1) as usize;
                let found = self.doc().and_then(|d| d.ed.text().match_indices(sval.as_str()).nth(nth - 1).map(|(i, _)| i));
                match found {
                    Some(off) => {
                        self.fake_hover = Some(off);
                        self.hide_hover();
                    }
                    None => self.fail(format!("hover_on: {sval:?} not in the document")),
                }
            }
            "hover_off" => {
                self.fake_hover = None;
                self.hide_hover();
            }
            // the hover popup: `title`, and `contains` (in the signature or the documentation); `value: "none"`: no popup
            "assert_hover" => {
                let info = self.hover.info.clone();
                if step["value"].as_str() == Some("none") {
                    if let Some(i) = info {
                        self.fail(format!("assert_hover: expected no popup, got {:?}", i.title));
                    }
                } else {
                    match info {
                        None => self.fail("assert_hover: no popup".to_string()),
                        Some(i) => {
                            let mut errors = vec![];
                            if let Some(t) = step["title"].as_str()
                                && i.title != t
                            {
                                errors.push(format!("title: expected {t:?}, got {:?}", i.title));
                            }
                            if let Some(c) = step["contains"].as_str()
                                && !i.signature.contains(c)
                                && !i.doc.contains(c)
                            {
                                errors.push(format!("{c:?} is in neither the signature {:?} nor the documentation {:?}", i.signature, i.doc));
                            }
                            if let Some(k) = step["kind"].as_str()
                                && i.kind != k
                            {
                                errors.push(format!("kind: expected {k:?}, got {:?}", i.kind));
                            }
                            if !errors.is_empty() {
                                self.fail(format!("assert_hover: {}", errors.join("; ")));
                            }
                        }
                    }
                }
            }
            "assert_toast" => {
                if !self.toasts.iter().any(|t| t.text.contains(&sval)) {
                    let all: Vec<_> = self.toasts.iter().map(|t| t.text.clone()).collect();
                    self.fail(format!("assert_toast: {sval:?} not in {all:?}"));
                }
            }
            // the theme: `value` is dark, light or custom; for custom, `dark` picks the palette it starts from, `set` is
            // a map of colour keys to hex codes and `reset` a list of keys (or `true`: all) to give back
            "theme" => {
                use ide_core::settings::{Theme, parse_color};
                self.settings.theme = match sval.as_str() {
                    "dark" => Theme::Dark,
                    "light" => Theme::Light,
                    "custom" => Theme::Custom,
                    other => {
                        self.fail(format!("theme: unknown theme {other}"));
                        return;
                    }
                };
                if let Some(d) = step["dark"].as_bool() {
                    self.settings.custom_theme.dark = d;
                }
                if step["reset"].as_bool() == Some(true) {
                    self.settings.custom_theme.reset_all();
                }
                for k in step["reset"].as_array().into_iter().flatten().filter_map(|k| k.as_str()) {
                    self.settings.custom_theme.reset(k);
                }
                for (k, v) in step["set"].as_object().into_iter().flatten() {
                    match v.as_str().and_then(parse_color) {
                        Some(c) if self.pal.color(k).is_some() => self.settings.custom_theme.set(k, c),
                        _ => self.fail(format!("theme: cannot set {k} to {v}")),
                    }
                }
                self.theme_changed();
                self.pal.apply(ctx);
            }
            // `value`: the theme in use (dark, light, custom); `dark`: whether the palette on screen is dark; `colors`: a map
            // of colour keys to the hex codes the palette has now
            "assert_theme" => {
                use ide_core::settings::{Theme, format_color};
                let got = match self.settings.theme {
                    Theme::Dark => "dark",
                    Theme::Light => "light",
                    Theme::Custom => "custom",
                };
                let mut errors = vec![];
                if !sval.is_empty() && got != sval {
                    errors.push(format!("theme {got}, expected {sval}"));
                }
                if let Some(d) = step["dark"].as_bool()
                    && d != self.pal.dark
                {
                    errors.push(format!("dark palette {}, expected {d}", self.pal.dark));
                }
                for (k, v) in step["colors"].as_object().into_iter().flatten() {
                    let now = self.pal.color(k).map(|c| format_color(c.to_srgba_unmultiplied()));
                    if now.as_deref() != v.as_str() {
                        errors.push(format!("{k} is {now:?}, expected {v}"));
                    }
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_theme: {}", errors.join("; ")));
                }
            }
            "language" => {
                let l = if sval.eq_ignore_ascii_case("ru") { ide_core::settings::Language::Russian } else { ide_core::settings::Language::English };
                self.settings.language = l;
                crate::i18n::set(l);
            }
            "open_local_commit" => self.open_local_view(step["value"].as_u64().unwrap_or(1)),
            "local_view" => {
                let p = std::mem::replace(&mut self.popup, Popup::None);
                if let Popup::LocalView(mut st) = p {
                    if let Some(id) = step["commit"].as_u64() {
                        self.local_view_goto(&mut st, id);
                    }
                    if let Some(f) = step["file"].as_str() {
                        match st.files.iter().position(|c| c.path == f) {
                            Some(i) => st.sel = i,
                            None => self.fail(format!("local_view: file {f} is not part of commit #{}", st.id)),
                        }
                    }
                    match step["mode"].as_str() {
                        Some("code") => st.mode = LocalViewMode::Code,
                        Some("changes") => st.mode = LocalViewMode::Changes,
                        _ => {}
                    }
                    self.popup = Popup::LocalView(st);
                } else {
                    self.popup = p;
                    self.fail("local_view: the commit browser is not open".to_string());
                }
            }
            "assert_local_view" => {
                let p = std::mem::replace(&mut self.popup, Popup::None);
                if let Popup::LocalView(mut st) = p {
                    self.local_view_load(&mut st);
                    if let Some(id) = step["commit"].as_u64()
                        && st.id != id
                    {
                        self.fail(format!("assert_local_view: expected commit #{id}, got #{}", st.id));
                    }
                    if let Some(n) = step["files"].as_u64()
                        && st.files.len() != n as usize
                    {
                        let names: Vec<_> = st.files.iter().map(|c| c.path.clone()).collect();
                        self.fail(format!("assert_local_view: expected {n} files, got {names:?}"));
                    }
                    if !sval.is_empty() {
                        let shown = match st.mode {
                            LocalViewMode::Code => st.code.join("\n"),
                            LocalViewMode::Changes => st.diff.iter().map(|l| format!("{}{}", match l.kind { ide_core::diff::LineKind::Added => "+", ide_core::diff::LineKind::Removed => "-", ide_core::diff::LineKind::Same => " " }, l.text)).collect::<Vec<_>>().join("\n"),
                        };
                        if !shown.contains(&sval.replace("\\n", "\n")) {
                            self.fail(format!("assert_local_view: {sval:?} not shown in {:?} view: {shown:?}", st.mode));
                        }
                    }
                    self.popup = Popup::LocalView(st);
                } else {
                    self.popup = p;
                    self.fail("assert_local_view: the commit browser is not open".to_string());
                }
            }
            "gutter_click" => {
                let line = step["line"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
                match (self.gutter_rect, self.docs.get(self.active)) {
                    (Some(g), Some(d)) => {
                        let pos = crate::editor_view::gutter_mark_pos(g, d.view.scroll.y, self.settings.font_size, line);
                        let s = self.script.as_mut().unwrap();
                        s.pending.push(Event::PointerMoved(pos));
                        s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                        s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
                    }
                    _ => self.fail("gutter_click: no editor on screen".to_string()),
                }
            }
            "assert_hunk_popup" => {
                if step["none"].as_bool().unwrap_or(false) {
                    if self.hunk_popup.is_some() {
                        self.fail("assert_hunk_popup: a popup is open but none was expected".to_string());
                    }
                } else {
                    let hp = self.hunk_popup.clone();
                    match &hp {
                        None => self.fail("assert_hunk_popup: no popup is open".to_string()),
                        Some(h) => {
                            let old = h.hunk.old_lines.join("\n");
                            if !sval.is_empty() && !old.contains(&sval) {
                                self.fail(format!("assert_hunk_popup: {sval:?} not in the old code {old:?}"));
                            }
                            if let Some(n) = step["old_lines"].as_u64()
                                && h.hunk.old_lines.len() != n as usize
                            {
                                self.fail(format!("assert_hunk_popup: expected {n} old lines, got {}", h.hunk.old_lines.len()));
                            }
                        }
                    }
                }
            }
            "hunk_revert" => self.hunk_revert(),
            "diff_edit_beside" => self.diff_edit_beside(),
            "clone_tab" => {
                if let Popup::Clone(c) = &mut self.popup {
                    c.tab = if sval == "github" { CloneTab::GitHub } else { CloneTab::Url };
                } else {
                    self.fail("clone_tab: the clone dialog is not open".to_string());
                }
            }
            "github_sign_in" => self.github_sign_in_token(&sval, step["remember"].as_bool().unwrap_or(false)),
            "github_sign_in_saved" => self.github_sign_in_saved(),
            "github_sign_in_browser" => self.github_sign_in_browser(),
            "github_cancel_browser" => self.github_cancel_browser(),
            "github_show_user" => self.github_show_user(&sval),
            "github_sign_out" => self.github_sign_out(),
            "github_filter" => {
                self.github.filter = sval.clone();
                if let Some(b) = step["forks"].as_bool() {
                    self.github.include_forks = b;
                }
                if let Some(b) = step["archived"].as_bool() {
                    self.github.include_archived = b;
                }
            }
            "github_select" => {
                let repo = self.github.repos.iter().find(|r| r.full_name == sval).cloned();
                match (repo, &mut self.popup) {
                    (Some(r), Popup::Clone(c)) => {
                        c.url = r.clone_url;
                        c.gh_selected = Some(r.full_name);
                    }
                    (None, _) => self.fail(format!("github_select: {sval} is not in the list")),
                    _ => self.fail("github_select: the clone dialog is not open".to_string()),
                }
            }
            "github_click" => {
                let hit = self.github.rects.iter().find(|(n, _)| *n == sval).map(|(_, r)| egui::pos2(r.center().x, r.center().y));
                match hit {
                    Some(pos) => {
                        let presses = if step["double"].as_bool().unwrap_or(false) { 2 } else { 1 };
                        let s = self.script.as_mut().unwrap();
                        s.pending.push(Event::PointerMoved(pos));
                        for _ in 0..presses {
                            s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                            s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
                        }
                    }
                    None => self.fail(format!("github_click: {sval} is not a visible row")),
                }
            }
            "assert_github" => {
                let g = &self.github;
                let mut errors: Vec<String> = vec![];
                if let Some(b) = step["signed_in"].as_bool()
                    && g.signed_in() != b
                {
                    errors.push(format!("signed_in is {}, expected {b}", g.signed_in()));
                }
                if let Some(l) = step["login"].as_str() {
                    let shown = g.shown.as_ref().map(|a| a.login.as_str()).unwrap_or("");
                    if shown != l {
                        errors.push(format!("listing {shown:?}, expected {l:?}"));
                    }
                }
                if let Some(n) = step["repos"].as_u64()
                    && g.repos.len() != n as usize
                {
                    errors.push(format!("{} repositories, expected {n}", g.repos.len()));
                }
                if let Some(n) = step["visible"].as_u64()
                    && g.visible().len() != n as usize
                {
                    errors.push(format!("{} visible, expected {n}", g.visible().len()));
                }
                if let Some(t) = step["error_contains"].as_str() {
                    let e = g.error.as_ref().map(crate::app_github::error_text).unwrap_or_default();
                    if !e.contains(t) {
                        errors.push(format!("error {e:?} does not contain {t:?}"));
                    }
                }
                if let Some(t) = step["login_error_contains"].as_str() {
                    let e = g.login_error.as_ref().and_then(crate::app_github::login_error_text).unwrap_or_default();
                    if !e.contains(t) {
                        errors.push(format!("login error {e:?} does not contain {t:?}"));
                    }
                }
                if step["no_login_error"].as_bool().unwrap_or(false) && g.login_error.is_some() {
                    errors.push(format!("unexpected login error {:?}", g.login_error));
                }
                if let Some(b) = step["waiting"].as_bool()
                    && g.browser_cancel.is_some() != b
                {
                    errors.push(format!("waiting for the browser is {}, expected {b}", g.browser_cancel.is_some()));
                }
                if step["no_error"].as_bool().unwrap_or(false) && g.error.is_some() {
                    errors.push(format!("unexpected error {:?}", g.error));
                }
                if let Some(k) = step["listing"].as_str() {
                    let now = match &g.listing {
                        None => "none".to_string(),
                        Some(crate::app_github::Listing::Mine) => "mine".to_string(),
                        Some(crate::app_github::Listing::User(u)) => format!("user:{u}"),
                    };
                    if now != k {
                        errors.push(format!("listing {now}, expected {k}"));
                    }
                }
                if let Some(s) = step["selected"].as_str() {
                    let now = if let Popup::Clone(c) = &self.popup { c.gh_selected.clone().unwrap_or_default() } else { String::new() };
                    if now != s {
                        errors.push(format!("selected {now:?}, expected {s:?}"));
                    }
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_github: {}", errors.join("; ")));
                }
            }
            "settings_page" => {
                if let Popup::Settings { page, .. } = &mut self.popup {
                    *page = step["value"].as_u64().unwrap_or(0) as usize;
                } else {
                    self.fail("settings_page: the settings dialog is not open".to_string());
                }
            }
            "wsl_action" => {
                let arg = step["arg"].as_str().unwrap_or("").to_string();
                match sval.as_str() {
                    "probe" => self.wsl_probe(),
                    "install_wsl" => self.wsl_install_wsl(),
                    "install_distro" => self.wsl_install_distro(&arg),
                    "install_file" => self.wsl_install_file(Path::new(&arg)),
                    "install_sbcl" => self.wsl_install_sbcl(&arg),
                    "use" => self.wsl_use(&arg),
                    "use_windows" => self.use_windows(),
                    "check_tools" => self.wsl_check_tools(&arg),
                    "install_tools" => self.wsl_install_tools(&arg),
                    "git_setup" => self.wsl_git_setup(&arg),
                    other => self.fail(format!("wsl_action: unknown action {other}")),
                }
            }
            // the developer tools: `value` is choose / skip (a package `name`, or every package of the group `group`),
            // open (the packages of the group `group` are listed), add (a tool of one's own: `name`, `packages` and
            // `programs`; what is wrong with it is kept in the form) or remove (the tool `name`)
            "linux_tool" => {
                let arg = |k: &str| step[k].as_str().unwrap_or("").to_string();
                match sval.as_str() {
                    "choose" | "skip" => {
                        let on = sval == "choose";
                        let mut names = vec![];
                        if step["name"].is_string() {
                            names.push(arg("name"));
                        }
                        if let Some(g) = self.tool_groups().iter().find(|g| g.name == arg("group")) {
                            names.extend(g.package_list().into_iter().map(String::from));
                        } else if step["group"].is_string() {
                            self.fail(format!("linux_tool: there is no group {}", arg("group")));
                        }
                        for n in names {
                            self.settings.linux_tools.choose(&n, on);
                        }
                    }
                    "open" => {
                        self.wsl.tools_open.insert(arg("group"));
                    }
                    "add" => {
                        self.wsl.tool_draft = Some(crate::app_wsl::ToolDraft { index: None, name: arg("name"), packages: arg("packages"), programs: arg("programs"), error: String::new() });
                        self.wsl_tool_save();
                    }
                    "remove" => match self.settings.linux_tools.custom.iter().position(|c| c.name == arg("name")) {
                        Some(i) => {
                            self.settings.linux_tools.custom.remove(i);
                            self.wsl_tools_changed();
                        }
                        None => self.fail(format!("linux_tool: there is no tool {}", arg("name"))),
                    },
                    other => self.fail(format!("linux_tool: unknown action {other}")),
                }
            }
            // `groups`: names of the groups offered; `custom`: the names of the tools of one's own, in order; `install`: the
            // packages that installing puts in (every one of `install_has` is in, none of `install_lacks`); `draft`: the
            // form of a tool is open (`true`) or not, and `draft_error` what it complains about
            "assert_linux_tools" => {
                let list = |k: &str| -> Vec<String> { step[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() };
                let mut errors = vec![];
                let groups: Vec<String> = self.tool_groups().into_iter().map(|g| g.name).collect();
                for g in list("groups") {
                    if !groups.contains(&g) {
                        errors.push(format!("no group {g}; have {groups:?}"));
                    }
                }
                if step["custom"].is_array() {
                    let got: Vec<String> = self.settings.linux_tools.custom.iter().map(|c| c.name.clone()).collect();
                    if got != list("custom") {
                        errors.push(format!("custom tools {got:?}, expected {:?}", list("custom")));
                    }
                }
                let install: Vec<String> = self.chosen_tool_groups(&[]).iter().flat_map(|g| g.package_list().into_iter().map(String::from).collect::<Vec<_>>()).collect();
                for p in list("install_has") {
                    if !install.contains(&p) {
                        errors.push(format!("{p} is not installed by the tools; the packages are {install:?}"));
                    }
                }
                for p in list("install_lacks") {
                    if install.contains(&p) {
                        errors.push(format!("{p} is installed by the tools although it was left out"));
                    }
                }
                if let Some(open) = step["draft"].as_bool()
                    && open != self.wsl.tool_draft.is_some()
                {
                    errors.push(format!("the form is {}", if open { "closed" } else { "open" }));
                }
                if let Some(e) = step["draft_error"].as_str() {
                    let got = self.wsl.tool_draft.as_ref().map(|d| d.error.clone()).unwrap_or_default();
                    if !got.contains(e) {
                        errors.push(format!("the form says {got:?}, expected {e:?}"));
                    }
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_linux_tools: {}", errors.join("; ")));
                }
            }
            "assert_wsl_progress" => {
                // the running step: its stage, how far it is, and that the markers of the scripts are not in the log
                let Some(t) = self.wsl.task.as_ref() else {
                    self.fail("assert_wsl_progress: no setup step is running".to_string());
                    return;
                };
                let p = t.tracker.progress();
                let mut errors = vec![];
                if let Some(s) = step["stage"].as_str()
                    && format!("{:?}", p.stage) != s
                {
                    errors.push(format!("stage {:?}, expected {s}", p.stage));
                }
                let f = p.fraction;
                if let Some(min) = step["min"].as_f64()
                    && !f.is_some_and(|f| f as f64 >= min)
                {
                    errors.push(format!("fraction {f:?}, expected >= {min}"));
                }
                if let Some(max) = step["max"].as_f64()
                    && !f.is_some_and(|f| f as f64 <= max)
                {
                    errors.push(format!("fraction {f:?}, expected <= {max}"));
                }
                if self.wsl.log.iter().any(|l| l.contains("@@light")) {
                    errors.push("the log shows the progress markers".to_string());
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_wsl_progress: {}", errors.join("; ")));
                }
            }
            "assert_wsl" => {
                let w = &self.wsl;
                let presence = match w.probe.as_ref().map(|p| &p.presence) {
                    None => "unknown",
                    Some(lisp_runtime::wsl::Presence::Unsupported) => "unsupported",
                    Some(lisp_runtime::wsl::Presence::NotInstalled) => "not_installed",
                    Some(lisp_runtime::wsl::Presence::Installed { .. }) => "installed",
                };
                let mut errors = vec![];
                if let Some(p) = step["presence"].as_str()
                    && p != presence
                {
                    errors.push(format!("presence {presence}, expected {p}"));
                }
                if let Some(n) = step["distros"].as_u64()
                    && w.distros().len() != n as usize
                {
                    errors.push(format!("distros {:?}, expected {n}", w.distros()));
                }
                if let Some(s) = step["selected"].as_str()
                    && w.selected != s
                {
                    errors.push(format!("selected {:?}, expected {s}", w.selected));
                }
                if let Some(s) = step["sbcl"].as_str() {
                    let found = w.sbcl_path(&w.selected).is_some();
                    if (s == "found") != found {
                        errors.push(format!("sbcl in {:?}: {:?}, expected {s}", w.selected, w.sbcl.get(&w.selected)));
                    }
                }
                if let Some(s) = step["tools"].as_str() {
                    let got = match w.tools.get(&w.selected) {
                        Some(Ok(v)) if v.iter().all(|(_, f)| *f) => "ready",
                        Some(Ok(_)) => "missing",
                        Some(Err(_)) => "error",
                        None => "unknown",
                    };
                    if got != s {
                        errors.push(format!("tools in {:?}: {got} {:?}, expected {s}", w.selected, w.tools.get(&w.selected)));
                    }
                }
                if let Some(e) = step["run_env"].as_str() {
                    let now = if self.settings.run_env == ide_core::settings::RunEnv::Wsl { "wsl" } else { "windows" };
                    if now != e {
                        errors.push(format!("run_env {now}, expected {e}"));
                    }
                }
                if let Some(t) = step["outcome_contains"].as_str() {
                    let o = format!("{:?}", w.outcome);
                    if !o.contains(t) {
                        errors.push(format!("outcome {o}, expected it to contain {t:?}"));
                    }
                }
                if let Some(t) = step["log_contains"].as_str()
                    && !w.log.iter().any(|l| l.contains(t))
                {
                    errors.push(format!("the log of the step does not contain {t:?}"));
                }
                if let Some(t) = step["log_lacks"].as_str()
                    && w.log.iter().any(|l| l.contains(t))
                {
                    errors.push(format!("the log of the step contains {t:?}"));
                }
                // the packages the fake distribution got in its last install
                if let Some(t) = step["tools_found"].as_object() {
                    let found = w.tools.get(&w.selected).and_then(|r| r.as_ref().ok());
                    for (name, want) in t {
                        let got = found.and_then(|f| f.iter().find(|(n, _)| n == name)).map(|(_, ok)| *ok);
                        if got != want.as_bool() {
                            errors.push(format!("{name} found: {got:?}, expected {want}"));
                        }
                    }
                }
                if !errors.is_empty() {
                    self.fail(format!("assert_wsl: {}", errors.join("; ")));
                }
            }
            "assert_split" => {
                let (layout, focus, other) = match self.split.as_ref() {
                    None => ("none", "", None),
                    Some(sp) => (if sp.vertical { "vertical" } else { "horizontal" }, if sp.second_focused { "second" } else { "first" }, self.split_other_index().map(|i| self.docs[i].name())),
                };
                if layout != sval {
                    self.fail(format!("assert_split: expected layout {sval}, got {layout}"));
                }
                if let Some(f) = step["focus"].as_str()
                    && focus != f
                {
                    self.fail(format!("assert_split: expected the {f} pane focused, got {focus}"));
                }
                if let Some(o) = step["other"].as_str()
                    && other.as_deref() != Some(o)
                {
                    self.fail(format!("assert_split: expected {o} in the other pane, got {other:?}"));
                }
            }
            "pane_click" => {
                let k = step["pane"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
                match self.pane_rects.get(k).copied() {
                    Some(r) => {
                        let pos = egui::pos2(r.center().x, r.min.y + r.height() * 0.6);
                        let s = self.script.as_mut().unwrap();
                        s.pending.push(Event::PointerMoved(pos));
                        s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                        s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
                    }
                    None => self.fail("pane_click: the editor area is not split".to_string()),
                }
            }
            "diff_open" => self.open_diff_tab(&sval),
            "commit_click" => {
                let hit = self.commit_rects.iter().find(|(p, _, _)| *p == sval).map(|(_, r, cb)| if step["checkbox"].as_bool().unwrap_or(false) { cb.center() } else { egui::pos2(r.min.x + r.width() * 0.6, r.center().y) });
                match hit {
                    Some(pos) => {
                        let s = self.script.as_mut().unwrap();
                        s.pending.push(Event::PointerMoved(pos));
                        s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
                        s.pending.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
                    }
                    None => self.fail(format!("commit_click: {sval} is not a visible row of the Commit window")),
                }
            }
            "assert_commit_rows" => {
                let n = step["value"].as_u64().unwrap_or(0) as usize;
                if self.commit_rects.len() != n {
                    let names: Vec<_> = self.commit_rects.iter().map(|r| r.0.clone()).collect();
                    self.fail(format!("assert_commit_rows: expected {n} visible files, got {names:?}"));
                }
            }
            "assert_checked" => {
                let n = step["value"].as_u64().unwrap_or(0) as usize;
                if self.git.checked.len() != n {
                    self.fail(format!("assert_checked: expected {n} checked files, got {:?}", self.git.checked));
                }
                if let Some(p) = step["path"].as_str()
                    && self.git.checked.contains(p) != step["on"].as_bool().unwrap_or(true)
                {
                    self.fail(format!("assert_checked: {p} checked state is not {}", step["on"].as_bool().unwrap_or(true)));
                }
            }
            "assert_diff" => {
                let t = self.active_diff().map(|t| (t.rel.clone(), t.hunk_count(), t.cur, t.unified, t.old.clone(), t.new.clone(), t.sbs.added, t.sbs.removed));
                match t {
                    None => self.fail("assert_diff: the active tab is not a diff tab".to_string()),
                    Some((rel, hunks, cur, unified, old, new, added, removed)) => {
                        if let Some(f) = step["file"].as_str()
                            && rel != f
                        {
                            self.fail(format!("assert_diff: expected file {f}, got {rel}"));
                        }
                        if let Some(n) = step["hunks"].as_u64()
                            && hunks != n as usize
                        {
                            self.fail(format!("assert_diff: expected {n} differences, got {hunks}"));
                        }
                        if let Some(n) = step["current"].as_u64()
                            && cur != Some(n as usize)
                        {
                            self.fail(format!("assert_diff: expected current difference {n}, got {cur:?}"));
                        }
                        if let Some(m) = step["mode"].as_str()
                            && unified != (m == "unified")
                        {
                            self.fail(format!("assert_diff: expected mode {m}, unified={unified}"));
                        }
                        if let Some(n) = step["added"].as_u64()
                            && added != n as usize
                        {
                            self.fail(format!("assert_diff: expected {n} added lines, got {added}"));
                        }
                        if let Some(n) = step["removed"].as_u64()
                            && removed != n as usize
                        {
                            self.fail(format!("assert_diff: expected {n} removed lines, got {removed}"));
                        }
                        // `\n` written out in the script is a line break, as in `set_text`
                        if let Some(s) = step["old_contains"].as_str()
                            && !old.contains(&s.replace("\\n", "\n"))
                        {
                            self.fail(format!("assert_diff: old text does not contain {s:?}: {old:?}"));
                        }
                        if let Some(s) = step["new_contains"].as_str()
                            && !new.contains(&s.replace("\\n", "\n"))
                        {
                            self.fail(format!("assert_diff: new text does not contain {s:?}: {new:?}"));
                        }
                    }
                }
            }
            "diff_mode" => {
                if let Some(t) = self.docs.get_mut(self.active).and_then(|d| d.diff.as_mut()) {
                    t.unified = sval == "unified";
                }
            }
            "local_restore" => {
                let id = step["value"].as_u64().unwrap_or(1);
                match step["file"].as_str() {
                    Some(f) => self.local_restore_file(id, f),
                    None => self.local_restore(id),
                }
            }
            "quit" => {
                let s = self.script.as_mut().unwrap();
                s.steps.clear();
            }
            other => self.fail(format!("unknown op '{other}'")),
        }
        if let Some(s) = self.script.as_mut() {
            // give the UI a couple of frames to react to the step
            if s.wait_frames == 0 {
                s.wait_frames = 2;
            }
        }
    }
}

#[cfg(test)]
mod input_isolation_tests {
    use super::*;

    #[test]
    fn scripted_runs_ignore_real_desktop_input() {
        let mut app = App::new(ide_core::settings::Settings::default());
        app.script = Some(Script { steps: VecDeque::new(), pending: vec![Event::Text("script".into())], wait_frames: 0, wait_cond: None, shot: None, failures: vec![], total: 0, done: false, started: 0.0 });
        let mut raw = egui::RawInput { events: vec![Event::Text("external".into())], focused: false, ..Default::default() };
        app.script_inject(&mut raw);
        assert!(raw.focused);
        assert_eq!(raw.events, vec![Event::ModifiersChanged(egui::Modifiers::NONE), Event::Text("script".into())]);
        raw.events.push(Event::Text("external".into()));
        app.script_inject(&mut raw);
        assert_eq!(raw.events, vec![Event::ModifiersChanged(egui::Modifiers::NONE)]);
    }
}
