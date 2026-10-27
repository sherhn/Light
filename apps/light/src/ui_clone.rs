//! "Get from Version Control": clone a GitHub (or any git) repository into a folder and open it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use eframe::egui::{self, Align2, Key, Margin, RichText, Sense, Vec2};
use ide_core::clone;

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::input::dirs_home;
use crate::state::*;
use crate::widgets::{self, TextField};

impl App {
    pub fn open_clone_dialog(&mut self) {
        let parent = if self.settings.clone_dir.trim().is_empty() { dirs_home().join("LightProjects").to_string_lossy().into_owned() } else { self.settings.clone_dir.clone() };
        let linux_distro = self.suggested_distro().unwrap_or_default();
        self.popup = Popup::Clone(Box::new(CloneState {
            tab: CloneTab::Url,
            gh_selected: None,
            url: String::new(),
            dir_touched: false,
            parent,
            branch: String::new(),
            shallow: false,
            error: None,
            just_opened: true,
            run: None,
            linux: self.wsl_target().is_some(),
            linux_distro,
            linux_dir: "~/projects".into(),
            build_in_linux: false,
            modules: vec![],
            show_modules: false,
        }));
        // which Linux distributions there are, for "Clone into Linux"
        if cfg!(windows) && self.wsl.probe.is_none() && !self.wsl.probing() {
            self.wsl_probe();
        }
        // a login the CLI or Git keeps for us is picked up again on its own
        if self.settings.github_autologin && !self.github.signed_in() && !self.github.signing_in {
            self.github_sign_in_saved();
        }
    }

