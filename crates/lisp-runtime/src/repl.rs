//! An interactive Lisp session driven through the structured protocol of `repl-server.lisp`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::discover::{Arg, Implementation, Launch};
use crate::wsl::Cleanup;
use crate::protocol::{self, SOH};

const SERVER_SOURCE: &str = include_str!("../lisp/repl-server.lisp");

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplEvent {
    /// The server finished booting (`SBCL 2.6.8`).
    Ready(String),
    /// Text printed by user code (stdout and stderr merged, in order).
    Output(String),
    /// Printed values of an evaluation (one per line).
    Value { id: u64, text: String },
    Error { id: u64, message: String },
    Trace { id: u64, text: String },
    /// Evaluation finished; carries the current `*package*` name.
    Done { id: u64, package: String },
    /// Answer to a non-eval request (completion, arglist, doc, ...).
    Reply { id: u64, payload: String },
    Exited(Option<i32>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalOutcome {
    pub output: String,
    pub values: Option<String>,
    pub error: Option<String>,
    pub trace: Option<String>,
    pub package: String,
}

pub struct ReplSession {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Mutex<Option<ChildStdin>>,
    rx: Mutex<Receiver<ReplEvent>>,
    next_id: AtomicU64,
    server_file: PathBuf,
    cleanup: Option<Cleanup>,
    pub implementation: Implementation,
}

fn write_server_file() -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join("light");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("repl-server-{}-{}.lisp", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    std::fs::write(&path, SERVER_SOURCE)?;
    Ok(path)
}

/// Split a byte stream into plain output and protocol messages.
struct LineAssembler {
    pending: String,
    /// True while we are in the middle of a plain (non-protocol) line that was already emitted.
    mid_plain_line: bool,
    utf8_tail: Vec<u8>,
}

impl LineAssembler {
    fn new() -> Self {
        LineAssembler { pending: String::new(), mid_plain_line: false, utf8_tail: Vec::new() }
    }

    fn feed(&mut self, bytes: &[u8], tx: &Sender<ReplEvent>) {
        let mut buf = std::mem::take(&mut self.utf8_tail);
        buf.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&buf) {
            Ok(_) => buf.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(), // incomplete trailing sequence
            Err(e) => {
                // invalid byte: lossy-convert everything
                let _ = e;
                buf.len()
            }
        };
        let tail = buf.split_off(valid);
        self.utf8_tail = tail;
        let chunk = String::from_utf8_lossy(&buf).into_owned();
        self.pending.push_str(&chunk);
        loop {
            if let Some(nl) = self.pending.find('\n') {
                let mut line: String = self.pending.drain(..=nl).collect();
                line.pop(); // \n
                if line.ends_with('\r') {
                    line.pop();
                }
                if !self.mid_plain_line && line.starts_with(SOH) {
                    if let Some(m) = protocol::parse_message(&line) {
                        let _ = tx.send(to_event(m));
                    }
                } else {
                    line.push('\n');
                    let _ = tx.send(ReplEvent::Output(line));
                }
                self.mid_plain_line = false;
            } else {
                // Partial line: plain text is shown immediately (prompts etc.); a possible
                // protocol line (starts with SOH) is held back until it is complete.
                if !self.pending.is_empty() && (self.mid_plain_line || !self.pending.starts_with(SOH)) {
                    let text = std::mem::take(&mut self.pending);
                    self.mid_plain_line = true;
                    let _ = tx.send(ReplEvent::Output(text));
                }
                break;
            }
        }
    }

    fn finish(&mut self, tx: &Sender<ReplEvent>) {
        if !self.pending.is_empty() {
            let text = std::mem::take(&mut self.pending);
            if !text.starts_with(SOH) {
                let _ = tx.send(ReplEvent::Output(text));
            }
        }
    }
}

fn to_event(m: protocol::Message) -> ReplEvent {
    match m.kind.as_str() {
        "ready" => ReplEvent::Ready(m.payload),
        "value" => ReplEvent::Value { id: m.id, text: m.payload },
        "error" => ReplEvent::Error { id: m.id, message: m.payload },
        "trace" => ReplEvent::Trace { id: m.id, text: m.payload },
        "done" => ReplEvent::Done { id: m.id, package: m.payload },
        _ => ReplEvent::Reply { id: m.id, payload: m.payload },
    }
}

impl ReplSession {
    pub fn start(implementation: &Implementation, cwd: Option<&Path>) -> io::Result<ReplSession> {
        let server_file = write_server_file()?;
        let args = [Arg::Text("--noinform"), Arg::Text("--disable-ldb"), Arg::Text("--lose-on-corruption"), Arg::Text("--no-userinit"), Arg::Text("--no-sysinit"), Arg::Text("--load"), Arg::Path(&server_file)];
        let Launch { mut cmd, cleanup } = implementation.launch(cwd, &args);
        crate::no_window(&mut cmd);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let (tx, rx) = mpsc::channel();

        let tx_out = tx.clone();
        std::thread::Builder::new().name("lisp-stdout".into()).spawn(move || {
            let mut asm = LineAssembler::new();
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => asm.feed(&buf[..n], &tx_out),
                }
            }
            asm.finish(&tx_out);
        })?;
        // stderr is only used for startup failures and raw runtime crashes; the server merges
        // *error-output* into stdout so ordering is preserved for normal output.
        let tx_err = tx.clone();
        std::thread::Builder::new().name("lisp-stderr".into()).spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = tx_err.send(ReplEvent::Output(String::from_utf8_lossy(&buf[..n]).into_owned()));
                    }
                }
            }
        })?;

        let child = Arc::new(Mutex::new(Some(child)));
        let waiter = child.clone();
        std::thread::Builder::new().name("lisp-wait".into()).spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(50));
                let mut guard = waiter.lock().unwrap();
                match guard.as_mut() {
                    None => return, // killed explicitly
                    Some(c) => {
                        if let Ok(Some(status)) = c.try_wait() {
                            *guard = None;
                            drop(guard);
                            // let the reader threads flush first
                            std::thread::sleep(Duration::from_millis(150));
                            let _ = tx.send(ReplEvent::Exited(status.code()));
                            return;
                        }
                    }
                }
            }
        })?;

        Ok(ReplSession {
            child,
            stdin: Mutex::new(stdin),
            rx: Mutex::new(rx),
            next_id: AtomicU64::new(1),
            server_file,
            cleanup,
            implementation: implementation.clone(),
        })
    }

    /// Send a raw request; returns its id.
    pub fn request(&self, kind: &str, payload: &str) -> io::Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let line = protocol::request_line(kind, id, payload);
        let mut guard = self.stdin.lock().unwrap();
        let stdin = guard.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "REPL is not running"))?;
        stdin.write_all(line.as_bytes())?;
        stdin.flush()?;
        Ok(id)
    }

    pub fn eval(&self, code: &str) -> io::Result<u64> {
        self.request("eval", code)
    }

    pub fn try_recv(&self) -> Option<ReplEvent> {
        self.rx.lock().unwrap().try_recv().ok()
    }

    pub fn recv_timeout(&self, d: Duration) -> Option<ReplEvent> {
        self.rx.lock().unwrap().recv_timeout(d).ok()
    }

    pub fn is_alive(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    pub fn kill(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
            if let Some(cl) = &self.cleanup {
                cl.run();
            }
        }
        *self.stdin.lock().unwrap() = None;
    }

    /// Wait for the `Ready` event (server booted). Other events are returned in `leftover`.
    pub fn wait_ready(&self, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        let mut seen = String::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("timed out waiting for the REPL to start. Output so far: {seen}"));
            }
            match self.recv_timeout(left) {
                Some(ReplEvent::Ready(v)) => return Ok(v),
                Some(ReplEvent::Output(t)) => seen.push_str(&t),
                Some(ReplEvent::Exited(c)) => return Err(format!("Lisp exited during startup (code {c:?}): {seen}")),
                Some(_) => {}
                None => {}
            }
        }
    }

    /// Evaluate and block until finished; for CLI use and tests.
    pub fn eval_sync(&self, code: &str, timeout: Duration) -> Result<EvalOutcome, String> {
        let id = self.eval(code).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let mut out = EvalOutcome::default();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("evaluation timed out; output so far: {}", out.output));
            }
            match self.recv_timeout(left) {
                Some(ReplEvent::Output(t)) => out.output.push_str(&t),
                Some(ReplEvent::Value { id: i, text }) if i == id => out.values = Some(text),
                Some(ReplEvent::Error { id: i, message }) if i == id => out.error = Some(message),
                Some(ReplEvent::Trace { id: i, text }) if i == id => out.trace = Some(text),
                Some(ReplEvent::Done { id: i, package }) if i == id => {
                    out.package = package;
                    return Ok(out);
                }
                Some(ReplEvent::Exited(c)) => return Err(format!("Lisp exited (code {c:?}); output: {}", out.output)),
                _ => {}
            }
        }
    }

    /// Send a request and block for its `Reply`.
    pub fn query_sync(&self, kind: &str, payload: &str, timeout: Duration) -> Result<String, String> {
        let id = self.request(kind, payload).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("{kind} request timed out"));
            }
            match self.recv_timeout(left) {
                Some(ReplEvent::Reply { id: i, payload }) if i == id => return Ok(payload),
                Some(ReplEvent::Exited(c)) => return Err(format!("Lisp exited (code {c:?})")),
                _ => {}
            }
        }
    }
}

