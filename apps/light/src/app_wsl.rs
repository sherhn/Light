//! Running development tools inside WSL: what the Linux (WSL) settings page knows (WSL, distributions, tools) and the
//! setup actions behind its buttons. Checks run on background threads and installs as child processes;
//! [`App::poll_wsl`] collects their results every frame. Nothing here removes or changes an existing distribution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use ide_core::settings::{RunEnv, Settings};
use lisp_runtime::setup_progress::{Kind, MARKER, Tracker};
use lisp_runtime::wsl::{self, ImageKind, OnlineDistro, Presence, Probe, SbclCheck, Target};
use lisp_runtime::{Implementation, RunEvent, RunHandle, runner};

use crate::app::App;
use crate::editor_view::Sev;
use crate::i18n::{self, tr};

/// Lines of setup output kept for the page.
const LOG_LINES: usize = 400;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupTask {
    InstallWsl,
    InstallDistro(String),
    ImportFile(String),
    InstallSbcl(String),
    /// make, gcc (32-bit too), nasm, gdb, git and QEMU for C/assembly projects.
    InstallTools(String),
    /// The same developer tools on this computer, when Light runs on Linux itself.
    InstallHostTools,
    /// Git in the distribution: name, e-mail and the Windows credential manager.
    GitSetup(String),
}

/// Where the tools found on this computer are kept in [`WslState::tools`] (no distribution is called that).
pub const HOST: &str = "@host";

/// Light runs on Linux itself: its developer tools are this computer's, installed with its package manager.
pub fn host_is_linux() -> bool {
    cfg!(target_os = "linux")
}

pub struct RunningTask {
    pub task: SetupTask,
    pub handle: RunHandle,
    /// How far the step has got, read from its output.
    pub tracker: Tracker,
    pub started: Instant,
}

impl SetupTask {
    /// What its output looks like, for the progress tracker.
    fn kind(&self) -> Kind {
        match self {
            SetupTask::InstallWsl => Kind::Wsl,
            SetupTask::InstallDistro(_) => Kind::Distro,
            SetupTask::ImportFile(_) => Kind::Import,
            SetupTask::InstallSbcl(_) | SetupTask::InstallTools(_) | SetupTask::InstallHostTools => Kind::Packages,
            SetupTask::GitSetup(_) => Kind::Config,
        }
    }
}

/// Remove the lines the setup scripts print for the progress bar (an unfinished last line stays: more of it may come).
pub fn strip_markers(log: &mut Vec<String>, open: bool) {
    let last = log.len().saturating_sub(1);
    let mut i = 0;
    log.retain(|l| {
        let keep = !l.starts_with(MARKER) || (open && i == last);
        i += 1;
        keep
    });
}

/// What the check of the developer tools found in a distribution: (program, found), or why the check failed.
pub type ToolsFound = Result<Vec<(String, bool)>, String>;

#[derive(Default)]
pub struct WslState {
    pub probe: Option<Probe>,
    probe_rx: Option<Receiver<Probe>>,
    pub online: Vec<OnlineDistro>,
    pub online_error: Option<String>,
    online_rx: Option<Receiver<Result<Vec<OnlineDistro>, String>>>,
    /// SBCL found in each distribution (`Err`: the check itself failed).
    pub sbcl: HashMap<String, Result<SbclCheck, String>>,
    sbcl_rx: Option<Receiver<(String, Result<SbclCheck, String>)>>,
    pub checking_sbcl: Option<String>,
    /// Developer tools found in each distribution: (program, found) (`Err`: the check failed).
    pub tools: HashMap<String, ToolsFound>,
    tools_rx: Option<Receiver<(String, ToolsFound)>>,
    pub task: Option<RunningTask>,
    /// Output of the last setup task; a progress bar redrawing its line replaces the last entry.
    pub log: Vec<String>,
    log_open: bool,
    /// How the last setup task ended: Ok(what was done) / Err(what went wrong).
    pub outcome: Option<Result<String, String>>,
    /// The distribution the page shows SBCL for and switches to.
    pub selected: String,
    /// The distribution offered for installation.
    pub chosen: String,
    /// Folder to install a distribution into; empty = where WSL keeps them by default.
    pub location: String,
    /// Free space on the drive a new distribution goes to: (drive letter, bytes).
    pub free: Option<(char, u64)>,
    free_rx: Option<Receiver<Option<(char, u64)>>>,
    /// The page has started its first check.
    pub requested: bool,
    /// The groups of developer tools whose packages are listed for choosing (by name).
    pub tools_open: std::collections::HashSet<String>,
    /// The form for adding or changing a tool of one's own, while it is open.
    pub tool_draft: Option<ToolDraft>,
}

