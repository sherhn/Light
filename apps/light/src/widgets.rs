//! The design system: a flat "glass" look built from tonal layers of grey (no borders, no shadows), floating
//! rounded cards over a soft gradient, translucent hover/selection states and generous spacing.

use crate::scrollbars::FadingScroll as _;
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Margin, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};

use crate::fonts;
use crate::icons::{self, Icon};
use crate::theme::Palette;

/// Height of a text field / segmented control row.
pub const FIELD_H: f32 = 36.0;
pub const MENU_H: f32 = 36.0;
pub const TOOLBAR_H: f32 = 54.0;
pub const STRIP_W: f32 = 56.0;
pub const TAB_H: f32 = 48.0;
pub const STATUS_H: f32 = 30.0;
pub const ROW_H: f32 = 30.0;
pub const CRUMB_H: f32 = 30.0;
pub const PANEL_HEADER_H: f32 = 44.0;
pub const RADIUS: u8 = 10;
pub const CARD_RADIUS: u8 = 18;
pub const GAP: i8 = 8;

pub fn radius() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

// ---------------------------------------------------------------------- surfaces

/// A soft round glow: `color` at the centre fading to nothing at `radius`.
fn glow(painter: &Painter, center: Pos2, radius: f32, color: Color32) {
    const N: usize = 40;
    let mut m = Mesh::default();
    m.colored_vertex(center, color);
    for i in 0..N {
        let a = i as f32 / N as f32 * std::f32::consts::TAU;
        m.colored_vertex(Pos2::new(center.x + radius * a.cos(), center.y + radius * a.sin()), Color32::TRANSPARENT);
    }
    for i in 0..N {
        m.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % N) as u32);
    }
    painter.add(Shape::mesh(m));
}

/// The window background: a dark gradient with a green glow in one corner and a blue one in the opposite one, which
/// the glass cards on top let shine through.
pub fn paint_base(painter: &Painter, rect: Rect, pal: &Palette) {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), pal.base_top);
    m.colored_vertex(rect.right_top(), pal.base_top);
    m.colored_vertex(rect.right_bottom(), pal.base_bottom);
    m.colored_vertex(rect.left_bottom(), pal.base_bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(m));
    let big = rect.width().max(rect.height());
    let alpha = if pal.dark { 58 } else { 70 };
    let tint = |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha);
    glow(painter, Pos2::new(rect.min.x + rect.width() * 0.12, rect.min.y + rect.height() * 0.05), big * 0.62, tint(pal.glow_a));
    glow(painter, Pos2::new(rect.max.x - rect.width() * 0.08, rect.max.y - rect.height() * 0.02), big * 0.58, tint(pal.glow_b));
}

/// A floating tool-window card: translucent glass with a light rim, no shadow.
pub fn card_frame(pal: &Palette, fill: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(CARD_RADIUS)).stroke(Stroke::new(1.0, pal.border))
}

/// "Glass" surface for popups and dialogs (no border, no shadow; a soft sheen on the top edge instead).
pub fn glass_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new().fill(pal.glass).corner_radius(CornerRadius::same(CARD_RADIUS + 2)).inner_margin(Margin::same(18)).stroke(Stroke::new(1.0, pal.border))
}

/// Thin highlight along the top edge of a glass surface (a light reflection, not a border).
pub fn sheen(painter: &Painter, rect: Rect, pal: &Palette) {
    // brightest in the middle, gone at both ends: the look of light on a curved edge
    let y = rect.min.y + 0.5;
    let (x0, x1) = (rect.min.x + 14.0, rect.max.x - 14.0);
    let mid = (x0 + x1) / 2.0;
    let clear = Color32::TRANSPARENT;
    let mut m = Mesh::default();
    for (x, c) in [(x0, clear), (mid, pal.sheen), (x1, clear)] {
        m.colored_vertex(Pos2::new(x, y), c);
        m.colored_vertex(Pos2::new(x, y + 1.5), clear);
    }
    m.add_triangle(0, 2, 1);
    m.add_triangle(1, 2, 3);
    m.add_triangle(2, 4, 3);
    m.add_triangle(3, 4, 5);
    painter.add(Shape::mesh(m));
}

/// Dim everything behind a modal dialog.
pub fn scrim(ctx: &egui::Context, pal: &Palette) {
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("scrim"));
    let alpha = if pal.dark { 110 } else { 60 };
    ctx.layer_painter(layer).rect_filled(ctx.content_rect(), CornerRadius::ZERO, Color32::from_black_alpha(alpha));
}

