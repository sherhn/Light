//! Checks of a language's files by the command its module names (a syntax check): run on the saved file in the
//! background, in the project's Linux or on Windows like a run command, and shown as problems in the code.

use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ide_core::modules::CheckDef;
use ide_core::modules::check::{self, CheckProblem, Severity};
use lisp_runtime::discover::Launch;
use lisp_runtime::runner::{self, RunEvent};
use lisp_runtime::wsl;

use crate::app::App;
use crate::editor_view::{Marker, Sev};

/// The longest a check may take.
const LIMIT: Duration = Duration::from_secs(20);

pub struct CheckDone {
    pub doc: u64,
    pub rev: u64,
    pub problems: Vec<CheckProblem>,
}

pub struct CheckState {
    tx: Sender<CheckDone>,
    rx: Receiver<CheckDone>,
    /// Documents with a check running, and those whose file was saved again meanwhile.
    running: HashSet<u64>,
    again: HashSet<u64>,
}

impl Default for CheckState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        CheckState { tx, rx, running: HashSet::new(), again: HashSet::new() }
    }
}

impl CheckState {
    /// Nothing is running (the UI tests wait for this).
    pub fn idle(&self) -> bool {
        self.running.is_empty()
    }
}

impl App {
    /// The check of a language, from the modules that are on.
    pub fn check_def_for(&self, lang: &str) -> Option<CheckDef> {
        self.mods.installed.iter().filter(|m| m.enabled).flat_map(|m| m.manifest.check.iter()).find(|c| c.language == lang).cloned()
    }

    /// Check the saved file of tab `idx`, if its language has a check. The result comes back later (`poll_checks`).
    pub fn schedule_check(&mut self, idx: usize) {
        let Some(d) = self.docs.get(idx) else { return };
        let (Some(path), Some(lang)) = (d.path.clone(), d.ed.language().map(|l| l.id().to_string())) else { return };
        // the file on disk is what gets checked: wait for it to be written
        if d.diff.is_some() || d.ed.is_modified() {
            return;
        }
        let Some(def) = self.check_def_for(&lang) else { return };
        let Ok(re) = check::compile(&def.pattern) else { return };
        let id = d.id;
        let rev = d.ed.revision();
        if self.checks.running.contains(&id) {
            self.checks.again.insert(id);
            return;
        }
        let root = self.project_root().map(Path::to_path_buf).or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let target = self.wsl_target();
        let dir = path.parent().unwrap_or(Path::new("."));
        let (file_s, dir_s) = match &target {
            Some(t) => (wsl::to_linux(&path, t), wsl::to_linux(dir, t)),
            None => (path.display().to_string().replace('\\', "/"), dir.display().to_string().replace('\\', "/")),
        };
        let line = def.command_line(&file_s, &dir_s, target.is_none());
        let launch = match &target {
            Some(t) => {
                let marker = wsl::new_marker();
                let cmd = wsl::shell(t, &root, &line, &[], &marker);
                Launch { cmd, cleanup: Some(wsl::Cleanup { target: t.clone(), marker }) }
            }
            None => Launch { cmd: ide_core::cmdscript::shell_command(&line, &root, &[]), cleanup: None },
        };
        self.checks.running.insert(id);
        let tx = self.checks.tx.clone();
        std::thread::spawn(move || {
            let mut output = String::new();
            if let Ok(h) = runner::spawn_launch(launch) {
                let start = Instant::now();
                loop {
                    match h.recv_timeout(Duration::from_millis(100)) {
                        Some(RunEvent::Output { text, .. }) => output.push_str(&text),
                        Some(RunEvent::Exited(_)) => break,
                        None if start.elapsed() > LIMIT => {
                            h.kill();
                            break;
                        }
                        None => {}
                    }
                }
            }
            let _ = tx.send(CheckDone { doc: id, rev, problems: check::problems(&re, &output, 200) });
        });
    }

    /// Take the results of the checks that finished.
    pub fn poll_checks(&mut self) {
        while let Ok(done) = self.checks.rx.try_recv() {
            self.checks.running.remove(&done.doc);
            let Some(i) = self.docs.iter().position(|d| d.id == done.doc) else { continue };
            if self.checks.again.remove(&done.doc) {
                // the file was saved again while this ran: its result is old
                self.schedule_check(i);
            }
            let d = &mut self.docs[i];
            if d.ed.revision() != done.rev {
                continue;
            }
            let text_len = d.ed.text().len();
            d.check_markers = done
                .problems
                .iter()
                .map(|p| {
                    let line = (p.line - 1).min(d.ed.line_count().saturating_sub(1));
                    let ls = d.ed.line_start(line);
                    let text = d.ed.line_text(line);
                    let (start, end) = match p.col {
                        // the column the tool names, up to the end of the word there
                        Some(c) => {
                            let s = d.ed.offset_of(line, c - 1).min(text_len);
                            let e = d.ed.text()[s..].find(|ch: char| !(ch.is_alphanumeric() || ch == '_')).map_or(s + 1, |n| s + n.max(1)).min(ls + text.len()).max(s);
                            (s, e)
                        }
                        // the whole line, from its first character that is not a blank
                        None => (ls + (text.len() - text.trim_start().len()), ls + text.trim_end().len()),
                    };
                    Marker { start, end: end.max(start), sev: if p.severity == Severity::Error { Sev::Error } else { Sev::Warning }, msg: p.message.clone() }
                })
                .collect();
            d.check_rev = done.rev;
        }
    }

    /// Look at every open file again (the modules changed).
    pub fn recheck_all(&mut self) {
        for i in 0..self.docs.len() {
            self.schedule_check(i);
        }
    }
}