/// A tool of one's own being typed in on the Linux page.
#[derive(Clone, Debug, Default)]
pub struct ToolDraft {
    /// Which of `Settings::linux_tools.custom` it replaces (`None`: a new one).
    pub index: Option<usize>,
    pub name: String,
    pub packages: String,
    pub programs: String,
    /// What is wrong with it, to be shown under the form.
    pub error: String,
}

impl WslState {
    pub fn probing(&self) -> bool {
        self.probe_rx.is_some()
    }

    pub fn busy(&self) -> bool {
        self.probe_rx.is_some() || self.online_rx.is_some() || self.sbcl_rx.is_some() || self.tools_rx.is_some() || self.task.is_some()
    }

    pub fn wsl_installed(&self) -> bool {
        matches!(self.probe.as_ref().map(|p| &p.presence), Some(Presence::Installed { .. }))
    }

    pub fn distros(&self) -> Vec<String> {
        // Docker Desktop's own distributions are not for working in
        self.probe.as_ref().map(|p| p.distros.iter().map(|d| d.name.clone()).filter(|n| crate::app_env::usable_distro(n)).collect()).unwrap_or_default()
    }

    /// SBCL path in `distro`, when the last check found one.
    pub fn sbcl_path(&self, distro: &str) -> Option<String> {
        self.sbcl.get(distro).and_then(|r| r.as_ref().ok()).and_then(|c| c.path.clone())
    }

    /// Display name of a distribution from the online catalog (`Ubuntu 24.04 LTS`), else its name.
    pub fn friendly(&self, name: &str) -> String {
        self.online.iter().find(|o| o.name == name).map(|o| o.friendly.clone()).filter(|f| !f.is_empty()).unwrap_or_else(|| match name {
            "Ubuntu-24.04" => "Ubuntu 24.04 LTS".to_string(),
            "Ubuntu-22.04" => "Ubuntu 22.04 LTS".to_string(),
            "Ubuntu-26.04" => "Ubuntu 26.04 LTS".to_string(),
            "Debian" => "Debian GNU/Linux".to_string(),
            other => other.to_string(),
        })
    }
}

/// Append child output to `log`. `open` says whether the last entry is an unfinished line; a carriage return
/// (progress bars) redraws that line instead of adding new ones. Escape sequences are dropped.
pub fn push_output(log: &mut Vec<String>, open: &mut bool, text: &str) {
    let text = text.replace("\r\n", "\n");
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\n' => {
                if !*open {
                    log.push(String::new());
                }
                *open = false;
            }
            '\r' => {
                if *open && let Some(l) = log.last_mut() {
                    l.clear();
                }
            }
            '\u{1b}' => {
                // CSI sequence: ESC [ ... final letter
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            }
            '\0' => {}
            c => {
                if !*open {
                    log.push(String::new());
                    *open = true;
                }
                if let Some(l) = log.last_mut() {
                    l.push(c);
                }
            }
        }
    }
    if log.len() > LOG_LINES {
        let cut = log.len() - LOG_LINES;
        log.drain(..cut);
    }
}

/// The drive a path is on (`C:\x` -> 'C').
fn drive_of(p: &Path) -> Option<char> {
    let s = p.to_string_lossy();
    let c = s.chars().next()?;
    (c.is_ascii_alphabetic() && s[1..].starts_with(':')).then(|| c.to_ascii_uppercase())
}

/// Where WSL puts distributions when no folder is given.
fn default_location() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\"))
}

