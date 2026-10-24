//! The Modules tab: a card for every module with its cover, what it is, and what can be done with it - download,
//! switch on or off, update, remove. Also the banner that offers the modules a project needs, and the small list of
//! modules that the dialogs for a new or imported project show.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, UiBuilder, Vec2};

use crate::app::App;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::modules::{Busy, Card};
use crate::theme::Palette;
use crate::widgets;

const CARD_W: f32 = 300.0;
const GAP: f32 = 18.0;
const SIDE: f32 = 26.0;

enum Action {
    Install(String),
    Remove(String),
    Toggle(String, bool),
    Refresh,
    Welcome,
    InstallAll(Vec<String>),
}

/// A pill that filters the list.
fn chip(ui: &mut egui::Ui, pal: &Palette, label: &str, active: bool) -> egui::Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), fonts::ui(12.5), pal.text);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 26.0, 28.0), Sense::click());
    let fill = if active { pal.accent.gamma_multiply(0.30) } else if resp.hovered() { pal.hover } else { pal.input_bg.gamma_multiply(0.7) };
    ui.painter().rect_filled(r, CornerRadius::same(14), fill);
    if active {
        ui.painter().rect_stroke(r, CornerRadius::same(14), Stroke::new(1.0, pal.accent.gamma_multiply(0.8)), StrokeKind::Inside);
    }
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, fonts::ui(12.5), if active { pal.text } else { pal.text_dim });
    resp
}

/// What a card says about the module's size: `31 KB`.
fn kb(bytes: u64) -> String {
    if bytes >= 1024 * 1024 { format!("{:.1} MB", bytes as f64 / 1_048_576.0) } else { format!("{} KB", bytes.div_ceil(1024).max(1)) }
}

/// A line of text cut off after `rows` rows with an ellipsis.
fn paragraph(ui: &egui::Ui, text: &str, font: egui::FontId, color: Color32, width: f32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font, color, width);
    job.wrap.max_rows = rows;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('\u{2026}');
    ui.painter().layout_job(job)
}

