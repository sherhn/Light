//! The code editor widget: painting (highlighting, selection, caret, gutter, folds, change markers)
//! and mouse handling. Keyboard input is dispatched centrally by the app through the command registry.

use std::collections::HashSet;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2, text::LayoutJob};
use ide_core::diff::{Mark, MarkKind};
use ide_core::editor::Editor;
use lisp_syntax::highlight::{Span, highlight};

use crate::folding::{self, FoldKey, Region};
use crate::icons::{self, Icon};
use crate::theme::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sev {
    Error,
    Warning,
    Weak,
}

#[derive(Clone, Debug)]
pub struct Marker {
    pub start: usize,
    pub end: usize,
    pub sev: Sev,
    pub msg: String,
}

#[derive(Default)]
pub struct ViewState {
    pub scroll: Vec2,
    pub scroll_to_caret: bool,
    pub last_input_time: f64,
    spans: Option<(u64, Vec<Span>)>,
    regions: Option<(u64, Vec<Region>)>,
    max_cols: Option<(u64, usize)>,
    pub last_viewport_lines: usize,
    pub last_text_rect: Option<Rect>,
    /// The (fractional) source line at the top of the view, as of the last frame.
    pub top_line_f: f32,
    /// Scroll so that this (fractional) source line is at the top (used to keep a preview beside the code in step).
    pub scroll_to_line_f: Option<f32>,
}


impl ViewState {
    /// How the character at `offset` was coloured last (comment, string...), if it was drawn.
    pub fn class_at(&self, offset: usize) -> Option<lisp_syntax::highlight::Class> {
        let (_, spans) = self.spans.as_ref()?;
        let i = spans.partition_point(|s| s.end <= offset);
        spans.get(i).filter(|s| s.start <= offset).map(|s| s.class)
    }
}

pub struct ViewCfg<'a> {
    pub pal: &'a Palette,
    pub font_size: f32,
    pub line_numbers: bool,
    pub whitespace: bool,
    pub rainbow: bool,
    pub focused: bool,
    pub markers: &'a [Marker],
    pub find: &'a [(usize, usize)],
    pub find_current: Option<usize>,
    pub git_marks: &'a [Mark],
    /// Code overview (minimap) next to the scrollbar.
    pub minimap: bool,
}

#[derive(Default)]
pub struct ViewOut {
    pub clicked: bool,
    /// Ctrl/Cmd+click target offset (go to declaration).
    pub ctrl_click: Option<usize>,
    /// Screen position just below the caret, for popups.
    pub caret_screen: Option<Pos2>,
    pub hover_offset: Option<usize>,
    /// The character the pointer is exactly over (`None` past the end of a line).
    pub hover_char: Option<usize>,
    /// Where the pointer is.
    pub hover_pos: Option<Pos2>,
    pub text_rect: Option<Rect>,
    /// A fold chevron was clicked.
    pub fold_toggle: Option<FoldKey>,
    /// A change marker in the gutter was clicked: (0-based line, screen position just below that line).
    pub gutter_click: Option<(usize, Pos2)>,
    pub gutter_rect: Option<Rect>,
}

// Gutter, left to right: padding, line numbers, gap, change marker, fold chevron; then the code.
const GUTTER_PAD: f32 = 14.0;
const NUM_GAP: f32 = 10.0;
const MARK_W: f32 = 4.0;
const FOLD_W: f32 = 18.0;
const TEXT_PAD: f32 = 6.0;
/// Code overview (minimap): pixels per row and per character.
const MAP_W: f32 = 128.0;
const MAP_ROW: f32 = 5.0;
const MAP_COL: f32 = 2.0;
/// The scrollbar lane at the far right; it also shows where the changes and problems are in the whole file.
const BAR_W: f32 = 14.0;

/// Screen position of the change marker of display row `row` in a gutter (used by the UI tests to click it).
pub fn gutter_mark_pos(gutter: Rect, scroll_y: f32, font_size: f32, row: usize) -> Pos2 {
    let lh = line_height_for(font_size);
    Pos2::new(gutter.max.x - FOLD_W - MARK_W * 0.5, gutter.min.y + row as f32 * lh - scroll_y + lh * 0.5)
}

pub fn line_height_for(font_size: f32) -> f32 {
    (font_size * 1.62).round()
}

