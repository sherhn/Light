//! The banner that offers a language server Light can install for a language that has none: what it gives, then the
//! install as it runs, or why it failed. And the language servers of the Settings: where each stands, to install or
//! remove.

use eframe::egui::{self, Align, Layout, RichText, Sense, Stroke, Vec2};

use ide_core::lsp::install::Status;

use crate::app::App;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::widgets;

impl App {
    pub fn draw_lsp_banner(&mut self, ui: &mut egui::Ui) {
        if self.env_banner || self.doc().is_some_and(|d| d.page.is_some()) {
            return;
        }
        let Some(offer) = self.lsp.offer.clone() else { return };
        let pal = self.pal;
        let name = offer.def.name.clone();
        let (mut install, mut later) = (false, false);
        let failed = self.lsp.install_failed.clone().filter(|(s, _, _)| *s == name);
        let running = self.lsp.installing.as_ref().map(|i| i.last.clone());
        let tint = if failed.is_some() { pal.error } else { pal.accent };
        egui::Frame::new().fill(tint.gamma_multiply(0.12)).corner_radius(widgets::radius()).stroke(Stroke::new(1.0, tint.gamma_multiply(0.4))).inner_margin(egui::Margin::symmetric(14, 10)).outer_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| {
            // as wide as the editor whatever it holds (while it installs there are no buttons to stretch it)
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                let icon = match (&running, &failed) {
                    (Some(_), _) => Icon::Spinner,
                    (None, Some(_)) => Icon::Warning,
                    _ => Icon::Code,
                };
                icons::paint(ui.painter(), r, icon, tint);
                ui.vertical(|ui| {
                    let (title, hint) = match (&running, &failed) {
                        (Some(last), _) => (i18n::fmt("Installing {}...", &[&name]), last.clone()),
                        (None, Some((_, why, log))) => (i18n::fmt("Cannot install {0}: {1}", &[&name, why]), i18n::fmt("What it printed is in {}", &[log])),
                        _ => (
                            i18n::fmt("{0}: {1} gives Go to Declaration, Find Usages, Rename and the errors in the code", &[&offer.language, &name]),
                            tr("It is not installed. Light can download and install it, for you only.").to_string(),
                        ),
                    };
                    ui.label(RichText::new(title).font(fonts::semibold(13.0)));
                    ui.label(RichText::new(hint).color(pal.text_dim).size(12.0));
                });
                if running.is_none() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, &pal, Icon::Close, 26.0, pal.text_dim, tr("Not now")).clicked() {
                            later = true;
                        }
                        let label = if failed.is_some() { tr("Try again") } else { tr("Install...") };
                        if widgets::primary_button(ui, &pal, label).clicked() {
                            install = true;
                        }
                    });
                }
            });
        });
        if running.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        }
        if later {
            self.lsp_decline_install();
        }
        if install {
            self.lsp_ask_install();
        }
    }

    /// The language servers of the modules that are on, in Settings > Modules: where each stands where the project
    /// runs, Install (what it downloads is shown first) and Remove.
    pub fn draw_lsp_settings(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let (place, rows) = self.lsp_server_rows();
        if rows.is_empty() {
            return;
        }
        let installing = self.lsp.installing.as_ref().map(|i| (i.server.clone(), i.last.clone()));
        let confirm = self.lsp.settings_confirm.clone();
        // what to do, once the rows are drawn
        let mut ask: Option<String> = None;
        let mut cancel = false;
        let mut install: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let lines: Option<Result<(Vec<String>, String), String>> = confirm.as_ref().and_then(|name| rows.iter().find(|r| r.def.name == *name)).map(|r| self.lsp_install_lines(&r.module, &r.def));
        crate::ui_settings::group(ui, &pal, tr("Language servers"), |ui, list| {
            ui.add_space(10.0);
            ui.label(RichText::new(i18n::fmt("Go to Declaration, Find Usages, Rename, completion and the errors in the code come from them. For the open project they run on {}.", &[&place])).color(pal.text_dim).size(12.0));
            ui.add_space(4.0);
            for (i, r) in rows.iter().enumerate() {
                let title = match &r.def.install {
                    Some(inst) => format!("{} {}", r.def.name, inst.version),
                    None => r.def.name.clone(),
                };
                let state = match (&installing, &r.status) {
                    (Some((name, last)), _) if *name == r.def.name => i18n::fmt("installing: {}", &[last]),
                    (_, None) => tr("looking...").to_string(),
                    (_, Some(Status::Installed)) => tr("installed by Light").to_string(),
                    (_, Some(Status::OnPath)) => tr("installed (found on PATH)").to_string(),
                    (_, Some(Status::Missing)) if r.installable => tr("not installed").to_string(),
                    (_, Some(Status::Missing)) => tr("not installed; Light cannot install it here, install it yourself").to_string(),
                };
                let desc = format!("{} · {state}", r.languages);
                list.row(ui, &pal, &title, &desc, 150.0, |ui| {
                    if installing.is_some() {
                        return;
                    }
                    // what Light installed it can remove; what it can install and is not there, it offers
                    let removable = matches!(r.status, Some(Status::Installed)) && r.def.install.is_some();
                    let offered = matches!(r.status, Some(Status::Missing)) && r.installable && confirm.as_deref() != Some(r.def.name.as_str());
                    if removable && widgets::secondary_button(ui, &pal, tr("Remove")).clicked() {
                        remove = Some(i);
                    }
                    if offered && widgets::primary_button(ui, &pal, tr("Install...")).clicked() {
                        ask = Some(r.def.name.clone());
                    }
                });
                // what the install downloads, before it does
                if confirm.as_deref() == Some(r.def.name.as_str()) && installing.is_none() {
                    match &lines {
                        Some(Ok((what, into))) => {
                            ui.label(RichText::new(tr("Light will download and install:")).size(12.0));
                            for w in what {
                                ui.label(RichText::new(format!("  - {w}")).size(12.0));
                            }
                            ui.label(RichText::new(i18n::fmt("into {}. Each download is checked against the SHA-256 its module names, and npm runs no scripts of the packages.", &[into])).color(pal.text_dim).size(11.5));
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if widgets::primary_button(ui, &pal, tr("Install")).clicked() {
                                    install = Some(i);
                                }
                                if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                                    cancel = true;
                                }
                            });
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(i18n::fmt("Cannot install {0}: {1}", &[&r.def.name, e])).color(pal.error).size(12.0));
                        }
                        None => {}
                    }
                    ui.add_space(10.0);
                }
            }
            ui.add_space(6.0);
        });
        if installing.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
        }
        if rows.iter().any(|r| r.status.is_none()) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
        if let Some(name) = ask {
            self.lsp.settings_confirm = Some(name);
        }
        if cancel {
            self.lsp.settings_confirm = None;
        }
        if let Some(i) = install {
            self.lsp_install_from_settings(&rows[i].module, &rows[i].def);
        }
        if let Some(i) = remove {
            self.lsp_remove(&rows[i].module, &rows[i].def);
        }
    }
}
