//! Where a project is built and run: on Windows, or in Linux through WSL.
//!
//! A project on a WSL file system (`\\wsl.localhost\Ubuntu\home\...`) is always a Linux project. Any other project
//! can be switched; the choice is kept in `.light/project.json`, which git never sees. A Linux project runs its
//! terminal, make targets, Lisp and REPL in the distribution, and its git runs there when it lives there.

use std::path::Path;

use eframe::egui::{self, Align, Layout, RichText, Sense, Vec2};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::widgets;

/// A run scenario: console commands that run one after another (see `app_scenario`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scenario {
    pub name: String,
    pub commands: Vec<String>,
    /// Stop at the first command that fails (exits with another code than 0).
    pub stop_on_error: bool,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario { name: String::new(), commands: vec![], stop_on_error: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// The distribution the project is built and run in; `None`: Windows.
    pub linux: Option<String>,
    /// The user chose (answered the question or switched by hand); until then the global setting applies.
    pub decided: bool,
    /// Modules the project asks for (chosen when it was created or imported): they are offered when missing.
    pub modules: Vec<String>,
    /// Lists of console commands the project runs with one click.
    pub scenarios: Vec<Scenario>,
}

fn config_path(root: &Path) -> std::path::PathBuf {
    ide_core::paths::project_data_dir(root).join("project.json")
}

pub fn load_config(root: &Path) -> ProjectConfig {
    std::fs::read_to_string(config_path(root)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_config(root: &Path, cfg: &ProjectConfig) -> std::io::Result<()> {
    let p = config_path(root);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(cfg).unwrap_or_default())?;
    // like the local commits, the project settings never become a change in git
    if let Some(repo) = ide_core::git::find_repo(root) {
        ide_core::localvcs::LocalVcs::ensure_git_ignored(&repo);
    }
    Ok(())
}

/// A project that is built the Unix way: a makefile, a configure script or shell scripts at the top.
pub fn looks_like_linux_project(root: &Path) -> bool {
    if ide_core::build::find_makefile(root).is_some() || root.join("configure").is_file() || root.join("CMakeLists.txt").is_file() {
        return true;
    }
    std::fs::read_dir(root).map(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "sh"))).unwrap_or(false)
}

/// Distributions that are not for working in (Docker Desktop's own).
pub fn usable_distro(name: &str) -> bool {
    !name.to_ascii_lowercase().starts_with("docker-desktop")
}