/// Display column of char column `col` in `line` (tabs expand to 4 cells).
pub fn display_col(line: &str, col: usize) -> usize {
    line.chars().take(col).map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

fn char_col_from_display(line: &str, disp: usize) -> usize {
    let mut d = 0;
    for (i, c) in line.chars().enumerate() {
        let w = if c == '\t' { 4 } else { 1 };
        if d + w > disp {
            return i;
        }
        d += w;
    }
    line.chars().count()
}

fn wavy(painter: &egui::Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    let mut pts = Vec::new();
    let mut x = x0;
    let mut up = false;
    while x < x1 {
        pts.push(Pos2::new(x, if up { y - 2.0 } else { y }));
        x += 2.0;
        up = !up;
    }
    pts.push(Pos2::new(x1, y));
    if pts.len() >= 2 {
        painter.add(Shape::line(pts, Stroke::new(1.0, color)));
    }
}

pub fn show(ui: &mut egui::Ui, ed: &mut Editor, state: &mut ViewState, folds: &mut HashSet<FoldKey>, cfg: &ViewCfg) -> ViewOut {
    let pal = cfg.pal;
    let font = FontId::monospace(cfg.font_size);
    let lh = line_height_for(cfg.font_size);
    let cw = ui.fonts_mut(|f| f.glyph_width(&font, 'M')).max(1.0);
    let total_lines = ed.line_count();
    let digits = total_lines.to_string().len().max(3);
    let num_font = FontId::monospace((cfg.font_size * 0.9).round());
    let num_cw = ui.fonts_mut(|f| f.glyph_width(&num_font, '0')).max(1.0);
    let gutter_w = if cfg.line_numbers { GUTTER_PAD + num_cw * digits as f32 + NUM_GAP + MARK_W + FOLD_W } else { 8.0 + MARK_W + FOLD_W };

    // ---- caches
    let rev = ed.revision();
    if state.spans.as_ref().is_none_or(|(r, _)| *r != rev) {
        let spans = match ed.language_spans() {
            Some(s) => s.as_ref().clone(),
            None => highlight(ed.text(), &ed.parsed()),
        };
        state.spans = Some((rev, spans));
    }
    if state.regions.as_ref().is_none_or(|(r, _)| *r != rev) {
        state.regions = Some((rev, folding::regions(ed)));
    }
    let (spans_rev, spans) = state.spans.take().unwrap_or((0, Vec::new()));
    let (regions_rev, regions) = state.regions.take().unwrap_or((0, Vec::new()));

    // a caret inside a folded form unfolds it
    let (caret_line, caret_col) = ed.line_col(ed.caret());
    for line in [caret_line, ed.line_of(ed.anchor())] {
        if let Some(r) = folding::hidden_by(&regions, folds, line) {
            let key = r.key.clone();
            folds.remove(&key);
        }
    }
    let rows = folding::rows(total_lines, &regions, folds);
    let row_of_line = |line: usize| rows.row_of_line(line);
    if let Some(lf) = state.scroll_to_line_f.take() {
        let line = (lf.max(0.0).floor() as usize).min(total_lines.saturating_sub(1));
        state.scroll.y = ((row_of_line(line) as f32 + lf.max(0.0).fract()) * lh).max(0.0);
    }
    let region_start: std::collections::HashMap<usize, &Region> = regions.iter().map(|r| (r.start_line, r)).collect();

    let full = ui.available_rect_before_wrap();
    let gutter_rect = Rect::from_min_size(full.min, Vec2::new(gutter_w, full.height()));
    let map_w = if cfg.minimap && full.width() > 520.0 { MAP_W } else { 0.0 };
    // left to right: gutter, code, code overview, scrollbar
    let text_rect = Rect::from_min_max(Pos2::new(full.min.x + gutter_w, full.min.y), Pos2::new(full.max.x - BAR_W - map_w, (full.max.y - BAR_W).max(full.min.y)));
    let map_rect = Rect::from_min_max(Pos2::new(text_rect.max.x, full.min.y), Pos2::new(text_rect.max.x + map_w, full.max.y));
    let vbar = Rect::from_min_max(Pos2::new(full.max.x - BAR_W, full.min.y), full.max);
    let hbar = Rect::from_min_max(Pos2::new(text_rect.min.x, text_rect.max.y), Pos2::new(text_rect.max.x, full.max.y));
    state.last_text_rect = Some(text_rect);
    let mut out = ViewOut { text_rect: Some(text_rect), gutter_rect: Some(gutter_rect), ..Default::default() };

    let text = ed.text();
    let max_cols = match state.max_cols {
        Some((r, c)) if r == rev => c,
        _ => {
            let c = text.lines().map(|l| display_col(l, l.chars().count())).max().unwrap_or(0);
            state.max_cols = Some((rev, c));
            c
        }
    };
    let content_w = (max_cols as f32 + 10.0) * cw + TEXT_PAD;
    let content_h = rows.len() as f32 * lh + text_rect.height() * 0.45;

    let (sel_s, sel_e) = ed.selection();
    let caret = ed.caret();
    let bracket = ed.matching_bracket(caret);
    let now = ui.input(|i| i.time);
    let blink_on = ((now - state.last_input_time).max(0.0) * 1.7) as i64 % 2 == 0;

    #[derive(Clone, Copy)]
    enum Act {
        Caret { off: usize, extend: bool },
        Word(usize),
        Line(usize),
    }
    let mut acts: Vec<Act> = Vec::new();

    // code overview (VS Code style): 2 px per row; a file taller than the overview scrolls in it proportionally
    let max_scroll_y = (content_h - text_rect.height()).max(0.0);
    let map_h = rows.len() as f32 * MAP_ROW;
    let map_offset = |scroll_y: f32| if map_h > map_rect.height() && max_scroll_y > 0.0 { (scroll_y / max_scroll_y).clamp(0.0, 1.0) * (map_h - map_rect.height()) } else { 0.0 };
    let slider_h = (text_rect.height() / lh * MAP_ROW).max(12.0);
    let slider_top = |scroll_y: f32| map_rect.min.y + scroll_y / lh * MAP_ROW - map_offset(scroll_y);
    let mut map_resp = None;
    if map_w > 0.0 {
        let resp = ui.interact(map_rect, ui.scope_id().with("minimap"), Sense::click_and_drag());
        let on_slider = |p: Pos2| p.y >= slider_top(state.scroll.y) && p.y <= slider_top(state.scroll.y) + slider_h;
        if let Some(p) = resp.interact_pointer_pos()
            && (resp.clicked() || resp.drag_started())
            && !on_slider(p)
        {
            // jump: the clicked row comes to the middle of the editor
            let row = (p.y - map_rect.min.y + map_offset(state.scroll.y)) / MAP_ROW;
            state.scroll.y = (row * lh - text_rect.height() / 2.0).clamp(0.0, max_scroll_y);
        } else if resp.dragged() {
            // drag the slider: it covers the whole overview height when the file is taller than it
            let travel = if map_h > map_rect.height() { (map_rect.height() - slider_h).max(1.0) } else { MAP_ROW / lh * max_scroll_y.max(1.0) };
            state.scroll.y = (state.scroll.y + resp.drag_delta().y * max_scroll_y / travel.max(1.0)).clamp(0.0, max_scroll_y);
        }
        if resp.is_pointer_button_down_on() || resp.dragged() {
            ui.ctx().request_repaint();
        }
        map_resp = Some(resp);
    }

    // own scrollbars (the built-in one would sit between the code and the overview): dragging or clicking a track
    let max_scroll_x = (content_w - text_rect.width()).max(0.0);
    let vthumb_h = (text_rect.height() / content_h.max(1.0) * vbar.height()).clamp(28.0, vbar.height().max(28.0));
    let hthumb_w = (text_rect.width() / content_w.max(1.0) * hbar.width()).clamp(28.0, hbar.width().max(28.0));
    let vresp = ui.interact(vbar, ui.scope_id().with("vbar"), Sense::click_and_drag());
    if max_scroll_y > 0.0
        && let (true, Some(p)) = (vresp.is_pointer_button_down_on() || vresp.dragged(), vresp.interact_pointer_pos())
    {
        let f = ((p.y - vbar.min.y - vthumb_h / 2.0) / (vbar.height() - vthumb_h).max(1.0)).clamp(0.0, 1.0);
        state.scroll.y = f * max_scroll_y;
        ui.ctx().request_repaint();
    }
    let hresp = ui.interact(hbar, ui.scope_id().with("hbar"), Sense::click_and_drag());
    if max_scroll_x > 0.0
        && let (true, Some(p)) = (hresp.is_pointer_button_down_on() || hresp.dragged(), hresp.interact_pointer_pos())
    {
        let f = ((p.x - hbar.min.x - hthumb_w / 2.0) / (hbar.width() - hthumb_w).max(1.0)).clamp(0.0, 1.0);
        state.scroll.x = f * max_scroll_x;
        ui.ctx().request_repaint();
    }

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(text_rect.intersect(ui.clip_rect()));
    let scroll = egui::ScrollArea::both().min_scrolled_height(0.0).min_scrolled_width(0.0).max_height(text_rect.height()).max_width(text_rect.width()).auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).scroll_offset(state.scroll).id_salt("editor_scroll");
    let mut hover_marker: Option<String> = None;
    let area = scroll.show_viewport(&mut child, |ui, viewport| {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(content_w.max(viewport.width()), content_h.max(viewport.height())), Sense::click_and_drag());
        let painter = ui.painter_at(text_rect);
        let origin = rect.min + Vec2::new(TEXT_PAD, 0.0);
        let first = ((viewport.min.y / lh).floor().max(0.0)) as usize;
        let last = (((viewport.max.y / lh).ceil()) as usize + 1).min(rows.len());
        state.last_viewport_lines = ((text_rect.height() / lh) as usize).max(1);

        let mut caret_rect = None;
        for row in first..last {
            let line_no = rows.at(row);
            let ls = ed.line_start(line_no);
            let line = ed.line_text(line_no);
            let le = ls + line.len();
            let y = rect.min.y + row as f32 * lh;
            let x0 = origin.x;
            let ncols = line.chars().count();

            // current line highlight
            if line_no == caret_line && sel_s == sel_e {
                painter.rect_filled(Rect::from_min_size(Pos2::new(text_rect.min.x, y), Vec2::new(rect.width().max(text_rect.width()), lh)), CornerRadius::ZERO, pal.current_line);
            }
            // selection background
            if sel_s != sel_e && sel_s <= le && sel_e >= ls {
                let a = sel_s.max(ls) - ls;
                let b = sel_e.min(le) - ls;
                let xa = x0 + display_col(line, line[..a].chars().count()) as f32 * cw;
                let mut xb = x0 + display_col(line, line[..b].chars().count()) as f32 * cw;
                if sel_e > le {
                    xb += cw * 0.6;
                }
                if xb <= xa {
                    xb = xa + 2.0;
                }
                painter.rect_filled(Rect::from_min_max(Pos2::new(xa, y), Pos2::new(xb, y + lh)), CornerRadius::ZERO, pal.selection);
            }
            // find highlights
            for (i, &(fs, fe)) in cfg.find.iter().enumerate() {
                if fe < ls || fs > le {
                    continue;
                }
                let a = fs.max(ls) - ls;
                let b = fe.min(le) - ls;
                let xa = x0 + display_col(line, line[..a].chars().count()) as f32 * cw;
                let xb = x0 + display_col(line, line[..b].chars().count()) as f32 * cw;
                let col = if cfg.find_current == Some(i) { pal.find_current } else { pal.find_match };
                painter.rect_filled(Rect::from_min_max(Pos2::new(xa, y + 2.0), Pos2::new(xb.max(xa + 2.0), y + lh - 2.0)), CornerRadius::same(3), col);
            }
            // matching bracket
            if let Some((a, b)) = bracket {
                for off in [a, b] {
                    if off >= ls && off < le {
                        let xa = x0 + display_col(line, line[..off - ls].chars().count()) as f32 * cw;
                        painter.rect_filled(Rect::from_min_size(Pos2::new(xa - 1.0, y + 2.0), Vec2::new(cw + 2.0, lh - 4.0)), CornerRadius::same(3), pal.match_bracket);
                    }
                }
            }

            // text
            if !line.is_empty() {
                let mut job = LayoutJob::default();
                job.wrap.max_width = f32::INFINITY;
                let mut pos = ls;
                let mut si = spans.partition_point(|s| s.end <= ls);
                let push = |job: &mut LayoutJob, from: usize, to: usize, color: Color32| {
                    if to > from {
                        let t = &text[from..to];
                        let t = if t.contains('\t') { t.replace('\t', "    ") } else { t.to_string() };
                        job.append(&t, 0.0, egui::TextFormat::simple(font.clone(), color));
                    }
                };
                // a comment with a marker in it (TODO, FIXME, INFO...): from the marker on it is coloured and slanted
                let push_comment = |job: &mut LayoutJob, from: usize, to: usize| match ide_core::todo::marker_in(&text[from..to]) {
                    Some((at, kind)) => {
                        push(job, from, from + at, pal.comment);
                        let t = text[from + at..to].replace('\t', "    ");
                        let color = if kind == ide_core::todo::TodoKind::Alert { pal.alert } else { pal.todo };
                        job.append(&t, 0.0, egui::TextFormat { italics: true, ..egui::TextFormat::simple(font.clone(), color) });
                    }
                    None => push(job, from, to, pal.comment),
                };
                while pos < le {
                    if si < spans.len() && spans[si].start < le {
                        let s = &spans[si];
                        let a = s.start.max(pos);
                        if a > pos {
                            push(&mut job, pos, a, pal.plain);
                        }
                        let b = s.end.min(le);
                        if s.class == lisp_syntax::highlight::Class::Comment {
                            push_comment(&mut job, a, b);
                        } else {
                            push(&mut job, a, b, pal.syntax(s.class, s.depth, cfg.rainbow));
                        }
                        pos = b;
                        si += 1;
                    } else {
                        push(&mut job, pos, le, pal.plain);
                        pos = le;
                    }
                }
                let galley = painter.layout_job(job);
                let gy = y + (lh - galley.size().y) / 2.0;
                painter.galley(Pos2::new(x0, gy), galley, pal.plain);
            }

            // folded placeholder
            if let Some(r) = region_start.get(&line_no)
                && folds.contains(&r.key)
            {
                let xe = x0 + display_col(line, ncols) as f32 * cw + 8.0;
                let pill = Rect::from_min_size(Pos2::new(xe, y + 4.0), Vec2::new(32.0, lh - 8.0));
                painter.rect_filled(pill, CornerRadius::same(5), pal.hover);
                for k in 0..3 {
                    painter.circle_filled(Pos2::new(pill.min.x + 9.0 + k as f32 * 7.0, pill.center().y + 2.0), 1.4, pal.text_dim);
                }
            }

            if cfg.whitespace {
                for (i, ch) in line.chars().enumerate() {
                    if ch == ' ' {
                        let x = x0 + display_col(line, i) as f32 * cw + cw * 0.5;
                        painter.circle_filled(Pos2::new(x, y + lh * 0.5), 1.0, pal.whitespace);
                    }
                }
            }

            // problems
            for m in cfg.markers {
                if m.end < ls || m.start > le {
                    continue;
                }
                let a = m.start.max(ls).min(le) - ls;
                let b = m.end.min(le).max(m.start.max(ls).min(le)) - ls;
                let ca = line[..a].chars().count();
                let cb = line[..b].chars().count().max(ca + 1);
                let xa = x0 + display_col(line, ca) as f32 * cw;
                let xb = x0 + display_col(line, cb) as f32 * cw;
                let color = match m.sev {
                    Sev::Error => pal.error,
                    Sev::Warning => pal.warning,
                    Sev::Weak => pal.weak_warning,
                };
                wavy(&painter, xa, xb.max(xa + cw * 0.6), y + lh - 4.0, color);
            }

            if line_no == caret_line {
                let x = x0 + display_col(line, caret_col) as f32 * cw;
                caret_rect = Some(Rect::from_min_size(Pos2::new(x, y + 2.0), Vec2::new(2.0, lh - 4.0)));
            }
        }
        let caret_full = caret_rect.unwrap_or_else(|| {
            let line = ed.line_text(caret_line);
            Rect::from_min_size(Pos2::new(origin.x + display_col(line, caret_col) as f32 * cw, rect.min.y + row_of_line(caret_line) as f32 * lh + 2.0), Vec2::new(2.0, lh - 4.0))
        });
        if cfg.focused && blink_on && caret_rect.is_some() {
            painter.rect_filled(caret_full, CornerRadius::same(1), pal.caret);
        }
        if state.scroll_to_caret {
            ui.scroll_to_rect(caret_full.expand2(Vec2::new(cw * 6.0, lh * 2.0)), None);
            state.scroll_to_caret = false;
        }
        out.caret_screen = Some(Pos2::new(caret_full.min.x, caret_full.max.y + 2.0));

        // ---------------------------------------------------------------- mouse
        let pos_to_offset = |p: Pos2, ed: &Editor| -> usize {
            let rel = p - origin;
            let row = ((p.y - rect.min.y) / lh).floor().clamp(0.0, (rows.len().max(1) - 1) as f32) as usize;
            let line_no = rows.get(row).unwrap_or(0);
            let line = ed.line_text(line_no);
            let disp = (rel.x / cw + 0.5).floor().max(0.0) as usize;
            ed.offset_of(line_no, char_col_from_display(line, disp))
        };
        let char_at = |p: Pos2, ed: &Editor| -> Option<usize> {
            let row = ((p.y - rect.min.y) / lh).floor();
            if row < 0.0 || row as usize >= rows.len() {
                return None;
            }
            let line_no = rows.at(row as usize);
            let line = ed.line_text(line_no);
            let disp = ((p.x - origin.x) / cw).floor();
            if disp < 0.0 {
                return None;
            }
            let col = char_col_from_display(line, disp as usize);
            (col < line.chars().count()).then(|| ed.offset_of(line_no, col))
        };
        if resp.hovered() {
            let ctrl = ui.input(|i| i.modifiers.command);
            if let Some(hp) = ui.input(|i| i.pointer.hover_pos())
                && text_rect.contains(hp)
            {
                let off = pos_to_offset(hp, ed);
                out.hover_offset = Some(off);
                out.hover_char = char_at(hp, ed);
                out.hover_pos = Some(hp);
                ui.set_cursor_icon(if ctrl { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Text });
                if let Some(m) = cfg.markers.iter().find(|m| m.start <= off && off <= m.end.max(m.start + 1) && !m.msg.is_empty()) {
                    hover_marker = Some(m.msg.clone());
                }
            }
        }
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        if let Some(p) = ui.input(|i| i.pointer.interact_pos())
            && pressed
            && resp.hovered()
            && text_rect.contains(p)
        {
            let off = pos_to_offset(p, ed);
            let (shift, cmd) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
            out.clicked = true;
            state.last_input_time = now;
            if cmd && !shift {
                out.ctrl_click = Some(off);
            } else {
                acts.push(Act::Caret { off, extend: shift });
            }
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            acts.push(Act::Word(pos_to_offset(p, ed)));
        }
        if resp.triple_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            acts.push(Act::Line(pos_to_offset(p, ed)));
        }
        if resp.dragged()
            && !resp.double_clicked()
            && !resp.triple_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            acts.push(Act::Caret { off: pos_to_offset(p, ed), extend: true });
            ui.scroll_to_rect(Rect::from_center_size(p, Vec2::splat(4.0)), None);
        }
        if let Some(msg) = hover_marker.take() {
            resp.on_hover_ui_at_pointer(|ui| {
                ui.set_max_width(460.0);
                ui.label(egui::RichText::new(msg).size(12.5));
            });
        }
    });
    state.scroll = area.state.clamped_offset();
    {
        let row_f = state.scroll.y.max(0.0) / lh;
        let r = (row_f.floor() as usize).min(rows.len().saturating_sub(1));
        state.top_line_f = rows.at(r) as f32 + row_f.fract();
    }
    for a in acts {
        match a {
            Act::Caret { off, extend } => ed.set_caret(off, extend),
            Act::Word(o) => ed.select_word_at(o),
            Act::Line(o) => ed.select_line_at(o),
        }
    }

    // ---------------------------------------------------------------- gutter
    // same background as the code (JetBrains style); the caret row is highlighted across numbers and code
    let gp = ui.painter_at(gutter_rect);
    let gresp = ui.interact(gutter_rect, ui.scope_id().with("gutter"), Sense::click());
    let hover_gutter = gresp.hovered();
    let first = ((state.scroll.y / lh).floor().max(0.0)) as usize;
    let last = ((((state.scroll.y + gutter_rect.height()) / lh).ceil()) as usize + 1).min(rows.len());
    let mark_x = gutter_rect.max.x - FOLD_W - MARK_W;
    let num_right = mark_x - NUM_GAP;
    let fold_x = gutter_rect.max.x - FOLD_W * 0.5;
    for row in first..last {
        let line_no = rows.at(row);
        let y = gutter_rect.min.y + row as f32 * lh - state.scroll.y;
        if line_no == caret_line && sel_s == sel_e {
            gp.rect_filled(Rect::from_min_size(Pos2::new(gutter_rect.min.x, y), Vec2::new(gutter_rect.width(), lh)), CornerRadius::ZERO, pal.current_line);
        }
        if cfg.line_numbers {
            let color = if line_no == caret_line { pal.line_number_active } else { pal.line_number };
            gp.text(Pos2::new(num_right, y + lh * 0.5), Align2::RIGHT_CENTER, line_no + 1, num_font.clone(), color);
        }
        if let Some(r) = region_start.get(&line_no) {
            let folded = folds.contains(&r.key);
            if folded || hover_gutter {
                let cr = Rect::from_center_size(Pos2::new(fold_x, y + lh * 0.5), Vec2::splat(12.0));
                icons::paint(&gp, cr, if folded { Icon::ChevronRight } else { Icon::ChevronDown }, pal.line_number);
            }
        }
    }
    let mark_color = |k: MarkKind| match k {
        MarkKind::Added => pal.git_added,
        MarkKind::Modified => pal.git_modified,
        MarkKind::Deleted => pal.error,
    };
    for m in cfg.git_marks {
        let row = row_of_line(m.line);
        if rows.get(row) != Some(m.line) {
            continue;
        }
        let y = gutter_rect.min.y + row as f32 * lh - state.scroll.y;
        if y + lh < gutter_rect.min.y || y > gutter_rect.max.y {
            continue;
        }
        match m.kind {
            MarkKind::Deleted => {
                gp.add(Shape::convex_polygon(vec![Pos2::new(mark_x - 1.0, y - 3.5), Pos2::new(mark_x + 6.0, y), Pos2::new(mark_x - 1.0, y + 3.5)], mark_color(m.kind), Stroke::NONE));
            }
            k => {
                gp.rect_filled(Rect::from_min_size(Pos2::new(mark_x, y), Vec2::new(3.5, lh)), CornerRadius::ZERO, mark_color(k));
            }
        }
    }
    // hovering a change marker shows a pointer, clicking it opens the old code; the chevrons fold
    let mark_zone = |p: Pos2| p.x >= mark_x - 4.0 && p.x <= mark_x + MARK_W + 4.0;
    let row_at = |p: Pos2| (((p.y - gutter_rect.min.y + state.scroll.y) / lh).floor().max(0.0)) as usize;
    let marked_line = |p: Pos2| rows.get(row_at(p)).filter(|line| cfg.git_marks.iter().any(|m| m.line == *line));
    if let Some(p) = ui.input(|i| i.pointer.hover_pos())
        && gutter_rect.contains(p)
        && mark_zone(p)
        && marked_line(p).is_some()
    {
        ui.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if gresp.clicked()
        && let Some(p) = gresp.interact_pointer_pos()
    {
        if p.x > gutter_rect.max.x - FOLD_W {
            if let Some(line) = rows.get(row_at(p))
                && let Some(r) = region_start.get(&line)
            {
                out.fold_toggle = Some(r.key.clone());
            }
        } else if mark_zone(p)
            && let Some(line) = marked_line(p)
        {
            let y = gutter_rect.min.y + row_of_line(line) as f32 * lh - state.scroll.y;
            out.gutter_click = Some((line, Pos2::new(gutter_rect.max.x + 6.0, y + lh + 2.0)));
        }
    }

    // ---------------------------------------------------------------- code overview
    let text_len = ed.text().len();
    if let Some(resp) = &map_resp {
        let mp = ui.painter_at(map_rect);
        let offset = map_offset(state.scroll.y);
        let first = (offset / MAP_ROW).floor().max(0.0) as usize;
        let last = (((offset + map_rect.height()) / MAP_ROW).ceil() as usize + 1).min(rows.len());
        let x0 = map_rect.min.x + 8.0;
        let max_cols = ((map_rect.max.x - 6.0 - x0) / MAP_COL) as usize;
        let mini_font = FontId::monospace(MAP_COL / 0.6);
        for row in first..last {
            let Some(line_no) = rows.get(row) else { continue };
            let ls = ed.line_start(line_no);
            let line = ed.line_text(line_no);
            if line.trim().is_empty() {
                continue;
            }
            let le = ls + line.len();
            let y = map_rect.min.y + row as f32 * MAP_ROW - offset;
            let mut si = spans.partition_point(|s| s.end <= ls);
            while si < spans.len() && spans[si].start < le {
                let sp = &spans[si];
                si += 1;
                let a = sp.start.max(ls) - ls;
                let b = sp.end.min(le) - ls;
                if a > 600 || !line.is_char_boundary(a) || !line.is_char_boundary(b) {
                    break;
                }
                let c0 = display_col(line, line[..a].chars().count());
                if c0 >= max_cols {
                    break;
                }
                if line[a..b].trim().is_empty() {
                    continue;
                }
                let text: String = line[a..b].replace('\t', "    ").chars().take(max_cols - c0).collect();
                mp.text(Pos2::new(x0 + c0 as f32 * MAP_COL, y), Align2::LEFT_TOP, text, mini_font.clone(), pal.syntax(sp.class, sp.depth, cfg.rainbow));
            }
        }
        // changed lines: a thin bar along the left edge of the overview
        for m in cfg.git_marks {
            let row = row_of_line(m.line);
            if rows.get(row) != Some(m.line) {
                continue;
            }
            let y = map_rect.min.y + row as f32 * MAP_ROW - offset;
            if y >= map_rect.min.y - MAP_ROW && y <= map_rect.max.y {
                mp.rect_filled(Rect::from_min_size(Pos2::new(map_rect.min.x + 2.0, y), Vec2::new(2.0, MAP_ROW.max(if m.kind == MarkKind::Deleted { 2.0 } else { MAP_ROW }))), CornerRadius::ZERO, mark_color(m.kind));
            }
        }
        // the visible part of the file
        let hot = resp.hovered() || resp.dragged();
        let top = slider_top(state.scroll.y);
        let fill = if pal.dark { Color32::from_white_alpha(if hot { 22 } else { 11 }) } else { Color32::from_black_alpha(if hot { 22 } else { 11 }) };
        mp.rect_filled(Rect::from_min_size(Pos2::new(map_rect.min.x, top), Vec2::new(map_rect.width(), slider_h)), CornerRadius::ZERO, fill);
    }

    // ---------------------------------------------------------------- scrollbars
    // the vertical lane doubles as an overview ruler of the whole file: changes left, problems and matches right
    let vp = ui.painter_at(vbar);
    let n_rows = rows.len().max(1) as f32;
    let ruler_y = |line: usize| vbar.min.y + 4.0 + row_of_line(line) as f32 / n_rows * (vbar.height() - 8.0);
    for m in cfg.git_marks {
        vp.rect_filled(Rect::from_min_size(Pos2::new(vbar.min.x + 2.0, ruler_y(m.line)), Vec2::new(3.0, 3.0)), CornerRadius::ZERO, mark_color(m.kind));
    }
    for &(fs, _) in cfg.find {
        vp.rect_filled(Rect::from_min_size(Pos2::new(vbar.max.x - 6.0, ruler_y(ed.line_of(fs.min(text_len)))), Vec2::new(4.0, 2.0)), CornerRadius::ZERO, pal.find_current);
    }
    for m in cfg.markers {
        let color = match m.sev {
            Sev::Error => pal.error,
            Sev::Warning => pal.warning,
            Sev::Weak => pal.weak_warning,
        };
        vp.rect_filled(Rect::from_min_size(Pos2::new(vbar.max.x - 6.0, ruler_y(ed.line_of(m.start.min(text_len)))), Vec2::new(4.0, 3.0)), CornerRadius::ZERO, color);
    }
    let thumb = |resp: &egui::Response| {
        let a = if resp.dragged() { 70 } else if resp.hovered() { 50 } else { 30 };
        if pal.dark { Color32::from_white_alpha(a) } else { Color32::from_black_alpha(a) }
    };
    if max_scroll_y > 0.0 {
        let f = (state.scroll.y / max_scroll_y).clamp(0.0, 1.0);
        let t = Rect::from_min_size(Pos2::new(vbar.min.x + 4.0, vbar.min.y + 4.0 + f * (vbar.height() - 8.0 - vthumb_h).max(0.0)), Vec2::new(BAR_W - 8.0, (vthumb_h - 8.0).max(16.0)));
        vp.rect_filled(t, CornerRadius::same(3), thumb(&vresp));
    }
    if max_scroll_x > 0.0 {
        let f = (state.scroll.x / max_scroll_x).clamp(0.0, 1.0);
        let t = Rect::from_min_size(Pos2::new(hbar.min.x + 4.0 + f * (hbar.width() - 8.0 - hthumb_w).max(0.0), hbar.min.y + 4.0), Vec2::new((hthumb_w - 8.0).max(16.0), BAR_W - 8.0));
        ui.painter_at(hbar).rect_filled(t, CornerRadius::same(3), thumb(&hresp));
    }
    state.spans = Some((spans_rev, spans));
    state.regions = Some((regions_rev, regions));
    ui.advance_cursor_after_rect(full);
    out
}

