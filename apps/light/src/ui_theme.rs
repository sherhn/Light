//! Settings > Appearance > your own theme: every colour of the palette in a list, each with a swatch that opens a colour
//! picker, a field for its hex code and a button that gives the palette's own colour back; a sample of code shows the
//! result at once.

use eframe::egui::{self, Color32, CornerRadius, Margin, PopupCloseBehavior, RichText, Sense, Stroke, StrokeKind, text::LayoutJob, vec2};
use ide_core::settings::{CustomTheme, format_color, parse_color};
use lisp_syntax::highlight::Class;

use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::Icon;
use crate::theme::{Palette, SLOTS};
use crate::ui_settings::group;
use crate::widgets::{self, TextField};

/// The editor of the user's own theme. `pal` is the palette the dialog is drawn in. Returns whether a colour (or the
/// palette it starts from) changed.
pub(crate) fn theme_editor(ui: &mut egui::Ui, pal: &Palette, theme: &mut CustomTheme) -> bool {
    let mut changed = false;
    let current = Palette::custom(theme);
    let n_changed = theme.colors.keys().filter(|k| theme.get(k).is_some() && current.color(k).is_some()).count();

    group(ui, pal, tr("Your theme"), |ui, rows| {
        rows.row(ui, pal, tr("Start from"), tr("The palette the colors below change; the colors you set stay when you switch"), 190.0, |ui| {
            if let Some(i) = widgets::segmented(ui, pal, &[tr("Dark"), tr("Light")], usize::from(!theme.dark)) {
                theme.dark = i == 0;
                changed = true;
            }
        });
        let desc = if n_changed == 0 { tr("Nothing changed yet: pick a color below").to_string() } else { i18n::fmt("{} colors changed", &[&n_changed]) };
        rows.row(ui, pal, tr("Colors"), &desc, 170.0, |ui| {
            ui.add_enabled_ui(n_changed > 0, |ui| {
                if widgets::secondary_button(ui, pal, tr("Reset all colors")).clicked() {
                    theme.reset_all();
                    changed = true;
                }
            });
        });
        ui.add_space(12.0);
        sample(ui, &current);
        ui.add_space(14.0);
    });

    let base = if theme.dark { Palette::dark() } else { Palette::light() };
    let mut last_group = "";
    let slots = &*SLOTS;
    for (i, slot) in slots.iter().enumerate() {
        // one card per group of slots
        if slot.group == last_group {
            continue;
        }
        last_group = slot.group;
        let members = slots[i..].iter().take_while(|s| s.group == slot.group);
        group(ui, pal, tr(slot.group), |ui, rows| {
            for s in members {
                let (Some(now), Some(default)) = (current.color(s.key), base.color(s.key)) else { continue };
                let is_set = theme.get(s.key).is_some();
                rows.slim(ui, pal, tr(s.label), 214.0, |ui| {
                    // right to left: the swatch, its hex code, and the way back to the palette's own colour
                    let mut color = now;
                    if swatch(ui, pal, s.key, &mut color, s.translucent) {
                        theme.set(s.key, color.to_srgba_unmultiplied());
                        changed = true;
                    }
                    ui.add_space(4.0);
                    if let Some(c) = hex_field(ui, pal, s.key, now) {
                        theme.set(s.key, c);
                        changed = true;
                    }
                    if is_set {
                        let tip = i18n::fmt("Back to {}", &[&format_color(default.to_srgba_unmultiplied())]);
                        if widgets::icon_button(ui, pal, Icon::Undo, 28.0, pal.text_dim, &tip).clicked() {
                            theme.reset(s.key);
                            changed = true;
                        }
                    }
                });
            }
        });
    }
    changed
}

/// A colour button: the colour (over a light and a dark half, so that a translucent one is seen as such) that opens
/// the picker. Returns whether the colour was changed in it.
fn swatch(ui: &mut egui::Ui, pal: &Palette, key: &str, color: &mut Color32, translucent: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 28.0), Sense::click());
    let round = CornerRadius::same(8);
    if translucent {
        let (left, right) = (egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.max.y)), egui::Rect::from_min_max(egui::pos2(rect.center().x, rect.min.y), rect.max));
        ui.painter().with_clip_rect(left).rect_filled(rect, round, Color32::from_gray(225));
        ui.painter().with_clip_rect(right).rect_filled(rect, round, Color32::from_gray(40));
    }
    ui.painter().rect_filled(rect, round, *color);
    ui.painter().rect_stroke(rect, round, Stroke::new(1.0, if resp.hovered() { pal.text_dim } else { pal.border.gamma_multiply(2.0) }), StrokeKind::Inside);
    let mut changed = false;
    let alpha = if translucent { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
    egui::Popup::from_toggle_button_response(&resp).close_behavior(PopupCloseBehavior::CloseOnClickOutside).id(egui::Id::unique(("theme_picker", key))).show(|ui| {
        ui.set_min_width(250.0);
        changed = egui::color_picker::color_picker_color32(ui, color, alpha);
    });
    changed
}