impl App {
    /// Read the project's environment when it is opened.
    pub fn load_project_env(&mut self) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            self.project_cfg = ProjectConfig::default();
            self.env_banner = false;
            self.unix_project = false;
            return;
        };
        self.project_cfg = load_config(&root);
        self.unix_project = looks_like_linux_project(&root);
        self.project_modules.clear();
        self.env_banner_dismissed = false;
        self.forget_build_tools();
        self.update_env_banner();
        self.lisp = self.resolve_lisp();
        self.refresh_make_targets();
    }

    /// Does this project belong in Linux as far as anyone can tell: built the Unix way, or in languages whose modules
    /// bring tools for Linux (C, Python, assembly...)?
    pub fn linux_relevant(&self) -> bool {
        self.unix_project || !self.project_modules.is_empty()
    }

    /// Ask whether the project should run in Linux, when it is not decided and there is a reason to.
    pub fn update_env_banner(&mut self) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            self.env_banner = false;
            return;
        };
        let on_linux_fs = ide_core::wsl::wsl_path(&root).is_some();
        self.env_banner = cfg!(windows) && !on_linux_fs && !self.project_cfg.decided && !self.env_banner_dismissed && self.wsl_target().is_none() && self.linux_relevant();
        if self.env_banner && self.wsl.probe.is_none() && !self.wsl.probing() {
            self.wsl_probe();
        }
    }

    /// The project lives on a WSL file system (and can only be a Linux project).
    pub fn project_on_linux_fs(&self) -> bool {
        self.project_root().is_some_and(|r| ide_core::wsl::wsl_path(r).is_some())
    }

    /// The distribution the project asks for: the one it lives in, else the one chosen for it. `None` also when
    /// the project has not chosen: then the global setting decides.
    pub fn project_distro(&self) -> Option<String> {
        let root = self.project_root()?;
        if let Some(w) = ide_core::wsl::wsl_path(root) {
            return Some(w.distro);
        }
        self.project_cfg.linux.clone()
    }

    /// The project chose Windows explicitly.
    pub fn project_on_windows(&self) -> bool {
        self.project.is_some() && !self.project_on_linux_fs() && self.project_cfg.decided && self.project_cfg.linux.is_none()
    }

    /// Build and run the project in `distro` (`None`: on Windows) from now on.
    pub fn set_project_env(&mut self, distro: Option<String>) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else { return };
        if self.project_on_linux_fs() {
            return;
        }
        self.project_cfg = ProjectConfig { linux: distro.clone(), decided: true, ..self.project_cfg.clone() };
        if let Err(e) = save_config(&root, &self.project_cfg) {
            self.toast(i18n::fmt("Cannot save the project settings: {}", &[&e]), Sev::Error);
        }
        self.env_banner = false;
        self.forget_build_tools();
        self.lisp_environment_changed();
        match distro {
            Some(d) => self.toast(i18n::fmt("{0} is now built and run in Linux ({1})", &[&self.project.as_ref().map(|p| p.name.clone()).unwrap_or_default(), &d]), Sev::Weak),
            None => self.toast(i18n::fmt("{} is now built and run on Windows", &[&self.project.as_ref().map(|p| p.name.clone()).unwrap_or_default()]), Sev::Weak),
        }
    }

    /// The distribution to offer for a project: the one Lisp already uses, else the first usable one.
    pub fn suggested_distro(&self) -> Option<String> {
        let distros: Vec<String> = self.wsl.distros().into_iter().filter(|d| usable_distro(d)).collect();
        let cfg = self.settings.wsl_distro.trim();
        if !cfg.is_empty() && distros.iter().any(|d| d == cfg) {
            return Some(cfg.to_string());
        }
        self.wsl.probe.as_ref().and_then(|p| p.default_distro()).map(|d| d.name.clone()).filter(|d| usable_distro(d)).or_else(|| distros.into_iter().next())
    }

    // ------------------------------------------------------------------ build tools

    /// Look for the build tools in the project's distribution again (something may have been installed meanwhile) and
    /// offer to install them again if they are missing.
    fn forget_build_tools(&mut self) {
        self.tools_banner_dismissed = false;
        if let Some(t) = self.wsl_target() {
            self.wsl.tools.remove(&t.distro);
        }
    }

    /// The project builds in Linux and the distribution lacks tools it needs: (distribution, missing programs).
    pub fn missing_build_tools(&self) -> Option<(String, Vec<String>)> {
        if !self.linux_relevant() {
            return None;
        }
        let t = self.wsl_target()?;
        let Some(Ok(found)) = self.wsl.tools.get(&t.distro) else { return None };
        // what a project built the Unix way needs; the other developer tools are offered on the Linux page
        let mut missing = if self.unix_project { lisp_runtime::wsl::missing_core(found) } else { vec![] };
        // and what the languages of its files cannot do without (python3 for Python...)
        for m in self.mods.installed.iter().filter(|m| m.enabled && self.project_modules.contains(&m.manifest.id)) {
            for p in m.manifest.tools.iter().flat_map(|t| t.required_programs()) {
                if found.iter().any(|(n, ok)| *n == p && !*ok) && !missing.contains(&p) {
                    missing.push(p);
                }
            }
        }
        (!missing.is_empty()).then_some((t.distro, missing))
    }

    /// Whether the "install the build tools" banner is on screen.
    pub fn tools_banner_visible(&self) -> bool {
        !self.env_banner && !self.tools_banner_dismissed && self.wsl.task.is_none() && self.missing_build_tools().is_some()
    }

    /// What to tell when a command was not found in Linux (exit code 127): the program, if known. Also makes the
    /// banner come back, after a fresh look at what is installed. `None`: the project does not run in Linux.
    pub fn linux_command_not_found(&mut self, program: Option<&str>) -> Option<String> {
        let t = self.wsl_target()?;
        self.forget_build_tools();
        let known = lisp_runtime::wsl::all_programs(&self.tool_groups());
        let is_tool = |p: &str| lisp_runtime::wsl::BUILD_TOOLS.contains(&p) || p.starts_with("qemu") || matches!(p, "cc" | "g++" | "c++" | "as" | "ar" | "nm" | "objdump") || known.iter().any(|k| k == p);
        Some(match program.filter(|p| is_tool(p)) {
            Some(p) => i18n::fmt("{0} is not installed in Linux ({1}). Install the developer tools: Settings > Linux (WSL) > Install developer tools.", &[&p, &t.distro]),
            None => i18n::fmt("Command not found in Linux ({}). Developer tools (make, gcc, nasm, gdb, QEMU and more) can be installed in Settings > Linux (WSL).", &[&t.distro]),
        })
    }

    // ------------------------------------------------------------------ UI

    /// Offer to install what a project built with make needs, when its Linux lacks it.
    pub fn draw_tools_banner(&mut self, ui: &mut egui::Ui) {
        // the check runs once per project (and again when a command was not found)
        if self.linux_relevant()
            && let Some(t) = self.wsl_target()
            && !self.wsl.tools.contains_key(&t.distro)
            && !self.wsl_checking_tools()
        {
            self.wsl_check_tools(&t.distro);
        }
        if !self.tools_banner_visible() {
            return;
        }
        let Some((distro, missing)) = self.missing_build_tools() else { return };
        let pal = self.pal;
        let (mut install, mut later) = (false, false);
        let list = if missing.len() > 4 { format!("{}…", missing[..4].join(", ")) } else { missing.join(", ") };
        egui::Frame::new().fill(pal.warning.gamma_multiply(0.14)).corner_radius(widgets::radius()).inner_margin(egui::Margin::symmetric(14, 10)).outer_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Hammer, pal.warning);
                ui.vertical(|ui| {
                    ui.label(RichText::new(i18n::fmt("Build tools are missing in {}", &[&distro])).font(fonts::semibold(13.0)));
                    ui.label(RichText::new(i18n::fmt("Needed to build and run this project, but not found: {}", &[&list])).color(pal.text_dim).size(12.0));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, &pal, Icon::Close, 26.0, pal.text_dim, tr("Not now")).clicked() {
                        later = true;
                    }
                    if widgets::primary_button(ui, &pal, tr("Install developer tools")).clicked() {
                        install = true;
                    }
                });
            });
        });
        if later {
            self.tools_banner_dismissed = true;
        }
        if install {
            self.install_build_tools_here(&distro, &missing);
        }
    }

    /// Install the developer tools and show the progress on the Linux page; the groups that bring the `missing`
    /// programs (the project cannot be built without them) are installed whole, whatever was chosen.
    pub fn install_build_tools_here(&mut self, distro: &str, missing: &[String]) {
        self.wsl_install_tools_with(distro, missing);
        self.open_settings_page(crate::ui_settings::WSL_PAGE);
    }

    /// The question shown on top of the editor when a Unix-style project is opened on Windows.
    pub fn draw_env_banner(&mut self, ui: &mut egui::Ui) {
        if !self.env_banner {
            return;
        }
        let pal = self.pal;
        let suggested = self.suggested_distro();
        let checking = self.wsl.probing();
        let mut act: Option<Option<Option<String>>> = None;
        let mut setup = false;
        egui::Frame::new().fill(pal.accent.gamma_multiply(0.12)).corner_radius(widgets::radius()).inner_margin(egui::Margin::symmetric(14, 10)).outer_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Linux, pal.accent);
                ui.vertical(|ui| {
                    // a makefile or shell scripts say so; otherwise it is the languages of the files that can run there too
                    let title = if self.unix_project { tr("This project is built the Linux way (make, shell scripts)") } else { tr("This project can run in Linux") };
                    ui.label(RichText::new(title).font(fonts::semibold(13.0)));
                    ui.label(RichText::new(tr("Build it, run it and use the terminal in Linux through WSL? Files stay where they are.")).color(pal.text_dim).size(12.0));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, &pal, Icon::Close, 26.0, pal.text_dim, tr("Not now")).clicked() {
                        act = Some(None);
                    }
                    if widgets::secondary_button(ui, &pal, tr("Keep Windows")).clicked() {
                        act = Some(Some(None));
                    }
                    match &suggested {
                        Some(d) => {
                            if widgets::primary_button(ui, &pal, &i18n::fmt("Use Linux ({})", &[d])).clicked() {
                                act = Some(Some(Some(d.clone())));
                            }
                        }
                        None if checking => {
                            ui.add(egui::Spinner::new().size(16.0).color(pal.accent));
                        }
                        None => {
                            if widgets::primary_button(ui, &pal, tr("Set up Linux...")).clicked() {
                                setup = true;
                            }
                        }
                    }
                });
            });
        });
        if setup {
            self.open_settings_page(crate::ui_settings::WSL_PAGE);
        }
        match act {
            Some(None) => {
                self.env_banner = false;
                self.env_banner_dismissed = true;
            }
            Some(Some(choice)) => self.set_project_env(choice),
            None => {}
        }
    }

    /// Status bar: where the project (or Lisp) runs, and a menu to switch it.
    pub fn draw_env_chip(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let target = self.wsl_target();
        if self.project.is_none() && target.is_none() {
            return;
        }
        let (text, color) = match &target {
            Some(t) => (i18n::fmt("Linux: {}", &[&t.distro]), pal.accent),
            None => (tr("Windows").to_string(), pal.text_dim),
        };
        let resp = ui.add(egui::Label::new(RichText::new(text).color(color).font(fonts::ui(12.0))).sense(Sense::click())).on_hover_text(tr("Where this project is built and run - click to change"));
        let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
        icons::paint(ui.painter(), r, if target.is_some() { Icon::Linux } else { Icon::Terminal }, color);
        if resp.clicked() && self.wsl.probe.is_none() && !self.wsl.probing() {
            self.wsl_probe();
        }
        let linux_fs = self.project_on_linux_fs();
        let distros: Vec<String> = self.wsl.distros().into_iter().filter(|d| usable_distro(d)).collect();
        let current = self.project_distro();
        let has_project = self.project.is_some();
        let mut choice: Option<Option<String>> = None;
        let mut setup = false;
        egui::Popup::menu(&resp).width(260.0).show(|ui| {
            if !has_project {
                ui.label(RichText::new(tr("Open a project to choose where it runs")).color(pal.text_dim).size(12.0));
            } else if linux_fs {
                ui.label(RichText::new(tr("This project lives in Linux, so it always runs there")).color(pal.text_dim).size(12.0));
            } else {
                ui.label(RichText::new(tr("Build and run this project:")).color(pal.text_dim).size(12.0));
                if ui.selectable_label(current.is_none(), tr("On Windows")).clicked() {
                    choice = Some(None);
                    ui.close();
                }
                for d in &distros {
                    if ui.selectable_label(current.as_deref() == Some(d.as_str()), i18n::fmt("In Linux: {}", &[d])).clicked() {
                        choice = Some(Some(d.clone()));
                        ui.close();
                    }
                }
                if distros.is_empty() {
                    ui.label(RichText::new(if self.wsl.probing() { tr("Looking for Linux distributions...") } else { tr("No Linux distribution is installed yet") }).color(pal.text_dim).size(12.0));
                }
            }
            ui.separator();
            if ui.button(tr("Linux (WSL) setup...")).clicked() {
                setup = true;
                ui.close();
            }
        });
        if let Some(c) = choice {
            self.set_project_env(c);
        }
        if setup {
            self.open_settings_page(crate::ui_settings::WSL_PAGE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_project_scenarios_keep_their_configuration() {
        let root = std::env::temp_dir().join(format!("light-legacy-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".air")).unwrap();
        let cfg = ProjectConfig { decided: true, modules: vec!["python".into()], scenarios: vec![Scenario { name: "test".into(), commands: vec!["python -m unittest".into()], stop_on_error: true }], ..Default::default() };
        std::fs::write(root.join(".air/project.json"), serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(load_config(&root), cfg);
        save_config(&root, &cfg).unwrap();
        assert!(!root.join(".light").exists());
        assert_eq!(load_config(&root).scenarios, cfg.scenarios);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_roundtrip_and_detection() {
        let d = std::env::temp_dir().join(format!("light-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(load_config(&d), ProjectConfig::default());
        assert!(!looks_like_linux_project(&d));
        std::fs::write(d.join("makefile"), "all:\n").unwrap();
        assert!(looks_like_linux_project(&d));
        let cfg = ProjectConfig { linux: Some("Ubuntu-24.04".into()), decided: true, modules: vec!["python".into()], scenarios: vec![Scenario { name: "build".into(), commands: vec!["make".into(), "make test".into()], stop_on_error: false }] };
        save_config(&d, &cfg).unwrap();
        assert_eq!(load_config(&d), cfg);
        assert!(usable_distro("Ubuntu") && !usable_distro("docker-desktop") && !usable_distro("docker-desktop-data"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