// ---------------------------------------------------------------------- buttons

/// A hoverable rounded button with optional leading icon, label and trailing chevron.
pub struct Pill<'a> {
    pub icon: Option<(Icon, Color32)>,
    pub label: &'a str,
    pub chevron: bool,
    pub filled: bool,
    pub min_w: f32,
    pub height: f32,
}

impl<'a> Pill<'a> {
    pub fn new(label: &'a str) -> Self {
        Pill { icon: None, label, chevron: false, filled: false, min_w: 0.0, height: 32.0 }
    }
    pub fn icon(mut self, i: Icon, c: Color32) -> Self {
        self.icon = Some((i, c));
        self
    }
    pub fn chevron(mut self) -> Self {
        self.chevron = true;
        self
    }
    /// Resting fill (a quiet tonal button) instead of transparent.
    pub fn bordered(mut self) -> Self {
        self.filled = true;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn show(self, ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
        let font = fonts::ui(13.5);
        let galley = ui.painter().layout_no_wrap(self.label.to_string(), font.clone(), pal.text);
        let mut w = 14.0 + galley.size().x + 14.0;
        if self.icon.is_some() {
            w += 24.0;
        }
        if self.chevron {
            w += 22.0;
        }
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w.max(self.min_w), self.height), Sense::click());
        let p = ui.painter();
        let r = CornerRadius::same((self.height / 2.0).min(14.0) as u8);
        if resp.is_pointer_button_down_on() {
            p.rect_filled(rect, r, pal.selected);
        } else if resp.hovered() {
            p.rect_filled(rect, r, if self.filled { pal.hover.gamma_multiply(1.5) } else { pal.hover });
        } else if self.filled {
            p.rect_filled(rect, r, pal.hover.gamma_multiply(0.55));
        }
        let mut x = rect.min.x + 14.0;
        if let Some((icon, color)) = self.icon {
            icons::paint(p, Rect::from_min_size(Pos2::new(x, rect.center().y - 8.0), Vec2::splat(16.0)), icon, color);
            x += 24.0;
        }
        p.text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, self.label, font, pal.text);
        if self.chevron {
            icons::paint(p, Rect::from_min_size(Pos2::new(rect.max.x - 24.0, rect.center().y - 6.0), Vec2::splat(12.0)), Icon::ChevronDown, pal.text_dim);
        }
        resp
    }
}

/// Square icon button with a soft rounded hover.
pub fn icon_button(ui: &mut egui::Ui, pal: &Palette, icon: Icon, size: f32, color: Color32, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if resp.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, CornerRadius::same((size / 3.0) as u8), pal.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same((size / 3.0) as u8), pal.hover);
    }
    let inset = (size * 0.27).round();
    icons::paint(ui.painter(), rect.shrink(inset), icon, color);
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

/// Filled accent button for the main action of a dialog.
pub fn primary_button(ui: &mut egui::Ui, pal: &Palette, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), fonts::semibold(13.5), pal.text_on_accent);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 36.0, 36.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() { pal.accent.gamma_multiply(0.8) } else if resp.hovered() { pal.accent.gamma_multiply(1.15) } else { pal.accent };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    // a light edge along the top: the button is a drop of glass
    ui.painter().rect_stroke(rect, CornerRadius::same(12), Stroke::new(1.0, Color32::from_white_alpha(if pal.dark { 60 } else { 90 })), StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, fonts::semibold(13.5), pal.text_on_accent);
    resp
}

/// Quiet tonal secondary button.
pub fn secondary_button(ui: &mut egui::Ui, pal: &Palette, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), fonts::ui(13.5), pal.text);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 36.0, 36.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() { pal.selected } else if resp.hovered() { pal.hover.gamma_multiply(1.6) } else { pal.hover };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, fonts::ui(13.5), pal.text);
    resp
}

/// A single-line text input: a rounded tonal field of fixed height whose text (and hint) is centred vertically,
/// with an optional leading icon. Clicking anywhere on the field focuses it.
pub struct TextField<'a> {
    text: &'a mut String,
    hint: &'a str,
    icon: Option<Icon>,
    width: Option<f32>,
    height: f32,
    font: FontId,
    id: Option<egui::Id>,
    enabled: bool,
    password: bool,
}