/// The cover, or a plain glow where there is none yet.
fn paint_cover(ui: &mut egui::Ui, pal: &Palette, rect: Rect, tex: Option<&TextureHandle>) {
    match tex {
        Some(t) => {
            ui.put(rect, egui::Image::new((t.id(), rect.size())).corner_radius(CornerRadius::same(14)));
        }
        None => {
            ui.painter().rect_filled(rect, CornerRadius::same(14), pal.input_bg);
            let mut c = pal.accent;
            c = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 40);
            ui.painter().circle_filled(rect.center(), rect.height() * 0.28, c);
            icons::paint(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(30.0)), Icon::Puzzle, pal.text_dim);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_card(ui: &mut egui::Ui, pal: &Palette, rect: Rect, card: &Card, tex: Option<&TextureHandle>, busy: Option<&Busy>, recommended: bool, actions: &mut Vec<Action>) {
    let hovered = ui.rect_contains_pointer(rect);
    // The same restrained language watermark used by the website's module cards.
    ui.painter().rect_filled(rect, CornerRadius::same(widgets::CARD_RADIUS), pal.panel_bg);
    ui.painter().rect_stroke(rect, CornerRadius::same(widgets::CARD_RADIUS), Stroke::new(1.0, if hovered { pal.accent.gamma_multiply(0.7) } else { pal.border }), StrokeKind::Inside);
    let cover = Rect::from_min_size(rect.min + Vec2::splat(9.0), Vec2::new(rect.width() - 18.0, (rect.width() - 18.0) * 9.0 / 16.0));
    paint_cover(ui, pal, cover, tex);
    if recommended {
        let g = ui.painter().layout_no_wrap(tr("Recommended").to_string(), fonts::semibold(11.0), pal.text_on_accent);
        let r = Rect::from_min_size(Pos2::new(cover.max.x - g.size().x - 26.0, cover.min.y + 12.0), Vec2::new(g.size().x + 16.0, 22.0));
        ui.painter().rect_filled(r, CornerRadius::same(11), pal.accent);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, tr("Recommended"), fonts::semibold(11.0), pal.text_on_accent);
    }
    let x0 = rect.min.x + 16.0;
    let w = rect.width() - 32.0;
    // The title stays readable on the dark cover in both IDE themes.
    let title = paragraph(ui, &card.name, fonts::semibold(27.0), Color32::from_rgb(0xe5, 0xe9, 0xe7), cover.width() - 28.0, 2);
    ui.painter().galley(Pos2::new(cover.min.x + 14.0, cover.max.y - 18.0 - title.size().y), title, Color32::WHITE);
    let mut y = cover.max.y + 16.0;
    let cat = i18n::tr_dyn(&card.category).to_string();
    ui.painter().text(Pos2::new(x0, y), Align2::LEFT_TOP, cat, fonts::ui(11.0), pal.text_dim);
    y += 23.0;
    let g = paragraph(ui, &card.summary, fonts::ui(12.5), pal.text_dim, w, 3);
    ui.painter().galley(Pos2::new(x0, y), g, pal.text_dim);
    // the bottom row: what it is, and what to do
    let bottom = Rect::from_min_max(Pos2::new(x0, rect.max.y - 50.0), Pos2::new(rect.max.x - 16.0, rect.max.y - 14.0));
    let info = match (&card.installed, card.size) {
        (Some(_), _) => format!("v{}", card.version),
        (None, 0) => format!("v{}", card.version),
        (None, s) => format!("v{}  \u{b7}  {}", card.version, kb(s)),
    };
    ui.painter().text(Pos2::new(bottom.min.x, bottom.center().y), Align2::LEFT_CENTER, info, fonts::ui(12.0), pal.text_dim);
    ui.scope_builder(UiBuilder::new().max_rect(bottom).layout(Layout::right_to_left(Align::Center)), |ui| {
        if let Some(b) = busy {
            let frac = if b.total > 0 { b.done as f32 / b.total as f32 } else { 0.0 };
            ui.label(RichText::new(if b.removing { tr("Removing...").to_string() } else { format!("{:.0}%", frac * 100.0) }).color(pal.text_dim).size(12.0));
            widgets::progress_bar_opt(ui, pal, if b.removing { None } else { Some(frac) }, 90.0);
            return;
        }
        match &card.installed {
            None => {
                if widgets::primary_button(ui, pal, tr("Install")).clicked() {
                    actions.push(Action::Install(card.id.clone()));
                }
            }
            Some(m) => {
                let resp = widgets::icon_button(ui, pal, Icon::More, 30.0, pal.text_dim, tr("More"));
                egui::Popup::menu(&resp).width(180.0).show(|ui| {
                    if ui.button(tr("Remove")).clicked() {
                        actions.push(Action::Remove(card.id.clone()));
                        ui.close();
                    }
                });
                if let Some(v) = &card.update
                    && widgets::secondary_button(ui, pal, &i18n::fmt("Update to {}", &[v])).clicked()
                {
                    actions.push(Action::Install(card.id.clone()));
                }
                let mut on = m.enabled;
                ui.label(RichText::new(if on { tr("On") } else { tr("Off") }).color(if on { pal.accent } else { pal.text_dim }).size(12.5));
                if widgets::toggle(ui, pal, &mut on).clicked() {
                    actions.push(Action::Toggle(card.id.clone(), on));
                }
            }
        }
    });
}

impl App {
    /// Open the Modules tab (or go to it) and ask the registry what it has.
    pub fn open_modules_tab(&mut self) {
        match self.docs.iter().position(|d| d.page == Some(crate::doc::Page::Modules)) {
            Some(i) => self.active = i,
            None => {
                self.docs.push(crate::doc::Doc::modules_tab());
                self.active = self.docs.len() - 1;
            }
        }
        if self.mods.index.is_none() {
            self.modules_fetch_index();
        }
        self.editor_focus = false;
    }

