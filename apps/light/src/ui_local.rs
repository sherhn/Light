//! Local commits in the UI: the list in the Git tool window, the commit browser dialog (files, diff / code,
//! switching between commits, restore) and the small "old code" popup of a clicked gutter marker.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Key, Margin, Pos2, Rect, RichText, Sense, Vec2, pos2, vec2};
use ide_core::diff::LineKind;
use ide_core::git::ChangeKind;
use ide_core::localvcs::{Age, Base};

use crate::app::App;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::state::*;
use crate::widgets::{self, ROW_H};

/// "5 min ago" in the current language.
pub fn age_text(time: u64) -> String {
    match ide_core::localvcs::age(time) {
        Age::JustNow => tr("just now").to_string(),
        Age::Minutes(n) => i18n::fmt("{} min ago", &[&n]),
        Age::Hours(n) => i18n::fmt("{} h ago", &[&n]),
        Age::Days(n) => i18n::fmt("{} d ago", &[&n]),
    }
}

fn kind_color(app: &App, k: ChangeKind) -> Color32 {
    app.git_color(k)
}

impl App {
    pub fn local_restore_confirm(&mut self, id: u64) {
        let message = i18n::fmt("Restore the working tree to local commit #{}? Files changed since then are overwritten, files created since then are deleted.", &[&id]);
        self.popup = Popup::Confirm { kind: ConfirmKind::RestoreLocal(id), message };
    }

    // ------------------------------------------------------------------ the Local tab of the Git tool window

