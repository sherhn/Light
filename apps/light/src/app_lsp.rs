//! Language servers of the open project (see [`ide_core::lsp`]): one for each server a module names, started when a
//! file of one of its languages is open - in the project's Linux or on Windows, like the checks - and told about the
//! open files as they change. Go to Declaration, Find Usages, Rename, completion, the hover popup and Reformat Code
//! ask it first, and what it finds wrong is shown in the code. Without a server (none named, not installed, stopped)
//! everything works from the text as before.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;
use ide_core::lsp::install::{self, Place as InstallPlace};
use ide_core::lsp::proto::{self, CompletionItem, Diagnostic, TextEdit};
use ide_core::lsp::{Capabilities, Client, Encoding, LineMap, Output, Session, uri};
use ide_core::modules::LspDef;
use ide_core::search::FileHit;
use lisp_runtime::wsl::{self, Target};
use serde_json::{Value, json};

use crate::app::App;
use crate::editor_view::{Marker, Sev};
use crate::hover::HoverInfo;
use crate::i18n::{self, tr};
use crate::state::{CItem, ConfirmKind, FindResults, Popup};

/// How long typing pauses before the server hears of the new text.
const SETTLE: f64 = 0.25;
/// A server that ran this long before it stopped is started again; one that stops sooner would only stop again.
const STABLE_SECS: f64 = 30.0;
const MAX_RESTARTS: u32 = 3;
/// How long a server may take to stop when asked.
const STOP_WAIT: Duration = Duration::from_millis(800);

/// What a request was for.
enum Pending {
    Definition { name: String },
    References { name: String },
    /// `revs`: the revision of each document of the server when it was asked.
    Rename { old: String, revs: HashMap<u64, u64> },
    Completion { doc: u64, start: usize, prefix: String, explicit: bool },
    Hover { key: String, doc: u64, name: String, end: usize },
    Format { doc: u64, rev: u64 },
}

/// A document the server was told about.
struct SentDoc {
    uri: String,
    /// The revision of the text the server has.
    rev: u64,
    /// Which revision each of the last versions the server heard of is.
    versions: Vec<(i64, u64)>,
}

struct Server {
    id: u64,
    module: String,
    def: LspDef,
    root: PathBuf,
    target: Option<Target>,
    session: Option<Session>,
    cleanup: Option<wsl::Cleanup>,
    docs: HashMap<u64, SentDoc>,
    /// When the process was started (UI clock).
    started: f64,
    restarts: u32,
    /// Why it is not running, once it will not be started again.
    dead: Option<String>,
}

/// A rename the server answered, waiting for the look through the project's text: does the name appear in files of
/// the server's languages the rename leaves alone? A server that does not see the whole project (clangd without a
/// compile_commands.json) renames only what it sees, and that would break the code without a word.
struct RenameCheck {
    server: u64,
    old: String,
    revs: HashMap<u64, u64>,
    changes: Vec<(String, Vec<TextEdit>)>,
    rx: Receiver<Vec<FileHit>>,
}

/// Find Usages answered by a server, waiting for the look through the files it did not name: the server's places are
/// shown at once, the uses the text has elsewhere are added (a server that does not see the whole project knows only
/// some of them).
struct UsagesCheck {
    server: u64,
    name: String,
    rx: Receiver<Vec<FileHit>>,
}

/// A rename that leaves the name in other files, waiting for the user's word.
struct PartialRename {
    server: u64,
    revs: HashMap<u64, u64>,
    changes: Vec<(String, Vec<TextEdit>)>,
}

/// A server Light can install for a language that has none: what the banner offers.
#[derive(Clone)]
pub struct Offer {
    pub module: String,
    pub def: LspDef,
    /// The language's name, as the user knows it.
    pub language: String,
}

/// What an install says while it runs.
enum InstallMsg {
    Line(String),
    Done(Result<(), String>),
}

/// The install that runs.
pub struct Installing {
    pub server: String,
    /// The last line it printed.
    pub last: String,
    log: Vec<String>,
    rx: Receiver<InstallMsg>,
}

/// The last completions a server gave.
struct Completions {
    doc: u64,
    start: usize,
    /// What was typed of the word when they were asked for.
    prefix: String,
    /// With the bytes before the word each one replaces too.
    items: Vec<(CompletionItem, usize)>,
    /// Typing more must ask again rather than narrow these down.
    incomplete: bool,
}

#[derive(Default)]
pub struct LspState {
    servers: Vec<Server>,
    next_id: u64,
    /// (server id, request id) -> what it was for.
    pending: HashMap<(u64, u64), Pending>,
    completions: Option<Completions>,
    /// Where Rename was asked for: (document id, offset, revision).
    pub rename_at: Option<(u64, usize, u64)>,
    rename_check: Option<RenameCheck>,
    partial_rename: Option<PartialRename>,
    usages_check: Option<UsagesCheck>,
    pub offer: Option<Offer>,
    /// Servers the user said "not now" to, by name (for this run of the IDE).
    declined: HashSet<String>,
    pub installing: Option<Installing>,
    /// The last install that failed: (server, why, where its log is).
    pub install_failed: Option<(String, String, String)>,
    /// Servers and messages the user has been told about (once per run of the IDE).
    told: HashSet<String>,
}

/// `LIGHT_LSP=0` keeps language servers off (the UI tests that are not about them must not depend on a clangd that
/// happens to be installed).
fn switched_off() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("LIGHT_LSP").is_some_and(|v| v == "0"))
}

/// The platform servers run on: the project's Linux, or this computer.
fn platform_of(target: Option<&Target>) -> &'static str {
    match target {
        Some(_) => install::wsl_platform(),
        None => install::host_platform(),
    }
    .unwrap_or("")
}

/// A path of this computer as the server sees it.
fn server_path(p: &Path, target: Option<&Target>) -> String {
    match target {
        Some(t) => wsl::to_linux(p, t),
        None => p.to_string_lossy().replace('\\', "/"),
    }
}

/// A path the server names, as a path of this computer.
fn local_path(s: &str, target: Option<&Target>) -> PathBuf {
    match target {
        Some(t) => wsl::to_windows(s, t),
        None if cfg!(windows) => PathBuf::from(s.replace('/', "\\")),
        None => PathBuf::from(s),
    }
}

/// A path for comparing: Windows paths ignore case and have two spellings for WSL folders.
fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
    if !cfg!(windows) {
        return s;
    }
    let low = s.to_lowercase();
    match low.strip_prefix(r"\\wsl$\") {
        Some(rest) => format!(r"\\wsl.localhost\{rest}"),
        None => low,
    }
}

/// What the user is told about a server that is not installed on this computer.
fn not_found(program: &str) -> String {
    i18n::fmt("{0} was not found. Install it to have Go to Declaration, Find Usages, Rename and the errors in the code from it.", &[&program])
}