/// A distribution name for an image file that does not clash with installed ones: `ubuntu-24.04.wsl` -> `ubuntu-24.04`.
pub fn name_for_image(file: &Path, taken: &[String]) -> String {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stem = name.as_str();
    for ext in [".tar.gz", ".tar.xz", ".tar.zst", ".tgz", ".tar", ".wsl"] {
        if let Some(s) = stem.strip_suffix(ext) {
            stem = s;
            break;
        }
    }
    let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' { c } else { '-' }).collect();
    let base = if clean.trim_matches('-').is_empty() { "Linux".to_string() } else { clean };
    let mut n = base.clone();
    let mut k = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&n)) {
        n = format!("{base}-{k}");
        k += 1;
    }
    n
}

/// The Lisp `settings` ask for: SBCL inside the chosen WSL distribution, or SBCL for Windows.
pub fn resolve_lisp(settings: &Settings) -> Option<Implementation> {
    match settings.run_env {
        RunEnv::Wsl => wsl_target(settings).map(|t| Implementation::in_wsl(t, &settings.wsl_sbcl)),
        RunEnv::Windows => lisp_runtime::discover::find_sbcl(&settings.lisp_path),
    }
}

/// The distribution of the WSL mode, when it is on.
pub fn wsl_target(settings: &Settings) -> Option<Target> {
    let d = settings.wsl_distro.trim();
    (settings.run_env == RunEnv::Wsl && !d.is_empty()).then(|| Target::new(d, &settings.wsl_mount_root))
}

impl App {
    /// The Lisp the settings ask for: SBCL inside the chosen WSL distribution, or SBCL for Windows.
    pub fn resolve_lisp(&self) -> Option<Implementation> {
        match self.wsl_target() {
            Some(t) => {
                // the SBCL found by the setup belongs to the distribution it was found in
                let sbcl = if t.distro == self.settings.wsl_distro { self.settings.wsl_sbcl.clone() } else { String::new() };
                Some(Implementation::in_wsl(t, &sbcl))
            }
            None if self.project.is_some() => lisp_runtime::discover::find_sbcl(&self.settings.lisp_path),
            None => resolve_lisp(&self.settings),
        }
    }

    /// The distribution Lisp and the terminal run in, when the WSL mode is on.
    pub fn wsl_target(&self) -> Option<Target> {
        // the project decides first: where it lives, or what was chosen for it
        if let Some(d) = self.project_distro() {
            let mount = if d == self.settings.wsl_distro { self.settings.wsl_mount_root.as_str() } else { "" };
            return Some(Target::new(d, mount));
        }
        if self.project_on_windows() {
            return None;
        }
        wsl_target(&self.settings)
    }

    /// `p` as the Lisp of the REPL (or the configured one) names files.
    pub fn lisp_path_of(&self, p: &Path) -> String {
        match self.repl.session.as_ref().map(|s| &s.implementation).or(self.lisp.as_ref()) {
            Some(imp) => imp.lisp_path(p),
            None => runner::lisp_path(p),
        }
    }

    // ------------------------------------------------------------------ checks

