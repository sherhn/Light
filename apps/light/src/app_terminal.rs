//! A line-oriented terminal: each command runs in the platform shell with streamed output. `cd`, `set`, `clear`
//! and `exit` are handled internally so the working directory and variables persist between commands.
//!
//! Command scripts: `run <file>` (or the Scripts menu / "Run script" button) executes a text file with one command
//! per line (see `ide_core::cmdscript`), or a native `.bat` / `.cmd` / `.ps1` / `.sh` file with its interpreter.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use ide_core::cmdscript::{self, Builtin, Step};
use lisp_runtime::runner::{self, RunEvent, RunHandle};
use lisp_runtime::{Launch, wsl};

use crate::app::App;
use crate::i18n::{self, tr};
use crate::state::*;

/// A command script that is being executed step by step.
pub struct ScriptRun {
    pub name: String,
    pub steps: VecDeque<Step>,
    pub total: usize,
    pub done: usize,
    /// The step whose process is running right now.
    pub current: Option<Step>,
}

pub struct TerminalState {
    pub cwd: PathBuf,
    pub log: Vec<LogLine>,
    pub input: String,
    pub history: Vec<String>,
    pub hist_pos: Option<usize>,
    pub handle: Option<RunHandle>,
    pub focus_input: bool,
    /// Variables set with `set NAME=value`, passed to every command.
    pub env: Vec<(String, String)>,
    pub script: Option<ScriptRun>,
}

impl Default for TerminalState {
    fn default() -> Self {
        TerminalState { cwd: PathBuf::from("."), log: vec![], input: String::new(), history: vec![], hist_pos: None, handle: None, focus_input: false, env: vec![], script: None }
    }
}

/// Resolve `cd <arg>` relative to `cwd`.
pub use ide_core::cmdscript::change_dir;

/// Largest script file that is read (a command script is a few lines; this only guards against a wrong pick).
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

impl App {
    pub fn terminal_start_dir(&mut self) {
        if self.term.cwd == Path::new(".")
            && let Some(r) = self.project_root()
        {
            self.term.cwd = r.to_path_buf();
        }
        if self.term.log.is_empty() {
            self.term_info(tr("Terminal - commands run in your system shell. 'cd' changes the working directory. Type 'help' for scripts."));
        }
    }

    fn term_push(&mut self, kind: LogKind, text: impl Into<String>) {
        self.term.log.push(LogLine { kind, text: text.into() });
    }

    fn term_info(&mut self, text: impl Into<String>) {
        self.term_push(LogKind::Info, text);
    }

    fn term_error(&mut self, text: impl Into<String>) {
        self.term_push(LogKind::Error, text);
    }

    fn term_echo(&mut self, text: &str) {
        // in WSL the prompt shows the Linux side of the directory, shell style
        let line = match self.wsl_target() {
            Some(t) => format!("{}:{}$ {text}", t.distro, wsl::to_linux(&self.term.cwd, &t)),
            None => format!("{}> {text}", self.term.cwd.display()),
        };
        self.term_push(LogKind::Input, line);
    }

    /// Is a command or a script running?
    pub fn term_busy(&self) -> bool {
        self.term.handle.is_some() || self.term.script.is_some()
    }

    // ------------------------------------------------------------------ interactive input

    pub fn terminal_run(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.terminal_start_dir();
        self.term_echo(line);
        if self.term.history.last().map(String::as_str) != Some(line) {
            self.term.history.push(line.to_string());
        }
        self.term.hist_pos = None;
        if self.term_busy() && cmdscript::builtin(line).is_none() {
            self.term_error(tr("A command is still running (press Stop)."));
            return;
        }
        if self.terminal_special(line) {
            return;
        }
        match self.exec_step(line) {
            StepResult::Spawned | StepResult::Done => {}
            StepResult::Failed(e) => self.term_error(e),
            StepResult::Exit(_) => {}
        }
    }