/// The badge of a completion: `f` function, `m` keyword, `v` variable, `C` type, `s` anything else.
fn badge(kind: u64) -> &'static str {
    match kind {
        2..=4 => "f",
        14 => "m",
        5 | 6 | 10 | 12 | 20 | 21 => "v",
        7 | 8 | 13 | 22 | 25 => "C",
        _ => "s",
    }
}

/// The problems of a text as markers. An empty range covers the word (or the character) there.
fn markers(text: &str, list: &[Diagnostic], enc: Encoding) -> Vec<Marker> {
    let map = LineMap::new(text);
    list.iter()
        .filter(|d| d.severity <= 3)
        .map(|d| {
            let (s, mut e) = map.range(d.range, enc);
            if s == e {
                let line_end = text[s..].find('\n').map_or(text.len(), |n| s + n);
                let word = text[s..line_end].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(line_end - s);
                e = if word > 0 { s + word } else { text[s..line_end].chars().next().map_or(s, |c| s + c.len_utf8()) };
            }
            let sev = match d.severity {
                1 => Sev::Error,
                2 => Sev::Warning,
                _ => Sev::Weak,
            };
            Marker { start: s, end: e, sev, msg: d.message.clone() }
        })
        .collect()
}

/// A text read from disk as the editor has it: lines end with `\n` alone.
fn read_text(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    Some(if t.contains('\r') { t.replace("\r\n", "\n") } else { t })
}

/// A place a server named, found in its file.
struct Place {
    path: PathBuf,
    offset: usize,
    hit: FileHit,
}

impl App {
    /// The language servers the modules name for `lang`, best first: (module id, its definition).
    fn lsp_defs_for(&self, lang: &str) -> Vec<(String, LspDef)> {
        self.mods.installed.iter().filter(|m| m.enabled).flat_map(|m| m.manifest.lsp.iter().filter(|s| s.languages.iter().any(|l| l == lang)).map(|s| (m.manifest.id.clone(), s.clone()))).collect()
    }

    fn server_mut(&mut self, id: u64) -> Option<&mut Server> {
        self.lsp.servers.iter_mut().find(|s| s.id == id)
    }

