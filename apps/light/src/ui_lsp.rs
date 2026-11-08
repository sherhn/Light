//! The banner that offers a language server Light can install for a language that has none: what it gives, then the
//! install as it runs, or why it failed. And the language servers of the Settings: where each stands, to install or
//! remove.

use eframe::egui::{self, Margin, RichText, Sense, Stroke, Vec2};

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
        egui::Frame::new().fill(tint.gamma_multiply(0.09)).corner_radius(widgets::radius())
            .stroke(Stroke::new(1.0, tint.gamma_multiply(0.30))).inner_margin(Margin::symmetric(12, 9))
            .outer_margin(Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    ui.painter().rect_filled(rect, widgets::radius(), tint.gamma_multiply(0.15));
                    let icon = if failed.is_some() { Icon::Warning } else { Icon::Code };
                    icons::paint(ui.painter(), rect.shrink(7.0), icon, tint);
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(format!("{} · {}", tr("Language server"), offer.language))
                            .color(pal.text_dim).size(11.5)).truncate());
                        ui.add(egui::Label::new(RichText::new(&name).font(fonts::semibold(15.0))).truncate()).on_hover_text(&name);
                    });
                });
                ui.add_space(4.0);
                match (&running, &failed) {
                    (Some(last), _) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.label(RichText::new(i18n::fmt("Installing {}...", &[&name])).strong());
                        });
                        if !last.is_empty() {
                            ui.label(RichText::new(last).color(pal.text_dim).size(12.0));
                        }
                    }
                    (None, Some((_, why, log))) => {
                        ui.label(RichText::new(i18n::fmt("Cannot install {0}: {1}", &[&name, why])).color(pal.error).size(12.0));
                        let details = i18n::fmt("What it printed is in {}", &[log]);
                        ui.add(egui::Label::new(RichText::new(&details).color(pal.text_dim).size(12.0)).truncate()).on_hover_text(details);
                    }
                    (None, None) => {
                        ui.label(RichText::new(tr("Install to enable navigation, rename, completion and diagnostics.")).color(pal.text_dim).size(12.0));
                    }
                }
                if running.is_none() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let label = if failed.is_some() { tr("Try again") } else { tr("Install...") };
                        install = widgets::primary_button(ui, &pal, label).clicked();
                        later = widgets::secondary_button(ui, &pal, tr("Not now")).clicked();
                    });
                }
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
            ui.add_space(8.0);
            widgets::card_frame(&pal, pal.accent.gamma_multiply(0.08)).corner_radius(widgets::radius())
                .inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new(tr("Server environment")).font(fonts::semibold(12.0)).color(pal.text_dim));
                    ui.label(RichText::new(&place).font(fonts::semibold(14.0)));
                });
            ui.add_space(6.0);
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
                    widgets::card_frame(&pal, pal.hover.gamma_multiply(0.6)).corner_radius(widgets::radius())
                        .inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            match &lines {
                                Some(Ok((what, into))) => {
                                    ui.label(RichText::new(tr("Light will download and install:")).font(fonts::semibold(13.0)));
                                    for download in what {
                                        ui.label(RichText::new(format!("• {download}")).size(12.0));
                                    }
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(i18n::fmt("into {}. Each download is checked against the SHA-256 its module names, and npm runs no scripts of the packages.", &[into])).color(pal.text_dim).size(11.5));
                                    ui.add_space(6.0);
                                    ui.horizontal_wrapped(|ui| {
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
                                    if widgets::secondary_button(ui, &pal, tr("Cancel")).clicked() {
                                        cancel = true;
                                    }
                                }
                                None => {}
                            }
                        });
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