    /// `help`, `scripts` and the script runners. Returns true when `line` was one of them.
    fn terminal_special(&mut self, line: &str) -> bool {
        let lower = line.to_lowercase();
        if lower == "help" || lower == "?" {
            for l in [
                "Terminal commands",
                "  cd <dir>             change the working directory",
                "  set NAME=value       set a variable for the following commands",
                "  clear                clear the output",
                "  run <file>           run a command script (one command per line) or a .bat/.cmd/.ps1/.sh file",
                "  scripts              list the scripts of the project",
                "Script syntax: '#' comment, '-cmd' ignores a failing exit code,",
                "  '@cmd' is not echoed, 'exit [n]' stops the script.",
            ] {
                self.term_info(tr(l));
            }
            return true;
        }
        if lower == "scripts" {
            let list = self.project_scripts();
            if list.is_empty() {
                self.term_info(tr("No scripts found in the project (.als, .bat, .cmd, .ps1, .sh)."));
            }
            for (rel, _) in list {
                self.term_info(format!("  run {rel}"));
            }
            return true;
        }
        let (word, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(w, r)| (w, r.trim()));
        let word_l = word.to_lowercase();
        if matches!(word_l.as_str(), "run" | "source" | "script" | ".") && !rest.is_empty() {
            let arg = rest.trim_matches('"');
            let p = Path::new(arg);
            let path = if p.is_absolute() { p.to_path_buf() } else { self.term.cwd.join(p) };
            if path.is_file() {
                self.terminal_run_script(&path);
                return true;
            }
            if word_l != "run" {
                self.term_error(i18n::fmt("Script not found: {}", &[&arg]));
                return true;
            }
        }
        false
    }

    // ------------------------------------------------------------------ steps

    /// Execute one command line: built-ins immediately, anything else as a shell process.
    fn exec_step(&mut self, line: &str) -> StepResult {
        match cmdscript::builtin(line) {
            Some(Builtin::Clear) => {
                self.term.log.clear();
                StepResult::Done
            }
            Some(Builtin::Cd(arg)) => match self.terminal_cd(&arg) {
                Ok(p) => {
                    self.term.cwd = p;
                    StepResult::Done
                }
                Err(e) => StepResult::Failed(e),
            },
            Some(Builtin::Set(name, value)) => {
                match self.term.env.iter_mut().find(|(k, _)| *k == name) {
                    Some(e) => e.1 = value,
                    None => self.term.env.push((name, value)),
                }
                StepResult::Done
            }
            Some(Builtin::Exit(code)) => StepResult::Exit(code),
            None => match self.wsl_target() {
                // the distribution's login shell, tagged so Stop can end it inside Linux too
                Some(t) => {
                    let marker = wsl::new_marker();
                    let cmd = wsl::shell(&t, &self.term.cwd, line, &self.term.env, &marker);
                    self.spawn_launch(Launch { cmd, cleanup: Some(wsl::Cleanup { target: t, marker }) })
                }
                None => {
                    let cmd = cmdscript::shell_command(line, &self.term.cwd, &self.term.env);
                    self.spawn(cmd)
                }
            },
        }
    }

    /// `cd`: Windows paths always; in WSL also Linux ones (`/home/me`, `/mnt/c/x`).
    fn terminal_cd(&self, arg: &str) -> Result<PathBuf, String> {
        match self.wsl_target() {
            Some(t) if arg.trim().starts_with('/') => change_dir(&self.term.cwd, &wsl::to_windows(arg.trim(), &t).to_string_lossy()),
            _ => change_dir(&self.term.cwd, arg),
        }
    }

    fn spawn(&mut self, cmd: std::process::Command) -> StepResult {
        self.spawn_launch(Launch { cmd, cleanup: None })
    }

    fn spawn_launch(&mut self, l: Launch) -> StepResult {
        match runner::spawn_launch(l) {
            Ok(h) => {
                self.term.handle = Some(h);
                StepResult::Spawned
            }
            Err(e) => StepResult::Failed(i18n::fmt("cannot run: {}", &[&e])),
        }
    }

    // ------------------------------------------------------------------ scripts

