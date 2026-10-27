//! Scrollbars that stay out of the way: thin and faint, shown only while the area is being used
//! (pointer moving over it, wheel, drag, keyboard scrolling) and faded out a moment later.
//!
//! egui draws floating scrollbars itself but fades them with `Style::animation_time`, which this app
//! sets to zero so menus never fade through the code. Each scroll area therefore keeps its own
//! "last used" time here and passes the resulting opacity to egui for the frame.

use eframe::egui::scroll_area::ScrollAreaOutput;
use eframe::egui::{self, Context, Id, Rect, ScrollArea, Ui, Vec2};
use std::ops::Range;

/// How long a bar stays fully visible after the last use, and how long it then takes to disappear.
const HOLD: f64 = 1.0;
const FADE: f64 = 0.35;
/// Resting look: how wide the bar is while idle and while the pointer is on it, and how strong the handle is.
const THIN: f32 = 3.0;
const WIDE: f32 = 8.0;
const HANDLE: f32 = 0.30;
const HANDLE_HOVER: f32 = 0.55;
const TRACK_HOVER: f32 = 0.16;
/// Space kept free beside content that must not be covered by the bar (diffs, file lists).
const GUTTER: f32 = 8.0;

/// The style every scroll area starts from; `Palette::apply` installs it.
pub fn style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating_width: THIN,
        bar_width: WIDE,
        foreground_color: true,
        dormant_handle_opacity: 0.0,
        active_handle_opacity: HANDLE,
        interact_handle_opacity: HANDLE_HOVER,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: TRACK_HOVER,
        ..egui::style::ScrollStyle::floating()
    }
}

/// Keep a strip free for the bar so it never covers content, while it still fades like the others.
pub fn reserve_gutter(ui: &mut Ui) {
    ui.spacing_mut().scroll.floating_allocated_width = GUTTER;
}

/// What the last frame knew about one scrollable region: where it was scrolled and when it was last used.
#[derive(Clone, Copy)]
struct Seen {
    offset: Vec2,
    used_at: f64,
}

/// Opacity factor `0..=1` for a region last used at `used_at`.
fn visibility(used_at: f64, now: f64) -> f32 {
    let idle = now - used_at;
    if idle <= HOLD {
        1.0
    } else {
        let t = ((idle - HOLD) / FADE).clamp(0.0, 1.0) as f32;
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// Visibility of the region `key` this frame, from what the previous frame recorded.
fn current(ctx: &Context, key: Id) -> f32 {
    let now = ctx.input(|i| i.time);
    ctx.data(|d| d.get_temp::<Seen>(key)).map_or(0.0, |seen| visibility(seen.used_at, now))
}

/// Record this frame's use of the region `key` (laid out at `rect`, scrolled to `offset`) and return its
/// visibility. `engaged` marks use the caller knows about, such as dragging its own bar.
pub fn touch(ctx: &Context, key: Id, rect: Rect, offset: Vec2, engaged: bool) -> f32 {
    let (now, pointer_used) = ctx.input(|i| {
        let over = i.pointer.hover_pos().is_some_and(|p| rect.contains(p));
        (i.time, over && (i.pointer.delta() != Vec2::ZERO || i.smooth_scroll_delta != Vec2::ZERO || i.pointer.any_down()))
    });
    let previous: Option<Seen> = ctx.data(|d| d.get_temp(key));
    let moved = previous.is_some_and(|p| p.offset != offset);
    let used_at = if engaged || pointer_used || moved { now } else { previous.map_or(f64::NEG_INFINITY, |p| p.used_at) };
    ctx.data_mut(|d| d.insert_temp(key, Seen { offset, used_at }));
    let visible = visibility(used_at, now);
    let shown_before = previous.map_or(0.0, |p| visibility(p.used_at, now));
    if used_at == now && shown_before < 1.0 {
        // the handle was drawn from the previous frame's state: draw it again right away
        ctx.request_repaint();
    } else if visible >= 1.0 {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOLD - (now - used_at) + 0.02));
    } else if visible > 0.0 {
        ctx.request_repaint();
    }
    visible
}

/// `ScrollArea` with scrollbars that fade out while idle.
pub trait FadingScroll: Sized {
    fn show_fading<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R>;
    fn show_rows_fading<R>(self, ui: &mut Ui, row_height: f32, rows: usize, add: impl FnOnce(&mut Ui, Range<usize>) -> R) -> ScrollAreaOutput<R>;
}

/// Run `show` with this area's handle faded to its current visibility, then record what happened.
fn faded<R>(ui: &mut Ui, show: impl FnOnce(&mut Ui) -> ScrollAreaOutput<R>) -> ScrollAreaOutput<R> {
    let key = ui.scope_id().with(ui.next_auto_id()).with("scroll_fade");
    let visible = current(ui.ctx(), key);
    let saved = ui.spacing().scroll;
    let handle = saved.active_handle_opacity * visible;
    {
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.dormant_handle_opacity = handle;
        scroll.active_handle_opacity = handle;
    }
    let out = show(ui);
    ui.spacing_mut().scroll = saved;
    // the bar sits at the right and bottom edge, partly outside the content rectangle
    let region = Rect::from_min_max(out.inner_rect.min, out.inner_rect.max + Vec2::splat(WIDE));
    touch(ui.ctx(), key, region, out.state.clamped_offset(), false);
    out
}

