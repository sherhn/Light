//! Drawing of the diff tab: a header with navigation, then the old and the new text in two aligned, tinted columns
//! (or as one unified list).

use crate::scrollbars::FadingScroll as _;
use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Layout, Painter, Rect, RichText, Sense, UiBuilder, pos2, vec2};
use ide_core::diff::{LineKind, RowKind, SideRow};

use crate::app::App;
use crate::fonts;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme::Palette;
use crate::widgets;

const HEADER_H: f32 = 46.0;
/// Below this width the header wraps into two rows and the columns give way to the unified view.
const NARROW_W: f32 = 640.0;
const WRAP_HEADER_W: f32 = 860.0;
const COLUMN_HEAD_H: f32 = 26.0;
const ROW_H: f32 = 21.0;
const NUM_W: f32 = 52.0;

/// Fill of the empty side of a row that exists only on the other side.
fn filler(pal: &Palette) -> Color32 {
    pal.hover.gamma_multiply(0.5)
}

#[allow(clippy::too_many_arguments)]
fn paint_side_row(painter: &Painter, pal: &Palette, r: Rect, half: f32, row: &SideRow, mono: &FontId, cw: f32, current: bool) {
    let left = Rect::from_min_size(r.min, vec2(half, r.height()));
    let right = Rect::from_min_size(pos2(r.min.x + half + 1.0, r.min.y), vec2(half, r.height()));
    let (red, green) = (pal.error, pal.git_added);
    let (lbg, rbg) = match row.kind {
        RowKind::Same => (Color32::TRANSPARENT, Color32::TRANSPARENT),
        RowKind::Changed => (red.gamma_multiply(0.15), green.gamma_multiply(0.15)),
        RowKind::Removed => (red.gamma_multiply(0.15), filler(pal)),
        RowKind::Added => (filler(pal), green.gamma_multiply(0.15)),
    };
    painter.rect_filled(left, CornerRadius::ZERO, lbg);
    painter.rect_filled(right, CornerRadius::ZERO, rbg);
    painter.rect_filled(Rect::from_min_size(pos2(r.min.x + half, r.min.y), vec2(1.0, r.height())), CornerRadius::ZERO, pal.hover);
    for (cell, area, tint) in [(&row.left, left, red), (&row.right, right, green)] {
        let Some(c) = cell else { continue };
        let p = painter.with_clip_rect(area.intersect(painter.clip_rect()));
        let text_x = area.min.x + NUM_W + 8.0;
        if let Some((a, b)) = c.hl {
            let hl = Rect::from_min_max(pos2(text_x + a as f32 * cw, area.min.y + 1.0), pos2(text_x + b as f32 * cw, area.max.y - 1.0));
            p.rect_filled(hl, CornerRadius::same(3), tint.gamma_multiply(0.34));
        }
        p.text(pos2(area.min.x + NUM_W - 8.0, area.center().y), Align2::RIGHT_CENTER, c.no, mono.clone(), pal.line_number);
        p.text(pos2(text_x, area.center().y), Align2::LEFT_CENTER, c.text.replace('\t', "    "), mono.clone(), pal.plain);
    }
    if current {
        painter.rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), CornerRadius::ZERO, pal.accent);
    }
}