    /// Scripts of the project: (relative path, absolute path), sorted.
    pub fn project_scripts(&self) -> Vec<(String, PathBuf)> {
        let Some(p) = &self.project else { return vec![] };
        let mut v: Vec<(String, PathBuf)> = p.files.iter().zip(p.files_rel.iter()).filter(|(f, _)| cmdscript::is_script_file(f)).map(|(f, r)| (r.clone(), f.clone())).collect();
        v.sort();
        v.truncate(60);
        v
    }

    /// Pick a script file and run it.
    pub fn terminal_pick_script(&mut self) {
        self.terminal_start_dir();
        let mut dlg = rfd::FileDialog::new().set_title(tr("Run script")).add_filter(tr("Scripts"), &["als", "bat", "cmd", "ps1", "sh", "txt"]).add_filter(tr("All files"), &["*"]);
        if self.term.cwd != Path::new(".") {
            dlg = dlg.set_directory(&self.term.cwd);
        }
        if let Some(f) = dlg.pick_file() {
            self.terminal_run_script(&f);
        }
    }

    /// Run a script file in the terminal.
    pub fn terminal_run_script(&mut self, path: &Path) {
        self.terminal_start_dir();
        self.bottom_tool = Some(BottomTool::Terminal);
        if self.term_busy() {
            self.term_error(tr("A command is still running (press Stop)."));
            return;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        // native scripts run in their own interpreter as one process
        // in WSL a shell script runs in Linux; .bat/.cmd/.ps1 stay Windows programs
        if let Some(t) = self.wsl_target()
            && path.extension().is_some_and(|x| x.eq_ignore_ascii_case("sh"))
        {
            self.term_echo(&format!("run {}", path.display()));
            let marker = wsl::new_marker();
            let cmd = wsl::shell(&t, &self.term.cwd, &format!("sh {}", wsl::sh_quote(&wsl::to_linux(path, &t))), &self.term.env, &marker);
            if let StepResult::Failed(e) = self.spawn_launch(Launch { cmd, cleanup: Some(wsl::Cleanup { target: t, marker }) }) {
                self.term_error(e);
            }
            return;
        }
        if cmdscript::NATIVE_EXTENSIONS.iter().any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e))) {
            match cmdscript::native_command(path, &self.term.cwd, &self.term.env) {
                Some(cmd) => {
                    self.term_echo(&format!("run {}", path.display()));
                    if let StepResult::Failed(e) = self.spawn(cmd) {
                        self.term_error(e);
                    }
                }
                None => self.term_error(i18n::fmt("Cannot run {} on this system", &[&name])),
            }
            return;
        }
        if std::fs::metadata(path).map(|m| m.len() > MAX_SCRIPT_BYTES).unwrap_or(false) {
            self.term_error(i18n::fmt("{} is too large for a command script", &[&name]));
            return;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.term_error(i18n::fmt("Cannot read {0}: {1}", &[&name, &e]));
                return;
            }
        };
        let steps = cmdscript::parse(&text);
        if steps.is_empty() {
            self.term_info(i18n::fmt("{} contains no commands", &[&name]));
            return;
        }
        self.term_info(i18n::fmt("Running script {0} ({1} commands)", &[&name, &steps.len()]));
        let total = steps.len();
        self.term.script = Some(ScriptRun { name, steps: steps.into(), total, done: 0, current: None });
        self.script_advance();
    }

    /// Run the following steps of the script until one starts a process (or the script ends).
    fn script_advance(&mut self) {
        loop {
            let Some(run) = self.term.script.as_mut() else { return };
            let Some(step) = run.steps.pop_front() else {
                let (name, n) = (run.name.clone(), run.total);
                self.term.script = None;
                self.term_info(i18n::fmt("Script {0} finished ({1} commands)", &[&name, &n]));
                return;
            };
            run.done += 1;
            if !step.silent {
                self.term_echo(&step.text);
            }
            match self.exec_step(&step.text) {
                StepResult::Done => {}
                StepResult::Spawned => {
                    if let Some(run) = self.term.script.as_mut() {
                        run.current = Some(step);
                    }
                    return;
                }
                StepResult::Exit(code) => {
                    let name = self.term.script.take().map(|r| r.name).unwrap_or_default();
                    self.term_info(i18n::fmt("Script {0} ended by 'exit' (code {1})", &[&name, &code]));
                    return;
                }
                StepResult::Failed(e) => {
                    self.term_error(e);
                    if !step.ignore_errors {
                        self.script_abort(&step, None);
                        return;
                    }
                }
            }
        }
    }

    fn script_abort(&mut self, step: &Step, code: Option<i32>) {
        let name = self.term.script.take().map(|r| r.name).unwrap_or_default();
        let msg = match code {
            Some(c) => i18n::fmt("Script {0} stopped: the command on line {1} failed (exit code {2})", &[&name, &step.line, &c]),
            None => i18n::fmt("Script {0} stopped: the command on line {1} failed", &[&name, &step.line]),
        };
        self.term_error(msg);
    }

    /// Ctrl+C: end the running command (or script) and everything it started, as a terminal does.
    pub fn terminal_interrupt(&mut self) {
        if self.term_busy() {
            self.term_info("^C");
            self.terminal_stop();
        }
    }

    pub fn terminal_stop(&mut self) {
        let had = self.term.handle.is_some();
        if let Some(h) = self.term.handle.take() {
            h.kill();
        }
        if let Some(run) = self.term.script.take() {
            self.term_info(i18n::fmt("Script {} aborted", &[&run.name]));
        } else if had {
            self.term_info(tr("Process terminated"));
        }
    }

    pub fn poll_terminal(&mut self) {
        let mut exited: Option<Option<i32>> = None;
        let mut lines: Vec<(String, bool)> = vec![];
        if let Some(h) = &self.term.handle {
            while let Some(ev) = h.try_recv() {
                match ev {
                    RunEvent::Output { text, is_err } => lines.push((text, is_err)),
                    RunEvent::Exited(code) => {
                        exited = Some(code);
                        if code != Some(0) {
                            lines.push((format!("[exit code {}]\n", code.map_or("?".into(), |c| c.to_string())), true));
                        }
                    }
                }
            }
        }
        for (text, is_err) in lines {
            let text = text.replace('\r', "");
            let kind = if is_err { LogKind::Error } else { LogKind::Output };
            for seg in text.split_inclusive('\n') {
                match self.term.log.last_mut() {
                    Some(l) if l.kind == kind && !l.text.ends_with('\n') => l.text.push_str(seg),
                    _ => self.term.log.push(LogLine { kind, text: seg.to_string() }),
                }
            }
        }
        if let Some(code) = exited {
            self.term.handle = None;
            if code == Some(127) {
                // "not found": in Linux that is usually a build tool that was never installed
                let program = self.term.history.last().and_then(|l| l.split_whitespace().find(|w| *w != "sudo")).map(str::to_string);
                if let Some(hint) = self.linux_command_not_found(program.as_deref()) {
                    self.term_info(hint);
                }
            }
            self.git.last_refresh = -100.0; // commands may have changed the repository
            let finished = self.term.script.as_mut().and_then(|r| r.current.take());
            if let Some(step) = finished {
                if code != Some(0) && !step.ignore_errors {
                    self.script_abort(&step, code);
                } else {
                    self.script_advance();
                }
            }
        }
        if self.term.log.len() > 5000 {
            self.term.log.drain(..1000);
        }
    }
}

