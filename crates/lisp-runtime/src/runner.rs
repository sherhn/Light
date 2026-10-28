//! Running files and one-off expressions as separate processes with streamed output.

use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::discover::{Arg, Implementation, Launch};
use crate::wsl::Cleanup;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEvent {
    Output { text: String, is_err: bool },
    Exited(Option<i32>),
}

pub struct RunHandle {
    rx: Receiver<RunEvent>,
    child: Arc<Mutex<Option<Child>>>,
    cleanup: Option<Cleanup>,
}

impl RunHandle {
    pub fn try_recv(&self) -> Option<RunEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, d: Duration) -> Option<RunEvent> {
        self.rx.recv_timeout(d).ok()
    }

    pub fn is_running(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    pub fn kill(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            // on Windows `kill` ends only the process itself: a `cmd` would leave what it started running
            kill_tree(c.id());
            let _ = c.kill();
            let _ = c.wait();
            if let Some(cl) = &self.cleanup {
                cl.run();
            }
        }
    }

    /// Block until the process exits; returns (merged output, exit code). `None` code means killed/timeout.
    pub fn wait_output(&self, timeout: Duration) -> (String, Option<i32>, bool) {
        let deadline = Instant::now() + timeout;
        let mut out = String::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.kill();
                return (out, None, true);
            }
            match self.recv_timeout(left) {
                Some(RunEvent::Output { text, .. }) => out.push_str(&text),
                Some(RunEvent::Exited(code)) => {
                    // drain anything still queued
                    while let Some(RunEvent::Output { text, .. }) = self.try_recv() {
                        out.push_str(&text);
                    }
                    return (out, code, false);
                }
                None => {}
            }
        }
    }
}

impl Drop for RunHandle {
    fn drop(&mut self) {
        self.kill();
    }
}

/// UTF-8 as is; anything else is console output in the system's OEM code page.
/// End a process and everything it started (Windows: `taskkill /T`; elsewhere the caller's `kill` has to do).
#[cfg(windows)]
pub fn kill_tree(pid: u32) {
    let mut c = Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    crate::no_window(&mut c);
    // waited for: the children are found through their parent, which must still be there when taskkill looks
    let _ = c.status();
}

#[cfg(not(windows))]
pub fn kill_tree(_pid: u32) {}

fn decode_chunk(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => crate::oem::decode_non_utf8(bytes),
    }
}

pub fn spawn_streaming(mut cmd: Command) -> io::Result<RunHandle> {
    crate::no_window(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let (tx, rx) = mpsc::channel();
    let mut readers = Vec::new();
    for (mut stream, is_err) in [(Box::new(stdout) as Box<dyn Read + Send>, false), (Box::new(stderr) as Box<dyn Read + Send>, true)] {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut tail: Vec<u8> = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        tail.extend_from_slice(&buf[..n]);
                        let valid = match std::str::from_utf8(&tail) {
                            Ok(_) => tail.len(),
                            Err(e) if e.error_len().is_none() => e.valid_up_to(),
                            Err(_) => tail.len(),
                        };
                        let rest = tail.split_off(valid);
                        let text = crate::wsl::strip_notices(&decode_chunk(&tail));
                        tail = rest;
                        if !text.is_empty() {
                            let _ = tx.send(RunEvent::Output { text, is_err });
                        }
                    }
                }
            }
            if !tail.is_empty() {
                let text = crate::wsl::strip_notices(&decode_chunk(&tail));
                if !text.is_empty() {
                    let _ = tx.send(RunEvent::Output { text, is_err });
                }
            }
        }));
    }
    let child = Arc::new(Mutex::new(Some(child)));
    let waiter = child.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(30));
            let mut g = waiter.lock().unwrap();
            match g.as_mut() {
                None => return,
                Some(c) => {
                    if let Ok(Some(status)) = c.try_wait() {
                        *g = None;
                        drop(g);
                        for r in readers {
                            let _ = r.join();
                        }
                        let _ = tx.send(RunEvent::Exited(status.code()));
                        return;
                    }
                }
            }
        }
    });
    Ok(RunHandle { rx, child, cleanup: None })
}

/// Spawn a [`Launch`]; killing the handle also ends what it started inside WSL.
pub fn spawn_launch(l: Launch) -> io::Result<RunHandle> {
    let mut h = spawn_streaming(l.cmd)?;
    h.cleanup = l.cleanup;
    Ok(h)
}

/// Lisp string literal for `s`.
pub fn lisp_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        if c == '"' || c == '\\' {
            o.push('\\');
        }
        o.push(c);
    }
    o.push('"');
    o
}

pub fn lisp_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// A batch SBCL process with `extra` arguments after the usual non-interactive options.
fn batch(imp: &Implementation, cwd: Option<&Path>, extra: &[Arg]) -> io::Result<RunHandle> {
    let mut args = vec![Arg::Text("--noinform"), Arg::Text("--non-interactive"), Arg::Text("--no-userinit"), Arg::Text("--no-sysinit")];
    args.extend_from_slice(extra);
    spawn_launch(imp.launch(cwd, &args))
}

