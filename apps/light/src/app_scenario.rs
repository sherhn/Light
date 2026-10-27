//! Run scenarios: a named list of console commands of a project that run one after another, in the project's
//! folder - in its Linux or on Windows - with their output in the Run window. They are kept in `.light/project.json`
//! and offered next to the files, make targets and ASDF systems in the run configuration picker.

use std::path::Path;

use lisp_runtime::discover::Launch;
use lisp_runtime::runner;
use lisp_runtime::wsl;

use crate::app::App;
use crate::app_env::Scenario;
use crate::app_run::{MakeRun, RunConfig};
use crate::editor_view::Sev;
use crate::i18n::{self, tr};
use crate::state::*;

/// A scenario that is running: what is left to run.
pub struct ScenarioRun {
    pub name: String,
    pub commands: Vec<String>,
    /// Index of the next command to start.
    pub next: usize,
    pub stop_on_error: bool,
}

/// The commands of `text`, one per line: blank lines and lines that start with `#` are left out.
pub fn commands_of(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string).collect()
}

impl App {
    pub fn scenarios(&self) -> &[Scenario] {
        &self.project_cfg.scenarios
    }

    /// Keep the scenarios of the project (`.light/project.json`).
    pub fn save_scenarios(&mut self, list: Vec<Scenario>) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else { return };
        self.project_cfg.scenarios = list;
        if let Err(e) = crate::app_env::save_config(&root, &self.project_cfg) {
            self.toast(i18n::fmt("Cannot save the project settings: {}", &[&e]), Sev::Error);
        }
        // a scenario that is gone cannot stay selected
        if let RunConfig::Scenario(n) = &self.run_config
            && !self.project_cfg.scenarios.iter().any(|s| &s.name == n)
        {
            self.run_config = RunConfig::CurrentFile;
        }
    }

    /// Start the scenario `name`: its first command now, the others as each one ends.
    pub fn run_scenario(&mut self, name: &str) {
        let Some(sc) = self.project_cfg.scenarios.iter().find(|s| s.name == name).cloned() else {
            self.toast(i18n::fmt("There is no scenario {}", &[&name]), Sev::Weak);
            return;
        };
        if self.project.is_none() {
            self.toast(tr("Open a project first"), Sev::Weak);
            return;
        }
        let commands = commands_of(&sc.commands.join("\n"));
        if commands.is_empty() {
            self.toast(i18n::fmt("The scenario {} has no commands", &[&name]), Sev::Weak);
            return;
        }
        self.save_all();
        self.prepare_run(format!("scenario:{name}"));
        self.run_request = Some(crate::app_run::RunRequest::Scenario(name.to_string()));
        self.run = RunState { handle: None, title: name.to_string(), log: vec![], running: true, exit_code: None };
        self.bottom_tool = Some(BottomTool::Run);
        self.scenario = Some(ScenarioRun { name: name.to_string(), commands, next: 0, stop_on_error: sc.stop_on_error });
        self.scenario_step();
    }

    /// Replace `{file}`, `{dir}`, `{name}` and `{stem}` by what the active file is (as the shell sees it).
    fn expand_scenario_command(&self, cmd: &str) -> String {
        let target = self.wsl_target();
        let path = self.doc().filter(|d| d.diff.is_none()).and_then(|d| d.path.clone());
        let (file, dir, name, stem) = match &path {
            Some(p) => {
                let d = p.parent().unwrap_or(Path::new("."));
                let (f, dd) = match &target {
                    Some(t) => (wsl::to_linux(p, t), wsl::to_linux(d, t)),
                    None => (p.display().to_string().replace('\\', "/"), d.display().to_string().replace('\\', "/")),
                };
                (f, dd, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            }
            None => (String::new(), String::new(), String::new(), String::new()),
        };
        cmd.replace("{file}", &file).replace("{dir}", &dir).replace("{name}", &name).replace("{stem}", &stem)
    }

    /// Start the next command of the running scenario, or finish it.
    pub fn scenario_step(&mut self) {
        let Some(s) = self.scenario.as_mut() else { return };
        if s.next >= s.commands.len() {
            let name = s.name.clone();
            self.scenario = None;
            self.run.running = false;
            self.run.log.push(LogLine { kind: LogKind::Info, text: i18n::fmt("Scenario {} finished", &[&name]) });
            self.status = i18n::fmt("Scenario {} finished", &[&name]);
            return;
        }
        let (n, total, raw) = (s.next + 1, s.commands.len(), s.commands[s.next].clone());
        s.next += 1;
        let line = self.expand_scenario_command(&raw);
        let root = self.project_root().map(Path::to_path_buf).unwrap_or_default();
        let target = self.wsl_target();
        let launch = match &target {
            Some(t) => {
                let marker = wsl::new_marker();
                let cmd = wsl::shell(t, &root, &format!("{line} 2>&1"), &[], &marker);
                Launch { cmd, cleanup: Some(wsl::Cleanup { target: t.clone(), marker }) }
            }
            None => Launch { cmd: ide_core::cmdscript::shell_command(&line, &root, &[]), cleanup: None },
        };
        self.run.log.push(LogLine { kind: LogKind::Info, text: format!("[{n}/{total}] {line}") });
        match runner::spawn_launch(launch) {
            Ok(h) => {
                self.run.handle = Some(h);
                self.run.running = true;
                self.run.exit_code = None;
                // the messages of the compilers in the output are found like those of make
                self.make_run = Some(MakeRun { root, target, label: self.run.title.clone() });
            }
            Err(e) => {
                self.run.log.push(LogLine { kind: LogKind::Error, text: i18n::fmt("Cannot run: {}", &[&e]) });
                self.scenario_ended(false);
            }
        }
    }

    /// A command of the scenario ended with `code`: go on, or stop at a failure.
    pub fn scenario_after_exit(&mut self, code: Option<i32>) {
        let Some(s) = self.scenario.as_ref() else { return };
        if code == Some(0) || !s.stop_on_error {
            self.scenario_step();
        } else {
            self.scenario_ended(false);
        }
    }

    fn scenario_ended(&mut self, ok: bool) {
        let Some(s) = self.scenario.take() else { return };
        self.run.running = false;
        if !ok {
            let msg = i18n::fmt("Scenario {0} stopped at command {1} of {2}", &[&s.name, &s.next, &s.commands.len()]);
            self.run.log.push(LogLine { kind: LogKind::Error, text: msg.clone() });
            self.status = msg;
        }
    }

    /// Stop what is running, a scenario included.
    pub fn cancel_scenario(&mut self) {
        self.scenario = None;
    }
}