/// Outcome of running one command line.
pub enum StepResult {
    /// Handled internally (cd, set, clear) or nothing to wait for.
    Done,
    /// A process was started; its exit is reported by `poll_terminal`.
    Spawned,
    Failed(String),
    /// `exit [code]`.
    Exit(i32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_core::settings::Settings;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("light-term-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pump(app: &mut App) {
        let t = std::time::Instant::now();
        while app.term_busy() {
            app.poll_terminal();
            assert!(t.elapsed().as_secs() < 60, "script did not finish: {:?}", app.term.log.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    fn log_text(app: &App) -> String {
        app.term.log.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("")
    }

    fn app_in(dir: &Path) -> App {
        let mut app = App::new(Settings::default());
        app.term.cwd = dir.to_path_buf();
        app
    }

    #[test]
    fn runs_a_command_script_with_cd_set_and_ignored_errors() {
        let dir = temp("ok");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let var = if cfg!(windows) { "echo %GREETING%" } else { "echo $GREETING" };
        std::fs::write(dir.join("s.als"), format!("# demo\necho first\ncd sub\nset GREETING=hello-script\n{var}\n-git definitely-not-a-command\n@echo quiet-line\necho last\n")).unwrap();
        let mut app = app_in(&dir);
        app.terminal_run("run s.als");
        pump(&mut app);
        let out = log_text(&app);
        for want in ["first", "hello-script", "quiet-line", "last", "finished (7 commands)"] {
            assert!(out.contains(want), "missing {want:?} in {out:?}");
        }
        assert!(app.term.cwd.ends_with("sub"), "cd persists after the script: {:?}", app.term.cwd);
        assert!(!out.contains("@echo"), "'@' lines are not echoed");
        assert!(app.term.script.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_command_stops_the_script() {
        let dir = temp("fail");
        std::fs::write(dir.join("f.als"), "echo before\ngit definitely-not-a-command\necho must-not-run\n").unwrap();
        let mut app = app_in(&dir);
        app.terminal_run("script f.als");
        pump(&mut app);
        let out = log_text(&app);
        assert!(out.contains("before"), "{out:?}");
        assert!(out.contains("stopped"), "{out:?}");
        assert!(!out.contains("must-not-run"), "{out:?}");
        assert!(app.term.script.is_none() && app.term.handle.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_ends_the_script_and_a_bad_cd_stops_it() {
        let dir = temp("exit");
        std::fs::write(dir.join("e.als"), "echo one\nexit 0\necho two\n").unwrap();
        std::fs::write(dir.join("c.als"), "cd no-such-dir\necho three\n").unwrap();
        let mut app = app_in(&dir);
        app.terminal_run("run e.als");
        pump(&mut app);
        let out = log_text(&app);
        assert!(out.contains("one") && !out.contains("two") && out.contains("ended by 'exit'"), "{out:?}");
        app.terminal_run("run c.als");
        pump(&mut app);
        let out = log_text(&app);
        assert!(!out.contains("three") && out.contains("stopped"), "{out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_scripts_run_in_their_interpreter() {
        let dir = temp("native");
        let (name, body) = if cfg!(windows) { ("n.bat", "@echo off\r\necho native-script-ok\r\n") } else { ("n.sh", "echo native-script-ok\n") };
        std::fs::write(dir.join(name), body).unwrap();
        let mut app = app_in(&dir);
        app.terminal_run(&format!("run {name}"));
        pump(&mut app);
        assert!(log_text(&app).contains("native-script-ok"), "{:?}", log_text(&app));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_aborts_a_running_script_and_busy_blocks_new_commands() {
        let dir = temp("stop");
        let sleeper = if cfg!(windows) { "ping -n 20 127.0.0.1 >nul" } else { "sleep 20" };
        std::fs::write(dir.join("w.als"), format!("{sleeper}\necho after-sleep\n")).unwrap();
        let mut app = app_in(&dir);
        app.terminal_run("run w.als");
        assert!(app.term_busy());
        app.terminal_run("echo blocked");
        assert!(log_text(&app).contains("still running"), "{:?}", log_text(&app));
        app.terminal_stop();
        assert!(!app.term_busy());
        assert!(log_text(&app).contains("aborted"));
        assert!(!log_text(&app).contains("after-sleep"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn help_scripts_and_missing_files() {
        let dir = temp("help");
        let mut app = app_in(&dir);
        app.terminal_run("help");
        assert!(log_text(&app).contains("run <file>"));
        app.terminal_run("script nope.als");
        assert!(log_text(&app).contains("Script not found"));
        app.terminal_run("scripts");
        assert!(log_text(&app).contains("No scripts found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cd_resolves_relative_and_rejects_missing() {
        let base = temp("cd");
        std::fs::create_dir_all(base.join("sub")).unwrap();
        let base = change_dir(&base, ".").unwrap();
        let sub = change_dir(&base, "sub").unwrap();
        assert!(sub.ends_with("sub"));
        assert_eq!(change_dir(&sub, "..").unwrap(), base);
        assert!(change_dir(&base, "does-not-exist").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
