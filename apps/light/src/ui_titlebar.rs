//! Custom window frame: the OS title bar is removed (`with_decorations(false)`), so the app draws its
//! own title bar (logo, main menu, title, minimize/maximize/close), drag-to-move and edge resizing.

use eframe::egui::{self, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, ViewportCommand};

use crate::app::App;
use crate::fonts;
use crate::widgets;

pub const TITLEBAR_H: f32 = widgets::MENU_H + 4.0;
const BTN_W: f32 = 48.0;
const RESIZE: f32 = 6.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctl {
    Minimize,
    Maximize,
    Close,
}

impl App {
    pub fn draw_titlebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let bar = ui.max_rect();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        // the empty part of the bar drags the window; widgets added later take priority
        let drag = ui.interact(bar, ui.scope_id().with("titlebar_drag"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        // centered title (only when it cannot collide with the menu)
        let title = match (&self.project, self.doc()) {
            (Some(p), Some(d)) => format!("{}  -  {}", p.name, d.name()),
            (Some(p), None) => p.name.clone(),
            (None, Some(d)) => d.name(),
            (None, None) => "Light".to_string(),
        };
        let tg = ui.painter().layout_no_wrap(title, fonts::ui(12.5), pal.text_dim);
        // the main menu occupies roughly the first 640 px
        if bar.center().x - tg.size().x / 2.0 > bar.min.x + 700.0 {
            ui.painter().galley(Pos2::new(bar.center().x - tg.size().x / 2.0, bar.center().y - tg.size().y / 2.0), tg, pal.text_dim);
        }

        // window controls, painted at the far right
        let mut action: Option<Ctl> = None;
        for (i, ctl) in [Ctl::Minimize, Ctl::Maximize, Ctl::Close].into_iter().enumerate() {
            let x = bar.max.x - BTN_W * (3 - i) as f32;
            let r = Rect::from_min_size(Pos2::new(x, bar.min.y), Vec2::new(BTN_W, bar.height()));
            let resp = ui.interact(r, ui.scope_id().with(("winctl", i)), Sense::click());
            let hot = resp.hovered();
            if hot {
                ui.painter().rect_filled(r, CornerRadius::ZERO, if ctl == Ctl::Close { Color32::from_rgb(0xE8, 0x11, 0x23) } else { pal.hover });
            }
            let c = r.center();
            let color = if ctl == Ctl::Close && hot { Color32::WHITE } else { pal.text };
            let s = Stroke::new(1.2, color);
            let p = ui.painter();
            match ctl {
                Ctl::Minimize => {
                    p.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(5.0, 0.0)], s);
                }
                Ctl::Maximize => {
                    if maximized {
                        p.rect_stroke(Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(8.0)), CornerRadius::ZERO, s, StrokeKind::Inside);
                        p.line_segment([c + Vec2::new(-1.0, -4.0), c + Vec2::new(5.0, -4.0)], s);
                        p.line_segment([c + Vec2::new(5.0, -4.0), c + Vec2::new(5.0, 3.0)], s);
                    } else {
                        p.rect_stroke(Rect::from_center_size(c, Vec2::splat(9.0)), CornerRadius::ZERO, s, StrokeKind::Inside);
                    }
                }
                Ctl::Close => {
                    p.line_segment([c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)], s);
                    p.line_segment([c + Vec2::new(5.0, -5.0), c + Vec2::new(-5.0, 5.0)], s);
                }
            }
            if resp.clicked() {
                action = Some(ctl);
            }
        }
        match action {
            Some(Ctl::Minimize) => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
            Some(Ctl::Maximize) => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
            Some(Ctl::Close) => self.run_command(ctx, "app.exit"),
            None => {}
        }

        // logo + menu
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(bar.min, Pos2::new(bar.max.x - BTN_W * 3.0, bar.max.y))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        inner.add_space(12.0);
        let (r, _) = inner.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
        crate::brand::paint_logo(inner.painter(), r, self.logo.as_ref(), pal.accent);
        inner.add_space(8.0);
        self.draw_menu_bar(&mut inner, ctx);
    }

    /// Invisible edge/corner handles that start an OS-level resize.
    pub fn draw_resize_handles(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
            return;
        }
        use egui::viewport::ResizeDirection as D;
        let s = ctx.content_rect();
        let c = RESIZE * 2.0;
        let zones: [(Rect, D, CursorIcon); 8] = [
            (Rect::from_min_max(Pos2::new(s.min.x + c, s.min.y), Pos2::new(s.max.x - c, s.min.y + RESIZE)), D::North, CursorIcon::ResizeVertical),
            (Rect::from_min_max(Pos2::new(s.min.x + c, s.max.y - RESIZE), Pos2::new(s.max.x - c, s.max.y)), D::South, CursorIcon::ResizeVertical),
            (Rect::from_min_max(Pos2::new(s.min.x, s.min.y + c), Pos2::new(s.min.x + RESIZE, s.max.y - c)), D::West, CursorIcon::ResizeHorizontal),
            (Rect::from_min_max(Pos2::new(s.max.x - RESIZE, s.min.y + c), Pos2::new(s.max.x, s.max.y - c)), D::East, CursorIcon::ResizeHorizontal),
            (Rect::from_min_size(s.min, Vec2::splat(c)), D::NorthWest, CursorIcon::ResizeNwSe),
            (Rect::from_min_size(Pos2::new(s.max.x - c, s.min.y), Vec2::splat(c)), D::NorthEast, CursorIcon::ResizeNeSw),
            (Rect::from_min_size(Pos2::new(s.min.x, s.max.y - c), Vec2::splat(c)), D::SouthWest, CursorIcon::ResizeNeSw),
            (Rect::from_min_size(Pos2::new(s.max.x - c, s.max.y - c), Vec2::splat(c)), D::SouthEast, CursorIcon::ResizeNwSe),
        ];
        egui::Area::new(egui::Id::unique("resize_handles")).order(egui::Order::Foreground).fixed_pos(Pos2::ZERO).interactable(true).show(ctx, |ui| {
            for (i, (rect, dir, cursor)) in zones.into_iter().enumerate() {
                let resp = ui.interact(rect, egui::Id::unique(("resize", i)), Sense::click_and_drag());
                if resp.hovered() || resp.dragged() {
                    ctx.set_cursor_icon(cursor);
                }
                if resp.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
                }
            }
        });
    }
}