/// The hex code of a colour, editable: a code that reads as a colour is taken at once. While the field has the focus
/// it keeps what is typed; otherwise it shows the colour as it is.
fn hex_field(ui: &mut egui::Ui, pal: &Palette, key: &str, now: Color32) -> Option<[u8; 4]> {
    let id = egui::Id::unique(("theme_hex", key));
    let now = now.to_srgba_unmultiplied();
    let editing = ui.memory(|m| m.has_focus(id));
    let mut text = if editing { ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| format_color(now)) } else { format_color(now) };
    let resp = TextField::new(&mut text).id(id).width(104.0).height(30.0).font(egui::FontId::monospace(12.5)).show(ui, pal);
    if resp.has_focus() || resp.changed() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    }
    if resp.changed() { parse_color(&text).filter(|c| *c != now) } else { None }
}

/// A few lines of code in the colours of `pal`, with the backgrounds of the current line, a selection and a search hit.
fn sample(ui: &mut egui::Ui, pal: &Palette) {
    use Class::*;
    let font = egui::FontId::monospace(13.0);
    let mut job = LayoutJob::default();
    let mut put = |text: &str, color: Color32, background: Color32| {
        job.append(text, 0.0, egui::TextFormat { font_id: font.clone(), color, background, ..Default::default() });
    };
    let none = Color32::TRANSPARENT;
    let br = |depth: u32| pal.syntax(Bracket, depth, true);
    put("; Greets someone ", pal.syntax(Comment, 0, true), none);
    put("TODO: more greetings\n", pal.todo, none);
    put("(", br(0), none);
    put("defun", pal.syntax(SpecialForm, 0, true), none);
    put(" ", pal.plain, none);
    put("greet", pal.syntax(Definition, 0, true), none);
    put(" (", br(1), none);
    put("name", pal.plain, pal.selection);
    put(" ", pal.plain, none);
    put("&key", pal.syntax(Keyword, 0, true), none);
    put(" (", br(2), none);
    put("times", pal.plain, none);
    put(" ", pal.plain, none);
    put("2", pal.syntax(Number, 0, true), none);
    put(")", br(2), none);
    put(")", br(1), none);
    put("\n  ", pal.plain, pal.current_line);
    put("\"Say hello.\"", pal.syntax(String, 0, true), pal.current_line);
    put("                    ", pal.plain, pal.current_line);
    put("\n  (", br(1), none);
    put("format", pal.syntax(Builtin, 0, true), none);
    put(" ", pal.plain, none);
    put("t", pal.syntax(Constant, 0, true), none);
    put(" ", pal.plain, none);
    put("\"", pal.syntax(String, 0, true), none);
    put("Hello", pal.syntax(String, 0, true), pal.find_match);
    put(", ~a\"", pal.syntax(String, 0, true), none);
    put(" ", pal.plain, none);
    put("*greeting*", pal.syntax(SpecialVar, 0, true), none);
    put(" ", pal.plain, none);
    put(":ready", pal.syntax(Keyword, 0, true), none);
    put("))\n", br(0), none);
    put("(", br(0), none);
    put("check", pal.syntax(Builtin, 0, true), none);
    put(" ", pal.plain, none);
    put("oops", pal.syntax(Error, 0, true), none);
    put(") ", br(0), none);
    put("FIXME: not a function", pal.alert, none);
    egui::Frame::new().fill(pal.editor_bg).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(job);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (text, color) in [(tr("Error"), pal.error), (tr("Warning"), pal.warning), (tr("Success"), pal.ok), (tr("Link"), pal.link)] {
                ui.label(RichText::new(text).font(fonts::semibold(12.0)).color(color));
                ui.add_space(8.0);
            }
            ui.label(RichText::new(tr("Secondary text")).font(fonts::ui(12.0)).color(pal.text_dim));
        });
    });
}
