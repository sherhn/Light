//! Run configurations: current file, load/test an ASDF system, a make target (in Linux for Linux projects).

use std::path::{Path, PathBuf};
use std::process::Command;

use ide_core::build::{self, Level};
use lisp_runtime::discover::Launch;
use lisp_runtime::runner;
use lisp_runtime::wsl::{self, Target};

use crate::app::App;
use crate::editor_view::Sev;
use crate::i18n::{self, tr};
use crate::state::*;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum RunConfig {
    #[default]
    CurrentFile,
    Load(String),
    Test(String),
    Make(String),
    /// A scenario of the project: console commands one after another.
    Scenario(String),
    Module(String, String),
}

/// Where a make run happened, to find the files its compiler messages name.
#[derive(Clone, Debug)]
pub struct MakeRun {
    pub root: PathBuf,
    pub target: Option<Target>,
    /// What to call the run in messages: `make`, or the name of the run command.
    pub label: String,
}

/// `a/b/../c` -> `a/c` for `/`-separated paths.
fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    let joined = out.join("/");
    if p.starts_with('/') { format!("/{joined}") } else { joined }
}

/// The Windows path of a file named in make output.
pub fn message_path(run: &MakeRun, dir: &str, file: &str) -> PathBuf {
    match &run.target {
        Some(t) => {
            let root = wsl::to_linux(&run.root, t);
            let base = if dir.is_empty() { root } else { dir.to_string() };
            let full = if file.starts_with('/') { file.to_string() } else { format!("{base}/{file}") };
            wsl::to_windows(&normalize(&full), t)
        }
        None => {
            let f = Path::new(file);
            if f.is_absolute() {
                f.to_path_buf()
            } else {
                let base = if dir.is_empty() { run.root.clone() } else { PathBuf::from(dir) };
                base.join(f)
            }
        }
    }
}

impl App {
    pub fn html_browser_available(&self) -> bool {
        self.doc().is_some_and(|doc| doc.diff.is_none() && doc.page.is_none()
            && (doc.ed.language().is_some_and(|language| language.id() == "html")
                || doc.path.as_ref().and_then(|path| path.extension()).is_some_and(|extension|
                    ["html", "htm", "xhtml"].iter().any(|candidate| extension.eq_ignore_ascii_case(candidate)))))
    }

    pub fn show_html_in_browser(&mut self) {
        self.show_html_in_browser_with(|path| {
            let path = path.to_str().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid file path"))?;
            webbrowser::open(path)
        });
    }

    fn show_html_in_browser_with(&mut self, open: impl FnOnce(&Path) -> std::io::Result<()>) {
        if !self.html_browser_available() {
            self.toast(tr("Open an HTML file first"), Sev::Weak);
            return;
        }
        if !self.save_doc(self.active) { return; }
        let Some(path) = self.doc().and_then(|doc| doc.path.as_ref()) else { return };
        if let Err(error) = std::path::absolute(path).and_then(|path| open(&path)) {
            self.toast(i18n::fmt("Cannot open in browser: {}", &[&error]), Sev::Error);
        }
    }

    pub fn run_config_label(&self) -> String {
        match &self.run_config {
            RunConfig::CurrentFile => self.doc().filter(|d| d.diff.is_none()).map_or(tr("Current File").to_string(), |d| d.name()),
            RunConfig::Load(s) => i18n::fmt("Load {}", &[&s]),
            RunConfig::Test(s) => i18n::fmt("Test {}", &[&s]),
            RunConfig::Make(t) => format!("make {t}"),
            RunConfig::Scenario(n) => n.clone(),
            RunConfig::Module(_, name) => name.clone(),
        }
    }

