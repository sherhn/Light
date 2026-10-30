//! The dialog that edits the run scenarios of the project: a list of them, and for the chosen one its name and the
//! console commands that run one after another.

use eframe::egui::{self, Key, RichText, Vec2};

use crate::app::App;
use crate::app_env::Scenario;
use crate::fonts;
use crate::i18n::tr;
use crate::state::{Popup, ScenariosState};
use crate::widgets::{self, TextField};

impl App {
    /// Open the editor of the project's scenarios.
    pub fn open_scenarios_dialog(&mut self) {
        if self.project.is_none() {
            self.toast(tr("Open a project first"), crate::editor_view::Sev::Weak);
            return;
        }
        let list = self.project_cfg.scenarios.clone();
        let texts = list.iter().map(|s| s.commands.join("\n")).collect();
        self.popup = Popup::Scenarios(Box::new(ScenariosState { list, texts, selected: 0, just_opened: true }));
    }

    pub fn draw_scenarios(&mut self, ctx: &egui::Context) {
        if !matches!(self.popup, Popup::Scenarios(_)) {
            return;
        }
        let Popup::Scenarios(mut st) = std::mem::replace(&mut self.popup, Popup::None) else { return };
        let pal = self.pal;
        let esc = ctx.input(|i| i.key_pressed(Key::Escape));
        let (mut save, mut cancel) = (false, esc);
        let mut add = false;
        let mut delete = false;
        self.dialog_card(ctx, "scenarios", tr("Run scenarios"), 740.0, |ui| {
            ui.label(RichText::new(tr("A scenario is a list of console commands that run one after another in the project folder. Pick it in the run list, then Run.")).color(pal.text_dim).size(12.0));
            ui.add_space(12.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(200.0);
                    for (i, s) in st.list.iter().enumerate() {
                        let name = if s.name.trim().is_empty() { tr("(no name)").to_string() } else { s.name.clone() };
                        let (r, resp) = ui.allocate_exact_size(egui::vec2(200.0, 32.0), egui::Sense::click());
                        let chosen = i == st.selected;
                        if chosen || resp.hovered() {
                            ui.painter().rect_filled(r, egui::CornerRadius::same(9), if chosen { pal.selected } else { pal.hover });
                        }
                        crate::icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(r.min.x + 18.0, r.center().y), egui::Vec2::splat(15.0)), crate::icons::Icon::Script, if chosen { pal.accent } else { pal.text_dim });
                        ui.painter().with_clip_rect(r.shrink(2.0)).text(egui::pos2(r.min.x + 36.0, r.center().y), egui::Align2::LEFT_CENTER, &name, crate::fonts::ui(13.5), if chosen { pal.text } else { pal.text_dim });
                        if resp.clicked() {
                            st.selected = i;
                        }
                    }
                    ui.add_space(8.0);
                    if widgets::secondary_button(ui, &pal, tr("New scenario")).clicked() {
                        add = true;
                    }
                });
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.set_width(480.0);
                    let i = st.selected;
                    if i >= st.list.len() {
                        ui.label(RichText::new(tr("No scenarios yet. Make one with New scenario.")).color(pal.text_dim));
                        return;
                    }
                    ui.label(RichText::new(tr("Name")).color(pal.text_dim).size(12.0));
                    let r = TextField::new(&mut st.list[i].name).hint(tr("e.g. build and test")).show(ui, &pal);
                    widgets::focus_once(ui, &r, &mut st.just_opened);
                    ui.add_space(10.0);
                    ui.label(RichText::new(tr("Commands, one per line")).color(pal.text_dim).size(12.0));
                    egui::TextEdit::multiline(&mut st.texts[i]).font(egui::FontId::monospace(13.0)).desired_rows(8).desired_width(f32::INFINITY).hint_text("make\nmake test").show(ui);
                    ui.add_space(4.0);
                    ui.label(RichText::new(tr("{file}, {dir}, {name} and {stem} stand for the file that is open. A line that starts with # is left out.")).color(pal.text_dim).size(11.5));
                    ui.add_space(8.0);
                    ui.checkbox(&mut st.list[i].stop_on_error, tr("Stop at the first command that fails"));
                    ui.add_space(8.0);
                    if widgets::secondary_button(ui, &pal, tr("Delete this scenario")).clicked() {
                        delete = true;
                    }
                });
            });
            let (ok, c) = self.ok_cancel(ui, tr("Save"));
            save |= ok;
            cancel |= c;
        });
        if add {
            st.list.push(Scenario { name: String::new(), commands: vec![], stop_on_error: true });
            st.texts.push(String::new());
            st.selected = st.list.len() - 1;
            st.just_opened = true;
        }
        if delete && st.selected < st.list.len() {
            let i = st.selected;
            st.list.remove(i);
            st.texts.remove(i);
            st.selected = st.selected.saturating_sub(1).min(st.list.len().saturating_sub(1));
        }
        if save {
            // names are what scenarios are picked by: filled in and told apart
            let mut out: Vec<Scenario> = vec![];
            for (n, (mut s, text)) in st.list.into_iter().zip(st.texts).enumerate() {
                s.commands = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
                let base = if s.name.trim().is_empty() { format!("{} {}", tr("Scenario"), n + 1) } else { s.name.trim().to_string() };
                let mut name = base.clone();
                let mut k = 2;
                while out.iter().any(|o| o.name == name) {
                    name = format!("{base} {k}");
                    k += 1;
                }
                s.name = name;
                out.push(s);
            }
            self.save_scenarios(out);
            self.editor_focus = true;
        } else if cancel {
            self.editor_focus = true;
        } else {
            self.popup = Popup::Scenarios(st);
        }
        let _ = (Vec2::ZERO, fonts::ui(12.0));
    }
}