    /// Keep the servers in step with the open documents and take what they said. Called every frame.
    pub fn poll_lsp(&mut self, ctx: &egui::Context) {
        if switched_off() {
            return;
        }
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            if !self.lsp.servers.is_empty() {
                self.stop_lsp();
            }
            return;
        };
        let target = self.wsl_target();
        // servers of another project or another place to run go
        let (keep, gone): (Vec<Server>, Vec<Server>) = std::mem::take(&mut self.lsp.servers).into_iter().partition(|s| s.root == root && s.target == target);
        self.lsp.servers = keep;
        for s in gone {
            self.end_server(s);
        }
        // which server each open document belongs to
        let mut wanted: HashMap<u64, Vec<usize>> = HashMap::new();
        for i in 0..self.docs.len() {
            let d = &self.docs[i];
            if d.diff.is_some() || d.path.is_none() {
                continue;
            }
            let Some(lang) = d.ed.language().map(|l| l.id().to_string()) else { continue };
            // the first server that has not given up serves it; one that is not installed gives way to the next
            let candidates = self.lsp_defs_for(&lang);
            let dead = |m: &str, n: &str| self.lsp.servers.iter().find(|s| s.module == m && s.def.name == n).and_then(|s| s.dead.clone());
            let Some((module, def)) = candidates.iter().find(|(m, d)| dead(m, &d.name).is_none()).cloned() else {
                // none is left: Light offers to install one it can; else the user hears why the best one is not there
                let platform = platform_of(target.as_ref());
                let installable = candidates.iter().find(|(_, d)| d.install.as_ref().is_some_and(|i| i.available_for(platform)) && !self.lsp.declined.contains(&d.name)).cloned();
                match installable {
                    Some((module, def)) => {
                        if self.lsp.offer.is_none() && self.lsp.installing.is_none() {
                            let language = d.ed.language().map(|l| l.name().to_string()).unwrap_or_else(|| lang.clone());
                            self.lsp.offer = Some(Offer { module, def, language });
                        }
                    }
                    None => {
                        if let Some(why) = candidates.first().and_then(|(m, d)| dead(m, &d.name)) {
                            self.tell_once(format!("lsp:{lang}"), why, Sev::Weak);
                        }
                    }
                }
                continue;
            };
            let found = self.lsp.servers.iter().position(|s| s.module == module && s.def.name == def.name);
            let si = match found {
                Some(si) => si,
                None => {
                    self.lsp.next_id += 1;
                    let id = self.lsp.next_id;
                    self.lsp.servers.push(Server { id, module, def, root: root.clone(), target: target.clone(), session: None, cleanup: None, docs: HashMap::new(), started: 0.0, restarts: 0, dead: None });
                    self.lsp.servers.len() - 1
                }
            };
            let sid = self.lsp.servers[si].id;
            wanted.entry(sid).or_default().push(i);
        }
        let now = self.now;
        let mut waiting = false;
        for si in 0..self.lsp.servers.len() {
            let sid = self.lsp.servers[si].id;
            let docs = wanted.remove(&sid).unwrap_or_default();
            let s = &mut self.lsp.servers[si];
            if s.dead.is_some() {
                continue;
            }
            if s.session.is_none() {
                if docs.is_empty() {
                    continue;
                }
                self.start_server(si, ctx);
            }
            let s = &mut self.lsp.servers[si];
            let Some(session) = s.session.as_mut() else { continue };
            // documents closed (or now in another language) are closed for the server too
            let ids: HashSet<u64> = docs.iter().map(|&i| self.docs[i].id).collect();
            let closed: Vec<u64> = s.docs.keys().filter(|id| !ids.contains(id)).copied().collect();
            for id in closed {
                if let Some(sent) = s.docs.remove(&id) {
                    session.close(&sent.uri);
                }
            }
            for i in docs {
                let d = &self.docs[i];
                let rev = d.ed.revision();
                match s.docs.get_mut(&d.id) {
                    None => {
                        let path = d.path.as_deref().unwrap_or(Path::new(""));
                        let uri = uri::from_path(&server_path(path, s.target.as_ref()));
                        let lang = d.ed.language().map(|l| l.id().to_string()).unwrap_or_default();
                        session.open(&uri, s.def.language_id(&lang), d.ed.text());
                        let version = session.version(&uri).unwrap_or(1);
                        s.docs.insert(d.id, SentDoc { uri, rev, versions: vec![(version, rev)] });
                    }
                    Some(sent) if sent.rev != rev && now - d.view.last_input_time >= SETTLE => {
                        session.change(&sent.uri, d.ed.text());
                        record(sent, session, rev);
                    }
                    Some(sent) => waiting |= sent.rev != rev,
                }
            }
        }
        self.take_lsp_outputs();
        self.poll_install(ctx);
        self.poll_rename_check();
        self.poll_usages_check();
        // what the server says wakes the UI by itself; the text that waits for typing to pause does not
        if waiting {
            ctx.request_repaint_after(Duration::from_secs_f64(SETTLE));
        }
    }

    fn start_server(&mut self, si: usize, ctx: &egui::Context) {
        let now = self.now;
        let s = &mut self.lsp.servers[si];
        let platform = platform_of(s.target.as_ref());
        // a server Light installed comes first: its folders lead PATH
        let installed = match s.target {
            Some(_) => vec![],
            None => install::host_dir().map(|dir| install::host_paths(&s.def, &dir, platform)).unwrap_or_default(),
        };
        let path = std::env::join_paths(installed.iter().cloned().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).ok();
        // on this computer a server that is not installed is known before it is started (what `cmd` says when it
        // cannot find a program is no different from a failure); in Linux the shell's 127 tells
        if s.target.is_none() && ide_core::cmdscript::find_program(s.def.program(), path.as_deref()).is_none() {
            s.dead = Some(not_found(s.def.program()));
            return;
        }
        let line = s.def.command_line(s.target.is_none()).to_string();
        let (mut cmd, cleanup) = match &s.target {
            Some(t) => {
                let marker = wsl::new_marker();
                let line = install::wsl_line(&s.def, &install::wsl_dir(), platform, &line);
                (wsl::shell(t, &s.root, &line, &[], &marker), Some(wsl::Cleanup { target: t.clone(), marker }))
            }
            None => {
                let mut c = ide_core::cmdscript::shell_command(&line, &s.root, &[]);
                if let Some(p) = &path {
                    c.env("PATH", p);
                }
                (c, None)
            }
        };
        lisp_runtime::no_window(&mut cmd);
        let ctx = ctx.clone();
        match Client::spawn(cmd, Some(Arc::new(move || ctx.request_repaint()))) {
            Ok(client) => {
                s.session = Some(Session::start(client, &server_path(&s.root, s.target.as_ref()), s.def.options.clone()).with_settings(s.def.settings.clone()));
                s.cleanup = cleanup;
                s.started = now;
            }
            Err(e) => {
                let why = i18n::fmt("Cannot start {0}: {1}", &[&s.def.name, &e]);
                s.dead = Some(why.clone());
                let key = format!("start:{}", s.def.name);
                self.tell_once(key, why, Sev::Weak);
            }
        }
    }

    /// Stop a server: politely, and by force when it does not listen.
    fn end_server(&mut self, mut s: Server) {
        self.lsp.pending.retain(|(id, _), _| *id != s.id);
        if let Some(mut session) = s.session.take() {
            let pid = session.client().pid();
            if !session.stop(STOP_WAIT)
                && let Some(pid) = pid
            {
                lisp_runtime::runner::kill_tree(pid);
            }
            session.client_mut().kill();
        }
        if let Some(c) = s.cleanup.take() {
            c.run();
        }
        for d in self.docs.iter_mut().filter(|d| s.docs.contains_key(&d.id)) {
            d.lsp_rev = u64::MAX;
        }
    }

    /// Stop every server (the project closes, the IDE exits, the modules changed); they start again when needed.
    pub fn stop_lsp(&mut self) {
        for s in std::mem::take(&mut self.lsp.servers) {
            self.end_server(s);
        }
        self.lsp.pending.clear();
        self.lsp.completions = None;
    }

    fn tell_once(&mut self, key: String, text: String, sev: Sev) {
        if self.lsp.told.insert(key) {
            self.toast(text, sev);
        }
    }

    fn take_lsp_outputs(&mut self) {
        let mut outs: Vec<(u64, Output)> = vec![];
        for s in &mut self.lsp.servers {
            if let Some(session) = s.session.as_mut() {
                outs.extend(session.poll().into_iter().map(|o| (s.id, o)));
            }
        }
        for (sid, o) in outs {
            match o {
                Output::Ready => {}
                Output::Diagnostics { uri, version, list } => self.lsp_diagnostics(sid, &uri, version, &list),
                Output::Response { id, result } => {
                    if let Some(p) = self.lsp.pending.remove(&(sid, id)) {
                        self.lsp_answer(sid, p, result.map_err(|e| e.message));
                    }
                }
                Output::Message { error, text } => {
                    let name = self.server_mut(sid).map(|s| s.def.name.clone()).unwrap_or_default();
                    self.tell_once(format!("msg:{text}"), format!("{name}: {text}"), if error { Sev::Error } else { Sev::Weak });
                }
                Output::Failed(why) => self.server_stopped(sid, Some(why)),
                Output::Exited { expected: false } => self.server_stopped(sid, None),
                Output::Exited { expected: true } => {}
            }
        }
    }

    /// A server stopped without being asked: started again if it had been working a while, else left off (the next
    /// server of its languages takes over, and when there is none the user is told why). What was asked of it is
    /// answered from the text.
    fn server_stopped(&mut self, sid: u64, why: Option<String>) {
        let now = self.now;
        let pending: Vec<Pending> = {
            let keys: Vec<(u64, u64)> = self.lsp.pending.keys().filter(|(s, _)| *s == sid).copied().collect();
            keys.into_iter().filter_map(|k| self.lsp.pending.remove(&k)).collect()
        };
        let target = self.wsl_target();
        let Some(s) = self.server_mut(sid) else { return };
        let mut session = s.session.take();
        if let Some(c) = s.cleanup.take() {
            c.run();
        }
        let docs: HashSet<u64> = s.docs.drain().map(|(id, _)| id).collect();
        let name = s.def.name.clone();
        let program = s.def.program().to_string();
        let code = session.as_mut().and_then(|x| x.client_mut().exit_code(Duration::from_millis(500)));
        let log = session.as_ref().map(|x| x.client().log()).unwrap_or_default();
        let missing = matches!(code, Some(127) | Some(9009));
        let ran = now - s.started;
        if !missing && why.is_none() && ran >= STABLE_SECS && s.restarts < MAX_RESTARTS {
            // it worked for a while: start it again (next frame)
            s.restarts += 1;
        } else {
            let reason = if missing {
                match target {
                    Some(_) => self.linux_command_not_found(Some(&program)).unwrap_or_default(),
                    None => not_found(&program),
                }
            } else {
                let last = why.or_else(|| log.iter().rev().find(|l| !l.trim().is_empty()).cloned()).unwrap_or_default();
                i18n::fmt("{0} stopped: {1}", &[&name, &last])
            };
            if let Some(s) = self.server_mut(sid) {
                s.dead = Some(reason);
            }
        }
        drop(session);
        for d in self.docs.iter_mut().filter(|d| docs.contains(&d.id)) {
            d.lsp_rev = u64::MAX;
        }
        for p in pending {
            self.lsp_answer(sid, p, Err(String::new()));
        }
    }

    fn lsp_diagnostics(&mut self, sid: u64, uri: &str, version: Option<i64>, list: &[Diagnostic]) {
        let Some(s) = self.lsp.servers.iter().find(|s| s.id == sid) else { return };
        let Some(session) = s.session.as_ref() else { return };
        let Some((&doc, sent)) = s.docs.iter().find(|(_, d)| d.uri == uri) else { return };
        // problems of an older text than the server has are followed by newer ones
        let latest = session.version(uri);
        if version.is_some() && version != latest {
            return;
        }
        let Some(rev) = sent.versions.iter().find(|(v, _)| Some(*v) == latest).map(|(_, r)| *r) else { return };
        let (Some(text), enc) = (session.text(uri), session.caps.encoding) else { return };
        let found = markers(text, list, enc);
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == doc) {
            d.lsp_markers = found;
            d.lsp_rev = rev;
        }
    }

    /// The server of document `idx` when it is ready and can do `need`, told the document's current text: (server id,
    /// the document's URI).
    fn lsp_doc(&mut self, idx: usize, need: fn(&Capabilities) -> bool) -> Option<(u64, String)> {
        let d = self.docs.get(idx)?;
        let s = self.lsp.servers.iter_mut().find(|s| s.docs.contains_key(&d.id))?;
        let session = s.session.as_mut().filter(|x| x.ready() && need(&x.caps))?;
        let sent = s.docs.get_mut(&d.id)?;
        if sent.rev != d.ed.revision() {
            session.change(&sent.uri, d.ed.text());
            record(sent, session, d.ed.revision());
        }
        Some((s.id, sent.uri.clone()))
    }

    /// Tell server `sid` the current text of every document it has (before a request about the whole project).
    fn lsp_flush(&mut self, sid: u64) {
        let Some(s) = self.lsp.servers.iter_mut().find(|s| s.id == sid) else { return };
        let Some(session) = s.session.as_mut() else { return };
        for d in &self.docs {
            if let Some(sent) = s.docs.get_mut(&d.id)
                && sent.rev != d.ed.revision()
            {
                session.change(&sent.uri, d.ed.text());
                record(sent, session, d.ed.revision());
            }
        }
    }

    /// `textDocument` and `position` of byte `offset` in a document the server has.
    fn lsp_position(&self, sid: u64, uri: &str, offset: usize) -> Value {
        let session = self.lsp.servers.iter().find(|s| s.id == sid).and_then(|s| s.session.as_ref());
        let (text, enc) = session.map_or(("", Encoding::Utf16), |x| (x.text(uri).unwrap_or(""), x.caps.encoding));
        json!({"textDocument": {"uri": uri}, "position": LineMap::new(text).position(offset, enc)})
    }

    fn lsp_request(&mut self, sid: u64, method: &str, params: Value, p: Pending) -> bool {
        let Some(s) = self.server_mut(sid) else { return false };
        let Some(id) = s.session.as_mut().and_then(|x| x.request(method, params)) else { return false };
        self.lsp.pending.insert((sid, id), p);
        true
    }

    /// The places a server named, each in the text it is in: the server's own copy of an open document, else the
    /// file on disk.
    fn lsp_places(&self, sid: u64, locs: &[proto::Location]) -> Vec<Place> {
        let Some(s) = self.lsp.servers.iter().find(|s| s.id == sid) else { return vec![] };
        let Some(session) = s.session.as_ref() else { return vec![] };
        let enc = session.caps.encoding;
        let mut texts: HashMap<String, Option<String>> = HashMap::new();
        let mut out = vec![];
        for l in locs {
            let Some(p) = uri::to_path(&l.uri) else { continue };
            let path = local_path(&p, s.target.as_ref());
            let text = texts.entry(l.uri.clone()).or_insert_with(|| session.text(&l.uri).map(str::to_string).or_else(|| read_text(&path)));
            let Some(text) = text.as_deref() else { continue };
            let map = LineMap::new(text);
            let (a, b) = map.range(l.range, enc);
            let line = l.range.start.line as usize;
            let line_start = a - (a - text[..a].rfind('\n').map_or(0, |n| n + 1));
            out.push(Place {
                offset: a,
                hit: FileHit { path: path.clone(), line, col: text[line_start..a].chars().count(), len: text[a..b].chars().count(), line_text: map.line(line).trim_end().to_string() },
                path,
            });
        }
        out
    }

    // ------------------------------------------------------------------ installing servers

    /// Where the offered server is installed, the folder its module is in, and the platform.
    fn install_place(&self) -> Option<(InstallPlace, PathBuf, &'static str)> {
        let o = self.lsp.offer.as_ref()?;
        let module = self.mods.installed.iter().find(|m| m.manifest.id == o.module)?.dir.clone();
        let target = self.wsl_target();
        let platform = platform_of(target.as_ref());
        let place = match target {
            Some(t) => InstallPlace::Wsl { distro: t.distro, user: None, dir: install::wsl_dir(), mount_root: t.mount_root },
            None => InstallPlace::Host(install::host_dir()?),
        };
        Some((place, module, platform))
    }

    /// The banner's Install: say what is downloaded, from where and how much, and where it goes, before anything is.
    pub fn lsp_ask_install(&mut self) {
        let Some(o) = self.lsp.offer.clone() else { return };
        let Some((place, module, platform)) = self.install_place() else { return };
        let units = match install::plan(&o.def, &module, platform) {
            Ok(u) => u,
            Err(e) => {
                self.toast(i18n::fmt("Cannot install {0}: {1}", &[&o.def.name, &e]), Sev::Error);
                return;
            }
        };
        let list: Vec<String> = units.iter().map(|u| format!("- {} ({}, {} MB)", u.title, u.source, u.size.div_ceil(1_000_000))).collect();
        let into = match &place {
            InstallPlace::Host(dir) => dir.display().to_string(),
            InstallPlace::Wsl { distro, dir, .. } => format!("{dir} ({distro})"),
        };
        let message = i18n::fmt(
            "Light will download and install:\n{0}\ninto {1}. Each download is checked against the SHA-256 its module names, and npm runs no scripts of the packages. What is there already is not downloaded again.",
            &[&list.join("\n"), &into],
        );
        self.popup = Popup::Confirm { kind: ConfirmKind::InstallServer, message };
    }

    /// Install the offered server (the user said yes), in the background.
    pub fn lsp_install(&mut self) {
        let Some(o) = self.lsp.offer.clone() else { return };
        let Some((place, module, platform)) = self.install_place() else { return };
        let Some(downloads) = install::host_dir().map(|d| d.join("downloads")) else { return };
        let units = match install::plan(&o.def, &module, platform) {
            Ok(u) => u,
            Err(e) => {
                self.toast(i18n::fmt("Cannot install {0}: {1}", &[&o.def.name, &e]), Sev::Error);
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = install::run(&units, &place, &downloads, &mut |l| {
                let _ = tx.send(InstallMsg::Line(l.to_string()));
            });
            let _ = tx.send(InstallMsg::Done(r));
        });
        self.lsp.install_failed = None;
        self.lsp.installing = Some(Installing { server: o.def.name.clone(), last: String::new(), log: vec![], rx });
    }

    /// The banner's "Not now": no more offers of this server in this run of the IDE.
    pub fn lsp_decline_install(&mut self) {
        if let Some(o) = self.lsp.offer.take() {
            self.lsp.declined.insert(o.def.name);
        }
        self.lsp.install_failed = None;
    }

    /// Take what the running install said; when it is done, the server starts (or the banner says why not).
    fn poll_install(&mut self, ctx: &egui::Context) {
        let Some(inst) = self.lsp.installing.as_mut() else { return };
        ctx.request_repaint_after(Duration::from_millis(150));
        let mut done = None;
        while let Ok(m) = inst.rx.try_recv() {
            match m {
                InstallMsg::Line(l) => {
                    inst.last = l.clone();
                    inst.log.push(l);
                }
                InstallMsg::Done(r) => done = Some(r),
            }
        }
        let Some(result) = done else { return };
        let Some(inst) = self.lsp.installing.take() else { return };
        // the log is kept beside the servers, for when something went wrong
        let log_path = install::host_dir().map(|d| d.join("install.log"));
        if let Some(p) = &log_path {
            let _ = std::fs::create_dir_all(p.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(p, inst.log.join("\n") + "\n");
        }
        match result {
            Ok(()) => {
                self.lsp.offer = None;
                // it starts afresh, with the folders it was installed in
                let (gone, keep): (Vec<Server>, Vec<Server>) = std::mem::take(&mut self.lsp.servers).into_iter().partition(|s| s.def.name == inst.server);
                self.lsp.servers = keep;
                for s in gone {
                    self.end_server(s);
                }
                self.toast(i18n::fmt("{} is installed", &[&inst.server]), Sev::Weak);
            }
            Err(e) => {
                let where_log = log_path.map(|p| p.display().to_string()).unwrap_or_default();
                self.lsp.install_failed = Some((inst.server, e, where_log));
            }
        }
    }

    // ------------------------------------------------------------------ what the editor asks

    /// Ask the server where the name at the caret is defined. False when no server can tell (the caller looks in
    /// the text); otherwise the answer comes later.
    pub fn lsp_goto_declaration(&mut self, name: &str) -> bool {
        let idx = self.active;
        let Some((sid, uri)) = self.lsp_doc(idx, |c| c.definition) else { return false };
        let params = self.lsp_position(sid, &uri, self.docs[idx].ed.caret());
        self.lsp_request(sid, "textDocument/definition", params, Pending::Definition { name: name.to_string() })
    }

    pub fn lsp_find_usages(&mut self, name: &str) -> bool {
        let idx = self.active;
        let Some((sid, uri)) = self.lsp_doc(idx, |c| c.references) else { return false };
        self.lsp_flush(sid);
        let mut params = self.lsp_position(sid, &uri, self.docs[idx].ed.caret());
        params["context"] = json!({"includeDeclaration": true});
        let title = i18n::fmt("Usages of '{}'", &[&name]);
        self.find_results = FindResults { title: i18n::fmt("{} - searching...", &[&title]), hits: vec![] };
        self.bottom_tool = Some(crate::state::BottomTool::Find);
        self.lsp_request(sid, "textDocument/references", params, Pending::References { name: name.to_string() })
    }

    /// Rename asked for at the caret of the active document: is it the server's to do?
    pub fn lsp_can_rename(&mut self) -> bool {
        self.lsp_doc(self.active, |c| c.rename).is_some()
    }

    /// Rename `old`, which Rename was opened on, to `new`. False when no server does it.
    pub fn lsp_rename(&mut self, old: &str, new: &str) -> bool {
        let Some((doc, offset, rev)) = self.lsp.rename_at.take() else { return false };
        let Some(idx) = self.docs.iter().position(|d| d.id == doc && d.ed.revision() == rev) else { return false };
        let Some((sid, uri)) = self.lsp_doc(idx, |c| c.rename) else { return false };
        self.lsp_flush(sid);
        let revs = self.lsp.servers.iter().find(|s| s.id == sid).map(|s| s.docs.iter().map(|(id, sent)| (*id, sent.rev)).collect()).unwrap_or_default();
        let mut params = self.lsp_position(sid, &uri, offset);
        params["newName"] = json!(new);
        self.lsp.partial_rename = None;
        self.lsp_request(sid, "textDocument/rename", params, Pending::Rename { old: old.to_string(), revs })
    }

    /// Reformat the active document with its server. False when it has none that formats.
    pub fn lsp_format(&mut self) -> bool {
        let idx = self.active;
        let Some((sid, uri)) = self.lsp_doc(idx, |c| c.formatting) else { return false };
        let d = &self.docs[idx];
        let size = d.ed.language().map_or(4, |l| l.indent_size());
        let (doc, rev) = (d.id, d.ed.revision());
        self.lsp_request(sid, "textDocument/formatting", json!({"textDocument": {"uri": uri}, "options": {"tabSize": size, "insertSpaces": true}}), Pending::Format { doc, rev })
    }

    /// Ask the server what the hover popup says about the name ending at `end`. False when no server can tell.
    pub fn lsp_hover(&mut self, idx: usize, name: &str, end: usize, key: &str) -> bool {
        let Some((sid, uri)) = self.lsp_doc(idx, |c| c.hover) else { return false };
        let params = self.lsp_position(sid, &uri, end.saturating_sub(1));
        let doc = self.docs[idx].id;
        self.lsp_request(sid, "textDocument/hover", params, Pending::Hover { key: key.to_string(), doc, name: name.to_string(), end })
    }

    /// Did the user just type what makes the server offer completions (`.`, `->`) at the caret of document `idx`?
    pub fn lsp_triggered(&self, idx: usize) -> bool {
        let Some(d) = self.docs.get(idx) else { return false };
        let Some(s) = self.lsp.servers.iter().find(|s| s.docs.contains_key(&d.id)) else { return false };
        let Some(session) = s.session.as_ref().filter(|x| x.ready() && x.caps.completion) else { return false };
        let before = &d.ed.text()[..d.ed.caret()];
        session.caps.completion_triggers.iter().any(|t| !t.is_empty() && before.ends_with(t.as_str()))
    }

    /// The server's completions for the word that starts at `start` in document `idx`, of which `prefix` is typed:
    /// those it gave before when they still hold, else `None` and they are asked for (the list is made again when
    /// they come).
    pub fn lsp_completions(&mut self, idx: usize, start: usize, prefix: &str, explicit: bool) -> Option<Vec<CItem>> {
        let doc = self.docs.get(idx)?.id;
        if let Some(c) = &self.lsp.completions
            && c.doc == doc
            && c.start == start
            && (!c.incomplete || c.prefix == prefix)
            && prefix.starts_with(c.prefix.as_str())
        {
            let low = prefix.to_lowercase();
            let mut items: Vec<&(CompletionItem, usize)> = c.items.iter().filter(|(i, _)| i.filter.to_lowercase().starts_with(&low)).collect();
            items.sort_by(|a, b| a.0.sort.cmp(&b.0.sort));
            return Some(items.into_iter().map(|(i, back)| CItem { text: i.insert.clone(), back: *back, kind: badge(i.kind).to_string(), source: 0 }).collect());
        }
        let asked = self.lsp.pending.values().any(|p| matches!(p, Pending::Completion { doc: d, start: s, prefix: p, .. } if *d == doc && *s == start && p == prefix));
        if !asked && let Some((sid, uri)) = self.lsp_doc(idx, |c| c.completion) {
            let params = self.lsp_position(sid, &uri, self.docs[idx].ed.caret());
            self.lsp_request(sid, "textDocument/completion", params, Pending::Completion { doc, start, prefix: prefix.to_string(), explicit });
        }
        None
    }

    /// Nothing is asked of a server and every one that runs has the documents as they are and said what is wrong in
    /// them (the UI tests wait for this).
    pub fn lsp_idle(&self) -> bool {
        self.lsp.pending.is_empty()
            && self.lsp.installing.is_none()
            && self.lsp.rename_check.is_none()
            && self.lsp.usages_check.is_none()
            && self.lsp.servers.iter().all(|s| {
                let Some(session) = s.session.as_ref() else { return s.dead.is_some() || s.docs.is_empty() };
                session.ready()
                    && s.docs.iter().all(|(id, sent)| self.docs.iter().find(|d| d.id == *id).is_none_or(|d| sent.rev == d.ed.revision() && d.lsp_rev == sent.rev))
            })
    }

    /// The file was saved.
    pub fn lsp_saved(&mut self, idx: usize) {
        let Some((sid, uri)) = self.lsp_doc(idx, |c| c.sync) else { return };
        if let Some(session) = self.server_mut(sid).and_then(|s| s.session.as_mut()) {
            session.save(&uri);
        }
    }

    // ------------------------------------------------------------------ answers

    fn lsp_answer(&mut self, sid: u64, p: Pending, result: Result<Value, String>) {
        match p {
            Pending::Definition { name } => {
                let places = result.map(|v| self.lsp_places(sid, &proto::locations(&v))).unwrap_or_default();
                match places.len() {
                    0 => self.goto_language_declaration_text(Some(name).filter(|n| !n.is_empty())),
                    1 => self.goto(&places[0].path, places[0].offset, true),
                    _ => {
                        use crate::state::{Item, PTarget, PaletteMode, PaletteState, Popup};
                        let root = self.project_root().map(Path::to_path_buf);
                        let items: Vec<Item> = places
                            .iter()
                            .map(|p| Item {
                                label: p.hit.line_text.trim().to_string(),
                                detail: format!("{}:{}", root.as_ref().map_or_else(|| p.path.display().to_string(), |r| ide_core::workspace::relative(r, &p.path)), p.hit.line + 1),
                                badge: "f".into(),
                                section: "",
                                target: PTarget::Symbol { path: p.path.clone(), offset: p.offset },
                                hits: vec![],
                                shortcut: String::new(),
                            })
                            .collect();
                        self.popup = Popup::Palette(PaletteState { mode: PaletteMode::Choose, query: String::new(), items: items.clone(), selected: 0, built_for: None, fixed: Some(items), just_opened: true });
                    }
                }
            }
            Pending::References { name } => {
                let places = result.map(|v| self.lsp_places(sid, &proto::locations(&v))).unwrap_or_default();
                if places.is_empty() {
                    // nothing the server knows of: what the text says
                    self.find_usages_everywhere(&name);
                    return;
                }
                let mut hits: Vec<FileHit> = places.into_iter().map(|p| p.hit).collect();
                hits.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
                hits.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.col == b.col);
                let named: HashSet<String> = hits.iter().map(|h| path_key(&h.path)).collect();
                let rx = self.text_uses_elsewhere(sid, &name, &named);
                self.find_results = FindResults { title: i18n::fmt("Usages of '{}'", &[&name]), hits };
                self.bottom_tool = Some(crate::state::BottomTool::Find);
                self.lsp.usages_check = Some(UsagesCheck { server: sid, name, rx });
            }
            Pending::Rename { old, revs } => match result.and_then(|v| proto::workspace_edit(&v)) {
                Ok(changes) if changes.is_empty() => self.toast(tr("Nothing to rename here"), Sev::Weak),
                Ok(changes) => self.check_rename(sid, old, revs, changes),
                Err(e) if e.is_empty() => self.toast(tr("The language server stopped - nothing was renamed"), Sev::Error),
                Err(e) => self.toast(i18n::fmt("Cannot rename: {}", &[&e]), Sev::Error),
            },
            Pending::Completion { doc, start, prefix, explicit } => {
                let Ok(v) = result else { return };
                let (items, incomplete) = proto::completion(&v);
                // an item that replaces text before the word (typescript-language-server's `.max` for the `.` of
                // `Math.`): what it repeats of that text is left out of it, and what it does not is replaced
                let s = self.lsp.servers.iter().find(|s| s.id == sid);
                let text = s.and_then(|s| s.docs.get(&doc)).and_then(|sent| s.and_then(|s| s.session.as_ref()).and_then(|x| x.text(&sent.uri))).unwrap_or("");
                let enc = s.and_then(|s| s.session.as_ref()).map_or(Encoding::Utf16, |x| x.caps.encoding);
                let map = LineMap::new(text);
                let items = items
                    .into_iter()
                    .map(|mut i| {
                        let from = i.range.map_or(start, |r| map.offset(r.start, enc)).min(start);
                        let before = text.get(from..start).unwrap_or("");
                        match i.insert.strip_prefix(before) {
                            Some(rest) if !before.is_empty() => {
                                i.insert = rest.to_string();
                                i.filter = i.filter.strip_prefix(before).map_or(i.filter.clone(), str::to_string);
                                (i, 0)
                            }
                            _ => (i, start - from),
                        }
                    })
                    .collect();
                self.lsp.completions = Some(Completions { doc, start, prefix, items, incomplete });
                // the list is made again, if the caret is still in that word
                let still = self.docs.get(self.active).is_some_and(|d| d.id == doc && d.ed.identifier_range_at(d.ed.caret()).0 == start);
                if still {
                    self.refresh_language_completion(explicit);
                }
            }
            Pending::Hover { key, doc, name, end } => {
                let server = self.lsp.servers.iter().find(|s| s.id == sid).map(|s| s.def.name.clone()).unwrap_or_default();
                let info = result.ok().and_then(|v| proto::hover_text(&v)).map(|md| {
                    let p = proto::hover_parts(&md);
                    HoverInfo { title: name.clone(), kind: p.kind, signature: p.signature, doc: p.doc, source: server, fallback: false }
                });
                // the server knows nothing of it: what the project's text says
                let info = info.or_else(|| self.docs.iter().position(|d| d.id == doc).and_then(|i| self.language_hover(i, &name, end)));
                self.hover_answer(&key, info);
            }
            Pending::Format { doc, rev } => {
                let Ok(v) = result else { return };
                let enc = self.lsp.servers.iter().find(|s| s.id == sid).and_then(|s| s.session.as_ref()).map_or(Encoding::Utf16, |x| x.caps.encoding);
                let Some(d) = self.docs.iter_mut().find(|d| d.id == doc && d.ed.revision() == rev) else { return };
                if let Ok(ranges) = proto::edit_ranges(d.ed.text(), &proto::text_edits(&v), enc)
                    && !ranges.is_empty()
                {
                    d.ed.apply_edits(&ranges);
                    d.view.scroll_to_caret = true;
                }
            }
        }
    }

    /// Look for uses of `name` - by the text, as without a server - in the files of the server's languages other than
    /// `except` (path keys), in a thread: the project may be big.
    fn text_uses_elsewhere(&self, sid: u64, name: &str, except: &HashSet<String>) -> Receiver<Vec<FileHit>> {
        let (tx, rx) = mpsc::channel();
        let Some(s) = self.lsp.servers.iter().find(|s| s.id == sid) else {
            let _ = tx.send(vec![]);
            return rx;
        };
        let langs = self.mods.langs.clone();
        let served = s.def.languages.clone();
        let files: Vec<PathBuf> = self
            .project
            .as_ref()
            .map(|p| p.files.iter().filter(|f| !except.contains(&path_key(f))).filter(|f| langs.find(f, None).is_some_and(|l| served.iter().any(|x| x == l.id()))).cloned().collect())
            .unwrap_or_default();
        let open: Vec<(PathBuf, String)> = self.docs.iter().filter(|d| d.diff.is_none()).filter_map(|d| d.path.clone().map(|p| (p, d.ed.text().to_string()))).collect();
        let name = name.to_string();
        std::thread::spawn(move || {
            let mut found = vec![];
            for path in files {
                let text = match open.iter().find(|(p, _)| *p == path) {
                    Some((_, t)) => t.clone(),
                    None => match std::fs::metadata(&path) {
                        Ok(m) if m.len() <= 2_000_000 => match read_text(&path) {
                            Some(t) => t,
                            None => continue,
                        },
                        _ => continue,
                    },
                };
                if !text.contains(name.as_str()) {
                    continue;
                }
                let Some(lang) = langs.find(&path, None) else { continue };
                found.extend(crate::hover::usages_in(&path, &text, std::slice::from_ref(&name), &lang.highlight(&text)));
                if found.len() >= 2000 {
                    break;
                }
            }
            let _ = tx.send(found);
        });
        rx
    }

    /// Before a rename is made: does the name appear in files of the server's languages it leaves alone? What is found
    /// decides whether the rename is made at once or asked about.
    fn check_rename(&mut self, sid: u64, old: String, revs: HashMap<u64, u64>, changes: Vec<(String, Vec<TextEdit>)>) {
        let target = self.lsp.servers.iter().find(|s| s.id == sid).and_then(|s| s.target.clone());
        let touched: HashSet<String> = changes.iter().filter_map(|(u, _)| uri::to_path(u)).map(|p| path_key(&local_path(&p, target.as_ref()))).collect();
        let rx = self.text_uses_elsewhere(sid, &old, &touched);
        self.lsp.rename_check = Some(RenameCheck { server: sid, old, revs, changes, rx });
    }

    /// The uses the text has in files the server did not name are added to its places.
    fn poll_usages_check(&mut self) {
        let Some(c) = &self.lsp.usages_check else { return };
        let more = match c.rx.try_recv() {
            Ok(more) => more,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => vec![],
        };
        let Some(c) = self.lsp.usages_check.take() else { return };
        if more.is_empty() {
            return;
        }
        let server = self.lsp.servers.iter().find(|s| s.id == c.server).map(|s| s.def.name.clone()).unwrap_or_default();
        let files = more.iter().map(|h| path_key(&h.path)).collect::<HashSet<_>>().len();
        self.find_results.title = i18n::fmt("Usages of '{0}' - and {1} found by the text in {2} file(s) {3} does not see", &[&c.name, &more.len(), &files, &server]);
        self.find_results.hits.extend(more);
    }

    /// The look through the project for a rename is done: make the rename, or ask first when it leaves the name
    /// elsewhere.
    fn poll_rename_check(&mut self) {
        let Some(c) = &self.lsp.rename_check else { return };
        let found = match c.rx.try_recv() {
            Ok(found) => found,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => vec![],
        };
        let Some(c) = self.lsp.rename_check.take() else { return };
        let mut seen: Vec<String> = vec![];
        let found: Vec<(PathBuf, usize)> = found.into_iter().fold(vec![], |mut acc, h| {
            match seen.iter().position(|k| *k == path_key(&h.path)) {
                Some(i) => acc[i].1 += 1,
                None => {
                    seen.push(path_key(&h.path));
                    acc.push((h.path, 1));
                }
            }
            acc
        });
        if found.is_empty() {
            self.finish_rename(c.server, &c.revs, c.changes);
            return;
        }
        let name = self.lsp.servers.iter().find(|s| s.id == c.server).map(|s| s.def.name.clone()).unwrap_or_default();
        let root = self.project_root().map(Path::to_path_buf);
        let rel = |p: &Path| root.as_ref().map_or_else(|| p.display().to_string(), |r| ide_core::workspace::relative(r, p));
        let mut list: Vec<String> = found.iter().take(4).map(|(p, _)| rel(p)).collect();
        if found.len() > 4 {
            list.push(i18n::fmt("{} more", &[&(found.len() - 4)]));
        }
        let places: usize = c.changes.iter().map(|(_, e)| e.len()).sum();
        let message = i18n::fmt(
            "{0} would rename {1} occurrences in {2} file(s), but '{3}' is also used in {4} - files {0} does not see as part of the project (a C or C++ project needs a compile_commands.json for that). Renaming only what {0} found may break the code.",
            &[&name, &places, &c.changes.len(), &c.old, &list.join(", ")],
        );
        self.lsp.partial_rename = Some(PartialRename { server: c.server, revs: c.revs, changes: c.changes });
        self.popup = Popup::Confirm { kind: ConfirmKind::PartialRename, message };
    }

    /// The user agreed to a rename that leaves the name elsewhere.
    pub fn accept_partial_rename(&mut self) {
        if let Some(p) = self.lsp.partial_rename.take() {
            self.finish_rename(p.server, &p.revs, p.changes);
        }
    }

    fn finish_rename(&mut self, sid: u64, revs: &HashMap<u64, u64>, changes: Vec<(String, Vec<TextEdit>)>) {
        match self.lsp_apply(sid, revs, changes) {
            Ok((n, files)) => self.toast(i18n::fmt("Renamed {0} occurrences in {1} file(s)", &[&n, &files]), Sev::Weak),
            Err(e) if e.is_empty() => self.toast(tr("The language server stopped - nothing was renamed"), Sev::Error),
            Err(e) => self.toast(i18n::fmt("Cannot rename: {}", &[&e]), Sev::Error),
        }
    }

    /// Make the edits of a rename: all of them or none. Open documents are edited in the editor (one undo step each),
    /// other files on disk. Returns (edits made, files changed).
    fn lsp_apply(&mut self, sid: u64, revs: &HashMap<u64, u64>, changes: Vec<(String, Vec<TextEdit>)>) -> Result<(usize, usize), String> {
        let Some(s) = self.lsp.servers.iter().find(|s| s.id == sid) else { return Err(String::new()) };
        let enc = s.session.as_ref().map_or(Encoding::Utf16, |x| x.caps.encoding);
        let target = s.target.clone();
        enum Plan {
            Doc(usize, Vec<(usize, usize, String)>),
            File(PathBuf, String, String),
        }
        let mut plans = vec![];
        let mut count = 0;
        for (u, edits) in &changes {
            let p = uri::to_path(u).ok_or_else(|| format!("not a file: {u}"))?;
            let path = local_path(&p, target.as_ref());
            let key = path_key(&path);
            match self.docs.iter().position(|d| d.diff.is_none() && d.path.as_deref().is_some_and(|x| path_key(x) == key)) {
                Some(i) => {
                    // the server's places are in the text it had when it was asked
                    let d = &self.docs[i];
                    let stale = match revs.get(&d.id) {
                        Some(rev) => *rev != d.ed.revision(),
                        None => d.ed.is_modified(),
                    };
                    if stale {
                        return Err(tr("the text changed meanwhile - try again").into());
                    }
                    plans.push(Plan::Doc(i, proto::edit_ranges(d.ed.text(), edits, enc)?));
                }
                None => {
                    let raw = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let crlf = raw.contains("\r\n");
                    let new = proto::apply(&raw.replace("\r\n", "\n"), edits, enc)?;
                    let disk = if crlf { new.replace('\n', "\r\n") } else { new.clone() };
                    plans.push(Plan::File(path, disk, new));
                }
            }
            count += edits.len();
        }
        let files = plans.len();
        let mut changed_uris = vec![];
        for plan in plans {
            match plan {
                Plan::Doc(i, ranges) => {
                    self.docs[i].ed.apply_edits(&ranges);
                    if let Some(p) = self.docs[i].path.clone() {
                        let text = self.docs[i].ed.text().to_string();
                        self.lang_index_saved(&p, &text);
                    }
                }
                Plan::File(path, disk, text) => {
                    std::fs::write(&path, disk).map_err(|e| format!("{}: {e}", path.display()))?;
                    self.lang_index_saved(&path, &text);
                    if let (Some(proj), Some(dialect)) = (self.project.as_mut(), lisp_syntax::Dialect::from_path(&path)) {
                        proj.index.update_file(&path, &text, dialect);
                    }
                    changed_uris.push(uri::from_path(&server_path(&path, target.as_ref())));
                }
            }
        }
        if let Some(session) = self.server_mut(sid).and_then(|s| s.session.as_mut()) {
            session.files_changed(&changed_uris);
        }
        Ok((count, files))
    }
}

