//! The debugger: an adapter of a module ([`DebugDef`], vscode-js-debug for Node.js) speaking the Debug Adapter
//! Protocol ([`ide_core::dap`]), breakpoints set from the gutter, and the Debug window - where the program stopped,
//! its call stack, the variables of a frame, its output and expressions evaluated in it.
//!
//! An adapter may start more than one session: js-debug answers the launch with a `startDebugging` request, and the
//! program itself is debugged in the session that starts then, over a connection of its own.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use ide_core::dap::{self, Connection, Incoming};
use ide_core::modules::{DebugDef, DownloadDef};
use lisp_runtime::discover::Launch;
use lisp_runtime::runner::{self, RunEvent, RunHandle};
use lisp_runtime::wsl;
use serde_json::{Value, json};

use crate::app::App;
use crate::editor_view::Sev;
use crate::i18n::{self, tr};
use crate::state::{LogKind, LogLine};

/// How the debugger is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugStatus {
    #[default]
    Idle,
    /// The adapter is being downloaded.
    Downloading,
    /// The adapter is starting, the program not yet running.
    Starting,
    Running,
    Paused,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    /// The file as this computer sees it (none for code without a file, such as Node's own).
    pub path: Option<PathBuf>,
    /// 1-based.
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub kind: String,
    /// Non-zero: it has children, asked for with this reference.
    pub reference: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub name: String,
    pub reference: i64,
}

/// Where the program stopped.
#[derive(Clone, Debug, Default)]
pub struct Stopped {
    pub session: usize,
    pub thread: i64,
    pub reason: String,
    pub frames: Vec<Frame>,
    pub frame: usize,
    pub scopes: Vec<Scope>,
}

/// What a request was sent for, to know what to do with its response.
#[derive(Clone, Debug)]
enum Pending {
    StackTrace,
    Scopes,
    Variables(i64),
    Evaluate,
    Other,
}

struct Session {
    /// A stable number (the index in the list changes when a session ends).
    id: u64,
    conn: Connection,
    pending: HashMap<i64, Pending>,
}

/// A connection made in the background and the requests to send on it first.
type Connected = (Connection, Vec<(String, Value)>);

#[derive(Default)]
pub struct DebugState {
    pub status: DebugStatus,
    pub def: Option<DebugDef>,
    /// The file being debugged.
    pub file: Option<PathBuf>,
    adapter: Option<RunHandle>,
    port: u16,
    sessions: Vec<Session>,
    next_id: u64,
    /// Connections being made in the background (an adapter may take a moment to listen) with the requests to send.
    connecting: Vec<Receiver<Result<Connected, String>>>,
    download: Option<Receiver<Result<PathBuf, String>>>,
    pub stopped: Option<Stopped>,
    pub variables: HashMap<i64, Vec<Variable>>,
    pub expanded: HashSet<i64>,
    /// What the program printed, what the adapter said, and the expressions evaluated with their values.
    pub console: Vec<LogLine>,
    pub input: String,
    /// The 0-based lines of breakpoints of each file.
    pub breakpoints: HashMap<PathBuf, BTreeSet<usize>>,
}

/// Where downloaded adapters are kept.
fn adapters_dir() -> Option<PathBuf> {
    ide_core::settings::Settings::config_dir().map(|d| d.join("adapters"))
}

fn adapter_dir(dl: &DownloadDef) -> Option<PathBuf> {
    adapters_dir().map(|d| d.join(format!("{}-{}", dl.name, dl.version)))
}