#[cfg(test)]
mod scrollbar_tests {
    use super::*;

    #[test]
    fn horizontal_scrollbar_drags_without_covering_or_selecting_the_only_line() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let pal = Palette::for_settings(&ide_core::settings::Settings::default());
        let mut editor = Editor::new(&"x".repeat(500), lisp_syntax::Dialect::CommonLisp);
        let mut state = ViewState::default();
        let mut folds = HashSet::new();
        let cfg = ViewCfg { pal: &pal, font_size: 14.0, line_numbers: true, whitespace: false, rainbow: false, focused: true, markers: &[], find: &[], find_current: None, git_marks: &[], minimap: false };
        let mut track = Pos2::ZERO;
        for frame in 0..6 {
            let position = track + Vec2::new(if frame >= 3 { 80.0 } else { 0.0 }, 0.0);
            let events = match frame {
                2 => vec![egui::Event::PointerMoved(position), egui::Event::PointerButton { pos: position, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }],
                3 => vec![egui::Event::PointerMoved(position)],
                4 => vec![egui::Event::PointerButton { pos: position, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
                _ => vec![],
            };
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 64.0))), events, ..Default::default() }, |ui| {
                let full = ui.available_rect_before_wrap();
                let out = show(ui, &mut editor, &mut state, &mut folds, &cfg);
                let text = out.text_rect.unwrap();
                assert!(text.height() >= line_height_for(cfg.font_size));
                assert!(full.bottom() - text.bottom() >= BAR_W - 0.01);
                track = Pos2::new(text.left() + 20.0, text.bottom() + BAR_W / 2.0);
            });
            output.textures_delta.clear();
        }
        assert!(state.scroll.x > 0.0, "scrollbar did not scroll: {:?}", state.scroll);
        assert_eq!(editor.caret(), 0);
        assert!(!editor.has_selection());
    }
}

pub fn link_at(text: &str, offset: usize) -> Option<String> {
    if offset > text.len() || !text.is_char_boundary(offset) { return None; }
    let delimiter = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`');
    let start = text[..offset].rfind(delimiter).map_or(0, |i| i + text[i..].chars().next().unwrap().len_utf8());
    let end = text[offset..].find(delimiter).map_or(text.len(), |i| offset + i);
    let token = &text[start..end];
    let scheme = token.find("https://").or_else(|| token.find("http://"))?;
    let url = token[scheme..].trim_end_matches([')', ']', '}', ',', '.', ';']);
    (offset >= start + scheme && offset < start + scheme + url.len()).then(|| url.to_string())
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn links_in_code_and_markdown() {
        for text in ["; https://example.org/path?q=1#part", "[site](https://example.org/path?q=1#part).", "\"https://example.org/path?q=1#part\""] {
            let offset = text.find("example").unwrap();
            assert_eq!(link_at(text, offset).as_deref(), Some("https://example.org/path?q=1#part"));
        }
        assert_eq!(link_at("(print value)", 3), None);
        assert_eq!(link_at("λ https://example.org", 1), None);
    }

}