impl<'a> TextField<'a> {
    pub fn new(text: &'a mut String) -> Self {
        TextField { text, hint: "", icon: None, width: None, height: FIELD_H, font: fonts::ui(13.5), id: None, enabled: true, password: false }
    }
    /// Show bullets instead of the characters (a token or password).
    pub fn password(mut self, on: bool) -> Self {
        self.password = on;
        self
    }
    pub fn hint(mut self, h: &'a str) -> Self {
        self.hint = h;
        self
    }
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }
    /// Fixed width (default: the rest of the available width).
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn font(mut self, f: FontId) -> Self {
        self.font = f;
        self
    }
    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = Some(id);
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn show(self, ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
        // derived from (not equal to) the id the allocation below gets, or the focusable edit clashes with that hover-only widget
        let id = self.id.unwrap_or_else(|| ui.next_auto_id().with("text_field"));
        let w = self.width.unwrap_or_else(|| ui.available_width()).max(40.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, self.height), Sense::hover());
        let bg = ui.interact(rect, id.with("field_bg"), Sense::click());
        ui.painter().rect_filled(rect, CornerRadius::same(12), if self.enabled { pal.input_bg } else { pal.input_bg.gamma_multiply(0.6) });
        let mut x = rect.min.x + 12.0;
        if let Some(i) = self.icon {
            icons::paint(ui.painter(), Rect::from_center_size(Pos2::new(x + 8.0, rect.center().y), Vec2::splat(16.0)), i, pal.text_dim);
            x += 26.0;
        }
        let inner = Rect::from_min_max(Pos2::new(x, rect.min.y), Pos2::new((rect.max.x - 10.0).max(x + 10.0), rect.max.y));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(Align::Center)));
        let te = egui::TextEdit::singleline(self.text)
            .id(id)
            .hint_text(egui::RichText::new(self.hint).font(self.font.clone()).color(pal.text_dim))
            .font(self.font)
            .text_color(pal.text)
            .password(self.password)
            .frame(egui::Frame::NONE)
            .margin(Margin::ZERO)
            .align(Align2::LEFT_CENTER)
            .desired_width(inner.width())
            .min_size(Vec2::new(inner.width(), self.height));
        let resp = child.add_enabled(self.enabled, te);
        if self.enabled && bg.clicked() {
            ui.memory_mut(|m| m.request_focus(id));
        }
        resp
    }
}

/// Focus `resp` once when a dialog has just opened. egui lays a new popup out invisibly for one frame (the sizing pass),
/// where a focus request is lost, so the flag is only consumed in a real frame.
pub fn focus_once(ui: &egui::Ui, resp: &egui::Response, flag: &mut bool) {
    if *flag && !ui.is_sizing_pass() {
        resp.request_focus();
        *flag = false;
    }
}

/// A full-width row of fixed `height` whose children are laid out right-to-left and centred vertically
/// (plain `with_layout(right_to_left(Center))` would stretch to all the available height).
pub fn right_row<R>(ui: &mut egui::Ui, height: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(Align::Center)));
    add(&mut child)
}

/// A multi-line text area on a tonal card with even padding (text starts at the top-left).
pub fn text_area(ui: &mut egui::Ui, pal: &Palette, text: &mut String, hint: &str, height: f32, id: Option<egui::Id>) -> egui::Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), pal.input_bg);
    let inner = rect.shrink2(Vec2::new(12.0, 10.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    let mut te = egui::TextEdit::multiline(text)
        .hint_text(egui::RichText::new(hint).font(fonts::ui(13.5)).color(pal.text_dim))
        .font(fonts::ui(13.5))
        .text_color(pal.text)
        .frame(egui::Frame::NONE)
        .margin(Margin::ZERO)
        .desired_width(inner.width())
        .min_size(inner.size());
    if let Some(id) = id {
        te = te.id(id);
    }
    egui::ScrollArea::vertical().id_salt(("text_area", rect.min.x as i32, rect.min.y as i32)).auto_shrink([false, false]).show_fading(&mut child, |ui| ui.add(te)).inner
}

/// The rounded checkbox square used by [`checkbox`] and by checkable lists.
pub fn checkbox_box(painter: &Painter, rect: Rect, on: bool, pal: &Palette) {
    if on {
        painter.rect_filled(rect, CornerRadius::same(5), pal.accent);
        icons::paint(painter, rect.shrink(rect.width() * 0.16), Icon::Check, Color32::WHITE);
    } else {
        painter.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.4, pal.text_dim), StrokeKind::Inside);
    }
}

