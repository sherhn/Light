//! Creation destinations: local folders or a regular WSL user's ~/projects.

use crate::ui_project_browser::{Explorer, Outcome, Purpose, Side, display_path};
use crate::{
    i18n::tr,
    icons::Icon,
    theme::Palette,
    widgets::{self, TextField},
};
use eframe::egui::{self, RichText};
use std::{path::PathBuf, sync::mpsc, time::Duration};

pub struct ProjectLocation {
    pub linux: bool,
    pub distro: String,
    pub parent: String,
    pub job: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    pub error: Option<String>,
    /// The explorer choosing the folder in Linux, while it is open.
    pub browse: Option<Box<Explorer>>,
}

impl ProjectLocation {
    pub fn new(distro: String, linux: bool) -> Self {
        Self {
            linux,
            distro,
            parent: "~/projects".into(),
            job: None,
            error: None,
            browse: None,
        }
    }

    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        distros: &[String],
        local_parent: &mut String,
        busy: bool,
    ) {
        ui.add_enabled_ui(!busy, |ui| {
            if cfg!(windows) {
                if let Some(index) = widgets::segmented(
                    ui,
                    pal,
                    &[tr("Windows"), tr("Linux (WSL)")],
                    usize::from(self.linux),
                ) {
                    self.linux = index == 1;
                    self.error = None;
                }
                ui.add_space(8.0);
            }
            if self.linux {
                if self.distro.is_empty()
                    && let Some(distro) = distros.first()
                {
                    self.distro = distro.clone();
                }
                let shown = self.distro.clone();
                widgets::dropdown(ui, pal, "new_project_distro", 260.0, &shown, |ui| {
                    for distro in distros {
                        let icon = if *distro == self.distro { Icon::Check } else { Icon::Linux };
                        if widgets::menu_row(ui, pal, Some(icon), distro, "").clicked() {
                            self.distro = distro.clone();
                            ui.close();
                        }
                    }
                });
                ui.add_space(6.0);
                widgets::right_row(ui, widgets::FIELD_H, |ui| {
                    if widgets::secondary_button(ui, pal, tr("Browse...")).clicked() && !self.distro.is_empty() {
                        let mut explorer = Explorer::new(Side::Linux, Purpose::Pick, self.distro.clone(), self.parent.clone());
                        explorer.start();
                        self.browse = Some(Box::new(explorer));
                    }
                    TextField::new(&mut self.parent).hint("~/projects").icon(crate::icons::Icon::Linux).show(ui, pal);
                });
                ui.label(
                    RichText::new(tr(
                        "Projects belong to your regular WSL user. ~ is their home folder.",
                    ))
                    .color(pal.text_dim)
                    .size(12.0),
                );
                if self.distro.is_empty() {
                    ui.label(tr("No Linux distribution is installed yet"));
                }
            } else {
                widgets::right_row(ui, widgets::FIELD_H, |ui| {
                    if widgets::secondary_button(ui, pal, tr("Browse...")).clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        *local_parent = dir.to_string_lossy().into_owned();
                    }
                    TextField::new(local_parent).show(ui, pal);
                });
            }
        });
        if self.job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Creating project..."));
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(pal.error, crate::i18n::tr_dyn(error));
        }
        // the folder in Linux, chosen in an explorer over the dialog
        if let Some(explorer) = self.browse.as_mut() {
            match crate::ui_project_browser::explorer_window(ui.ctx(), pal, "new_project_location", tr("Where to create the project"), explorer, distros, false) {
                Outcome::Choose(path) => {
                    self.parent = display_path(&path);
                    self.distro = explorer.distro.clone();
                    self.browse = None;
                }
                Outcome::Close => self.browse = None,
                _ => {}
            }
        }
    }

    pub fn start(
        &mut self,
        local_parent: &str,
        name: &str,
        files: std::collections::BTreeMap<String, String>,
    ) {
        let linux = if self.linux {
            Some((self.distro.clone(), self.parent.clone()))
        } else {
            ide_core::wsl::wsl_path(std::path::Path::new(local_parent))
                .map(|path| (path.distro, path.linux))
        };
        let (local_parent, name) = (local_parent.to_string(), name.to_string());
        let (tx, rx) = mpsc::channel();
        self.error = None;
        self.job = Some(rx);
        std::thread::spawn(move || {
            let result = match linux {
                Some((distro, parent)) => create_linux_project(&distro, &parent, &name, &files),
                None => ide_core::workspace::create_project(
                    std::path::Path::new(&local_parent),
                    &name,
                    &files,
                )
                .map_err(|error| error.to_string()),
            };
            let _ = tx.send(result);
        });
    }

    pub fn poll(&mut self) -> Option<PathBuf> {
        let result = match self.job.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Project creation stopped. Try again.".into())
            }
        };
        self.job = None;
        match result {
            Ok(root) => Some(root),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

fn create_linux_project(
    distro: &str,
    parent: &str,
    name: &str,
    files: &std::collections::BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    if distro.is_empty() {
        return Err("Choose a Linux distribution first.".into());
    }
    let user = lisp_runtime::wsl::project_user(distro)?;
    let parent = lisp_runtime::wsl::project_parent(&user, parent)?;
    let name = ide_core::workspace::sanitize_project_name(name);
    let root = format!("{}/{name}", parent.trim_end_matches('/'));
    lisp_runtime::wsl::ensure_project_parent(distro, &user, &parent)?;
    // mkdir without -p refuses an existing destination; no files can be overwritten.
    let cmd =
        lisp_runtime::wsl::project_command(distro, &user, "mkdir", &["--".into(), root.clone()]);
    let output = lisp_runtime::wsl::capture(cmd, Duration::from_secs(30))
        .map_err(|error| error.to_string())?;
    if !output.ok() {
        return Err(output.text.trim().into());
    }
    if files.is_empty() {
        Ok(ide_core::wsl::windows_path(distro, &root))
    } else {
        ide_core::workspace::create_project(
            &ide_core::wsl::windows_path(distro, &parent),
            &name,
            files,
        )
        .map_err(|error| error.to_string())
    }
}