/// `sbcl --load FILE`; the working directory defaults to the file's folder.
pub fn run_file(imp: &Implementation, file: &Path, cwd: Option<&Path>) -> io::Result<RunHandle> {
    let dir = cwd.or_else(|| file.parent().filter(|p| !p.as_os_str().is_empty()));
    batch(imp, dir, &[Arg::Text("--load"), Arg::Path(file)])
}

/// Evaluate forms in a fresh process.
pub fn run_eval(imp: &Implementation, forms: &str, cwd: Option<&Path>) -> io::Result<RunHandle> {
    batch(imp, cwd, &[Arg::Text("--eval"), Arg::Text(forms)])
}

/// Load or test an ASDF system found under `root` in a fresh process.
pub fn run_asdf(imp: &Implementation, root: &Path, system: &str, test: bool) -> io::Result<RunHandle> {
    let op = if test { "test" } else { "load" };
    // `(require :asdf)` must be evaluated before the reader sees `asdf:` symbols, hence separate --eval forms.
    let setup = format!("(asdf:initialize-source-registry (list :source-registry (list :tree {}) :inherit-configuration))", lisp_string(&imp.lisp_path(root)));
    let work = format!("(progn (asdf:{op}-system {sys}) (format t \"~&; ASDF system ~a: {op} finished~%\" {sys}))", sys = lisp_string(system));
    batch(imp, Some(root), &[Arg::Text("--eval"), Arg::Text("(require :asdf)"), Arg::Text("--eval"), Arg::Text(&setup), Arg::Text("--eval"), Arg::Text(&work)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lisp_string_escaping() {
        assert_eq!(lisp_string(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(lisp_path(Path::new(r"C:\a\b.lisp")), "C:/a/b.lisp");
    }

    fn sbcl() -> Option<Implementation> {
        crate::discover::find_sbcl("")
    }

    fn tmp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("light-runner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn runs_a_file_and_streams_output() {
        let Some(imp) = sbcl() else { return };
        let f = tmp("hello.lisp", "(format t \"hello ~a~%\" (+ 1 2))\n(format t \"Привет, мир!~%\")\n(sb-ext:exit :code 5)\n");
        let h = run_file(&imp, &f, None).unwrap();
        let (out, code, timed_out) = h.wait_output(Duration::from_secs(30));
        assert!(!timed_out);
        assert_eq!(code, Some(5));
        assert!(out.contains("hello 3"), "{out:?}");
        assert!(out.contains("Привет, мир!"), "unicode output: {out:?}");
        assert!(!h.is_running());
    }

    #[test]
    fn reports_runtime_errors_with_nonzero_exit() {
        let Some(imp) = sbcl() else { return };
        let f = tmp("boom.lisp", "(error \"boom ~a\" 1)\n");
        let (out, code, _) = run_file(&imp, &f, None).unwrap().wait_output(Duration::from_secs(30));
        assert_ne!(code, Some(0));
        assert!(out.contains("boom 1"), "{out:?}");
    }

    #[test]
    fn loads_and_tests_an_asdf_system() {
        let Some(imp) = sbcl() else { return };
        let dir = std::env::temp_dir().join(format!("light-asdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mini.asd"), "(asdf:defsystem \"mini\" :components ((:file \"mini\")) :perform (test-op (o c) (format t \"MINI-TESTS-RAN~%\")))
").unwrap();
        std::fs::write(dir.join("mini.lisp"), "(format t \"MINI-LOADED~%\")
").unwrap();
        let (out, code, _) = run_asdf(&imp, &dir, "mini", false).unwrap().wait_output(Duration::from_secs(60));
        assert_eq!(code, Some(0), "{out}");
        assert!(out.contains("MINI-LOADED") && out.contains("load finished"), "{out}");
        let (out, code, _) = run_asdf(&imp, &dir, "mini", true).unwrap().wait_output(Duration::from_secs(60));
        assert_eq!(code, Some(0), "{out}");
        assert!(out.contains("MINI-TESTS-RAN"), "{out}");
        let (out, code, _) = run_asdf(&imp, &dir, "nope", false).unwrap().wait_output(Duration::from_secs(60));
        assert_ne!(code, Some(0), "unknown system must fail: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn can_kill_a_running_process() {
        let Some(imp) = sbcl() else { return };
        let f = tmp("loop.lisp", "(loop (sleep 1))\n");
        let h = run_file(&imp, &f, None).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert!(h.is_running());
        h.kill();
        assert!(!h.is_running());
    }

    #[test]
    fn reads_script_relative_to_its_directory() {
        let Some(imp) = sbcl() else { return };
        let _ = tmp("data.txt", "payload-123");
        let f = tmp("rel.lisp", "(with-open-file (s \"data.txt\") (write-line (read-line s)))\n");
        let (out, code, _) = run_file(&imp, &f, None).unwrap().wait_output(Duration::from_secs(30));
        assert_eq!(code, Some(0), "{out}");
        assert!(out.contains("payload-123"));
    }
}