    pub fn draw_modules_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        if self.mods.index.is_none() && !self.mods.loading_index && self.mods.index_error.is_none() {
            self.modules_fetch_index();
        }
        // what is shown: the cards that pass the search and the filter chips
        let all = self.mods.cards();
        let q = self.mods.filter.to_lowercase();
        let categories: Vec<String> = {
            let mut c: Vec<String> = all.iter().map(|c| c.category.clone()).collect();
            c.dedup();
            c
        };
        let shown: Vec<Card> = all
            .iter()
            .filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q) || c.summary.to_lowercase().contains(&q))
            .filter(|c| match self.mods.category.as_deref() {
                None => true,
                Some("@installed") => c.installed.is_some(),
                Some(cat) => c.category == cat,
            })
            .cloned()
            .collect();
        let textures: Vec<Option<TextureHandle>> = shown.iter().map(|c| self.cover_texture(ctx, &c.id)).collect();
        let needed = self.needed_modules.clone();
        let no_modules = self.mods.installed.is_empty();
        let welcome = no_modules && !self.settings.modules_welcome_done;
        let busy = self.mods.busy.clone();
        let loading = self.mods.loading_index;
        let error = self.mods.index_error.clone();
        let broken = self.mods.broken.clone();
        let mut filter = self.mods.filter.clone();
        let mut category = self.mods.category.clone();
        let mut actions: Vec<Action> = Vec::new();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width();
            ui.add_space(22.0);
            // ---- title
            ui.horizontal(|ui| {
                ui.add_space(SIDE);
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr("Modules")).font(fonts::semibold(28.0)));
                    ui.add_space(2.0);
                    ui.label(RichText::new(tr("Light is an editor, projects, git, a terminal and Linux through WSL. Languages and their tools come as modules: download what you work with.")).color(pal.text_dim).size(13.0));
                });
                widgets::right_row(ui, 36.0, |ui| {
                    ui.add_space(SIDE);
                    if loading {
                        ui.add(egui::Spinner::new().size(18.0).color(pal.accent));
                    } else if widgets::icon_button(ui, &pal, Icon::Refresh, 34.0, pal.text_dim, tr("Check the registry again")).clicked() {
                        actions.push(Action::Refresh);
                    }
                });
            });
            ui.add_space(14.0);
            // ---- a first start, or a project that wants modules
            if welcome {
                ui.horizontal(|ui| {
                    ui.add_space(SIDE);
                    egui::Frame::new().fill(pal.accent.gamma_multiply(0.14)).corner_radius(widgets::radius()).stroke(Stroke::new(1.0, pal.accent.gamma_multiply(0.5))).inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                        ui.set_width(width - SIDE * 2.0 - 34.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                // leave room for the button on the right
                                ui.set_max_width(width - SIDE * 2.0 - 34.0 - 150.0);
                                ui.label(RichText::new(tr("Welcome to Light")).font(fonts::semibold(15.0)));
                                ui.label(RichText::new(tr("Nothing language-specific is installed yet. Pick what you work with below - each module is a small download and can be removed again.")).color(pal.text_dim).size(12.5));
                            });
                            widgets::right_row(ui, 32.0, |ui| {
                                if widgets::secondary_button(ui, &pal, tr("Not now")).clicked() {
                                    actions.push(Action::Welcome);
                                }
                            });
                        });
                    });
                });
                ui.add_space(12.0);
            }
            if !needed.is_empty() {
                let names: Vec<String> = needed.iter().map(|id| all.iter().find(|c| &c.id == id).map_or(id.clone(), |c| c.name.clone())).collect();
                ui.horizontal(|ui| {
                    ui.add_space(SIDE);
                    egui::Frame::new().fill(pal.blue.gamma_multiply(0.16)).corner_radius(widgets::radius()).stroke(Stroke::new(1.0, pal.blue.gamma_multiply(0.5))).inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                        ui.set_width(width - SIDE * 2.0 - 34.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(i18n::fmt("This project works with: {}", &[&names.join(", ")])).size(13.0));
                            widgets::right_row(ui, 32.0, |ui| {
                                if widgets::primary_button(ui, &pal, tr("Install them")).clicked() {
                                    actions.push(Action::InstallAll(needed.clone()));
                                }
                            });
                        });
                    });
                });
                ui.add_space(12.0);
            }
            // ---- search and filters
            ui.horizontal(|ui| {
                ui.add_space(SIDE);
                widgets::TextField::new(&mut filter).hint(tr("Search modules")).icon(Icon::Search).width(260.0).show(ui, &pal);
                ui.add_space(8.0);
                if chip(ui, &pal, tr("All"), category.is_none()).clicked() {
                    category = None;
                }
                if chip(ui, &pal, tr("Installed"), category.as_deref() == Some("@installed")).clicked() {
                    category = Some("@installed".into());
                }
                for c in &categories {
                    if chip(ui, &pal, &i18n::tr_dyn(c), category.as_deref() == Some(c.as_str())).clicked() {
                        category = Some(c.clone());
                    }
                }
            });
            ui.add_space(14.0);
            if let Some(e) = &error {
                ui.horizontal(|ui| {
                    ui.add_space(SIDE);
                    ui.label(RichText::new(i18n::fmt("Cannot load the list of modules: {}", &[&crate::ui_chrome::elide(e, 160)])).color(pal.error).size(12.5));
                });
                ui.add_space(8.0);
            }
            for b in &broken {
                ui.horizontal(|ui| {
                    ui.add_space(SIDE);
                    ui.label(RichText::new(i18n::fmt("A module cannot be used: {}", &[&crate::ui_chrome::elide(b, 160)])).color(pal.warning).size(12.0));
                });
            }
            // ---- the cards
            let avail = width - SIDE * 2.0;
            let cols = (((avail + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
            let cw = ((avail - GAP * (cols - 1) as f32) / cols as f32).clamp(240.0, 420.0);
            let ch = (cw - 18.0) * 9.0 / 16.0 + 18.0 + 12.0 + 26.0 + 3.0 * 16.5 + 12.0 + 50.0;
            let rows = shown.len().div_ceil(cols);
            let (area, _) = ui.allocate_exact_size(Vec2::new(width, rows as f32 * (ch + GAP) + 10.0), Sense::hover());
            if shown.is_empty() && !loading {
                ui.painter().text(Pos2::new(area.min.x + SIDE, area.min.y + 20.0), Align2::LEFT_TOP, tr("No modules match."), fonts::ui(13.0), pal.text_dim);
            }
            for (i, card) in shown.iter().enumerate() {
                let (col, row) = (i % cols, i / cols);
                let rect = Rect::from_min_size(Pos2::new(area.min.x + SIDE + col as f32 * (cw + GAP), area.min.y + row as f32 * (ch + GAP)), Vec2::new(cw, ch));
                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    paint_card(ui, &pal, rect, card, textures[i].as_ref(), busy.get(&card.id), needed.contains(&card.id), &mut actions);
                });
            }
        });

        self.mods.filter = filter;
        self.mods.category = category;
        for a in actions {
            match a {
                Action::Install(id) => self.module_install(&id),
                Action::Remove(id) => self.module_remove(&id),
                Action::Toggle(id, on) => self.module_set_enabled(&id, on),
                Action::Refresh => {
                    self.mods.index = None;
                    self.modules_fetch_index();
                }
                Action::Welcome => {
                    self.settings.modules_welcome_done = true;
                    self.save_settings();
                }
                Action::InstallAll(ids) => {
                    for id in ids {
                        self.module_use(&id);
                    }
                }
            }
        }
    }

    /// The banner on top of the editor: the open project works with modules that are not on yet.
    pub fn draw_modules_banner(&mut self, ui: &mut egui::Ui) {
        // not on the Modules page itself: it says the same in its own words
        if self.needed_modules.is_empty() || self.modules_banner_dismissed || self.env_banner || self.doc().is_some_and(|d| d.page.is_some()) {
            return;
        }
        let pal = self.pal;
        let names: Vec<String> = self.needed_modules.iter().map(|id| self.mods.cards().into_iter().find(|c| &c.id == id).map_or(id.clone(), |c| c.name)).collect();
        let (mut install, mut open, mut later) = (false, false, false);
        // modules that are here but switched off only need switching on
        let all_here = self.needed_modules.iter().all(|id| self.mods.installed.iter().any(|m| &m.manifest.id == id));
        egui::Frame::new().fill(pal.accent.gamma_multiply(0.12)).corner_radius(widgets::radius()).stroke(Stroke::new(1.0, pal.accent.gamma_multiply(0.4))).inner_margin(egui::Margin::symmetric(14, 10)).outer_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Puzzle, pal.accent);
                ui.vertical(|ui| {
                    ui.label(RichText::new(i18n::fmt("This project works with: {}", &[&names.join(", ")])).font(fonts::semibold(13.0)));
                    let hint = if all_here { tr("Switch these modules on for highlighting, tools and run commands of these languages.") } else { tr("Install the modules for highlighting, tools and run commands of these languages.") };
                    ui.label(RichText::new(hint).color(pal.text_dim).size(12.0));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, &pal, Icon::Close, 26.0, pal.text_dim, tr("Not now")).clicked() {
                        later = true;
                    }
                    if widgets::secondary_button(ui, &pal, tr("Modules...")).clicked() {
                        open = true;
                    }
                    if (all_here || self.mods.index.is_some()) && widgets::primary_button(ui, &pal, if all_here { tr("Switch on") } else { tr("Install") }).clicked() {
                        install = true;
                    }
                });
            });
        });
        if later {
            self.modules_banner_dismissed = true;
        }
        if open {
            self.open_modules_tab();
        }
        if install {
            for id in self.needed_modules.clone() {
                self.module_use(&id);
            }
        }
        if self.mods.index.is_none() && !self.mods.loading_index && self.mods.index_error.is_none() {
            self.modules_fetch_index();
        }
    }
    /// What the module picker of a dialog shows: the cards and their covers (the list is asked for if needed).
    pub fn picker_data(&mut self, ctx: &egui::Context) -> PickerData {
        if self.mods.index.is_none() && !self.mods.loading_index && self.mods.index_error.is_none() {
            self.modules_fetch_index();
        }
        let cards = self.mods.cards();
        let textures = cards.iter().map(|c| self.cover_texture(ctx, &c.id)).collect();
        PickerData { cards, textures, loading: self.mods.loading_index, recommended: self.needed_modules.clone() }
    }
}