impl FadingScroll for ScrollArea {
    fn show_fading<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> ScrollAreaOutput<R> {
        faded(ui, |ui| self.show(ui, add))
    }

    fn show_rows_fading<R>(self, ui: &mut Ui, row_height: f32, rows: usize, add: impl FnOnce(&mut Ui, Range<usize>) -> R) -> ScrollAreaOutput<R> {
        faded(ui, |ui| self.show_rows(ui, row_height, rows, add))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(time: f64, ctx: &Context, events: Vec<egui::Event>, run: impl FnMut(&mut Ui)) {
        let input = egui::RawInput { time: Some(time), events, screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(400.0, 300.0))), ..Default::default() };
        let mut output = ctx.run_ui(input, run);
        output.textures_delta.clear();
    }

    #[test]
    fn a_bar_is_solid_while_used_then_fades_out_smoothly() {
        assert_eq!(visibility(10.0, 10.0), 1.0);
        assert_eq!(visibility(10.0, 10.0 + HOLD), 1.0);
        let mid = visibility(10.0, 10.0 + HOLD + FADE / 2.0);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert!(visibility(10.0, 10.0 + HOLD + FADE * 0.25) > mid);
        assert_eq!(visibility(10.0, 10.0 + HOLD + FADE), 0.0);
        assert_eq!(visibility(f64::NEG_INFINITY, 10.0), 0.0);
    }

    #[test]
    fn pointer_movement_over_the_region_shows_the_bar_and_idle_time_hides_it() {
        let ctx = Context::default();
        let key = Id::unique("region");
        let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(200.0, 200.0));
        let inside = egui::Pos2::new(50.0, 50.0);
        frame(1.0, &ctx, vec![], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::ZERO, false), 0.0));
        // the first position only places the pointer; moving it is what counts as use
        frame(1.05, &ctx, vec![egui::Event::PointerMoved(egui::Pos2::new(40.0, 40.0))], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::ZERO, false), 0.0));
        frame(1.1, &ctx, vec![egui::Event::PointerMoved(inside)], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::ZERO, false), 1.0));
        frame(1.1 + HOLD * 0.5, &ctx, vec![], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::ZERO, false), 1.0));
        frame(1.1 + HOLD + FADE + 0.1, &ctx, vec![], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::ZERO, false), 0.0));
        frame(5.0, &ctx, vec![egui::Event::PointerMoved(egui::Pos2::new(300.0, 250.0))], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::ZERO, false), 0.0));
    }

    #[test]
    fn scrolling_without_the_pointer_and_dragging_the_bar_also_count_as_use() {
        let ctx = Context::default();
        let key = Id::unique("region");
        let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(200.0, 200.0));
        frame(1.0, &ctx, vec![], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::ZERO, false), 0.0));
        frame(2.0, &ctx, vec![], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::new(0.0, 40.0), false), 1.0));
        frame(2.0 + HOLD + FADE + 0.1, &ctx, vec![], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::new(0.0, 40.0), false), 0.0));
        frame(9.0, &ctx, vec![], |ui| assert_eq!(touch(ui.ctx(), key, rect, Vec2::new(0.0, 40.0), true), 1.0));
    }

    #[test]
    fn the_resting_style_is_thin_and_faint_and_the_wrapper_restores_it() {
        let rest = style();
        assert_eq!(rest.dormant_handle_opacity, 0.0);
        assert!(rest.active_handle_opacity <= 0.35, "handle is too contrasty");
        assert!(rest.floating_width <= 4.0, "handle is too thick");
        let ctx = Context::default();
        let mut seen_inside = None;
        for (time, events) in [(1.0, vec![egui::Event::PointerMoved(egui::Pos2::new(100.0, 100.0))]), (1.1, vec![egui::Event::PointerMoved(egui::Pos2::new(102.0, 101.0))]), (1.2, vec![])] {
            frame(time, &ctx, events, |ui| {
                ui.spacing_mut().scroll = rest;
                let out = ScrollArea::vertical().id_salt("faded").auto_shrink([false, false]).show_fading(ui, |ui| {
                    seen_inside = Some(ui.spacing().scroll.active_handle_opacity);
                    ui.allocate_space(Vec2::new(100.0, 2000.0));
                });
                assert!(out.content_size.y > out.inner_rect.height());
                assert_eq!(ui.spacing().scroll.active_handle_opacity, rest.active_handle_opacity);
                assert_eq!(ui.spacing().scroll.dormant_handle_opacity, rest.dormant_handle_opacity);
            });
        }
        // the handle is drawn from what the previous frame saw: hidden until the pointer has moved over it, then shown
        assert_eq!(seen_inside, Some(rest.active_handle_opacity));
    }
}