    /// ASDF systems defined by `.asd` files of the project (system name = file stem).
    pub fn asd_systems(&self) -> Vec<String> {
        let Some(p) = &self.project else { return vec![] };
        // ASDF systems are the Lisp module's
        if !self.lisp_enabled() {
            return vec![];
        }
        let mut v: Vec<String> = p.files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("asd"))).filter_map(|f| f.file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Execute the selected run configuration.
    pub fn run_selected(&mut self) {
        match self.run_config.clone() {
            RunConfig::CurrentFile => self.run_current_file(),
            RunConfig::Load(s) => self.run_asdf_system(&s, false),
            RunConfig::Test(s) => self.run_asdf_system(&s, true),
            RunConfig::Make(t) => self.run_make(&t),
            RunConfig::Scenario(n) => self.run_scenario(&n),
            RunConfig::Module(module, name) => {
                let def = self.available_module_runs().into_iter().find(|(id, def)| *id == module && def.name == name).map(|(_, def)| def);
                if let (Some(def), Some(path)) = (def, self.doc().and_then(|d| d.path.clone())) { self.run_language_command(&def, &path); }
                else { self.toast(tr("Choose a file supported by this run command"), Sev::Weak); }
            }
        }
    }

    /// Re-read the targets of the project's makefile.
    pub fn refresh_make_targets(&mut self) {
        self.make_targets = self.project_root().and_then(build::find_makefile).and_then(|m| std::fs::read_to_string(m).ok()).map(|t| build::targets(&t)).unwrap_or_default();
        if let RunConfig::Make(t) = &self.run_config
            && !self.make_targets.contains(t)
        {
            self.run_config = RunConfig::CurrentFile;
        }
        // a project built with make starts with its first target selected
        if self.run_config == RunConfig::CurrentFile
            && let Some(t) = self.make_targets.first()
            && self.asd_systems().is_empty()
        {
            self.run_config = RunConfig::Make(t.clone());
        }
    }

    /// `make TARGET` in the project folder: inside the project's Linux distribution, or on Windows.
    pub fn run_make(&mut self, target: &str) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            self.toast(tr("Open a project first"), Sev::Weak);
            return;
        };
        self.save_all();
        let t = self.wsl_target();
        let launch = match &t {
            Some(t) => {
                let marker = wsl::new_marker();
                // one stream: make's "Entering directory" lines must come before the compiler messages they explain
                let cmd = wsl::shell(t, &root, &format!("make {} 2>&1", wsl::sh_quote(target)), &[], &marker);
                Launch { cmd, cleanup: Some(wsl::Cleanup { target: t.clone(), marker }) }
            }
            None => {
                let mut cmd = Command::new("make");
                cmd.arg(target).current_dir(&root);
                Launch { cmd, cleanup: None }
            }
        };
        if self.start_run(format!("make:{target}"), format!("make {target}"), launch, "make", root, t) {
            self.run_request = Some(RunRequest::Make(target.to_string()));
        }
    }

    /// Show `launch` in the Run window as the current run; its compiler messages become problems when it ends.
    fn start_run(&mut self, key: String, title: String, launch: Launch, label: &str, root: PathBuf, t: Option<Target>) -> bool {
        match runner::spawn_launch(launch) {
            Ok(h) => {
                let place = match &t {
                    Some(t) => i18n::fmt("in Linux ({0}): {1}", &[&t.distro, &wsl::to_linux(&root, t)]),
                    None => root.display().to_string(),
                };
                self.prepare_run(key);
                self.run = RunState { handle: Some(h), title: title.clone(), log: vec![], running: true, exit_code: None };
                self.run.log.push(LogLine { kind: LogKind::Info, text: format!("{title}   {place}") });
                self.make_run = Some(MakeRun { root, target: t, label: label.to_string() });
                self.compiler_problems.retain(|p| p.source != "make");
                self.bottom_tool = Some(BottomTool::Run);
                true
            }
            Err(e) if t.is_none() && e.kind() == std::io::ErrorKind::NotFound && label == "make" => {
                self.toast(tr("make is not installed on Windows - build this project in Linux (WSL): click Windows in the status bar"), Sev::Error);
                false
            }
            Err(e) => { self.toast(i18n::fmt("Cannot run: {}", &[&e]), Sev::Error); false }
        }
    }

    /// The command that runs files of language `lang`, from the modules that are on.
    pub fn run_def_for(&self, lang: &str) -> Option<ide_core::modules::RunDef> {
        self.mods.installed.iter().filter(|m| m.enabled).flat_map(|m| m.manifest.run.iter()).find(|r| r.language == lang).cloned()
    }

    pub fn available_module_runs(&self) -> Vec<(String, ide_core::modules::RunDef)> {
        let Some(language) = self.active_language_id() else { return vec![] };
        self.mods.installed.iter().filter(|m| m.enabled).flat_map(|m| m.manifest.run.iter().filter(|r| r.language == language).map(|r| (m.manifest.id.clone(), r.clone()))).collect()
    }

    /// Run `file` with the run command of its language: in the project's Linux, or on Windows.
    pub fn run_language_command(&mut self, def: &ide_core::modules::RunDef, file: &Path) {
        self.save_all();
        let root = self.project_root().map(Path::to_path_buf).or_else(|| file.parent().map(Path::to_path_buf)).unwrap_or_default();
        let t = self.wsl_target();
        let dir = file.parent().unwrap_or(Path::new("."));
        let (file_s, dir_s) = match &t {
            Some(t) => (wsl::to_linux(file, t), wsl::to_linux(dir, t)),
            None => (file.display().to_string().replace('\\', "/"), dir.display().to_string().replace('\\', "/")),
        };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let line = def.command_line(&file_s, &dir_s, t.is_none());
        let launch = match &t {
            Some(t) => {
                let marker = wsl::new_marker();
                let cmd = wsl::shell(t, &root, &format!("{line} 2>&1"), &[], &marker);
                Launch { cmd, cleanup: Some(wsl::Cleanup { target: t.clone(), marker }) }
            }
            None => Launch { cmd: ide_core::cmdscript::shell_command(&line, &root, &[]), cleanup: None },
        };
        if self.start_run(format!("file:{}", file.display()), format!("{} '{}'", def.name, name), launch, &def.name, root, t) {
            self.run_request = Some(RunRequest::Language(def.clone(), file.to_path_buf()));
        }
    }

    /// A make run ended: its compiler messages become problems (and markers in open files).
    pub fn make_finished(&mut self, code: Option<i32>) {
        let Some(run) = self.make_run.take() else { return };
        if code == Some(127) && run.target.is_some() {
            // make itself was not found: the distribution has no build tools yet
            let program = if run.label == "make" { Some("make") } else { None };
            if let Some(hint) = self.linux_command_not_found(program) {
                self.run.log.push(LogLine { kind: LogKind::Error, text: hint.clone() });
                self.toast(hint, Sev::Error);
                self.status = tr(if program.is_some() { "make is not installed" } else { "The command was not found" }).into();
                return;
            }
        }
        let output: String = self.run.log.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        let messages = build::parse_output(&output);
        if messages.is_empty() && code != Some(0) {
            self.status = i18n::fmt("{} failed", &[&run.label]);
        }
        let (mut errors, mut warnings) = (0, 0);
        let mut by_file: std::collections::HashMap<PathBuf, Vec<crate::editor_view::Marker>> = std::collections::HashMap::new();
        let files: Vec<PathBuf> = self.project.as_ref().map(|p| p.files.clone()).unwrap_or_default();
        for m in &messages {
            let mut path = message_path(&run, &m.dir, &m.file);
            if !path.exists() {
                // the directory was not announced: the one project file with that name (and relative path)
                let tail: Vec<&str> = m.file.split(['/', '\\']).filter(|c| !c.is_empty() && *c != "." && *c != "..").collect();
                let hits: Vec<&PathBuf> = files.iter().filter(|f| {
                    let comps: Vec<String> = f.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                    comps.len() >= tail.len() && comps[comps.len() - tail.len()..].iter().zip(&tail).all(|(a, b)| a == b)
                }).collect();
                if hits.len() == 1 {
                    path = hits[0].clone();
                }
            }
            let text = match self.docs.iter().find(|d| d.path.as_deref() == Some(path.as_path())) {
                Some(d) => d.ed.text().to_string(),
                None => std::fs::read_to_string(&path).unwrap_or_default(),
            };
            let start = build::offset_of(&text, m.line, m.col.max(1));
            let end = text[start..].find('\n').map_or(text.len(), |e| start + e).max(start + 1).min(text.len().max(start));
            let sev = match m.level {
                Level::Error => {
                    errors += 1;
                    Sev::Error
                }
                Level::Warning => {
                    warnings += 1;
                    Sev::Warning
                }
                Level::Note => Sev::Weak,
            };
            by_file.entry(path.clone()).or_default().push(crate::editor_view::Marker { start, end, sev, msg: m.text.clone() });
            self.compiler_problems.push(ProblemRow { path: Some(path), start, end, sev, message: m.text.clone(), source: "make" });
        }
        for d in self.docs.iter_mut() {
            if let Some(ms) = d.path.as_ref().and_then(|p| by_file.remove(p)) {
                d.compiler_markers = ms;
                d.compiled_rev = d.ed.revision();
            }
        }
        if errors > 0 {
            self.status = i18n::fmt("{0}: {1} error(s), {2} warning(s)", &[&run.label, &errors, &warnings]);
            self.toast(i18n::fmt("{0}: {1} error(s), {2} warning(s) - see Problems", &[&run.label, &errors, &warnings]), Sev::Error);
        } else if code == Some(0) {
            self.status = i18n::fmt("{0} finished: {1} warning(s)", &[&run.label, &warnings]);
        }
    }

    pub fn run_asdf_system(&mut self, system: &str, test: bool) {
        let Some(root) = self.project_root().map(|p| p.to_path_buf()) else {
            self.toast(tr("Open a project first"), Sev::Weak);
            return;
        };
        self.save_all();
        let Some(imp) = self.lisp.clone().or_else(|| self.resolve_lisp()) else {
            self.toast(tr("SBCL not found - set the path in Settings"), Sev::Error);
            return;
        };
        match runner::run_asdf(&imp, &root, system, test) {
            Ok(h) => {
                let verb = if test { tr("Test") } else { tr("Load") };
                self.prepare_run(format!("asdf:{test}:{system}"));
                self.run_request = Some(RunRequest::Asdf(system.to_string(), test));
                self.run = RunState { handle: Some(h), title: format!("{verb} '{system}'"), log: vec![], running: true, exit_code: None };
                self.run.log.push(LogLine { kind: LogKind::Info, text: i18n::fmt("{0} ASDF system '{1}' from {2}", &[&verb, &system, &root.display()]) });
                self.bottom_tool = Some(BottomTool::Run);
            }
            Err(e) => self.toast(i18n::fmt("Cannot run: {}", &[&e]), Sev::Error),
        }
    }

    /// Load the system into the live REPL.
    pub fn repl_load_system(&mut self) {
        let Some(root) = self.project_root().map(|p| p.to_path_buf()) else { return };
        let sys = match &self.run_config {
            RunConfig::Load(s) | RunConfig::Test(s) => Some(s.clone()),
            RunConfig::CurrentFile | RunConfig::Make(_) | RunConfig::Scenario(_) | RunConfig::Module(_, _) => self.asd_systems().into_iter().next(),
        };
        let Some(sys) = sys else {
            self.toast(tr("No .asd system found in this project"), Sev::Weak);
            return;
        };
        self.save_all();
        // separate top-level forms: `require` must run before the reader meets `asdf:` symbols
        let code = format!(
            "(require :asdf) (asdf:initialize-source-registry (list :source-registry (list :tree {}) :inherit-configuration)) (asdf:load-system {})",
            runner::lisp_string(&self.lisp_path_of(&root)),
            runner::lisp_string(&sys)
        );
        self.bottom_tool = Some(BottomTool::Repl);
        self.repl_eval(&code, true);
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;

    fn html_app() -> (App, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("light-html-browser-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("страница & #1.HTML");
        std::fs::write(&path, "<p>old</p>").unwrap();
        let mut app = App::new(ide_core::settings::Settings::default());
        app.docs = vec![crate::doc::Doc::open(&path).unwrap()];
        app.active = 0;
        (app, dir, path)
    }

    #[test]
    fn browser_receives_the_current_html_after_saving() {
        let (mut app, dir, path) = html_app();
        app.docs[0].ed.select_all();
        app.docs[0].ed.insert_text_raw("<h1>latest</h1>");
        let mut opened = false;
        app.show_html_in_browser_with(|target| {
            assert_eq!(target, path);
            assert_eq!(std::fs::read_to_string(target).unwrap(), "<h1>latest</h1>");
            opened = true;
            Ok(())
        });
        assert!(opened);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn browser_is_not_opened_when_saving_fails_or_the_file_is_not_html() {
        let (mut app, dir, path) = html_app();
        app.docs[0].path = Some(dir.join("missing-parent/page.html"));
        app.show_html_in_browser_with(|_| panic!("must not open an unsaved file"));
        assert!(app.toasts.iter().any(|toast| toast.sev == Sev::Error));
        app.docs[0].path = Some(dir.join("style.css"));
        assert!(!app.html_browser_available());
        app.show_html_in_browser_with(|_| panic!("must not open a CSS file"));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn browser_launch_failures_are_reported() {
        let (mut app, dir, path) = html_app();
        app.show_html_in_browser_with(|_| Err(std::io::Error::other("browser unavailable")));
        assert!(app.toasts.iter().any(|toast| toast.sev == Sev::Error && toast.text.contains("browser unavailable")));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

/// Each tab owns its process, output, compiler context, and scenario progress.
#[derive(Clone)]
pub enum RunRequest {
    File(PathBuf),
    Language(ide_core::modules::RunDef, PathBuf),
    Make(String),
    Asdf(String, bool),
    Scenario(String),
}

pub struct RunTab {
    pub key: String,
    pub request: Option<RunRequest>,
    pub run: RunState,
    pub make_run: Option<MakeRun>,
    pub scenario: Option<crate::app_scenario::ScenarioRun>,
}

impl App {
    pub fn select_run_tab(&mut self, index: usize) {
        let tab = &mut self.other_runs[index];
        std::mem::swap(&mut self.run_key, &mut tab.key);
        std::mem::swap(&mut self.run_request, &mut tab.request);
        std::mem::swap(&mut self.run, &mut tab.run);
        std::mem::swap(&mut self.make_run, &mut tab.make_run);
        std::mem::swap(&mut self.scenario, &mut tab.scenario);
    }

    pub fn prepare_run(&mut self, key: String) {
        if self.run_key != key {
            if let Some(index) = self.other_runs.iter().position(|tab| tab.key == key) {
                self.select_run_tab(index);
            } else {
                if !self.run_key.is_empty() {
                    self.other_runs.push(RunTab {
                        key: std::mem::take(&mut self.run_key), request: self.run_request.take(), run: std::mem::take(&mut self.run),
                        make_run: self.make_run.take(), scenario: self.scenario.take(),
                    });
                }
                self.run_key = key;
            }
        }
        if !self.run_order.contains(&self.run_key) {
            self.run_order.push(self.run_key.clone());
        }
        if let Some(handle) = self.run.handle.take() { handle.kill(); }
        self.make_run = None;
        self.scenario = None;
        self.run_request = None;
    }

    pub fn rerun_tab(&mut self) {
        match self.run_request.clone() {
            Some(RunRequest::File(path)) => { self.open_file(&path); self.run_current_file(); }
            Some(RunRequest::Language(def, path)) => self.run_language_command(&def, &path),
            Some(RunRequest::Make(target)) => self.run_make(&target),
            Some(RunRequest::Asdf(system, test)) => self.run_asdf_system(&system, test),
            Some(RunRequest::Scenario(name)) => self.run_scenario(&name),
            None => self.run_selected(),
        }
    }

    pub fn stop_other_runs(&mut self) {
        for tab in self.other_runs.drain(..) {
            if let Some(handle) = tab.run.handle { handle.kill(); }
        }
        let shown = self.run_key.clone();
        self.run_order.retain(|key| *key == shown);
    }

    /// Show the run tab `key` (its output, and what Stop and Rerun act on).
    pub fn show_run_tab(&mut self, key: &str) {
        if let Some(index) = self.other_runs.iter().position(|tab| tab.key == key) {
            self.select_run_tab(index);
        }
    }

    /// Close the run tab `key`: its program is stopped and its output goes. Closing the tab that is shown shows its
    /// neighbour (the one after it, or before it when it was the last).
    pub fn close_run_tab(&mut self, key: &str) {
        let Some(pos) = self.run_order.iter().position(|k| k == key) else { return };
        self.run_order.remove(pos);
        if self.run_key == key {
            if let Some(handle) = self.run.handle.take() { handle.kill(); }
            self.run_key.clear();
            self.run = RunState::default();
            self.run_request = None;
            self.make_run = None;
            self.scenario = None;
            let next = self.run_order.get(pos.min(self.run_order.len().saturating_sub(1))).cloned();
            if let Some(next) = next
                && let Some(index) = self.other_runs.iter().position(|tab| tab.key == next)
            {
                // the emptied tab takes the neighbour's place in the list, then leaves it
                self.select_run_tab(index);
                self.other_runs.remove(index);
            }
        } else if let Some(index) = self.other_runs.iter().position(|tab| tab.key == key) {
            let tab = self.other_runs.remove(index);
            if let Some(handle) = tab.run.handle { handle.kill(); }
        }
    }

    /// The run tabs in their order: key, title, and how its program is doing.
    pub fn run_tabs(&self) -> Vec<(String, String, RunStatus)> {
        self.run_order.iter().filter_map(|key| {
            let run = if *key == self.run_key { &self.run } else { &self.other_runs.iter().find(|tab| tab.key == *key)?.run };
            Some((key.clone(), run.title.clone(), RunStatus::of(run)))
        }).collect()
    }
}

/// How the program of a run tab is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    /// Nothing has finished in it yet, or it was stopped.
    Idle,
}

impl RunStatus {
    fn of(run: &RunState) -> RunStatus {
        match (run.running, run.exit_code) {
            (true, _) => RunStatus::Running,
            (false, Some(0)) => RunStatus::Succeeded,
            (false, Some(_)) => RunStatus::Failed,
            (false, None) => RunStatus::Idle,
        }
    }
}

#[cfg(all(test, windows))]
mod concurrent_run_tests {
    use super::*;

    fn launch(label: &str) -> Launch {
        let mut cmd = Command::new("cmd");
        cmd.args(["/D", "/C", &format!("ping -n 3 127.0.0.1 >nul & echo {label}")]);
        Launch { cmd, cleanup: None }
    }

    #[test]
    fn separate_files_keep_running_and_collect_their_own_output() {
        let mut app = App::new(ide_core::settings::Settings::default());
        let root = std::env::current_dir().unwrap();
        assert!(app.start_run("file:first".into(), "first".into(), launch("first-output"), "make", root.clone(), None));
        assert!(app.start_run("file:second".into(), "second".into(), launch("second-output"), "make", root, None));
        assert_eq!(app.other_runs.len(), 1);
        assert!(app.run.running && app.other_runs[0].run.running);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        while (app.run.running || app.other_runs[0].run.running) && std::time::Instant::now() < deadline {
            app.poll_run();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(app.run.exit_code, Some(0));
        assert_eq!(app.other_runs[0].run.exit_code, Some(0));
        let output = |run: &RunState| run.log.iter().map(|line| line.text.as_str()).collect::<String>();
        assert!(output(&app.run).contains("second-output"));
        assert!(!output(&app.run).contains("first-output"));
        assert!(output(&app.other_runs[0].run).contains("first-output"));
        app.select_run_tab(0);
        assert_eq!(app.run.title, "first");
    }

    #[test]
    fn rerunning_or_stopping_a_file_does_not_stop_another_file() {
        let mut app = App::new(ide_core::settings::Settings::default());
        for key in ["file:first", "file:second"] {
            app.prepare_run(key.into());
            app.run.handle = Some(runner::spawn_launch(launch(key)).unwrap());
            app.run.running = true;
        }
        app.prepare_run("file:first".into());
        assert_eq!(app.other_runs.len(), 1);
        assert_eq!(app.other_runs[0].key, "file:second");
        assert!(app.other_runs[0].run.handle.is_some());
        app.stop_run();
        assert!(app.other_runs[0].run.running);
        app.stop_other_runs();
    }

    #[test]
    fn tabs_keep_their_order_and_closing_one_shows_its_neighbour() {
        let mut app = App::new(ide_core::settings::Settings::default());
        for key in ["file:a", "file:b", "file:c"] {
            app.prepare_run(key.into());
            app.run.title = key.trim_start_matches("file:").into();
        }
        let keys = |app: &App| app.run_tabs().into_iter().map(|(key, _, _)| key).collect::<Vec<_>>();
        assert_eq!(keys(&app), ["file:a", "file:b", "file:c"]);
        // showing another tab or running one again does not move the tabs
        app.show_run_tab("file:a");
        assert_eq!(app.run.title, "a");
        app.prepare_run("file:b".into());
        assert_eq!(keys(&app), ["file:a", "file:b", "file:c"]);
        assert_eq!(app.run_key, "file:b");
        // the shown tab closes: the one after it is shown
        app.close_run_tab("file:b");
        assert_eq!(keys(&app), ["file:a", "file:c"]);
        assert_eq!((app.run_key.as_str(), app.run.title.as_str()), ("file:c", "c"));
        assert_eq!(app.other_runs.len(), 1);
        // a tab in the background closes without changing what is shown
        app.close_run_tab("file:a");
        assert_eq!(keys(&app), ["file:c"]);
        assert!(app.other_runs.is_empty());
        app.close_run_tab("file:c");
        assert!(app.run_tabs().is_empty() && app.run_key.is_empty());
    }
}