/// See [`App::picker_data`].
pub struct PickerData {
    pub cards: Vec<Card>,
    pub textures: Vec<Option<TextureHandle>>,
    pub loading: bool,
    /// Modules the open project looks like it needs.
    pub recommended: Vec<String>,
}

/// A short list of the modules with a check box each, for the dialogs that create or import a project: the ticked
/// ones are downloaded if needed and connected to the project.
pub fn module_picker(ui: &mut egui::Ui, pal: &Palette, data: &PickerData, selected: &mut Vec<String>) {
    if data.cards.is_empty() {
        ui.label(RichText::new(if data.loading { tr("Loading the list of modules...") } else { tr("The list of modules is not available.") }).color(pal.text_dim).size(12.5));
        return;
    }
    let cols = 2usize;
    let w = (ui.available_width() - 10.0) / cols as f32;
    let rows = data.cards.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), rows as f32 * 56.0), Sense::hover());
    for (i, c) in data.cards.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(area.min.x + (i % cols) as f32 * (w + 10.0), area.min.y + (i / cols) as f32 * 56.0), Vec2::new(w, 50.0));
        let on = selected.contains(&c.id);
        let resp = ui.interact(r, ui.scope_id().with(("pick", &c.id)), Sense::click());
        let fill = if on { pal.accent.gamma_multiply(0.20) } else if resp.hovered() { pal.hover } else { pal.input_bg.gamma_multiply(0.6) };
        ui.painter().rect_filled(r, CornerRadius::same(12), fill);
        if on {
            ui.painter().rect_stroke(r, CornerRadius::same(12), Stroke::new(1.0, pal.accent.gamma_multiply(0.8)), StrokeKind::Inside);
        }
        let thumb = Rect::from_min_size(r.min + Vec2::new(6.0, 6.0), Vec2::new(62.0, 38.0));
        paint_cover(ui, pal, thumb, data.textures[i].as_ref());
        ui.painter().text(Pos2::new(thumb.max.x + 10.0, r.min.y + 9.0), Align2::LEFT_TOP, &c.name, fonts::semibold(13.0), pal.text);
        let state = match &c.installed {
            Some(m) if m.enabled => tr("Installed").to_string(),
            Some(_) => tr("Installed, off").to_string(),
            None => format!("{}  {}", tr("Download"), kb(c.size)),
        };
        ui.painter().text(Pos2::new(thumb.max.x + 10.0, r.min.y + 27.0), Align2::LEFT_TOP, state, fonts::ui(11.5), pal.text_dim);
        if data.recommended.contains(&c.id) {
            ui.painter().text(Pos2::new(r.max.x - 40.0, r.center().y), Align2::RIGHT_CENTER, tr("Recommended"), fonts::semibold(10.5), pal.accent);
        }
        // the check box
        let cb = Rect::from_center_size(Pos2::new(r.max.x - 18.0, r.center().y), Vec2::splat(18.0));
        ui.painter().rect_filled(cb, CornerRadius::same(6), if on { pal.accent } else { pal.input_bg });
        if on {
            icons::paint(ui.painter(), cb.shrink(3.0), Icon::Check, pal.text_on_accent);
        }
        if resp.clicked() {
            if on {
                selected.retain(|s| s != &c.id);
            } else {
                selected.push(c.id.clone());
            }
        }
    }
}