impl App {
    /// Draw the diff tab `idx` into the remaining space of `ui`.
    pub fn draw_diff_view(&mut self, ui: &mut egui::Ui, idx: usize) {
        self.refresh_diff(idx, false);
        let pal = self.pal;
        let n_files = self.git.changes.iter().filter(|c| !c.path.ends_with('/')).count();
        let Some(tab) = self.docs.get(idx).and_then(|d| d.diff.as_deref()) else { return };
        if tab.rel.is_empty() {
            let msg = tr("Select a changed file in the Commit window to see its changes");
            self.empty_state(ui, Icon::FileDiff, pal.text_dim, msg);
            return;
        }
        let (rel, kind, unified, binary) = (tab.rel.clone(), tab.kind, tab.unified, tab.binary);
        let (added, removed, n_hunks, cur) = (tab.sbs.added, tab.sbs.removed, tab.hunk_count(), tab.cur);
        let base_label = tab.base_label.clone();
        let can_open = tab.abs.is_file();
        let kind_color = self.git_color(kind);
        let mono = FontId::monospace((self.settings.font_size - 0.5).max(9.0));
        let cw = ui.ctx().fonts_mut(|f| f.glyph_width(&mono, '0'));

        // ---------------------------------------------------------------- header
        let (mut step_file, mut step_change, mut open_file, mut edit_beside, mut set_unified) = (0i32, 0i32, false, false, None::<bool>);
        let two_rows = ui.available_width() < WRAP_HEADER_W;
        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), if two_rows { HEADER_H * 2.0 - 8.0 } else { HEADER_H }), Sense::hover());
        let inner = hr.shrink2(vec2(14.0, 0.0));
        let (top_row, bottom_row) = if two_rows {
            (Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_H - 4.0)), Rect::from_min_size(pos2(inner.min.x, inner.min.y + HEADER_H - 4.0), vec2(inner.width(), HEADER_H - 4.0)))
        } else {
            (inner, inner)
        };
        let mut left = ui.new_child(UiBuilder::new().max_rect(top_row).layout(Layout::left_to_right(Align::Center)));
        left.spacing_mut().item_spacing.x = 6.0;
        left.add_enabled_ui(n_files > 1, |ui| {
            let c = if n_files > 1 { pal.text } else { pal.text_dim.gamma_multiply(0.5) };
            if widgets::icon_button(ui, &pal, Icon::ChevronLeft, 30.0, c, tr("Previous file")).clicked() {
                step_file = -1;
            }
            if widgets::icon_button(ui, &pal, Icon::ChevronRight, 30.0, c, tr("Next file")).clicked() {
                step_file = 1;
            }
        });
        left.add_space(4.0);
        let (br, _) = left.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        left.painter().rect_filled(br, CornerRadius::same(6), kind_color.gamma_multiply(0.22));
        left.painter().text(br.center(), Align2::CENTER_CENTER, kind.letter(), fonts::semibold(11.5), kind_color);
        let (name, dir) = match rel.rsplit_once('/') {
            Some((d, n)) => (n.to_string(), d.to_string()),
            None => (rel.clone(), String::new()),
        };
        left.label(RichText::new(name).font(fonts::semibold(14.5)));
        if !dir.is_empty() {
            left.label(RichText::new(dir).color(pal.text_dim).size(12.0));
        }
        left.add_space(6.0);
        left.label(RichText::new(format!("+{added}")).color(pal.git_added).font(fonts::semibold(12.5)));
        left.label(RichText::new(format!("-{removed}")).color(pal.error).font(fonts::semibold(12.5)));

        let mut right = ui.new_child(UiBuilder::new().max_rect(bottom_row).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 6.0;
        if can_open && widgets::secondary_button(&mut right, &pal, tr("Open file")).on_hover_text(tr("Open the file in an editor tab")).clicked() {
            open_file = true;
        }
        if can_open && widgets::secondary_button(&mut right, &pal, tr("Edit beside")).on_hover_text(tr("Edit the file in a split next to the diff; the diff updates as you type")).clicked() {
            edit_beside = true;
        }
        // narrow panes always show the unified list: two columns would leave each of them a few characters
        let forced_unified = ui.available_width() < NARROW_W;
        if let Some(i) = widgets::segmented(&mut right, &pal, &[tr("Side by side"), tr("Unified")], usize::from(unified || forced_unified)) {
            set_unified = Some(i == 1);
        }
        right.add_space(6.0);
        right.add_enabled_ui(n_hunks > 0, |ui| {
            let c = if n_hunks > 0 { pal.text } else { pal.text_dim.gamma_multiply(0.5) };
            if widgets::icon_button(ui, &pal, Icon::ArrowDown, 30.0, c, tr("Next difference (F7)")).clicked() {
                step_change = 1;
            }
            if widgets::icon_button(ui, &pal, Icon::ArrowUp, 30.0, c, tr("Previous difference (Shift+F7)")).clicked() {
                step_change = -1;
            }
        });
        if n_hunks > 0 {
            let counter = format!("{} / {n_hunks}", cur.map_or(0, |c| c + 1));
            right.label(RichText::new(counter).color(pal.text_dim).size(12.0));
        }

        // ---------------------------------------------------------------- body
        let body = ui.available_rect_before_wrap();
        if binary {
            self.empty_state(ui, Icon::File, pal.text_dim, tr("Binary file: nothing to compare"));
        } else if n_hunks == 0 {
            self.empty_state(ui, Icon::Check, pal.ok, tr("No differences"));
        } else if unified || forced_unified {
            self.draw_unified_rows(ui, idx, &mono);
        } else {
            let col_h = COLUMN_HEAD_H;
            let (hr, _) = ui.allocate_exact_size(vec2(body.width(), col_h), Sense::hover());
            let half_head = (hr.width() - 1.0) / 2.0;
            let lbl = crate::ui_chrome::elide(&base_label, 44);
            ui.painter().text(pos2(hr.min.x + NUM_W + 8.0, hr.center().y), Align2::LEFT_CENTER, lbl, fonts::semibold(12.0), pal.text_dim);
            ui.painter().text(pos2(hr.min.x + half_head + 1.0 + NUM_W + 8.0, hr.center().y), Align2::LEFT_CENTER, tr("Working copy"), fonts::semibold(12.0), pal.text_dim);
            self.draw_side_rows(ui, idx, &mono, cw);
        }

        if step_file != 0 {
            self.diff_step_file(step_file);
        }
        if step_change != 0
            && self.docs.get(idx).is_some_and(|d| d.diff.is_some())
        {
            let was = self.active;
            self.active = idx;
            self.diff_step_change(step_change);
            self.active = was;
        }
        if let Some(u) = set_unified
            && let Some(t) = self.docs.get_mut(idx).and_then(|d| d.diff.as_mut())
        {
            t.unified = u;
            t.scroll_to = t.cur.and_then(|c| t.sbs.hunks.get(c)).map(|h| h.0);
        }
        if open_file || edit_beside {
            self.show_doc(idx);
            if edit_beside {
                self.diff_edit_beside();
            } else {
                self.diff_open_file();
            }
        }
    }

    fn draw_side_rows(&mut self, ui: &mut egui::Ui, idx: usize, mono: &FontId, cw: f32) {
        let pal = self.pal;
        let Some(tab) = self.docs.get_mut(idx).and_then(|d| d.diff.as_mut()) else { return };
        ui.spacing_mut().item_spacing.y = 0.0;
        crate::scrollbars::reserve_gutter(ui);
        let longest = tab.sbs.rows.iter().flat_map(|r| [&r.left, &r.right]).filter_map(|c| c.as_ref()).map(|c| ui.painter().layout_no_wrap(c.text.replace('\t', "    "), mono.clone(), pal.plain).size().x).fold(0.0, f32::max);
        let half = ((ui.available_width() - 1.0) / 2.0).max(NUM_W + 24.0 + longest).ceil();
        let total_w = half * 2.0 + 1.0;
        let mut area = egui::ScrollArea::both().id_salt(("diff_side", idx)).auto_shrink([false, false]);
        if let Some(r) = tab.scroll_to.take() {
            area = area.vertical_scroll_offset(r.saturating_sub(3) as f32 * ROW_H);
        }
        let hunks = tab.sbs.hunks.clone();
        let cur = tab.cur;
        let mut clicked: Option<usize> = None;
        area.show_rows_fading(ui, ROW_H, tab.sbs.rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let (r, resp) = ui.allocate_exact_size(vec2(total_w, ROW_H), Sense::click());
                let h = hunks.partition_point(|h| h.1 <= i);
                let in_hunk = hunks.get(h).filter(|k| k.0 <= i).map(|_| h);
                paint_side_row(ui.painter(), &pal, r, half, &tab.sbs.rows[i], mono, cw, in_hunk.is_some() && in_hunk == cur);
                if resp.clicked() {
                    clicked = in_hunk;
                }
            }
        });
        if clicked.is_some() {
            tab.cur = clicked;
        }
    }

    fn draw_unified_rows(&mut self, ui: &mut egui::Ui, idx: usize, mono: &FontId) {
        let pal = self.pal;
        let Some(tab) = self.docs.get_mut(idx).and_then(|d| d.diff.as_mut()) else { return };
        ui.spacing_mut().item_spacing.y = 0.0;
        crate::scrollbars::reserve_gutter(ui);
        let mut area = egui::ScrollArea::both().id_salt(("diff_unified", idx)).auto_shrink([false, false]);
        if let Some(r) = tab.scroll_to.take() {
            // `r` is a side-by-side row; the unified list has one row per removed line, so find the same change
            let target = tab.sbs.rows.get(r).and_then(|row| row.right.as_ref().map(|c| c.no).or(row.left.as_ref().map(|c| c.no)));
            let first = tab.lines.iter().position(|l| l.kind != LineKind::Same && (l.new_no.or(l.old_no) == target || l.old_no == target)).unwrap_or(0);
            area = area.vertical_scroll_offset(first.saturating_sub(3) as f32 * ROW_H);
        }
        let longest = tab.lines.iter().map(|l| ui.painter().layout_no_wrap(l.text.replace('\t', "    "), mono.clone(), pal.plain).size().x).fold(0.0, f32::max);
        let total_w = ui.available_width().max(150.0 + longest);
        area.show_rows_fading(ui, ROW_H, tab.lines.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for l in &tab.lines[range] {
                let (r, _) = ui.allocate_exact_size(vec2(total_w, ROW_H), Sense::hover());
                let (bg, mark, fg) = match l.kind {
                    LineKind::Added => (pal.git_added.gamma_multiply(0.16), "+", pal.git_added),
                    LineKind::Removed => (pal.error.gamma_multiply(0.16), "-", pal.error),
                    LineKind::Same => (Color32::TRANSPARENT, " ", pal.plain),
                };
                if l.kind != LineKind::Same {
                    ui.painter().rect_filled(r, CornerRadius::ZERO, bg);
                }
                let num = |n: Option<usize>| n.map_or("    ".to_string(), |n| format!("{n:>4}"));
                let text = l.text.replace('\t', "    ");
                ui.painter().text(pos2(r.min.x + 12.0, r.center().y), Align2::LEFT_CENTER, num(l.old_no), mono.clone(), pal.line_number);
                ui.painter().text(pos2(r.min.x + 62.0, r.center().y), Align2::LEFT_CENTER, num(l.new_no), mono.clone(), pal.line_number);
                ui.painter().text(pos2(r.min.x + 112.0, r.center().y), Align2::LEFT_CENTER, mark, mono.clone(), fg);
                ui.painter().text(pos2(r.min.x + 132.0, r.center().y), Align2::LEFT_CENTER, text, mono.clone(), if l.kind == LineKind::Same { pal.plain } else { fg });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_scroll_reaches_the_end_of_tabbed_unicode_lines() {
        for unified in [true, false] {
            let ctx = egui::Context::default();
            fonts::install(&ctx);
            let mut app = App::new(ide_core::settings::Settings::default());
            let text = format!("{} КОНЕЦ", "\tlong code != >> ".repeat(40));
            let mut doc = crate::doc::Doc::diff_tab();
            let tab = doc.diff.as_mut().unwrap();
            tab.lines = ide_core::diff::diff_lines("old", &text);
            tab.sbs = ide_core::diff::side_by_side("old", &text);
            app.docs.push(doc);
            let mono = FontId::monospace(22.0);
            let mut last_right = 0.0;
            let mut clip_right = 0.0;
            for frame in 0..30 {
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(700.0, 250.0))),
                    time: Some(frame as f64 / 30.0),
                    events: if frame == 2 { vec![egui::Event::PointerMoved(pos2(350.0, 80.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(-100_000.0, 0.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: egui::Modifiers::NONE }] } else { vec![] },
                    ..Default::default()
                }, |ui| {
                    if unified { app.draw_unified_rows(ui, 0, &mono); }
                    else { app.draw_side_rows(ui, 0, &mono, 13.2); }
                });
                for shape in &output.shapes {
                    if let egui::Shape::Text(t) = &shape.shape
                        && t.galley.job.text.ends_with("КОНЕЦ") {
                        last_right = t.pos.x + t.galley.size().x;
                        clip_right = shape.clip_rect.right();
                    }
                }
                output.textures_delta.clear();
            }
            assert!(last_right <= clip_right, "end is clipped: {last_right} > {clip_right}, unified={unified}");
            assert!(last_right > clip_right - 40.0, "scroll never reached the end: {last_right}, unified={unified}");
        }
    }
}