impl Drop for ReplSession {
    fn drop(&mut self) {
        self.kill();
        let _ = std::fs::remove_file(&self.server_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(chunks: &[&[u8]]) -> Vec<ReplEvent> {
        let (tx, rx) = mpsc::channel();
        let mut a = LineAssembler::new();
        for c in chunks {
            a.feed(c, &tx);
        }
        a.finish(&tx);
        drop(tx);
        rx.iter().collect()
    }

    fn joined_output(evs: &[ReplEvent]) -> String {
        evs.iter().filter_map(|e| if let ReplEvent::Output(t) = e { Some(t.as_str()) } else { None }).collect()
    }

    #[test]
    fn assembler_separates_protocol_from_output() {
        let evs = collect(&[b"hello\n\x01value\t3\t42\n", b"bye\n"]);
        assert_eq!(joined_output(&evs), "hello\nbye\n");
        assert!(evs.contains(&ReplEvent::Value { id: 3, text: "42".into() }));
    }

    #[test]
    fn assembler_handles_split_chunks_and_partial_lines() {
        let evs = collect(&[b"pro", b"mpt> ", b"x\n\x01do", b"ne\t5\tPKG\n"]);
        assert_eq!(joined_output(&evs), "prompt> x\n");
        assert!(evs.contains(&ReplEvent::Done { id: 5, package: "PKG".into() }));
    }

    #[test]
    fn assembler_handles_split_utf8() {
        let s = "жук\n".as_bytes();
        let evs = collect(&[&s[..3], &s[3..]]);
        assert_eq!(joined_output(&evs), "жук\n");
    }

    #[test]
    fn assembler_handles_crlf() {
        let evs = collect(&[b"a\r\n\x01reply\t1\tx\r\n"]);
        assert_eq!(joined_output(&evs), "a\n");
        assert!(evs.contains(&ReplEvent::Reply { id: 1, payload: "x".into() }));
    }

    fn session() -> Option<ReplSession> {
        let imp = crate::discover::find_sbcl("")?;
        let s = ReplSession::start(&imp, None).expect("spawn");
        s.wait_ready(Duration::from_secs(30)).expect("ready");
        Some(s)
    }

    #[test]
    fn real_sbcl_evaluates_and_keeps_state() {
        let Some(s) = session() else { return };
        let t = Duration::from_secs(20);
        let r = s.eval_sync("(+ 1 2)", t).unwrap();
        assert_eq!(r.values.as_deref(), Some("3"));
        assert_eq!(r.package, "COMMON-LISP-USER");
        s.eval_sync("(defvar *x* 41)", t).unwrap();
        let r = s.eval_sync("(1+ *x*)", t).unwrap();
        assert_eq!(r.values.as_deref(), Some("42"));
        let r = s.eval_sync("(floor 7 2)", t).unwrap();
        assert_eq!(r.values.as_deref(), Some("3\n1"), "multiple values");
        let r = s.eval_sync("(defpackage :foo (:use :cl)) (in-package :foo)", t).unwrap();
        assert_eq!(r.package, "FOO", "package switch persists");
        let r = s.eval_sync("(format nil \"~a\" *package*)", t).unwrap();
        assert!(r.values.unwrap().contains("FOO"));
    }

    #[test]
    fn real_sbcl_captures_output_errors_and_survives() {
        let Some(s) = session() else { return };
        let t = Duration::from_secs(20);
        let r = s.eval_sync("(progn (format t \"line1~%no-newline\") 7)", t).unwrap();
        assert_eq!(r.output, "line1\nno-newline\n", "fresh-line before protocol output");
        assert_eq!(r.values.as_deref(), Some("7"));
        let r = s.eval_sync("(/ 1 0)", t).unwrap();
        assert!(r.error.as_deref().unwrap().to_lowercase().contains("division"), "{:?}", r.error);
        assert!(r.trace.is_some());
        let r = s.eval_sync("(error \"custom ~a\" 5)", t).unwrap();
        assert_eq!(r.error.as_deref(), Some("custom 5"));
        let r = s.eval_sync("(car 5)", t).unwrap();
        assert!(r.error.is_some());
        let r = s.eval_sync("(+ 1 ", t).unwrap();
        assert!(r.error.is_some(), "reader error is reported, not a hang");
        let r = s.eval_sync("(warn \"careful\")", t).unwrap();
        assert!(r.output.contains("careful"), "{:?}", r.output);
        let r = s.eval_sync("(+ 20 22)", t).unwrap();
        assert_eq!(r.values.as_deref(), Some("42"), "session still healthy after errors");
    }

    #[test]
    fn real_sbcl_unicode_roundtrip() {
        let Some(s) = session() else { return };
        let t = Duration::from_secs(20);
        let r = s.eval_sync("(progn (princ \"Привет, мир! λ 日本\") (length \"жук\"))", t).unwrap();
        assert!(r.output.contains("Привет, мир! λ 日本"), "{:?}", r.output);
        assert_eq!(r.values.as_deref(), Some("3"));
        let r = s.eval_sync("\"ёж\"", t).unwrap();
        assert_eq!(r.values.as_deref(), Some("\"ёж\""));
    }

    #[test]
    fn real_sbcl_introspection() {
        let Some(s) = session() else { return };
        let t = Duration::from_secs(20);
        s.eval_sync("(defun my-add (a b &optional c) \"Adds.\" (+ a b (or c 0)))", t).unwrap();
        let c = s.query_sync("complete", "my-a", t).unwrap();
        assert!(c.lines().any(|l| l == "my-add\tf"), "{c}");
        let c = s.query_sync("complete", "defu", t).unwrap();
        assert!(c.lines().any(|l| l.starts_with("defun\t")), "{c}");
        let c = s.query_sync("complete", "cl:mapca", t).unwrap();
        assert!(c.lines().any(|l| l.starts_with("mapcar\t")), "{c}");
        let a = s.query_sync("arglist", "my-add", t).unwrap();
        assert_eq!(a, "(my-add a b &optional c)");
        let a = s.query_sync("arglist", "mapcar", t).unwrap();
        assert!(a.starts_with("(mapcar"), "{a}");
        let d = s.query_sync("doc", "my-add", t).unwrap();
        assert!(d.to_lowercase().contains("adds"), "{d}");
        let m = s.query_sync("macroexpand", "(when a b)", t).unwrap();
        assert!(m.to_lowercase().contains("if"), "{m}");
        assert_eq!(s.query_sync("ping", "", t).unwrap(), "pong");
        let ap = s.query_sync("apropos", "mapcar", t).unwrap();
        assert!(ap.contains("mapcar"));
    }

    #[test]
    fn real_sbcl_kill_and_exit_events() {
        let Some(s) = session() else { return };
        assert!(s.is_alive());
        s.kill();
        assert!(!s.is_alive());
        assert!(s.request("ping", "").is_err());
        let Some(s2) = session() else { return };
        let _ = s2.eval("(sb-ext:exit :code 3 :abort t)");
        let mut exited = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(ReplEvent::Exited(c)) = s2.recv_timeout(Duration::from_millis(200)) {
                exited = Some(c);
                break;
            }
        }
        assert_eq!(exited, Some(Some(3)));
    }

    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if src.is_dir() { copy_dir(&src, &dst) } else { std::fs::copy(&src, &dst).map(|_| ()).unwrap() }
        }
    }

    /// The distribution ships SBCL next to the app: it must run from a relocated folder using SBCL_HOME only,
    /// including `(require :asdf)` and sb-introspect from the bundled contribs.
    #[test]
    fn bundled_sbcl_runs_from_a_relocated_folder() {
        let Some(system) = crate::discover::find_sbcl("") else { return };
        let Some(install) = system.path.parent() else { return };
        if !install.join("sbcl.core").is_file() || !install.join("contrib").is_dir() {
            return; // other layouts (distro packages) cannot be relocated this way
        }
        let dir = std::env::temp_dir().join(format!("light-bundle-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(install, &dir);
        let imp = Implementation::bundled(&dir).expect("bundle layout");
        let s = ReplSession::start(&imp, None).expect("spawn bundled sbcl");
        s.wait_ready(Duration::from_secs(60)).expect("ready");
        let t = Duration::from_secs(60);
        assert_eq!(s.eval_sync("(+ 1 2)", t).unwrap().values.as_deref(), Some("3"));
        let r = s.eval_sync("(require :asdf) (asdf:asdf-version)", t).unwrap();
        assert!(r.error.is_none(), "asdf from the bundled contrib must load: {:?}", r.error);
        let a = s.query_sync("arglist", "mapcar", t).unwrap();
        assert!(a.starts_with("(mapcar"), "sb-introspect from contrib: {a}");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_executable_fails_cleanly() {
        let imp = Implementation::sbcl("Z:/no/such/sbcl.exe");
        assert!(ReplSession::start(&imp, None).is_err());
    }
}