/// The server was told revision `rev` of a document: remember which version that is.
fn record(sent: &mut SentDoc, session: &Session, rev: u64) {
    sent.rev = rev;
    if let Some(v) = session.version(&sent.uri) {
        sent.versions.push((v, rev));
        if sent.versions.len() > 8 {
            sent.versions.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_core::lsp::{Position, Range};

    #[test]
    fn paths_there_and_back() {
        let t = Target::new("Ubuntu", "/mnt/");
        let p = Path::new(r"E:\Projects\demo\main.c");
        assert_eq!(server_path(p, Some(&t)), "/mnt/e/Projects/demo/main.c");
        assert_eq!(path_key(&local_path("/mnt/e/Projects/demo/main.c", Some(&t))), path_key(p));
        assert_eq!(server_path(p, None), "E:/Projects/demo/main.c");
        if cfg!(windows) {
            assert_eq!(path_key(&local_path("E:/Projects/demo/main.c", None)), path_key(p));
            assert_eq!(path_key(Path::new(r"\\wsl$\Ubuntu\home\a.c")), path_key(Path::new(r"\\wsl.localhost\ubuntu\home\A.c")));
        }
    }

    #[test]
    fn problems_become_markers() {
        let text = "int main(void) {\n  return missing;\n}\n";
        let at = |l, c| Position { line: l, character: c };
        let list = [
            Diagnostic { range: Range { start: at(1, 9), end: at(1, 16) }, severity: 1, message: "undeclared".into() },
            // an empty range: the word there
            Diagnostic { range: Range { start: at(1, 9), end: at(1, 9) }, severity: 2, message: "here".into() },
            Diagnostic { range: Range { start: at(0, 0), end: at(0, 3) }, severity: 4, message: "a hint".into() },
        ];
        let m = markers(text, &list, Encoding::Utf16);
        assert_eq!(m.len(), 2);
        assert_eq!((&text[m[0].start..m[0].end], m[0].sev), ("missing", Sev::Error));
        assert_eq!((&text[m[1].start..m[1].end], m[1].sev), ("missing", Sev::Warning));
    }

    #[test]
    fn completion_badges() {
        assert_eq!([badge(3), badge(14), badge(6), badge(22), badge(1)], ["f", "m", "v", "C", "s"]);
    }
}