    /// Find out what WSL has (in the background).
    pub fn wsl_probe(&mut self) {
        if self.wsl.probe_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.wsl.probe_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(wsl::probe());
        });
        self.wsl_refresh_free();
    }

    /// Load the list of installable distributions (needs the internet).
    pub fn wsl_list_online(&mut self) {
        if self.wsl.online_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.wsl.online_rx = Some(rx);
        self.wsl.online_error = None;
        std::thread::spawn(move || {
            let _ = tx.send(wsl::list_online());
        });
    }

    /// Look for SBCL inside `distro` (in the background).
    pub fn wsl_check_sbcl(&mut self, distro: &str) {
        if self.wsl.sbcl_rx.is_some() || distro.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.wsl.sbcl_rx = Some(rx);
        self.wsl.checking_sbcl = Some(distro.to_string());
        let (d, sbcl) = (distro.to_string(), self.settings.wsl_sbcl.clone());
        std::thread::spawn(move || {
            let r = wsl::check_sbcl(&d, &sbcl);
            let _ = tx.send((d, r));
        });
    }

    /// Look for make, gcc, QEMU and friends inside `distro` (in the background).
    pub fn wsl_check_tools(&mut self, distro: &str) {
        if self.wsl.tools_rx.is_some() || distro.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.wsl.tools_rx = Some(rx);
        let d = distro.to_string();
        let groups = self.tool_groups();
        std::thread::spawn(move || {
            let r = wsl::check_tools(&d, &groups);
            let _ = tx.send((d, r));
        });
    }

    pub fn wsl_checking_tools(&self) -> bool {
        self.wsl.tools_rx.is_some()
    }

    /// Look for the developer tools on this computer (Light runs on Linux), in the background.
    pub fn host_check_tools(&mut self) {
        if self.wsl.tools_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.wsl.tools_rx = Some(rx);
        let groups = self.tool_groups();
        std::thread::spawn(move || {
            let _ = tx.send((HOST.to_string(), wsl::check_tools_here(&groups)));
        });
    }

    /// Install the chosen developer tools on this computer; the groups that bring a program of `needed` are installed
    /// whole. The desktop asks for the administrator password.
    pub fn host_install_tools_with(&mut self, needed: &[String]) {
        let groups = self.chosen_tool_groups(needed);
        if groups.is_empty() {
            self.toast(tr("No tool is chosen: tick the ones to install"), Sev::Weak);
            return;
        }
        let title = tr("Installing developer tools on this computer (confirm the administrator prompt)...").to_string();
        self.wsl_start(SetupTask::InstallHostTools, wsl::install_tools_here_command(&groups), title);
    }

    /// The set of tools changed (one was added, changed or removed): what was found in the distributions is looked
    /// for again.
    pub fn wsl_tools_changed(&mut self) {
        self.wsl.tools.clear();
        self.save_settings();
    }

    /// Keep the tool of one's own that the form holds: a new one, or the one it changes. What is wrong with it is
    /// written in the form, which then stays open.
    pub fn wsl_tool_save(&mut self) {
        let Some(d) = self.wsl.tool_draft.clone() else { return };
        let name = d.name.trim().to_string();
        let problem = if name.is_empty() {
            Some(tr("Give the tool a name").to_string())
        } else if d.packages.split_whitespace().next().is_none() {
            Some(tr("Name at least one package").to_string())
        } else if wsl::custom_group(&name, &d.packages, &d.programs).is_none() {
            Some(tr("Names of packages and programs may have letters, digits and . + - _ : only").to_string())
        } else if self.tool_groups().iter().any(|g| !g.custom && g.name == name) || self.settings.linux_tools.custom.iter().enumerate().any(|(i, c)| Some(i) != d.index && c.name.trim() == name) {
            Some(i18n::fmt("A tool called {} already exists", &[&name]))
        } else {
            None
        };
        if let Some(p) = problem {
            if let Some(draft) = self.wsl.tool_draft.as_mut() {
                draft.error = p;
            }
            return;
        }
        let tool = ide_core::settings::CustomTool { name, packages: d.packages.split_whitespace().collect::<Vec<_>>().join(" "), programs: d.programs.split_whitespace().collect::<Vec<_>>().join(" ") };
        match d.index.filter(|i| *i < self.settings.linux_tools.custom.len()) {
            Some(i) => self.settings.linux_tools.custom[i] = tool,
            None => self.settings.linux_tools.custom.push(tool),
        }
        self.wsl.tool_draft = None;
        self.wsl_tools_changed();
    }

    /// Install the developer tools that are chosen (compilers, debuggers, everyday tools, the user's own) inside `distro`.
    pub fn wsl_install_tools(&mut self, distro: &str) {
        self.wsl_install_tools_with(distro, &[]);
    }

    /// Like [`App::wsl_install_tools`], and the groups that bring a program of `needed` are installed whole, chosen or not.
    pub fn wsl_install_tools_with(&mut self, distro: &str, needed: &[String]) {
        let groups = self.chosen_tool_groups(needed);
        if groups.is_empty() {
            self.toast(tr("No tool is chosen: tick the ones to install"), Sev::Weak);
            return;
        }
        let title = i18n::fmt("Installing developer tools in {}...", &[&distro]);
        self.wsl_start(SetupTask::InstallTools(distro.to_string()), wsl::install_tools_command(distro, &groups), title);
    }

    /// Give git in `distro` the name and e-mail of Windows git and its credential manager (GitHub logins).
    pub fn wsl_git_setup(&mut self, distro: &str) {
        let get = |k: &str| {
            let mut c = Command::new("git");
            c.args(["config", "--global", k]);
            lisp_runtime::no_window(&mut c);
            c.output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
        };
        let (name, email) = (get("user.name"), get("user.email"));
        let t = Target::new(distro, if distro == self.settings.wsl_distro { self.settings.wsl_mount_root.as_str() } else { "" });
        let title = i18n::fmt("Setting up git in {}...", &[&distro]);
        self.wsl_start(SetupTask::GitSetup(distro.to_string()), wsl::git_setup_command(&t, &name, &email), title);
    }

    pub fn wsl_refresh_free(&mut self) {
        if self.wsl.free_rx.is_some() {
            return;
        }
        let loc = if self.wsl.location.trim().is_empty() { default_location() } else { PathBuf::from(self.wsl.location.trim()) };
        let Some(drive) = drive_of(&loc) else { return };
        let (tx, rx) = mpsc::channel();
        self.wsl.free_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(wsl::free_space(&PathBuf::from(format!("{drive}:\\"))).map(|b| (drive, b)));
        });
    }

    // ------------------------------------------------------------------ setup tasks

    fn wsl_start(&mut self, task: SetupTask, cmd: Command, title: String) {
        if self.wsl.task.is_some() {
            self.toast(tr("Another setup step is still running"), Sev::Weak);
            return;
        }
        self.wsl.log.clear();
        self.wsl.log_open = false;
        self.wsl.outcome = None;
        push_output(&mut self.wsl.log, &mut self.wsl.log_open, &format!("{title}\n"));
        match runner::spawn_streaming(cmd) {
            Ok(handle) => {
                let tracker = Tracker::new(task.kind());
                self.wsl.task = Some(RunningTask { task, handle, tracker, started: Instant::now() });
            }
            Err(e) => self.wsl.outcome = Some(Err(i18n::fmt("Cannot start: {}", &[&e]))),
        }
    }

    /// Install WSL itself. Windows asks for administrator rights and may want a restart afterwards.
    pub fn wsl_install_wsl(&mut self) {
        self.wsl_start(SetupTask::InstallWsl, wsl::install_wsl_command(), tr("Installing the Windows Subsystem for Linux (confirm the administrator prompt)...").to_string());
    }

    /// Download and install a distribution from the WSL catalog.
    pub fn wsl_install_distro(&mut self, name: &str) {
        let location = self.wsl.location.trim().to_string();
        let dir = (!location.is_empty()).then(|| PathBuf::from(&location).join(name));
        if let Some(d) = &dir
            && let Err(e) = std::fs::create_dir_all(d)
        {
            self.wsl.outcome = Some(Err(i18n::fmt("Cannot create {0}: {1}", &[&d.display(), &e])));
            return;
        }
        let title = i18n::fmt("Downloading and installing {}...", &[&self.wsl.friendly(name)]);
        self.wsl_start(SetupTask::InstallDistro(name.to_string()), wsl::install_distro_command(name, dir.as_deref()), title);
    }

    /// Install a distribution from an image file (`.wsl` or a root file system `.tar`).
    pub fn wsl_install_file(&mut self, file: &Path) {
        let name = name_for_image(file, &self.wsl.distros());
        let base = if self.wsl.location.trim().is_empty() { default_location().join("Light").join("WSL") } else { PathBuf::from(self.wsl.location.trim()) };
        let dir = base.join(&name);
        match wsl::install_file_command(file, &name, &dir) {
            Ok(cmd) => {
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    self.wsl.outcome = Some(Err(i18n::fmt("Cannot create {0}: {1}", &[&dir.display(), &e])));
                    return;
                }
                let title = i18n::fmt("Installing {0} from {1}...", &[&name, &file.display()]);
                self.wsl_start(SetupTask::ImportFile(name), cmd, title);
            }
            Err(ImageKind::Iso) => {
                self.wsl.outcome = Some(Err(tr("This is an installation disk (ISO) for a PC or a virtual machine; WSL cannot use it. Install a distribution from the list instead, or pick a WSL image (.wsl or .tar).").to_string()));
            }
            Err(_) => {
                self.wsl.outcome = Some(Err(tr("WSL installs distributions from .wsl files or root file system archives (.tar, .tar.gz, .tar.xz).").to_string()));
            }
        }
    }

    /// Install SBCL inside `distro` with its package manager.
    pub fn wsl_install_sbcl(&mut self, distro: &str) {
        let title = i18n::fmt("Installing SBCL in {}...", &[&distro]);
        self.wsl_start(SetupTask::InstallSbcl(distro.to_string()), wsl::install_sbcl_command(distro), title);
    }

    pub fn wsl_cancel(&mut self) {
        if let Some(t) = self.wsl.task.take() {
            t.handle.kill();
            self.wsl.outcome = Some(Err(tr("Cancelled").to_string()));
        }
    }

    /// Open an interactive window of the distribution (to create the Linux user on its first start).
    pub fn wsl_open_window(&mut self, distro: &str) {
        if let Err(e) = wsl::open_window(distro) {
            self.toast(i18n::fmt("Cannot start: {}", &[&e]), Sev::Error);
        }
    }

    // ------------------------------------------------------------------ switching

    /// Run development tools and the terminal inside `distro` from now on. SBCL is optional.
    pub fn wsl_use(&mut self, distro: &str) {
        if !self.wsl.wsl_installed() || !self.wsl.distros().iter().any(|d| d == distro) {
            self.toast(tr("Finish the setup first: install a Linux distribution"), Sev::Weak);
            return;
        }
        let path = self.wsl.sbcl_path(distro).unwrap_or_default();
        let mount = self.wsl.sbcl.get(distro).and_then(|r| r.as_ref().ok()).and_then(|c| c.mount_root.clone()).unwrap_or_default();
        self.settings.run_env = RunEnv::Wsl;
        self.settings.wsl_distro = distro.to_string();
        self.settings.wsl_sbcl = path;
        self.settings.wsl_mount_root = mount;
        self.lisp_environment_changed();
        self.toast(i18n::fmt("Development tools now run in {} (WSL)", &[&distro]), Sev::Weak);
    }

    pub fn use_windows(&mut self) {
        if self.settings.run_env == RunEnv::Windows {
            return;
        }
        self.settings.run_env = RunEnv::Windows;
        self.lisp_environment_changed();
        self.toast(tr("Development tools now run on Windows"), Sev::Weak);
    }

    /// The Lisp changed: pick it up, and start the next REPL there.
    pub fn lisp_environment_changed(&mut self) {
        self.lisp = self.resolve_lisp();
        if let Some(s) = self.repl.session.take() {
            s.kill();
            self.repl.ready = false;
            self.repl.starting = false;
            self.repl.busy.clear();
            self.repl.pending.clear();
        }
        self.save_settings();
    }

    // ------------------------------------------------------------------ polling

    pub fn poll_wsl(&mut self) {
        if let Some(rx) = &self.wsl.probe_rx
            && let Ok(p) = rx.try_recv()
        {
            self.wsl.probe_rx = None;
            let names: Vec<String> = p.distros.iter().map(|d| d.name.clone()).collect();
            if !names.contains(&self.wsl.selected) {
                let pick = if names.contains(&self.settings.wsl_distro) { Some(self.settings.wsl_distro.clone()) } else { p.default_distro().map(|d| d.name.clone()) };
                self.wsl.selected = pick.unwrap_or_default();
            }
            if self.wsl.chosen.is_empty() {
                self.wsl.chosen = wsl::RECOMMENDED_DISTRO.to_string();
            }
            self.wsl.sbcl.retain(|d, _| names.contains(d));
            self.wsl.probe = Some(p);
            let sel = self.wsl.selected.clone();
            if !sel.is_empty() && !self.wsl.sbcl.contains_key(&sel) {
                self.wsl_check_sbcl(&sel);
            }
        }
        if let Some(rx) = &self.wsl.online_rx
            && let Ok(r) = rx.try_recv()
        {
            self.wsl.online_rx = None;
            match r {
                Ok(list) => self.wsl.online = list,
                Err(e) => self.wsl.online_error = Some(e),
            }
        }
        if let Some(rx) = &self.wsl.tools_rx
            && let Ok((d, r)) = rx.try_recv()
        {
            self.wsl.tools_rx = None;
            self.wsl.tools.insert(d, r);
        }
        if let Some(rx) = &self.wsl.sbcl_rx
            && let Ok((d, r)) = rx.try_recv()
        {
            self.wsl.sbcl_rx = None;
            self.wsl.checking_sbcl = None;
            // the mode is on and this is its distribution: follow a SBCL that moved or appeared
            if self.settings.run_env == RunEnv::Wsl
                && self.settings.wsl_distro == d
                && let Ok(c) = &r
                && let Some(p) = &c.path
                && *p != self.settings.wsl_sbcl
            {
                self.settings.wsl_sbcl = p.clone();
                self.lisp = self.resolve_lisp();
            }
            self.wsl.sbcl.insert(d, r);
        }
        if let Some(rx) = &self.wsl.free_rx
            && let Ok(r) = rx.try_recv()
        {
            self.wsl.free_rx = None;
            self.wsl.free = r;
        }
        let mut finished: Option<(SetupTask, Option<i32>)> = None;
        if let Some(t) = self.wsl.task.as_mut() {
            while let Some(ev) = t.handle.try_recv() {
                match ev {
                    RunEvent::Output { text, .. } => {
                        push_output(&mut self.wsl.log, &mut self.wsl.log_open, &text);
                        strip_markers(&mut self.wsl.log, self.wsl.log_open);
                        t.tracker.feed(&text);
                    }
                    RunEvent::Exited(code) => {
                        t.tracker.finish(code == Some(0));
                        finished = Some((t.task.clone(), code));
                        break;
                    }
                }
            }
        }
        if let Some((task, code)) = finished {
            self.wsl.task = None;
            self.wsl_task_done(task, code);
        }
    }

    fn wsl_task_done(&mut self, task: SetupTask, code: Option<i32>) {
        let ok = code == Some(0);
        let last = self.wsl.log.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_default();
        let fail = |what: String| Err(if last.is_empty() { what } else { format!("{what}: {last}") });
        self.wsl.outcome = Some(match &task {
            SetupTask::InstallWsl if ok => Ok(tr("WSL is installed. If Windows asked for a restart, restart it, then press Check again.").to_string()),
            SetupTask::InstallWsl => fail(tr("WSL was not installed").to_string()),
            SetupTask::InstallDistro(n) | SetupTask::ImportFile(n) if ok => {
                self.wsl.selected = n.clone();
                Ok(i18n::fmt("{} is installed. Choose your developer tools below.", &[&n]))
            }
            SetupTask::InstallDistro(n) | SetupTask::ImportFile(n) => fail(i18n::fmt("{} was not installed", &[&n])),
            SetupTask::InstallSbcl(d) if ok => Ok(i18n::fmt("SBCL is installed in {}.", &[&d])),
            SetupTask::InstallSbcl(d) => fail(i18n::fmt("SBCL was not installed in {}", &[&d])),
            SetupTask::InstallTools(d) if ok => Ok(i18n::fmt("The developer tools are installed in {}.", &[&d])),
            SetupTask::InstallTools(d) => fail(i18n::fmt("The developer tools were not installed in {}", &[&d])),
            SetupTask::InstallHostTools if ok => Ok(tr("The developer tools are installed on this computer.").to_string()),
            SetupTask::InstallHostTools => fail(tr("The developer tools were not installed on this computer").to_string()),
            SetupTask::GitSetup(d) if ok => Ok(i18n::fmt("Git in {} uses your name, e-mail and Windows logins.", &[&d])),
            SetupTask::GitSetup(d) => fail(i18n::fmt("Git in {} was not set up", &[&d])),
        });
        match task {
            SetupTask::InstallTools(d) => {
                self.wsl.tools.remove(&d);
                self.wsl_check_tools(&d);
            }
            SetupTask::InstallHostTools => {
                self.wsl.tools.remove(HOST);
                self.tools_banner_dismissed = false;
                self.host_check_tools();
            }
            SetupTask::GitSetup(_) => {}
            SetupTask::InstallSbcl(d) => {
                self.wsl.sbcl.remove(&d);
                self.wsl_check_sbcl(&d);
            }
            _ => {
                // a new distribution (or WSL itself): look again; the SBCL check follows the probe
                self.wsl_probe();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_lines_handle_newlines_progress_redraws_and_escapes() {
        let (mut log, mut open) = (vec![], false);
        push_output(&mut log, &mut open, "Downloading: Ubuntu\r\n[=   5.0%   ]");
        push_output(&mut log, &mut open, "\r[====  42.5%  ]\r[=========  100.0% ]\nInstalling\n");
        push_output(&mut log, &mut open, "\u{1b}[32mdone\u{1b}[0m");
        assert_eq!(log, vec!["Downloading: Ubuntu", "[=========  100.0% ]", "Installing", "done"]);
        assert!(open);
    }

    #[test]
    fn log_is_capped() {
        let (mut log, mut open) = (vec![], false);
        let many: String = (0..LOG_LINES + 50).map(|i| format!("line {i}\n")).collect();
        push_output(&mut log, &mut open, &many);
        assert_eq!(log.len(), LOG_LINES);
        assert_eq!(log.last().unwrap(), &format!("line {}", LOG_LINES + 49));
    }

    #[test]
    fn image_names_are_cleaned_and_kept_unique() {
        let taken = vec!["ubuntu-24.04".to_string()];
        assert_eq!(name_for_image(Path::new("ubuntu-24.04.wsl"), &taken), "ubuntu-24.04-2");
        // a backslash separates folders only in a Windows path
        #[cfg(windows)]
        assert_eq!(name_for_image(Path::new(r"C:\d\ubuntu-24.04.wsl"), &taken), "ubuntu-24.04-2");
        assert_eq!(name_for_image(Path::new("debian rootfs.tar.gz"), &[]), "debian-rootfs");
        assert_eq!(name_for_image(Path::new(".tar"), &[]), "Linux");
    }

    #[test]
    fn drives_and_friendly_names() {
        assert_eq!(drive_of(Path::new(r"d:\WSL")), Some('D'));
        assert_eq!(drive_of(Path::new("relative")), None);
        let mut s = WslState::default();
        assert_eq!(s.friendly("Ubuntu-24.04"), "Ubuntu 24.04 LTS");
        s.online = vec![OnlineDistro { name: "Debian".into(), friendly: "Debian 13".into() }];
        assert_eq!(s.friendly("Debian"), "Debian 13");
        assert_eq!(s.friendly("Custom"), "Custom");
    }

    #[test]
    fn progress_markers_stay_out_of_the_log() {
        let (mut log, mut open) = (vec![], false);
        push_output(&mut log, &mut open, "Hit:1 http://x\n@@light stage install\nGet:2 http://y\n");
        strip_markers(&mut log, open);
        assert_eq!(log, vec!["Hit:1 http://x", "Get:2 http://y"]);
        // a marker that is still being written is kept until its line is complete
        push_output(&mut log, &mut open, "@@light sta");
        strip_markers(&mut log, open);
        assert_eq!(log.last().unwrap(), "@@light sta");
        push_output(&mut log, &mut open, "ge verify\nok\n");
        strip_markers(&mut log, open);
        assert_eq!(log, vec!["Hit:1 http://x", "Get:2 http://y", "ok"]);
    }
}