/// Download `dl`, check it and unpack it into `dir` (a marker file says it is complete).
fn download_adapter(dl: &DownloadDef, dir: &Path) -> Result<PathBuf, String> {
    let parent = dir.parent().ok_or("no folder for adapters")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let archive = parent.join(format!("{}-{}.tar.gz.part", dl.name, dl.version));
    let mut curl = std::process::Command::new("curl");
    curl.args(["-sSL", "--fail", "--max-time", "600", "-o"]).arg(&archive).arg(&dl.url);
    lisp_runtime::no_window(&mut curl);
    let out = curl.output().map_err(|e| format!("cannot run curl: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let bytes = std::fs::read(&archive).map_err(|e| e.to_string())?;
    let sum = ide_core::modules::sha256::sha256_hex(&bytes);
    if !sum.eq_ignore_ascii_case(&dl.sha256) {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("the download is not what it should be (SHA-256 {sum})"));
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut tar = std::process::Command::new("tar");
    tar.arg("-xzf").arg(&archive).arg("-C").arg(dir);
    lisp_runtime::no_window(&mut tar);
    let out = tar.output().map_err(|e| format!("cannot run tar: {e}"))?;
    let _ = std::fs::remove_file(&archive);
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    std::fs::write(dir.join(".complete"), &dl.sha256).map_err(|e| e.to_string())?;
    Ok(dir.to_path_buf())
}

/// The arguments of the `initialize` request: what Light can do.
fn initialize_args(adapter_id: &str) -> Value {
    json!({"clientID": "light", "clientName": "Light", "adapterID": adapter_id, "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsStartDebuggingRequest": true, "supportsVariableType": true, "locale": "en"})
}

fn frames_of(body: &Value, t: Option<&wsl::Target>) -> Vec<Frame> {
    body["stackFrames"].as_array().into_iter().flatten().map(|f| {
        let path = f["source"]["path"].as_str().filter(|p| !p.starts_with('<') && f["source"]["sourceReference"].as_i64().unwrap_or(0) == 0).map(|p| match t {
            Some(t) if p.starts_with('/') => wsl::to_windows(p, t),
            _ => PathBuf::from(p),
        }).filter(|p| p.is_file());
        Frame { id: f["id"].as_i64().unwrap_or(0), name: f["name"].as_str().unwrap_or("?").to_string(), path, line: f["line"].as_u64().unwrap_or(1) as usize }
    }).collect()
}

impl App {
    /// The debugger of the language of `path`, from the modules that are on.
    pub fn debug_def_for(&self, path: &Path) -> Option<DebugDef> {
        let language = self.language_for(Some(path), None)?;
        self.mods.installed.iter().filter(|m| m.enabled).flat_map(|m| m.manifest.debug.iter()).find(|d| d.languages.iter().any(|l| l == language.id())).cloned()
    }

    /// Can the files of document `idx` be debugged (so its gutter takes breakpoints)?
    pub fn debuggable(&self, idx: usize) -> bool {
        self.docs.get(idx).filter(|d| d.diff.is_none() && d.page.is_none()).and_then(|d| d.path.as_ref()).is_some_and(|p| self.debug_def_for(p).is_some())
    }

    // ------------------------------------------------------------------ breakpoints

    /// Read the breakpoints the project keeps.
    pub fn load_breakpoints(&mut self) {
        self.debug.breakpoints.clear();
        let Some(root) = self.project_root().map(Path::to_path_buf) else { return };
        for (rel, lines) in &self.project_cfg.breakpoints {
            self.debug.breakpoints.insert(root.join(rel), lines.iter().filter(|l| **l > 0).map(|l| l - 1).collect());
        }
    }

    fn save_breakpoints(&mut self) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else { return };
        self.project_cfg.breakpoints = self.debug.breakpoints.iter().filter(|(_, l)| !l.is_empty()).filter_map(|(p, lines)| {
            let rel = p.strip_prefix(&root).ok()?.to_string_lossy().replace('\\', "/");
            Some((rel, lines.iter().map(|l| l + 1).collect()))
        }).collect();
        if let Err(e) = crate::app_env::save_config(&root, &self.project_cfg) {
            self.toast(i18n::fmt("Cannot save the project settings: {}", &[&e]), Sev::Error);
        }
    }

    /// The 0-based lines with breakpoints in document `idx`.
    pub fn breakpoint_lines(&self, idx: usize) -> Vec<usize> {
        self.docs.get(idx).and_then(|d| d.path.as_ref()).and_then(|p| self.debug.breakpoints.get(p)).map(|s| s.iter().copied().collect()).unwrap_or_default()
    }

    /// Set or clear the breakpoint on `line` (0-based) of document `idx`; a running session learns it at once.
    pub fn toggle_breakpoint(&mut self, idx: usize, line: usize) {
        let Some(path) = self.docs.get(idx).and_then(|d| d.path.clone()) else { return };
        if self.debug_def_for(&path).is_none() {
            self.toast(tr("No debugger of the modules that are on debugs this file"), Sev::Weak);
            return;
        }
        let set = self.debug.breakpoints.entry(path.clone()).or_default();
        if !set.remove(&line) {
            set.insert(line);
        }
        self.save_breakpoints();
        for i in 0..self.debug.sessions.len() {
            self.send_breakpoints(i, &path);
        }
    }

    fn send_breakpoints(&mut self, session: usize, path: &Path) {
        let t = self.wsl_target();
        let lines: Vec<usize> = self.debug.breakpoints.get(path).map(|s| s.iter().copied().collect()).unwrap_or_default();
        let source = crate::app_run::shell_path(path, t.as_ref());
        // the adapter on this computer wants the system's own separators
        let source = if t.is_none() { source.replace('/', std::path::MAIN_SEPARATOR_STR) } else { source };
        let args = json!({"source": {"path": source, "name": path.file_name().map(|n| n.to_string_lossy().into_owned())}, "breakpoints": lines.iter().map(|l| json!({"line": l + 1})).collect::<Vec<_>>()});
        let s = &mut self.debug.sessions[session];
        let seq = s.conn.request("setBreakpoints", args);
        s.pending.insert(seq, Pending::Other);
    }

    // ------------------------------------------------------------------ the session

    /// Debug the file `path` with the debugger of its language: the adapter is downloaded first (when the user agrees)
    /// and the program built first (when the debugger says how).
    pub fn start_debug(&mut self, path: &Path) {
        if self.debug.status != DebugStatus::Idle {
            self.stop_debug();
        }
        let Some(def) = self.debug_def_for(path) else {
            self.toast(tr("No debugger of the modules that are on debugs this file"), Sev::Weak);
            return;
        };
        self.save_all();
        if let Some(dl) = &def.download {
            let Some(dir) = adapter_dir(dl) else { return };
            if !dir.join(".complete").is_file() {
                let size = crate::file_info::size(dl.size);
                self.popup = crate::state::Popup::Confirm {
                    kind: crate::state::ConfirmKind::DownloadDebugger(path.to_path_buf()),
                    message: i18n::fmt("To debug this file Light downloads {0} {1} ({2}, {3} licence) from {4}. Download it now?", &[&dl.name, &dl.version, &size, &dl.license, &dl.homepage]),
                };
                return;
            }
        }
        let t = self.wsl_target();
        let prepare = if t.is_none() && !def.windows_prepare.is_empty() { &def.windows_prepare } else { &def.prepare };
        if prepare.trim().is_empty() {
            self.launch_debugger(path, def);
            return;
        }
        // build with what the debugger needs, in a Run tab; the debugging starts when that succeeds
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let run = ide_core::modules::RunDef { command: prepare.clone(), ..Default::default() };
        let line = run.command_line(&crate::app_run::shell_path(path, t.as_ref()), &crate::app_run::shell_path(&dir, t.as_ref()), t.is_none());
        let launch = crate::app_run::shell_launch(&line, &dir, t.as_ref(), &[]);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = i18n::fmt("Build {} for debugging", &[&name]);
        if self.start_run(format!("debug-build:{}", path.display()), title.clone(), launch, &title, dir, t) {
            self.run_request = Some(crate::app_run::RunRequest::DebugBuild(path.to_path_buf()));
        }
    }

    /// The build for debugging ended: the debugging starts if it succeeded.
    pub fn debug_build_finished(&mut self, code: Option<i32>) {
        let Some(crate::app_run::RunRequest::DebugBuild(path)) = self.run_request.clone() else { return };
        if code != Some(0) {
            self.toast(tr("The build for debugging failed - see the Run window"), Sev::Error);
            return;
        }
        if let Some(def) = self.debug_def_for(&path) {
            self.launch_debugger(&path, def);
        }
    }

    /// Start the adapter of `def` and launch `path` under it.
    fn launch_debugger(&mut self, path: &Path, def: DebugDef) {
        let adapter = def.download.as_ref().and_then(adapter_dir).unwrap_or_default();
        let stdio = def.transport == "stdio";
        let port = if stdio {
            0
        } else {
            match dap::free_port() {
                Ok(p) => p,
                Err(e) => {
                    self.toast(i18n::fmt("Cannot start the debugger: {}", &[&e]), Sev::Error);
                    return;
                }
            }
        };
        let t = self.wsl_target();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let adapter_s = if adapter.as_os_str().is_empty() { String::new() } else { crate::app_run::shell_path(&adapter, t.as_ref()) };
        let port_s = port.to_string();
        let args: Vec<String> = def.command.iter().map(|a| a.replace("{adapter}", &adapter_s).replace("{port}", &port_s)).collect();
        let root = self.project_root().map(Path::to_path_buf).unwrap_or_else(|| dir.clone());
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let exe = if t.is_none() && cfg!(windows) { ".exe" } else { "" };
        let vars = [("file", crate::app_run::shell_path(path, t.as_ref())), ("dir", crate::app_run::shell_path(&dir, t.as_ref())), ("root", crate::app_run::shell_path(&root, t.as_ref())), ("stem", stem), ("exe", exe.to_string())];
        // an adapter on this computer gets paths with the system's separators
        let vars: Vec<(&str, String)> = vars.iter().map(|(k, v)| (*k, if t.is_none() && matches!(*k, "file" | "dir" | "root") { v.replace('/', std::path::MAIN_SEPARATOR_STR) } else { v.clone() })).collect();
        let launch_args = dap::fill(&def.launch, &vars.iter().map(|(k, v)| (*k, v.as_str())).collect::<Vec<_>>());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.debug = DebugState { breakpoints: std::mem::take(&mut self.debug.breakpoints), ..Default::default() };
        self.debug.console.push(LogLine { kind: LogKind::Info, text: i18n::fmt("Debugging {0} with {1}", &[&name, &def.name]) });
        if stdio {
            // the adapter speaks on its own standard streams: the connection is the process
            let cmd = match &t {
                Some(t) => wsl::exec(t, Some(&dir), &args[0], &args[1..]),
                None => {
                    let mut c = std::process::Command::new(&args[0]);
                    c.args(&args[1..]).current_dir(&dir);
                    c
                }
            };
            let mut cmd = cmd;
            lisp_runtime::no_window(&mut cmd);
            match Connection::spawn(cmd) {
                Ok(mut conn) => {
                    let mut pending = HashMap::new();
                    for (command, a) in [("initialize".to_string(), initialize_args(&def.adapter_id)), ("launch".to_string(), launch_args)] {
                        pending.insert(conn.request(&command, a), Pending::Other);
                    }
                    self.debug.next_id += 1;
                    self.debug.sessions.push(Session { id: self.debug.next_id, conn, pending });
                }
                Err(e) => {
                    self.debug.console.push(LogLine { kind: LogKind::Error, text: e.to_string() });
                    self.toast(i18n::fmt("Cannot start the debugger: {}", &[&e]), Sev::Error);
                    return;
                }
            }
        } else {
            let launch = match &t {
                Some(t) => {
                    let marker = wsl::new_marker();
                    let line = args.iter().map(|a| wsl::sh_quote(a)).collect::<Vec<_>>().join(" ");
                    Launch { cmd: wsl::shell(t, &dir, &format!("{line} 2>&1"), &[], &marker), cleanup: Some(wsl::Cleanup { target: t.clone(), marker }) }
                }
                None => {
                    let mut cmd = std::process::Command::new(&args[0]);
                    cmd.args(&args[1..]).current_dir(&dir);
                    Launch { cmd, cleanup: None }
                }
            };
            match runner::spawn_launch(launch) {
                Ok(h) => self.debug.adapter = Some(h),
                Err(e) => {
                    self.toast(i18n::fmt("Cannot start the debugger: {}", &[&e]), Sev::Error);
                    return;
                }
            }
            self.debug.port = port;
            self.connect_session(vec![("launch".into(), launch_args)], &def.adapter_id);
        }
        self.debug.status = DebugStatus::Starting;
        self.debug.file = Some(path.to_path_buf());
        self.debug.def = Some(def);
        self.bottom_tool = Some(crate::state::BottomTool::Debug);
    }

    /// Open a connection to the adapter in the background; `requests` go out after `initialize`.
    fn connect_session(&mut self, requests: Vec<(String, Value)>, adapter_id: &str) {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], self.debug.port));
        let (tx, rx) = mpsc::channel();
        let mut all = vec![("initialize".to_string(), initialize_args(adapter_id))];
        all.extend(requests);
        std::thread::spawn(move || {
            let _ = tx.send(Connection::connect(addr, Duration::from_secs(20)).map(|c| (c, all)).map_err(|e| e.to_string()));
        });
        self.debug.connecting.push(rx);
    }

    /// Download the adapter the debugger needs (the user agreed), then debug `path`.
    pub fn download_debugger(&mut self, path: &Path) {
        let Some(dl) = self.debug_def_for(path).and_then(|d| d.download) else { return };
        let Some(dir) = adapter_dir(&dl) else { return };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(download_adapter(&dl, &dir));
        });
        self.debug.download = Some(rx);
        self.debug.status = DebugStatus::Downloading;
        self.debug.file = Some(path.to_path_buf());
        self.debug.console = vec![LogLine { kind: LogKind::Info, text: tr("Downloading the debugger...").into() }];
        self.bottom_tool = Some(crate::state::BottomTool::Debug);
    }

    /// End the session: the program and the adapter stop.
    pub fn stop_debug(&mut self) {
        for s in &mut self.debug.sessions {
            s.conn.request("disconnect", json!({"terminateDebuggee": true}));
        }
        // let the adapter end the program before it is stopped itself
        std::thread::sleep(Duration::from_millis(150));
        self.debug.sessions.clear();
        self.debug.connecting.clear();
        if let Some(h) = self.debug.adapter.take() {
            h.kill();
        }
        if self.debug.status != DebugStatus::Idle {
            self.debug.console.push(LogLine { kind: LogKind::Info, text: tr("Debugging ended").into() });
        }
        self.debug.status = DebugStatus::Idle;
        self.debug.stopped = None;
    }

    fn stopped_session(&self) -> Option<(usize, i64)> {
        let s = self.debug.stopped.as_ref()?;
        Some((self.debug.sessions.iter().position(|x| x.id as usize == s.session)?, s.thread))
    }

    /// Continue, step or pause the program (`continue`, `next`, `stepIn`, `stepOut`, `pause`).
    pub fn debug_command(&mut self, command: &str) {
        if command == "pause" {
            if self.debug.status == DebugStatus::Running
                && let Some(s) = self.debug.sessions.last_mut()
            {
                let seq = s.conn.request("pause", json!({"threadId": 0}));
                s.pending.insert(seq, Pending::Other);
            }
            return;
        }
        let Some((i, thread)) = self.stopped_session() else { return };
        let s = &mut self.debug.sessions[i];
        let seq = s.conn.request(command, json!({"threadId": thread}));
        s.pending.insert(seq, Pending::Other);
        self.debug.stopped = None;
        self.debug.variables.clear();
        self.debug.status = DebugStatus::Running;
    }

    /// Evaluate `expr` in the frame shown (or globally while the program runs).
    pub fn debug_evaluate(&mut self, expr: &str) {
        let expr = expr.trim();
        if expr.is_empty() {
            return;
        }
        let frame = self.debug.stopped.as_ref().and_then(|s| s.frames.get(s.frame)).map(|f| f.id);
        let i = self.stopped_session().map(|(i, _)| i).or_else(|| self.debug.sessions.len().checked_sub(1));
        let Some(i) = i else { return };
        self.debug.console.push(LogLine { kind: LogKind::Input, text: format!("> {expr}") });
        let mut args = json!({"expression": expr, "context": "repl"});
        if let Some(f) = frame {
            args["frameId"] = json!(f);
        }
        let s = &mut self.debug.sessions[i];
        let seq = s.conn.request("evaluate", args);
        s.pending.insert(seq, Pending::Evaluate);
    }

    /// Show frame `n` of the stopped program: its file at its line, and its variables.
    pub fn select_frame(&mut self, n: usize) {
        let Some((i, _)) = self.stopped_session() else { return };
        let Some(stopped) = self.debug.stopped.as_mut() else { return };
        let Some(frame) = stopped.frames.get(n).cloned() else { return };
        stopped.frame = n;
        stopped.scopes.clear();
        self.debug.variables.clear();
        let s = &mut self.debug.sessions[i];
        let seq = s.conn.request("scopes", json!({"frameId": frame.id}));
        s.pending.insert(seq, Pending::Scopes);
        if let Some(path) = &frame.path
            && self.open_file(path)
            && let Some(d) = self.doc_mut()
        {
            let o = d.ed.line_start(frame.line.saturating_sub(1).min(d.ed.line_count().saturating_sub(1)));
            d.ed.set_caret(o, false);
            d.view.scroll_to_caret = true;
        }
    }

    /// Ask for the children of a variable (or the variables of a scope) once.
    pub fn expand_variables(&mut self, reference: i64) {
        if !self.debug.expanded.insert(reference) {
            self.debug.expanded.remove(&reference);
            return;
        }
        if self.debug.variables.contains_key(&reference) {
            return;
        }
        let Some((i, _)) = self.stopped_session() else { return };
        let s = &mut self.debug.sessions[i];
        let seq = s.conn.request("variables", json!({"variablesReference": reference}));
        s.pending.insert(seq, Pending::Variables(reference));
    }

    /// The line the program stopped on in document `idx` (0-based), if it is there.
    pub fn debug_line(&self, idx: usize) -> Option<usize> {
        let s = self.debug.stopped.as_ref()?;
        let f = s.frames.get(s.frame)?;
        let doc_path = self.docs.get(idx)?.path.as_ref()?;
        let path = f.path.as_ref()?;
        dap::same_path(&path.to_string_lossy(), &doc_path.to_string_lossy()).then(|| f.line.saturating_sub(1))
    }

    /// Every frame: what came from the adapter and from the debug sessions.
    pub fn poll_debug(&mut self) {
        if let Some(rx) = &self.debug.download
            && let Ok(result) = rx.try_recv()
        {
            self.debug.download = None;
            self.debug.status = DebugStatus::Idle;
            match result {
                Ok(_) => {
                    if let Some(file) = self.debug.file.clone() {
                        self.start_debug(&file);
                    }
                }
                Err(e) => {
                    self.debug.console.push(LogLine { kind: LogKind::Error, text: e.clone() });
                    self.toast(i18n::fmt("Cannot download the debugger: {}", &[&e]), Sev::Error);
                }
            }
        }
        if self.debug.status == DebugStatus::Idle {
            return;
        }
        // the adapter's own output, and its end
        let mut adapter_ended = None;
        if let Some(h) = &self.debug.adapter {
            while let Some(ev) = h.try_recv() {
                match ev {
                    RunEvent::Output { text, .. } => {
                        for line in text.lines().filter(|l| !l.trim().is_empty() && !l.contains("Debug server listening")) {
                            self.debug.console.push(LogLine { kind: LogKind::Trace, text: line.to_string() });
                        }
                    }
                    RunEvent::Exited(code) => adapter_ended = Some(code),
                }
            }
        }
        // connections that are ready become sessions
        let mut i = 0;
        while i < self.debug.connecting.len() {
            match self.debug.connecting[i].try_recv() {
                Ok(Ok((mut conn, requests))) => {
                    self.debug.connecting.remove(i);
                    let mut pending = HashMap::new();
                    for (command, args) in requests {
                        let seq = conn.request(&command, args);
                        pending.insert(seq, Pending::Other);
                    }
                    self.debug.next_id += 1;
                    self.debug.sessions.push(Session { id: self.debug.next_id, conn, pending });
                }
                Ok(Err(e)) => {
                    self.debug.connecting.remove(i);
                    self.debug.console.push(LogLine { kind: LogKind::Error, text: i18n::fmt("Cannot connect to the debugger: {}", &[&e]) });
                    if self.debug.sessions.is_empty() {
                        self.stop_debug();
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => i += 1,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.debug.connecting.remove(i);
                }
            }
        }
        let mut closed = Vec::new();
        for i in 0..self.debug.sessions.len() {
            for incoming in self.debug.sessions[i].conn.take() {
                match incoming {
                    Incoming::Message(m) => self.debug_message(i, m),
                    Incoming::Closed(_) => closed.push(self.debug.sessions[i].id),
                }
            }
        }
        if !closed.is_empty() {
            self.debug.sessions.retain(|s| !closed.contains(&s.id));
            if self.debug.stopped.as_ref().is_some_and(|s| closed.contains(&(s.session as u64))) {
                self.debug.stopped = None;
            }
        }
        let starting = self.debug.status == DebugStatus::Starting && !self.debug.connecting.is_empty();
        if adapter_ended.is_some() || (self.debug.sessions.is_empty() && !starting) {
            if let Some(code) = adapter_ended.flatten().filter(|c| *c != 0) {
                self.debug.console.push(LogLine { kind: LogKind::Error, text: i18n::fmt("The debugger ended with exit code {}", &[&code]) });
            }
            self.stop_debug();
        }
    }

    fn debug_message(&mut self, i: usize, m: Value) {
        let t = self.wsl_target();
        match m["type"].as_str() {
            Some("event") => match m["event"].as_str() {
                Some("initialized") => {
                    let files: Vec<PathBuf> = self.debug.breakpoints.iter().filter(|(_, l)| !l.is_empty()).map(|(p, _)| p.clone()).collect();
                    for f in files {
                        self.send_breakpoints(i, &f);
                    }
                    let s = &mut self.debug.sessions[i];
                    let seq = s.conn.request("configurationDone", json!({}));
                    s.pending.insert(seq, Pending::Other);
                    if self.debug.status == DebugStatus::Starting {
                        self.debug.status = DebugStatus::Running;
                    }
                }
                Some("stopped") => {
                    let thread = m["body"]["threadId"].as_i64().unwrap_or(0);
                    let reason = m["body"]["description"].as_str().or(m["body"]["reason"].as_str()).unwrap_or("paused").to_string();
                    let s = &mut self.debug.sessions[i];
                    self.debug.stopped = Some(Stopped { session: s.id as usize, thread, reason, ..Default::default() });
                    self.debug.variables.clear();
                    self.debug.status = DebugStatus::Paused;
                    let seq = s.conn.request("stackTrace", json!({"threadId": thread, "startFrame": 0, "levels": 100}));
                    s.pending.insert(seq, Pending::StackTrace);
                }
                Some("continued") => {
                    self.debug.stopped = None;
                    self.debug.status = DebugStatus::Running;
                }
                Some("output") => {
                    let category = m["body"]["category"].as_str().unwrap_or("console");
                    let wsl_notice = category == "adapter" && m["body"]["output"].as_str().is_some_and(|t| t.trim_start().starts_with("wsl:"));
                    if category != "telemetry" && !wsl_notice {
                        let kind = if category == "stderr" { LogKind::Error } else if category == "stdout" { LogKind::Output } else { LogKind::Trace };
                        let text = m["body"]["output"].as_str().unwrap_or_default();
                        for line in text.trim_end_matches('\n').split('\n') {
                            self.debug.console.push(LogLine { kind, text: line.trim_end_matches('\r').to_string() });
                        }
                    }
                }
                Some("terminated") => {
                    let id = self.debug.sessions[i].id;
                    self.debug.sessions[i].conn.request("disconnect", json!({}));
                    if self.debug.stopped.as_ref().is_some_and(|s| s.session as u64 == id) {
                        self.debug.stopped = None;
                    }
                }
                _ => {}
            },
            Some("request") => match m["command"].as_str() {
                Some("startDebugging") => {
                    self.debug.sessions[i].conn.respond(&m, true, json!({}));
                    let request = m["arguments"]["request"].as_str().unwrap_or("launch").to_string();
                    let config = m["arguments"]["configuration"].clone();
                    let id = self.debug.def.as_ref().map(|d| d.adapter_id.clone()).unwrap_or_default();
                    self.connect_session(vec![(request, config)], &id);
                }
                _ => {
                    // a terminal of the IDE's own is not offered: the program's output comes as events
                    self.debug.sessions[i].conn.respond(&m, false, json!({"error": {"id": 1, "format": "not supported by Light"}}));
                }
            },
            Some("response") => {
                let seq = m["request_seq"].as_i64().unwrap_or(-1);
                let pending = self.debug.sessions[i].pending.remove(&seq).unwrap_or(Pending::Other);
                if m["success"] == false {
                    let msg = m["message"].as_str().or(m["body"]["error"]["format"].as_str()).unwrap_or("failed").to_string();
                    match pending {
                        Pending::Evaluate => self.debug.console.push(LogLine { kind: LogKind::Error, text: msg }),
                        _ if m["command"] == "launch" => {
                            self.debug.console.push(LogLine { kind: LogKind::Error, text: msg.clone() });
                            self.toast(i18n::fmt("Cannot start debugging: {}", &[&msg]), Sev::Error);
                        }
                        _ => {}
                    }
                    return;
                }
                match pending {
                    Pending::StackTrace => {
                        let frames = frames_of(&m["body"], t.as_ref());
                        // the first frame of the program's own code is the one shown
                        let first = frames.iter().position(|f| f.path.is_some()).unwrap_or(0);
                        if let Some(s) = self.debug.stopped.as_mut() {
                            s.frames = frames;
                        }
                        self.select_frame(first);
                    }
                    Pending::Scopes => {
                        let raw = m["body"]["scopes"].as_array().cloned().unwrap_or_default();
                        let scopes: Vec<Scope> = raw.iter().map(|s| Scope { name: s["name"].as_str().unwrap_or("?").to_string(), reference: s["variablesReference"].as_i64().unwrap_or(0) }).collect();
                        // the scopes of the code itself are open from the start; the global one (huge) and the
                        // expensive ones wait for a click
                        for s in &raw {
                            let global = matches!(s["presentationHint"].as_str(), Some("globals" | "registers")) || s["name"].as_str().is_some_and(|n| n.starts_with("Global") || n == "Registers");
                            if s["expensive"] != true && !global && let Some(r) = s["variablesReference"].as_i64().filter(|r| *r != 0) {
                                self.debug.expanded.remove(&r);
                                self.expand_variables(r);
                            }
                        }
                        if let Some(s) = self.debug.stopped.as_mut() {
                            s.scopes = scopes;
                        }
                    }
                    Pending::Variables(reference) => {
                        let vars = m["body"]["variables"].as_array().into_iter().flatten().map(|v| Variable { name: v["name"].as_str().unwrap_or("?").to_string(), value: v["value"].as_str().unwrap_or_default().to_string(), kind: v["type"].as_str().unwrap_or_default().to_string(), reference: v["variablesReference"].as_i64().unwrap_or(0) }).collect();
                        self.debug.variables.insert(reference, vars);
                    }
                    Pending::Evaluate => {
                        let result = m["body"]["result"].as_str().unwrap_or_default().to_string();
                        self.debug.console.push(LogLine { kind: LogKind::Value, text: result });
                    }
                    Pending::Other => {}
                }
            }
            _ => {}
        }
    }
}