    pub fn draw_local_commits(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let commits: Vec<ide_core::localvcs::LocalCommit> = self.git.local.as_ref().map(|l| l.commits().iter().rev().cloned().collect()).unwrap_or_default();
        let base = self.git.local.as_ref().map(|l| l.base().clone()).unwrap_or_default();
        if commits.is_empty() {
            self.empty_state(ui, Icon::History, pal.text_dim, tr("No local commits yet"));
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(tr("A local commit is a snapshot kept outside of git (in .light/).\nAfter making one, diffs and change markers start from it - handy when you\ncannot commit upstream, e.g. working on someone else's repository.")).color(pal.text_dim).size(12.0));
            });
            return;
        }
        let mut open: Option<u64> = None;
        let mut use_base: Option<Base> = None;
        let mut restore: Option<u64> = None;
        let mut delete: Option<u64> = None;
        let mut show_changes = false;
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().scroll.floating = false;
        egui::ScrollArea::vertical().id_salt("local_commits").auto_shrink([false, false]).show_rows(ui, ROW_H + 16.0, commits.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let c = &commits[i];
                let is_base = base == Base::Local(c.id);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H + 16.0), Sense::click());
                widgets::row_background(ui, &pal, r, is_base, resp.hovered());
                let cy = r.center().y;
                let br = Rect::from_min_size(Pos2::new(r.min.x + 14.0, cy - 11.0), Vec2::new(40.0, 22.0));
                ui.painter().rect_filled(br, CornerRadius::same(6), pal.accent.gamma_multiply(0.22));
                ui.painter().text(br.center(), Align2::CENTER_CENTER, format!("#{}", c.id), fonts::semibold(12.0), pal.accent);
                let g = ui.painter().layout_no_wrap(c.message.clone(), fonts::semibold(13.5), pal.text);
                let text_rect = Rect::from_min_max(pos2(r.min.x + 66.0, r.min.y), pos2((r.max.x - if is_base { 100.0 } else { 50.0 }).max(r.min.x + 66.0), r.max.y));
                let text_painter = ui.painter().with_clip_rect(text_rect.intersect(ui.clip_rect()));
                text_painter.galley(Pos2::new(text_rect.min.x, r.min.y + 5.0), g, pal.text);
                let meta = i18n::fmt("{0}   -   {1} file(s)", &[&age_text(c.time), &c.files.len()]);
                text_painter.text(Pos2::new(text_rect.min.x, r.max.y - 10.0), Align2::LEFT_CENTER, meta, fonts::ui(11.5), pal.text_dim);
                if is_base {
                    ui.painter().text(Pos2::new(r.max.x - 16.0, cy), Align2::RIGHT_CENTER, tr("BASE"), fonts::semibold(11.0), pal.accent);
                }
                // one-click rollback, visible while the row is hovered
                let rb = Rect::from_center_size(Pos2::new(r.max.x - if is_base { 76.0 } else { 28.0 }, cy), Vec2::splat(28.0));
                if resp.hovered() || ui.rect_contains_pointer(rb) {
                    let b = ui.interact(rb, resp.id.with("restore"), Sense::click());
                    if b.hovered() {
                        ui.painter().rect_filled(rb, widgets::radius(), pal.hover);
                    }
                    icons::paint(ui.painter(), rb.shrink(6.0), Icon::Undo, if b.hovered() { pal.text } else { pal.text_dim });
                    if b.on_hover_text(tr("Restore working tree to this commit...")).clicked() {
                        restore = Some(c.id);
                    } else if resp.clicked() && !ui.rect_contains_pointer(rb) {
                        open = Some(c.id);
                    }
                } else if resp.clicked() {
                    open = Some(c.id);
                }
                resp.context_menu(|ui| {
                    if ui.button(tr("View files and code...")).clicked() {
                        open = Some(c.id);
                        ui.close();
                    }
                    if ui.button(tr("Compare working tree with this commit")).clicked() {
                        use_base = Some(Base::Local(c.id));
                        show_changes = true;
                        ui.close();
                    }
                    if ui.button(tr("Restore working tree to this commit...")).clicked() {
                        restore = Some(c.id);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(tr("Delete")).clicked() {
                        delete = Some(c.id);
                        ui.close();
                    }
                });
            }
        });
        if let Some(b) = use_base {
            self.local_set_base(b);
        }
        if show_changes {
            self.left_tool = Some(LeftTool::Commit);
        }
        if let Some(id) = open {
            self.open_local_view(id);
        }
        if let Some(id) = restore {
            self.local_restore_confirm(id);
        }
        if let Some(id) = delete {
            self.popup = Popup::Confirm { kind: ConfirmKind::DeleteLocal(id), message: i18n::fmt("Delete local commit #{}?", &[&id]) };
        }
    }

    // ------------------------------------------------------------------ the commit browser

    pub fn draw_local_view(&mut self, ctx: &egui::Context) {
        if !matches!(self.popup, Popup::LocalView(_)) {
            return;
        }
        let Popup::LocalView(mut st) = std::mem::replace(&mut self.popup, Popup::None) else { return };
        let pal = self.pal;
        let commits: Vec<(u64, String, u64)> = self.git.local.as_ref().map(|l| l.commits().iter().map(|c| (c.id, c.message.clone(), c.time)).collect()).unwrap_or_default();
        let Some(idx) = commits.iter().position(|c| c.0 == st.id) else {
            self.editor_focus = true; // the commit vanished (deleted)
            return;
        };
        let (esc, left, right) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::ArrowLeft), i.key_pressed(Key::ArrowRight)));
        ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft);
            i.consume_key(egui::Modifiers::NONE, Key::ArrowRight);
        });
        let mut close = esc;
        let mut goto: Option<u64> = None;
        if left && idx > 0 {
            goto = Some(commits[idx - 1].0);
        }
        if right && idx + 1 < commits.len() {
            goto = Some(commits[idx + 1].0);
        }
        let (mut restore_all, mut use_base, mut restore_file): (bool, bool, Option<String>) = (false, false, None);
        if let Some(id) = goto {
            self.local_view_goto(&mut st, id);
        }
        self.local_view_load(&mut st);

        let screen = ctx.content_rect();
        let w = (screen.width() * 0.88).clamp(760.0, 1240.0).min(screen.width() - 24.0);
        let h = (screen.height() * 0.84).clamp(420.0, 860.0).min(screen.height() - 24.0);
        let (_, message, time) = commits[idx].clone();
        let mono = FontId::monospace(13.0);
        egui::Area::new(egui::Id::unique("local_view")).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            let fr = widgets::glass_frame(&pal).inner_margin(Margin::same(20)).show(ui, |ui| {
                ui.set_width(w - 40.0);
                // ---------------------------------------------------------------- header
                ui.horizontal(|ui| {
                    ui.set_min_height(36.0);
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let prev_ok = idx > 0;
                    let next_ok = idx + 1 < commits.len();
                    ui.add_enabled_ui(prev_ok, |ui| {
                        if widgets::icon_button(ui, &pal, Icon::ChevronLeft, 32.0, if prev_ok { pal.text } else { pal.text_dim.gamma_multiply(0.4) }, tr("Previous local commit (Left)")).clicked() {
                            goto = Some(commits[idx.saturating_sub(1)].0);
                        }
                    });
                    ui.add_enabled_ui(next_ok, |ui| {
                        if widgets::icon_button(ui, &pal, Icon::ChevronRight, 32.0, if next_ok { pal.text } else { pal.text_dim.gamma_multiply(0.4) }, tr("Next local commit (Right)")).clicked() {
                            goto = Some(commits[(idx + 1).min(commits.len() - 1)].0);
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!("#{}", st.id)).font(fonts::semibold(15.0)).color(pal.accent));
                    ui.label(RichText::new(crate::ui_chrome::elide(&message, 60)).font(fonts::semibold(15.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new(i18n::fmt("{0}   -   {1} file(s)", &[&age_text(time), &st.files.len()])).color(pal.text_dim).size(12.5));
                    widgets::right_row(ui, 36.0, |ui| {
                        if widgets::icon_button(ui, &pal, Icon::Close, 32.0, pal.text_dim, tr("Close (Esc)")).clicked() {
                            close = true;
                        }
                        if widgets::secondary_button(ui, &pal, tr("Restore working tree...")).clicked() {
                            restore_all = true;
                        }
                        if widgets::secondary_button(ui, &pal, tr("Use as diff base")).on_hover_text(tr("Gutter markers and the changes list will compare with this commit")).clicked() {
                            use_base = true;
                        }
                    });
                });
                ui.add_space(14.0);

                // ---------------------------------------------------------------- body: files | diff / code
                let body_h = (h - 40.0 - 36.0 - 14.0).max(200.0);
                let (area, _) = ui.allocate_exact_size(vec2(w - 40.0, body_h), Sense::hover());
                let list = Rect::from_min_size(area.min, vec2(300.0, area.height()));
                let view = Rect::from_min_max(pos2(list.max.x + 12.0, area.min.y), area.max);

                ui.painter().rect_filled(list, CornerRadius::same(14), pal.hover);
                let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list.shrink2(vec2(6.0, 8.0))).layout(egui::Layout::top_down(Align::Min)));
                if st.files.is_empty() {
                    lui.add_space(24.0);
                    lui.vertical_centered(|ui| ui.label(RichText::new(tr("This commit changed no files")).color(pal.text_dim)));
                }
                let mut pick: Option<usize> = None;
                egui::ScrollArea::vertical().id_salt("lv_files").auto_shrink([false, false]).show_rows(&mut lui, ROW_H, st.files.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for i in range {
                        let c = &st.files[i];
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                        widgets::row_background(ui, &pal, r.expand2(vec2(2.0, 0.0)), i == st.sel, resp.hovered());
                        let col = kind_color(self, c.kind);
                        let br = Rect::from_min_size(pos2(r.min.x + 12.0, r.center().y - 9.0), Vec2::splat(18.0));
                        ui.painter().rect_filled(br, CornerRadius::same(5), col.gamma_multiply(0.2));
                        ui.painter().text(br.center(), Align2::CENTER_CENTER, c.kind.letter(), fonts::semibold(11.0), col);
                        let (name, dir) = match c.path.rsplit_once('/') {
                            Some((d, n)) => (n.to_string(), d.to_string()),
                            None => (c.path.clone(), String::new()),
                        };
                        let g = ui.painter().layout_no_wrap(name, fonts::ui(13.5), pal.text);
                        let gw = g.size().x;
                        let clip = ui.painter().with_clip_rect(r);
                        clip.galley(pos2(r.min.x + 38.0, r.center().y - g.size().y / 2.0), g, pal.text);
                        if !dir.is_empty() {
                            clip.text(pos2(r.min.x + 48.0 + gw, r.center().y), Align2::LEFT_CENTER, dir, fonts::ui(11.5), pal.text_dim);
                        }
                        if resp.clicked() {
                            pick = Some(i);
                        }
                    }
                });
                if let Some(i) = pick {
                    st.sel = i;
                }

                ui.painter().rect_filled(view, CornerRadius::same(14), pal.editor_bg);
                let top = Rect::from_min_max(view.min, pos2(view.max.x, view.min.y + 48.0));
                if let Some(c) = st.files.get(st.sel) {
                    ui.painter().with_clip_rect(top.shrink2(vec2(16.0, 0.0))).text(pos2(top.min.x + 16.0, top.center().y), Align2::LEFT_CENTER, &c.path, fonts::semibold(13.5), pal.text);
                    let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(top.shrink2(vec2(10.0, 4.0))).layout(egui::Layout::right_to_left(Align::Center)));
                    if widgets::secondary_button(&mut tui, &pal, tr("Restore file...")).on_hover_text(tr("Put this file back into its state in this commit")).clicked() {
                        restore_file = Some(c.path.clone());
                    }
                    let cur = if st.mode == LocalViewMode::Changes { 0 } else { 1 };
                    if let Some(i) = widgets::segmented(&mut tui, &pal, &[tr("Changes"), tr("Code")], cur) {
                        st.mode = if i == 0 { LocalViewMode::Changes } else { LocalViewMode::Code };
                    }
                }
                let body = Rect::from_min_max(pos2(view.min.x, top.max.y), view.max).shrink2(vec2(2.0, 4.0));
                let mut vui = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(Align::Min)));
                let row_h = 21.0;
                let first_change = st.scroll_to.take();
                vui.spacing_mut().item_spacing.y = 0.0;
                vui.spacing_mut().scroll.floating = false;
                let longest = st.diff.iter().map(|l| l.text.as_str()).chain(st.code.iter().map(String::as_str)).map(|text| vui.painter().layout_no_wrap(text.replace('\t', "    "), mono.clone(), pal.plain).size().x).fold(0.0, f32::max);
                let row_w = longest + 150.0;
                match st.mode {
                    LocalViewMode::Changes => {
                        if st.diff.is_empty() {
                            vui.add_space(20.0);
                            vui.vertical_centered(|ui| ui.label(RichText::new(tr("No differences")).color(pal.text_dim)));
                        }
                        let mut area = egui::ScrollArea::both().id_salt(("lv_diff", st.id, st.sel)).auto_shrink([false, false]);
                        if let Some(f) = first_change {
                            area = area.vertical_scroll_offset(f.saturating_sub(3) as f32 * row_h);
                        }
                        area.show_rows(&mut vui, row_h, st.diff.len(), |ui, range| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for l in &st.diff[range] {
                                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(row_w), row_h), Sense::hover());
                                let (bg, mark, fg) = match l.kind {
                                    LineKind::Added => (pal.git_added.gamma_multiply(0.16), "+", pal.git_added),
                                    LineKind::Removed => (pal.error.gamma_multiply(0.16), "-", pal.error),
                                    LineKind::Same => (Color32::TRANSPARENT, " ", pal.plain),
                                };
                                if l.kind != LineKind::Same {
                                    ui.painter().rect_filled(r, CornerRadius::ZERO, bg);
                                }
                                let num = |n: Option<usize>| n.map_or("    ".to_string(), |n| format!("{n:>4}"));
                                ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, num(l.old_no), mono.clone(), pal.line_number);
                                ui.painter().text(pos2(r.min.x + 58.0, r.center().y), Align2::LEFT_CENTER, num(l.new_no), mono.clone(), pal.line_number);
                                ui.painter().text(pos2(r.min.x + 108.0, r.center().y), Align2::LEFT_CENTER, mark, mono.clone(), fg);
                                ui.painter().text(pos2(r.min.x + 128.0, r.center().y), Align2::LEFT_CENTER, l.text.replace('\t', "    "), mono.clone(), if l.kind == LineKind::Same { pal.plain } else { fg });
                            }
                        });
                    }
                    LocalViewMode::Code => {
                        if st.code.is_empty() {
                            vui.add_space(20.0);
                            vui.vertical_centered(|ui| ui.label(RichText::new(if st.files.get(st.sel).is_some_and(|c| c.kind == ChangeKind::Deleted) { tr("The file does not exist in this commit") } else { tr("The file is empty") }).color(pal.text_dim)));
                        }
                        egui::ScrollArea::both().id_salt(("lv_code", st.id, st.sel)).auto_shrink([false, false]).show_rows(&mut vui, row_h, st.code.len(), |ui, range| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for i in range {
                                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(row_w), row_h), Sense::hover());
                                ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, format!("{:>4}", i + 1), mono.clone(), pal.line_number);
                                ui.painter().text(pos2(r.min.x + 64.0, r.center().y), Align2::LEFT_CENTER, st.code[i].replace('\t', "    "), mono.clone(), pal.plain);
                            }
                        });
                    }
                }
            });
            widgets::sheen(ui.painter(), fr.response.rect, &pal);
        });

        let id = st.id;
        if let Some(g) = goto {
            self.local_view_goto(&mut st, g);
        }
        if close {
            self.editor_focus = true;
        } else if restore_all {
            self.local_restore_confirm(id);
        } else if use_base {
            self.local_view_use_as_base(id);
            self.editor_focus = true;
        } else if let Some(rel) = restore_file {
            self.local_restore_file_confirm(id, rel);
        } else {
            self.popup = Popup::LocalView(st);
        }
    }

    // ------------------------------------------------------------------ old code popup of a gutter marker

    pub fn draw_hunk_popup(&mut self, ctx: &egui::Context) {
        let Some(h) = self.hunk_popup.clone() else { return };
        let stale = self.popup.is_open() || self.doc().is_none_or(|d| d.path.as_ref() != Some(&h.doc_path) || d.ed.revision() != h.rev || (d.view.scroll.y - h.scroll_y).abs() > 0.5);
        if stale || ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.hunk_popup = None;
            return;
        }
        let pal = self.pal;
        let screen = ctx.content_rect();
        let mono = FontId::monospace((self.settings.font_size - 1.0).max(10.0));
        let row_h = (ctx.fonts_mut(|f| f.row_height(&mono)) + 4.0).round();
        let width = 580.0f32.min(screen.width() - 24.0);
        let shown = h.hunk.old_lines.len().min(14);
        let body_h = if h.hunk.old_lines.is_empty() { 40.0 } else { shown as f32 * row_h + 12.0 };
        let est_h = 44.0 + body_h + 60.0;
        let lh = crate::editor_view::line_height_for(self.settings.font_size);
        let mut pos = h.anchor;
        pos.x = pos.x.min(screen.max.x - width - 12.0).max(8.0);
        if pos.y + est_h > screen.max.y - 8.0 {
            pos.y = (h.anchor.y - lh - est_h - 4.0).max(8.0);
        }
        let (mut close, mut revert, mut full_diff, mut copy) = (false, false, false, false);
        let mut rect = Rect::NOTHING;
        egui::Area::new(egui::Id::unique("hunk_popup")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            let fr = egui::Frame::new().fill(pal.popup_bg).corner_radius(CornerRadius::same(14)).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.set_width(width - 28.0);
                ui.horizontal(|ui| {
                    ui.set_min_height(28.0);
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    icons::paint(ui.painter(), r, Icon::History, pal.text_dim);
                    let title = if h.hunk.old_lines.is_empty() { tr("No previous code here") } else if h.hunk.new_len == 0 { tr("Deleted code") } else { tr("Previous code") };
                    ui.label(RichText::new(title).font(fonts::semibold(13.5)));
                    widgets::chip(ui, &pal, &crate::ui_chrome::elide(&h.base_label, 28));
                    widgets::right_row(ui, 28.0, |ui| {
                        if widgets::icon_button(ui, &pal, Icon::Close, 26.0, pal.text_dim, tr("Close (Esc)")).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(6.0);
                if h.hunk.old_lines.is_empty() {
                    let msg = if h.is_new_file { tr("This file is new: there is no earlier version.") } else { tr("These lines were added: there is no earlier code to show.") };
                    ui.label(RichText::new(msg).color(pal.text_dim).size(12.5));
                } else {
                    egui::Frame::new().fill(pal.editor_bg).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(0, 6)).show(ui, |ui| {
                        ui.spacing_mut().scroll.floating = false;
                        let longest = h.hunk.old_lines.iter().map(|l| ui.painter().layout_no_wrap(l.replace('\t', "    "), mono.clone(), pal.plain).size().x).fold(0.0, f32::max);
                        egui::ScrollArea::both().id_salt("hunk_old").max_height(14.0 * row_h).auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (i, l) in h.hunk.old_lines.iter().enumerate() {
                                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(longest + 70.0), row_h), Sense::hover());
                                ui.painter().rect_filled(r, CornerRadius::ZERO, pal.error.gamma_multiply(0.12));
                                ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, format!("{:>4}", h.hunk.old_start + i + 1), mono.clone(), pal.line_number);
                                ui.painter().text(pos2(r.min.x + 52.0, r.center().y), Align2::LEFT_CENTER, l.replace('\t', "    "), mono.clone(), pal.plain);
                            }
                        });
                    });
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if !h.is_new_file && widgets::secondary_button(ui, &pal, tr("Revert this change")).on_hover_text(tr("Replace the changed lines with the old code")).clicked() {
                        revert = true;
                    }
                    if widgets::secondary_button(ui, &pal, tr("Show full diff")).clicked() {
                        full_diff = true;
                    }
                    if !h.hunk.old_lines.is_empty() && widgets::secondary_button(ui, &pal, tr("Copy")).clicked() {
                        copy = true;
                    }
                });
            });
            rect = fr.response.rect;
        });
        // a click anywhere else (except on the gutter, where it toggles the popup) dismisses it
        let outside = ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.expand(2.0).contains(p) && !self.gutter_rect.is_some_and(|g| g.contains(p))));
        if copy {
            ctx.copy_text(self.hunk_old_text());
        }
        if revert {
            self.hunk_revert();
        } else if full_diff {
            self.hunk_popup = None;
            if let Some(rel) = self.current_vcs_rel_path() {
                self.git_show_diff(&rel);
            }
        } else if close || outside {
            self.hunk_popup = None;
        }
    }
}
