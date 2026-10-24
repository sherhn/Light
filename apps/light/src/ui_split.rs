//! Drawing of the split editor area: two panes, each with a small header, the focused one marked with an accent line.

use eframe::egui::{self, Align, CornerRadius, Layout, Rect, Sense, UiBuilder, pos2, vec2};

use crate::app::App;
use crate::fonts;
use crate::i18n::tr;
use crate::icons::{self, Icon};

const PANE_HEAD_H: f32 = 30.0;
const GAP: f32 = 1.0;

impl App {
    pub fn draw_split_panes(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(sp) = self.split.as_ref() else { return };
        let (vertical, second_focused) = (sp.vertical, sp.second_focused);
        let Some(other) = self.split_other_index() else { return };
        let active = self.active;
        let pal = self.pal;
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let (r1, r2) = if vertical {
            let w = ((full.width() - GAP) / 2.0).floor();
            (Rect::from_min_size(full.min, vec2(w, full.height())), Rect::from_min_size(pos2(full.min.x + w + GAP, full.min.y), vec2(full.width() - w - GAP, full.height())))
        } else {
            let h = ((full.height() - GAP) / 2.0).floor();
            (Rect::from_min_size(full.min, vec2(full.width(), h)), Rect::from_min_size(pos2(full.min.x, full.min.y + h + GAP), vec2(full.width(), full.height() - h - GAP)))
        };
        let divider = if vertical { Rect::from_min_max(pos2(r1.max.x, full.min.y), pos2(r2.min.x, full.max.y)) } else { Rect::from_min_max(pos2(full.min.x, r1.max.y), pos2(full.max.x, r2.min.y)) };
        ui.painter().rect_filled(divider, CornerRadius::ZERO, pal.hover);

        self.pane_rects = vec![r1, r2];
        let order = if second_focused { [other, active] } else { [active, other] };
        let focus = if second_focused { [false, true] } else { [true, false] };
        let same_doc = other == active;
        let pressed = ctx.input(|i| i.pointer.any_pressed()).then(|| ctx.input(|i| i.pointer.interact_pos())).flatten();
        let mut switch_focus = false;
        let mut unsplit = false;
        for (k, rect) in [r1, r2].into_iter().enumerate() {
            let (idx, focused) = (order[k], focus[k]);
            // each pane needs its own widget ids: the editor view and the diff view use fixed ones
            let mut pane = ui.new_child(UiBuilder::new().id_salt(("split_pane", k)).max_rect(rect).layout(Layout::top_down(Align::Min)));
            pane.set_clip_rect(rect);
            pane.spacing_mut().item_spacing.y = 0.0;
            // ---- header: file name, close button, accent line under the focused pane
            let (hr, _) = pane.allocate_exact_size(vec2(rect.width(), PANE_HEAD_H), Sense::hover());
            let (name, is_diff, modified) = {
                let d = &self.docs[idx];
                (d.name(), d.diff.is_some(), d.ed.is_modified() && d.path.is_none())
            };
            let icon = if is_diff { Icon::FileDiff } else { Icon::File };
            icons::paint(pane.painter(), Rect::from_center_size(pos2(hr.min.x + 22.0, hr.center().y), vec2(15.0, 15.0)), icon, if focused { pal.accent } else { pal.text_dim });
            let label = if modified { format!("{name}  \u{25CF}") } else { name };
            let font = if focused { fonts::semibold(12.5) } else { fonts::ui(12.5) };
            pane.painter().with_clip_rect(hr.shrink2(vec2(36.0, 0.0))).text(pos2(hr.min.x + 36.0, hr.center().y), egui::Align2::LEFT_CENTER, label, font, if focused { pal.text } else { pal.text_dim });
            let cr = Rect::from_center_size(pos2(hr.max.x - 18.0, hr.center().y), vec2(22.0, 22.0));
            let close = pane.interact(cr, pane.scope_id().with(("pane_close", k)), Sense::click());
            if close.hovered() {
                pane.painter().rect_filled(cr, CornerRadius::same(6), pal.hover);
            }
            icons::paint(pane.painter(), cr.shrink(5.0), Icon::Close, if close.hovered() { pal.text } else { pal.text_dim });
            if close.on_hover_text(tr("Close split")).clicked() {
                unsplit = true;
            }
            if focused {
                pane.painter().rect_filled(Rect::from_min_size(pos2(hr.min.x, hr.max.y - 2.0), vec2(hr.width(), 2.0)), CornerRadius::ZERO, pal.accent);
            }
            // ---- body
            let own_view = !focused && same_doc;
            self.draw_doc_pane(&mut pane, ctx, idx, focused, own_view);
            if !focused && pressed.is_some_and(|p| rect.contains(p)) {
                switch_focus = true;
            }
        }
        if unsplit {
            self.unsplit();
        } else if switch_focus {
            self.focus_other_pane();
        }
    }
}