/// A checkbox that can also show "some of the items are selected" (an accent box with a dash).
pub fn checkbox_state(painter: &Painter, rect: Rect, state: ide_core::changes::Check, pal: &Palette) {
    use ide_core::changes::Check;
    match state {
        Check::On => checkbox_box(painter, rect, true, pal),
        Check::Off => checkbox_box(painter, rect, false, pal),
        Check::Mixed => {
            painter.rect_filled(rect, CornerRadius::same(5), pal.accent);
            let y = rect.center().y;
            painter.line_segment([Pos2::new(rect.min.x + rect.width() * 0.26, y), Pos2::new(rect.max.x - rect.width() * 0.26, y)], Stroke::new(2.0, Color32::WHITE));
        }
    }
}

/// Checkbox with a label to its right.
pub fn checkbox(ui: &mut egui::Ui, pal: &Palette, on: &mut bool, label: &str) -> egui::Response {
    let font = fonts::ui(13.5);
    let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), pal.text);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(18.0 + 8.0 + g.size().x + 4.0, 28.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let b = Rect::from_center_size(Pos2::new(rect.min.x + 9.0, rect.center().y), Vec2::splat(18.0));
    checkbox_box(ui.painter(), b, *on, pal);
    ui.painter().galley(Pos2::new(rect.min.x + 26.0, rect.center().y - g.size().y / 2.0), g, pal.text);
    resp
}

/// A small toggleable pill ("Cc", "W", ...): accent-tinted when on.
pub fn toggle_chip(ui: &mut egui::Ui, pal: &Palette, label: &str, on: &mut bool, tooltip: &str) -> egui::Response {
    let font = fonts::semibold(12.5);
    let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), pal.text);
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new((g.size().x + 20.0).max(32.0), 30.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let r = CornerRadius::same(10);
    if *on {
        ui.painter().rect_filled(rect, r, pal.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, r, pal.hover);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, if *on { pal.accent } else { pal.text_dim });
    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

/// Horizontal slider with a value readout on its left. Returns true when the value changed.
pub fn slider(ui: &mut egui::Ui, pal: &Palette, value: &mut f32, range: std::ops::RangeInclusive<f32>, step: f32) -> bool {
    let (lo, hi) = (*range.start(), *range.end());
    let id = ui.next_auto_id();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(36.0 + 12.0 + 168.0, 28.0), Sense::hover());
    let track = Rect::from_min_max(Pos2::new(rect.min.x + 48.0 + 8.0, rect.center().y - 2.0), Pos2::new(rect.max.x - 8.0, rect.center().y + 2.0));
    let hit = Rect::from_min_max(Pos2::new(track.min.x - 8.0, rect.min.y), Pos2::new(track.max.x + 8.0, rect.max.y));
    let resp = ui.interact(hit, id, Sense::click_and_drag());
    let mut changed = false;
    if (resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let t = ((p.x - track.min.x) / track.width()).clamp(0.0, 1.0);
        let mut v = lo + t * (hi - lo);
        if step > 0.0 {
            v = (v / step).round() * step;
        }
        let v = v.clamp(lo, hi);
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            changed = true;
        }
    }
    let t = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let hx = track.min.x + t * track.width();
    let p = ui.painter();
    p.rect_filled(track, CornerRadius::same(2), pal.hover.gamma_multiply(1.8));
    p.rect_filled(Rect::from_min_max(track.min, Pos2::new(hx, track.max.y)), CornerRadius::same(2), pal.accent);
    let r = if resp.dragged() || resp.hovered() { 9.0 } else { 8.0 };
    p.circle_filled(Pos2::new(hx, rect.center().y), r, pal.accent);
    p.circle_filled(Pos2::new(hx, rect.center().y), r - 3.5, Color32::WHITE);
    let text = if step >= 1.0 { format!("{}", *value as i32) } else { format!("{:.1}", *value) };
    p.text(Pos2::new(rect.min.x + 40.0, rect.center().y), Align2::RIGHT_CENTER, text, fonts::semibold(13.5), pal.text);
    changed
}

/// iOS-style switch.
pub fn toggle(ui: &mut egui::Ui, pal: &Palette, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let track = if *on { pal.accent } else { pal.hover.gamma_multiply(1.8) };
    ui.painter().rect_filled(rect, CornerRadius::same(11), track);
    let x = rect.min.x + 11.0 + t * 18.0;
    ui.painter().circle_filled(Pos2::new(x, rect.center().y), 8.0, Color32::WHITE);
    resp
}