    fn start_clone(&mut self, st: &mut CloneState) {
        let url = match clone::normalize_url(&st.url) {
            Ok(u) => u,
            Err(e) => {
                st.error = Some(e);
                return;
            }
        };
        if st.linux && st.linux_distro.is_empty() {
            st.error = Some(tr("Choose a Linux distribution first.").into());
            return;
        }
        let selected_wsl = ide_core::wsl::wsl_path(std::path::Path::new(st.parent.trim()));
        let linux = st.linux || selected_wsl.is_some();
        if !linux && st.parent.trim().is_empty() {
            st.error = Some(tr("Choose a directory to clone into").into());
            return;
        }
        if linux && st.linux_dir.trim().is_empty() {
            st.error = Some(tr("Choose a folder in Linux to clone into").into());
            return;
        }
        let name = clone::default_dir_name(&url);
        let dest = PathBuf::from(st.parent.trim()).join(&name);
        // signed in to GitHub: private repositories clone too (the token reaches git for github.com only)
        let env = self.github.token.as_deref().map(|t| ide_core::github::git_auth_env(&url, t)).unwrap_or_default();
        let mut opts = clone::CloneOptions { shallow: st.shallow, branch: Some(st.branch.clone()).filter(|b| !b.trim().is_empty()), env, lf: st.build_in_linux && !linux, ..Default::default() };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (c2, d2) = (cancel.clone(), dest.clone());
        st.error = None;
        let (distro, linux_dir) = match selected_wsl.filter(|_| !st.linux) {
            Some(path) => (path.distro, path.linux),
            None => (st.linux_distro.clone(), st.linux_dir.trim().to_string()),
        };
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let mut dest = d2;
            if linux {
                let _ = tx.send(CloneMsg::Line(format!("Starting {distro}...")));
                let destination = (|| {
                    let user = lisp_runtime::wsl::project_user(&distro)?;
                    let parent = lisp_runtime::wsl::project_parent(&user, &linux_dir)?;
                    lisp_runtime::wsl::ensure_project_parent(&distro, &user, &parent)?;
                    Ok::<_, String>((user, parent))
                })();
                let (user, parent) = match destination {
                    Ok(destination) => destination,
                    Err(error) => { let _ = tx.send(CloneMsg::Done(Err(error))); return; }
                };
                if c2.load(Ordering::Relaxed) { let _ = tx.send(CloneMsg::Done(Err("Cancelled".into()))); return; }
                opts.wsl_user = Some(user.name);
                dest = ide_core::wsl::windows_path(&distro, &format!("{}/{name}", parent.trim_end_matches('/')));
                let _ = tx.send(CloneMsg::Dest(dest.clone()));
            }
            let r = clone::clone_repo(&url, &dest, &opts, &c2, &mut |l| {
                let _ = tx2.send(CloneMsg::Line(l.to_string()));
            });
            let _ = tx.send(CloneMsg::Done(r));
        });
        st.run = Some(CloneRun { rx, cancel, progress: 0.0, status: tr("Connecting...").into(), dest });
    }

    pub fn draw_clone(&mut self, ctx: &egui::Context) {
        if !matches!(self.popup, Popup::Clone(_)) {
            return;
        }
        let Popup::Clone(mut st) = std::mem::replace(&mut self.popup, Popup::None) else { return };
        let pal = self.pal;
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let picker = if st.show_modules { Some(self.picker_data(ctx)) } else { None };

        // progress from the background thread
        let mut finished: Option<Result<(), String>> = None;
        if let Some(run) = st.run.as_mut() {
            while let Ok(m) = run.rx.try_recv() {
                match m {
                    CloneMsg::Line(l) => {
                        if let Some(p) = clone::parse_progress(&l) {
                            run.progress = p / 100.0;
                        }
                        run.status = l;
                    }
                    CloneMsg::Dest(d) => run.dest = d,
                    CloneMsg::Done(r) => finished = Some(r),
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
        let mut open_dest: Option<PathBuf> = None;
        if let Some(r) = finished {
            let run = st.run.take().expect("running");
            match r {
                Ok(()) => open_dest = Some(run.dest),
                Err(e) => st.error = Some(i18n::tr_dyn(&e).into_owned()),
            }
        }

        let mut close = esc;
        let mut start = false;
        let running = st.run.is_some();
        // Enter belongs to the text field being edited: on the GitHub tab it signs in or searches, it does not clone
        let enter = enter && st.tab == CloneTab::Url;
        egui::Area::new(egui::Id::unique("clone_dialog")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            let r = widgets::glass_frame(&pal).show(ui, |ui| {
                ui.set_width(700.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                    crate::icons::paint(ui.painter(), r, Icon::Download, pal.accent);
                    ui.label(RichText::new(tr("Get from Version Control")).font(fonts::semibold(17.0)));
                });
                ui.add_space(4.0);
                ui.label(RichText::new(tr("Clone a repository from GitHub or any Git URL and open it as a project.")).color(pal.text_dim).size(12.5));
                ui.add_space(14.0);
                if !running && let Some(i) = widgets::segmented(ui, &pal, &[tr("Repository URL"), tr("GitHub")], usize::from(st.tab == CloneTab::GitHub)) {
                    st.tab = if i == 0 { CloneTab::Url } else { CloneTab::GitHub };
                    st.error = None;
                }
                ui.add_space(14.0);

                if st.tab == CloneTab::Url {
                    widgets::caption(ui, &pal, &tr("Repository URL").to_uppercase());
                    ui.add_space(6.0);
                    let id = egui::Id::unique("clone_url");
                    let r = TextField::new(&mut st.url).id(id).hint(tr("https://github.com/owner/repo  or  owner/repo")).icon(Icon::Globe).enabled(!running).show(ui, &pal);
                    if r.changed() {
                        st.error = None;
                        st.gh_selected = None;
                    }
                    widgets::focus_once(ui, &r, &mut st.just_opened);
                } else if self.draw_github_tab(ui, &mut st, running) {
                    start = true;
                }
                ui.add_space(16.0);

                let distros: Vec<String> = self.wsl.distros();
                if !distros.is_empty() {
                    widgets::caption(ui, &pal, &tr("Clone into").to_uppercase());
                    ui.add_space(6.0);
                    if st.linux_distro.is_empty() || !distros.contains(&st.linux_distro) {
                        st.linux_distro = self.suggested_distro().unwrap_or_else(|| distros[0].clone());
                    }
                    if !running && let Some(i) = widgets::segmented(ui, &pal, &[tr("Windows folder"), &i18n::fmt("Linux: {}", &[&st.linux_distro])], usize::from(st.linux)) {
                        st.linux = i == 1;
                    }
                    ui.add_space(12.0);
                }
                let repo_name = if st.tab == CloneTab::Url || st.gh_selected.is_some() {
                    clone::normalize_url(&st.url).ok().map(|url| clone::default_dir_name(&url)).unwrap_or_default()
                } else {
                    String::new()
                };
                if st.linux {
                    widgets::caption(ui, &pal, &tr("Folder in Linux").to_uppercase());
                    ui.add_space(6.0);
                    widgets::right_row(ui, widgets::FIELD_H, |ui| {
                        if distros.len() > 1 {
                            let resp = widgets::Pill::new(&st.linux_distro).icon(Icon::Linux, pal.text_dim).chevron().bordered().height(widgets::FIELD_H).show(ui, &pal);
                            egui::Popup::menu(&resp).width(220.0).show(|ui| {
                                for d in &distros {
                                    if ui.button(d).clicked() {
                                        st.linux_distro = d.clone();
                                        ui.close();
                                    }
                                }
                            });
                        }
                        let suffix = clone_dir_suffix(&st.linux_dir, &repo_name, true);
                        TextField::new(&mut st.linux_dir).hint("~/projects").suffix(&suffix).icon(Icon::Linux).enabled(!running).show(ui, &pal);
                    });
                    ui.label(RichText::new(tr("Projects belong to your regular WSL user. ~ is their home folder.")).color(pal.text_dim).size(12.0));
                } else {
                    widgets::caption(ui, &pal, &tr("Parent directory").to_uppercase());
                    ui.add_space(6.0);
                    widgets::right_row(ui, widgets::FIELD_H, |ui| {
                        if !running && widgets::secondary_button(ui, &pal, tr("Browse...")).clicked()
                            && let Some(d) = rfd::FileDialog::new().set_title(tr("Clone into")).pick_folder()
                        {
                            st.parent = d.to_string_lossy().into_owned();
                            st.dir_touched = true;
                        }
                        let suffix = clone_dir_suffix(&st.parent, &repo_name, false);
                        TextField::new(&mut st.parent).suffix(&suffix).icon(Icon::Folder).enabled(!running).show(ui, &pal);
                    });
                    if !distros.is_empty() {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if !running {
                                widgets::toggle(ui, &pal, &mut st.build_in_linux);
                            }
                            ui.label(RichText::new(i18n::fmt("Build and run it in Linux ({}): keeps Linux line endings", &[&st.linux_distro])).color(pal.text_dim).size(12.5));
                        });
                    }
                }
                if st.tab == CloneTab::GitHub && st.gh_selected.is_none() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(tr("Pick a repository from the list")).color(pal.text_dim).size(12.0));
                } else if let Ok(u) = clone::normalize_url(&st.url) {
                    let target = if st.linux {
                        i18n::fmt("{0}/{1}  in {2}", &[&st.linux_dir.trim().trim_end_matches('/'), &clone::default_dir_name(&u), &st.linux_distro])
                    } else {
                        PathBuf::from(st.parent.trim()).join(clone::default_dir_name(&u)).display().to_string()
                    };
                    ui.add_space(8.0);
                    ui.label(RichText::new(i18n::fmt("Will be cloned into  {}", &[&target])).color(pal.text_dim).size(12.0));
                }
                ui.add_space(16.0);

                let col_w = (ui.available_width() - 24.0) / 2.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 24.0;
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        widgets::caption(ui, &pal, &tr("Branch (optional)").to_uppercase());
                        ui.add_space(6.0);
                        TextField::new(&mut st.branch).hint(tr("default branch")).icon(Icon::Branch).enabled(!running).show(ui, &pal);
                    });
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        widgets::caption(ui, &pal, &tr("Shallow clone").to_uppercase());
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.set_min_height(widgets::FIELD_H);
                            if !running {
                                widgets::toggle(ui, &pal, &mut st.shallow);
                            }
                            ui.label(RichText::new(tr("latest commit only")).color(pal.text_dim).size(12.5));
                        });
                    });
                });

                // the modules the project is to work with: collapsed until asked for
                ui.add_space(16.0);
                let (hr, hresp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
                let label = if st.modules.is_empty() { tr("Modules").to_uppercase() } else { format!("{}  ({})", tr("Modules").to_uppercase(), st.modules.len()) };
                ui.painter().text(hr.left_center(), egui::Align2::LEFT_CENTER, label, fonts::semibold(11.5), pal.text_dim);
                icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(hr.right() - 10.0, hr.center().y), Vec2::splat(14.0)), if st.show_modules { Icon::ChevronUp } else { Icon::ChevronDown }, pal.text_dim);
                if hresp.clicked() && !running {
                    st.show_modules = !st.show_modules;
                }
                if let Some(data) = &picker {
                    ui.add_space(6.0);
                    crate::ui_modules::module_picker(ui, &pal, data, &mut st.modules);
                }

                if let Some(run) = st.run.as_ref() {
                    ui.add_space(18.0);
                    widgets::progress_bar(ui, &pal, run.progress, ui.available_width());
                    ui.add_space(6.0);
                    ui.label(RichText::new(run.status.trim()).color(pal.text_dim).size(12.0));
                }
                if let Some(e) = st.error.as_ref() {
                    ui.add_space(14.0);
                    egui::Frame::new().fill(pal.error.gamma_multiply(0.16)).corner_radius(widgets::radius()).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(e).color(pal.error).size(12.5));
                    });
                }

                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can = st.tab == CloneTab::Url || st.gh_selected.is_some();
                        ui.add_enabled_ui(!running && can, |ui| {
                            if widgets::primary_button(ui, &pal, tr("Clone")).clicked() {
                                start = true;
                            }
                        });
                        if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                            close = true;
                        }
                    });
                });
            });
            widgets::sheen(ui.painter(), r.response.rect, &pal);
        });

        if enter && !running {
            start = true;
        }
        if start && !running && (st.tab == CloneTab::Url || st.gh_selected.is_some()) {
            self.start_clone(&mut st);
        }
        if close {
            if let Some(run) = st.run.as_ref() {
                run.cancel.store(true, Ordering::Relaxed);
            }
            self.editor_focus = true;
            return;
        }
        if let Some(dest) = open_dest {
            // the project's own settings: where it is built and which modules it works with
            let linux_choice = st.build_in_linux && !st.linux && !st.linux_distro.is_empty();
            if linux_choice || !st.modules.is_empty() {
                let mut cfg = crate::app_env::load_config(&dest);
                if linux_choice {
                    // the project opens in Linux right away
                    cfg.linux = Some(st.linux_distro.clone());
                    cfg.decided = true;
                }
                cfg.modules = st.modules.clone();
                if let Err(e) = crate::app_env::save_config(&dest, &cfg) {
                    self.toast(i18n::fmt("Cannot save the project settings: {}", &[&e]), Sev::Error);
                }
            }
            for id in st.modules.clone() {
                if !self.mods.installed.iter().any(|m| m.manifest.id == id) {
                    self.module_install(&id);
                }
            }
            if !st.linux {
                self.settings.clone_dir = st.parent.trim().to_string();
            }
            self.save_settings();
            self.toast(i18n::fmt("Cloned into {}", &[&dest.display()]), Sev::Weak);
            self.request_open_project(&dest);
            self.editor_focus = true;
            return;
        }
        self.popup = Popup::Clone(st);
    }
}

/// Preview the repository folder using the parent path's separator style.
fn clone_dir_suffix(parent: &str, name: &str, linux: bool) -> String {
    let parent = parent.trim();
    if parent.is_empty() || name.is_empty() {
        return String::new();
    }
    if parent.ends_with('/') || (!linux && parent.ends_with('\\')) {
        return name.to_string();
    }
    let separator = if linux || parent.contains('/') { '/' } else { std::path::MAIN_SEPARATOR };
    format!("{separator}{name}")
}
