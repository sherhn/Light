//! The banner that offers a language server Light can install for a language that has none: what it gives, then the
//! install as it runs, or why it failed.

use eframe::egui::{self, Align, Layout, RichText, Sense, Stroke, Vec2};

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
}