/// Segmented control; returns the index chosen this frame.
pub fn segmented(ui: &mut egui::Ui, pal: &Palette, options: &[&str], selected: usize) -> Option<usize> {
    let mut chosen = None;
    let font = fonts::ui(13.0);
    // two controls with the same labels in one page (the dark/light choice of the theme and of your own theme) must not share ids
    let base = ui.next_auto_id();
    let widths: Vec<f32> = options.iter().map(|o| ui.painter().layout_no_wrap(o.to_string(), font.clone(), pal.text).size().x + 28.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 8.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, FIELD_H - 2.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), pal.input_bg);
    let mut x = rect.min.x + 4.0;
    for (i, o) in options.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y + 4.0), Vec2::new(widths[i], rect.height() - 8.0));
        let resp = ui.interact(r, base.with(("seg", i, *o)), Sense::click());
        if i == selected {
            ui.painter().rect_filled(r, CornerRadius::same(9), pal.tab_active_bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(9), pal.hover.gamma_multiply(0.6));
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, o, if i == selected { fonts::semibold(13.0) } else { font.clone() }, if i == selected { pal.text } else { pal.text_dim });
        if resp.clicked() {
            chosen = Some(i);
        }
        x += widths[i];
    }
    chosen
}

// ---------------------------------------------------------------------- small pieces

/// Section caption (small, dim).
pub fn caption(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(egui::RichText::new(text).font(fonts::semibold(11.5)).color(pal.text_dim));
}

/// Small rounded "chip" such as a keyboard hint.
pub fn chip(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    let g = ui.painter().layout_no_wrap(text.to_string(), fonts::ui(11.5), pal.text_dim);
    let (r, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 14.0, 22.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(7), pal.hover);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, fonts::ui(11.5), pal.text_dim);
}

/// A row background used by lists/trees: translucent selection or hover, rounded with an inset.
pub fn row_background(ui: &egui::Ui, pal: &Palette, rect: Rect, selected: bool, hovered: bool) {
    let r = rect.shrink2(Vec2::new(8.0, 1.0));
    if selected {
        ui.painter().rect_filled(r, CornerRadius::same(9), pal.selected);
    } else if hovered {
        ui.painter().rect_filled(r, CornerRadius::same(9), pal.hover);
    }
}

/// Barely visible hairline (used only where a separation is really needed).
pub fn divider(ui: &egui::Ui, pal: &Palette, y: f32, x0: f32, x1: f32) {
    ui.painter().line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(1.0, pal.border));
}

/// Thin determinate progress bar.
pub fn progress_bar(ui: &mut egui::Ui, pal: &Palette, fraction: f32, width: f32) {
    progress_bar_opt(ui, pal, Some(fraction), width);
}

/// A progress bar whose total may be unknown (`None`): a segment then slides back and forth while the work goes on.
pub fn progress_bar_opt(ui: &mut egui::Ui, pal: &Palette, fraction: Option<f32>, width: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), Sense::hover());
    paint_progress(ui, r, pal.input_bg, pal.accent, fraction);
}

/// Paint a progress bar of any size into `r` (the height gives the rounding).
pub fn paint_progress(ui: &egui::Ui, r: Rect, track: Color32, fill: Color32, fraction: Option<f32>) {
    let round = CornerRadius::same((r.height() / 2.0).round().clamp(1.0, 127.0) as u8);
    ui.painter().rect_filled(r, round, track);
    match fraction {
        Some(f) if f > 0.0 => {
            let mut done = r;
            done.set_width((r.width() * f.clamp(0.0, 1.0)).max(r.height()));
            ui.painter().rect_filled(done, round, fill);
        }
        Some(_) => {}
        None => {
            let seg = (r.width() * 0.3).max(r.height());
            let t = ui.input(|i| i.time) as f32;
            // there and back again, slowing down at the ends
            let phase = (t * 0.7) % 2.0;
            let p = if phase < 1.0 { phase } else { 2.0 - phase };
            let eased = p * p * (3.0 - 2.0 * p);
            let x = r.left() + (r.width() - seg) * eased;
            ui.painter().rect_filled(Rect::from_min_size(Pos2::new(x, r.top()), Vec2::new(seg, r.height())), round, fill);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}

// the grid is consistent: checked when the program is compiled
const _: () = assert!(TAB_H > ROW_H && CARD_RADIUS > RADIUS);
