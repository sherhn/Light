//! What the SQL console shows: the Results tab, a grid of the rows the last statement came back with.

use eframe::egui::{self, Align2, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, Vec2};

use ide_core::db::{DbValue, ResultSet};

use crate::app::App;
use crate::app_sql::ResultsView;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::theme::Palette;
use crate::widgets;

const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 30.0;

impl App {
    /// The Database window: the tables of the project's database, their columns; a click on a table shows its rows.
    pub fn draw_database_panel(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.sql.worker.is_none() && self.sql.open_error.is_none() {
            self.sql_ensure_worker();
        }
        let path = self.sql.db_path.clone();
        let name = path.as_deref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy());
        let busy = self.sql.running() || self.sql.opening();
        let mut choose = false;

        egui::Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                sql_icon(ui, &pal, Icon::Database);
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr("Database")).font(fonts::semibold(15.0)));
                    sql_badge(ui, "SQLite", pal.text_dim);
                });
            });
            ui.add_space(8.0);
            widgets::card_frame(&pal, pal.hover.gamma_multiply(0.42)).corner_radius(widgets::radius())
                .inner_margin(Margin::same(8)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let label = name.as_deref().unwrap_or_else(|| tr("No database"));
                    let response = ui.add(egui::Label::new(RichText::new(label).font(fonts::semibold(13.0))).truncate());
                    if let Some(path) = &path {
                        response.on_hover_text(path.display().to_string());
                    }
                    ui.horizontal_wrapped(|ui| {
                        if busy {
                            ui.add(egui::Spinner::new().size(14.0));
                        }
                        choose = ui.add_enabled(!busy, egui::Button::new(tr(if path.is_some() { "Change…" } else { "Open database" }))).clicked();
                    });
                });
        });
        if choose {
            self.sql_pick_database();
            return;
        }
        if let Some(error) = &self.sql.open_error {
            egui::Frame::new().inner_margin(Margin::symmetric(12, 5)).show(ui, |ui| {
                ui.label(RichText::new(error).color(pal.error).size(12.0));
            });
        }
        let Some(schema) = &self.sql.schema else {
            if !busy && self.sql.open_error.is_none() {
                egui::Frame::new().inner_margin(Margin::symmetric(12, 5)).show(ui, |ui| {
                    ui.label(RichText::new(tr("The SQL consoles of this project run on its SQLite database: the one chosen for it, or the only database file in it.")).color(pal.text_dim).size(12.0));
                });
            }
            return;
        };
        let mut show = None;
        let mut toggle: Option<String> = None;
        let selected = self.docs.get(self.active).and_then(|d| d.results.as_ref())
            .filter(|v| v.table).map(|v| v.title.as_str());
        egui::ScrollArea::vertical().id_salt("sql-schema").auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin::symmetric(8, 0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (label, views, group_icon) in [(tr("Tables"), false, Icon::Table), (tr("Views"), true, Icon::Eye)] {
                    let objects = || schema.tables.iter().filter(|t| t.is_view == views);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(RichText::new(label).font(fonts::semibold(12.0)).color(pal.text_dim));
                        sql_badge(ui, &objects().count().to_string(), pal.text_dim);
                    });
                    ui.add_space(3.0);
                    for table in objects() {
                        let open = self.sql.expanded.contains(&table.name);
                        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
                        let arrow = Rect::from_min_max(row.min, egui::pos2(row.left() + 25.0, row.bottom()));
                        let body = Rect::from_min_max(egui::pos2(arrow.right(), row.top()), row.max);
                        let disclosure = ui.interact(arrow, ui.scope_id().with(("sql-disclosure", &table.name)), Sense::click());
                        let object = ui.interact(body, ui.scope_id().with(("sql-object", &table.name)), Sense::click()).on_hover_text(&table.name);
                        if disclosure.clicked() {
                            toggle = Some(table.name.clone());
                        }
                        if object.clicked() {
                            show = Some(table.name.clone());
                        }
                        let selected = selected == Some(&table.name);
                        if selected || object.hovered() || disclosure.hovered() {
                            ui.painter().rect_filled(row, 7.0, if selected { pal.selected } else { pal.hover });
                        }
                        icons::paint(ui.painter(), Rect::from_center_size(arrow.center(), Vec2::splat(12.0)), if open { Icon::ChevronDown } else { Icon::ChevronRight }, pal.text_dim);
                        let mark = Rect::from_min_size(body.left_top() + Vec2::new(1.0, 6.0), Vec2::splat(16.0));
                        icons::paint(ui.painter(), mark, group_icon, pal.accent);
                        let label = Rect::from_min_max(egui::pos2(mark.right() + 8.0, row.top()), row.right_bottom() - Vec2::new(8.0, 0.0));
                        ui.painter_at(label).text(label.left_center(), Align2::LEFT_CENTER, &table.name, fonts::ui(13.0), pal.text);
                        if open {
                            for col in &table.columns {
                                let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
                                let rail = rect.left() + 12.0;
                                ui.painter().vline(rail, rect.y_range(), Stroke::new(1.0, pal.border));
                                let label = if col.pk { format!("{} {}", col.name, tr("(key)")) } else { col.name.clone() };
                                let name_rect = Rect::from_min_max(rect.left_top() + Vec2::new(25.0, 0.0), egui::pos2(rect.right() - rect.width() * 0.4, rect.bottom()));
                                let type_rect = Rect::from_min_max(egui::pos2(name_rect.right() + 8.0, rect.top()), rect.right_bottom() - Vec2::new(6.0, 0.0));
                                ui.painter_at(name_rect).text(name_rect.left_center(), Align2::LEFT_CENTER, label, fonts::ui(12.0), pal.text_dim);
                                ui.painter_at(type_rect).text(type_rect.right_center(), Align2::RIGHT_CENTER, &col.ty, FontId::monospace(11.0), pal.blue);
                            }
                        }
                    }
                }
                if schema.tables.is_empty() {
                    ui.add_space(10.0);
                    ui.label(RichText::new(tr("This database has no tables or views yet.")).color(pal.text_dim).size(12.0));
                }
            });
        });
        if let Some(name) = toggle {
            if let Some(i) = self.sql.expanded.iter().position(|n| *n == name) {
                self.sql.expanded.remove(i);
            } else {
                self.sql.expanded.push(name);
            }
        }
        if let Some(name) = show {
            self.sql_show_table(&name);
        }
    }

    pub fn draw_results(&mut self, ui: &mut egui::Ui, idx: usize) {
        egui::Frame::new().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| self.draw_sql_results(ui, idx));
    }

    fn draw_sql_results(&mut self, ui: &mut egui::Ui, idx: usize) {
        let pal = self.pal;
        let Some(view) = self.docs[idx].results.clone() else { return };
        let mut exported: Option<std::path::PathBuf> = None;
        let mut active = view.active;
        let mut action: Option<&'static str> = None;
        let busy = self.sql.running();

        widgets::card_frame(&pal, pal.panel_bg).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                sql_icon(ui, &pal, if view.table { Icon::Table } else { Icon::Terminal });
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr(if view.table { "Table data" } else { "SQL results" })).color(pal.text_dim).font(fonts::semibold(11.0)));
                    let title = if view.title.is_empty() { tr("No results yet") } else { &view.title };
                    ui.add(egui::Label::new(RichText::new(title).font(fonts::semibold(16.0))).truncate()).on_hover_text(title);
                });
            });
            if !view.database.is_empty() {
                ui.add_space(5.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::Label::new(RichText::new(&view.database).color(pal.text_dim).size(12.0)).truncate());
                    if view.error.is_none() {
                        sql_badge(ui, &format!("{} ms", view.ms), pal.text_dim);
                        if view.table {
                            let (mode, color) = if view.edit.is_some() { ("Editable", pal.accent) } else { ("Read-only", pal.text_dim) };
                            sql_badge(ui, tr(mode), color);
                        }
                        let pending = view.pending();
                        if pending > 0 {
                            sql_badge(ui, &i18n::fmt("{} not saved yet", &[&pending]), pal.warning);
                        }
                    }
                });
            }
            if view.sets.len() > 1 {
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for (i, set) in view.sets.iter().enumerate() {
                        let label = format!("{} · {}", i + 1, result_label(set));
                        if ui.selectable_label(i == active, label).clicked() {
                            active = i;
                        }
                    }
                });
            }
        });

        if view.edit.is_some() {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let pending = view.pending();
                let dirty = pending > 0 || view.editing.is_some();
                let add = ui.add_enabled(!busy, egui::Button::new(format!("+ {}", tr("Add Row"))).min_size(Vec2::new(0.0, 28.0)));
                let can_delete = view.selected.is_some_and(|(row, _)| !view.deleted.contains(&row));
                let del = ui.add_enabled(!busy && can_delete, egui::Button::new(tr("Delete Row")).min_size(Vec2::new(0.0, 28.0)));
                let revert = ui.add_enabled(!busy && dirty, egui::Button::new(tr("Discard")).min_size(Vec2::new(0.0, 28.0)));
                let save_label = RichText::new(tr("Save")).strong().color(if dirty && !busy { pal.text_on_accent } else { pal.text_dim });
                let mut save = egui::Button::new(save_label).min_size(Vec2::new(60.0, 28.0));
                if dirty && !busy {
                    save = save.fill(pal.accent);
                }
                let save = ui.add_enabled(!busy && dirty, save);
                if busy {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(tr("Saving changes...")).color(pal.text_dim).size(12.0));
                }
                let buttons = [("Add Row", add, "add"), ("Delete Row", del, "delete"), ("Discard", revert, "revert"), ("Save", save, "commit")];
                if self.script.is_some() {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("results_button_rects"),
                        buttons.iter().map(|(label, response, _)| (tr(label).to_string(), response.rect)).collect::<Vec<_>>()));
                }
                action = buttons.into_iter().find_map(|(_, response, action)| response.clicked().then_some(action));
            });
        }
        if active != view.active && let Some(v) = self.docs[idx].results.as_mut() {
            v.active = active;
        }

        if let Some(error) = &view.error {
            ui.add_space(16.0);
            egui::Frame::new().fill(pal.error.gamma_multiply(0.12)).inner_margin(Margin::same(12)).corner_radius(CornerRadius::same(10)).show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(720.0));
                ui.label(egui::RichText::new(tr("The statement did not run").to_string()).color(pal.error).strong());
                ui.add(egui::Label::new(RichText::new(error).monospace()).wrap());
                ui.label(egui::RichText::new(tr("The database stopped at the first statement it could not run.").to_string()).weak());
            });
            return;
        }
        let Some(set) = view.sets.get(active).cloned() else {
            ui.add_space(10.0);
            ui.label(RichText::new(tr("Run a statement or open a table to see its results here.")).color(pal.text_dim));
            return;
        };

        if set.columns.is_empty() {
            // a change, not a read: there is nothing to tabulate
            ui.add_space(16.0);
            ui.label(RichText::new(result_label(&set)).font(fonts::semibold(15.0)));
            return;
        }

        // a grid that writes back keeps `rowid` first and hidden: it is how the changes find their row
        let skip = usize::from(view.edit.is_some());
        let columns = &set.columns[skip..];
        let base_rows = set.rows.len();
        let total_rows = base_rows + view.added.len();

        ui.add_space(8.0);
        let font = FontId::monospace(12.5);
        let mut widths = column_widths(ui, &set, &view, skip, &font);
        fit_widths(&mut widths, ui.available_width() - 8.0);

        // the keyboard works on the grid when the Results tab is the one shown
        let keyboard = idx == self.active && !self.popup.is_open() && !self.term.shell_focused;
        let mut start_editing = false;
        let mut set_null = false;
        if !busy && keyboard && view.edit.is_some() {
            let editing = view.editing.is_some();
            let has = view.selected.is_some_and(|(row, col)| view.can_edit_cell(row, col));
            let pressed = |key: egui::Key| ui.input(|i| i.key_pressed(key) && i.modifiers.is_none());
            if !editing && has && pressed(egui::Key::Enter) {
                start_editing = true;
                ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            } else if !editing && has && pressed(egui::Key::Delete) {
                set_null = true;
                ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
            }
        }

        let style = GridStyle { widths: &widths, font: &font, pal: &pal };
        let content_width = widths.iter().sum::<f32>() + 4.0 + widths.len() as f32 * 2.0;
        let grid_width = content_width.max(ui.available_width() - 8.0);
        let grid_height = (ui.available_height() - HEADER_H - 56.0).max(ROW_H);

        // Keep the headers aligned with the scrollable rows, including on wide tables.
        let mut pick = None;
        let mut cell_rects = Vec::new();
        let canceled = keyboard && view.editing.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape));
        let mut finish_editing = canceled.then_some(false);
        egui::ScrollArea::horizontal().id_salt("sql-columns").show(ui, |ui| {
            ui.set_min_width(grid_width);
            ui.spacing_mut().item_spacing.y = 0.0;
            let head = ui.allocate_exact_size(Vec2::new(grid_width, HEADER_H), Sense::hover()).0;
            ui.painter().rect_filled(head, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, pal.panel_bg);
            for (col, rect) in style.cells(head).enumerate() {
                style.draw_text(ui, rect, &columns[col], pal.text, false);
            }
            ui.painter().hline(head.x_range(), head.bottom(), Stroke::new(1.0, pal.border));
            let results = self.docs[idx].results.as_mut().expect("checked above");
            egui::ScrollArea::vertical().id_salt("sql-rows").auto_shrink([false, true]).max_height(grid_height).show_rows(ui, ROW_H, total_rows, |ui, range| {
                let base = ui.available_rect_before_wrap();
                ui.set_min_width(grid_width);
                let first = range.start;
                for i in range {
                    let rect = Rect::from_min_size(egui::pos2(base.left(), base.top() + (i - first) as f32 * ROW_H), Vec2::new(grid_width, ROW_H));
                    let cells_rect = Rect::from_min_size(rect.min, Vec2::new(content_width, ROW_H));
                    let row_resp = ui.interact(cells_rect, ui.scope_id().with(("sql-row", i)), if view.edit.is_some() && !busy { Sense::click() } else { Sense::hover() });
                    // a deleted row is crossed out; an added one stands out; the rest keep the zebra
                    let fill = if results.deleted.contains(&i) {
                        Some(pal.error.gamma_multiply(0.10))
                    } else if i >= base_rows {
                        Some(pal.ok.gamma_multiply(0.08))
                    } else if row_resp.hovered() && view.edit.is_some() {
                        Some(pal.hover)
                    } else if i % 2 == 1 {
                        Some(pal.panel_bg.gamma_multiply(0.35))
                    } else {
                        None
                    };
                    if let Some(f) = fill {
                        ui.painter().rect_filled(rect, 0.0, f);
                    }
                    let cells: Vec<CellDraw> = (0..columns.len()).map(|c| {
                        let full = c + skip;
                        let (text, null, changed) = if i >= base_rows {
                            let value = results.added[i - base_rows].get(&full);
                            (value.cloned().flatten().unwrap_or_default(), matches!(value, Some(None)), value.is_some())
                        } else {
                            match results.changes.get(&(i, full)) {
                                Some(Some(s)) => (s.clone(), false, true),
                                Some(None) => (String::new(), true, true),
                                None => set.rows.get(i).and_then(|r| r.get(full)).map(|v| (v.render(), matches!(v, DbValue::Null), false)).unwrap_or_default(),
                            }
                        };
                        CellDraw { text, null, changed }
                    }).collect();
                    if row_resp.clicked() {
                        pick = Some((i, column_at(&widths, row_resp.interact_pointer_pos().unwrap_or(rect.center()).x - rect.left()) + skip));
                    }
                    if row_resp.double_clicked() {
                        start_editing = true;
                    }
                    if self.script.is_some() {
                        cell_rects.extend(style.cells(rect).enumerate().map(|(col, cell)| ((i, col), cell)));
                    }
                    if style.draw_row(ui, rect, &cells, results, i, skip).is_some_and(|response| response.lost_focus()) {
                        finish_editing.get_or_insert(true);
                    }
                }
            });
        });
        if let Some(keep) = finish_editing.or_else(|| pick.map(|_| true)) {
            self.sql_editing_finish(keep);
        }
        {
            let results = self.docs[idx].results.as_mut().expect("checked above");
            if pick.is_some() {
                results.selected = pick;
            }
            if let Some((row, col)) = results.selected
                && results.editing.is_none()
                && results.can_edit_cell(row, col)
            {
                if start_editing {
                    results.editing = Some(crate::app_sql::Editing { at: (row, col), buffer: results.cell_text(row, col), ..Default::default() });
                    ui.ctx().request_repaint();
                } else if set_null {
                    results.set_cell(row, col, None);
                }
            }
        }
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("results_cell_rects"), cell_rects));
        if let Some(action) = action {
            match action {
                "add" => self.sql_add_row(),
                "delete" => self.sql_delete_row(),
                "revert" => self.sql_revert(),
                "commit" => self.sql_commit(),
                _ => {}
            }
        }

        // the summary and the export sit under the grid
        if total_rows == 0 {
            ui.add_space(8.0);
            ui.label(RichText::new(tr("No rows to display")).color(pal.text_dim));
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            sql_badge(ui, &result_label(&set), pal.text_dim);
            if set.truncated {
                sql_badge(ui, tr("the row ceiling was reached"), pal.warning);
            }
            let clean = self.docs[idx].results.as_ref().is_some_and(|v| v.pending() == 0 && v.editing.is_none());
            let export = ui.add_enabled(clean && !busy, egui::Button::new(tr("Export CSV")));
            if !clean {
                export.clone().on_disabled_hover_text(tr("Save or discard the table edits first"));
            }
            if export.clicked() {
                exported = rfd::FileDialog::new().set_file_name("results.csv").save_file();
            }
        });
        if let Some(path) = exported {
            match std::fs::write(&path, set.to_csv_from(skip)) {
                Ok(()) => self.toast(i18n::fmt("Saved {}", &[&path.display().to_string()]), crate::editor_view::Sev::Weak),
                Err(e) => self.toast(i18n::fmt("Cannot save {}: {}", &[&path.display().to_string(), &e.to_string()]), crate::editor_view::Sev::Error),
            }
        }
    }
}

fn sql_icon(ui: &mut egui::Ui, pal: &Palette, icon: Icon) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
    ui.painter().rect_filled(rect, widgets::radius(), pal.accent.gamma_multiply(0.16));
    icons::paint(ui.painter(), rect.shrink(8.0), icon, pal.accent);
}

fn sql_badge(ui: &mut egui::Ui, label: &str, tint: egui::Color32) {
    egui::Frame::new().fill(tint.gamma_multiply(0.13)).corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(6, 2)).show(ui, |ui| {
            ui.label(RichText::new(label).color(tint).size(11.5));
        });
}

fn result_label(set: &ResultSet) -> String {
    if set.columns.is_empty() {
        i18n::fmt("{} rows changed", &[&set.affected])
    } else {
        let rows = format!("{}{}", set.rows.len(), if set.truncated { "+" } else { "" });
        i18n::fmt("{} rows", &[&rows])
    }
}

/// What one cell of the grid holds when it is drawn.
struct CellDraw {
    text: String,
    null: bool,
    changed: bool,
}

/// The column a click on `x` (from the row's left) landed in.
fn column_at(widths: &[f32], x: f32) -> usize {
    let mut left = 4.0;
    for (i, w) in widths.iter().enumerate() {
        if x < left + w {
            return i;
        }
        left += w + 2.0;
    }
    widths.len().saturating_sub(1)
}

struct GridStyle<'a> {
    widths: &'a [f32],
    font: &'a FontId,
    pal: &'a Palette,
}

impl GridStyle<'_> {
    fn cells(&self, row: Rect) -> impl Iterator<Item = Rect> {
        self.widths.iter().scan(row.left() + 4.0, move |x, width| {
            let rect = Rect::from_min_max(egui::pos2(*x, row.top()), egui::pos2(*x + width - 4.0, row.bottom()));
            *x += width + 2.0;
            Some(rect)
        })
    }

    fn draw_text(&self, ui: &egui::Ui, rect: Rect, text: &str, color: egui::Color32, right: bool) {
        let painter = ui.painter_at(rect);
        let galley = painter.layout(text.to_owned(), self.font.clone(), color, f32::INFINITY);
        let width = (rect.width() - 4.0).max(0.0);
        let shown = if galley.size().x > width {
            let keep = (width / galley.size().x * text.chars().count() as f32).floor() as usize;
            text.chars().take(keep.saturating_sub(1)).chain(['…']).collect()
        } else {
            text.to_owned()
        };
        let pos = egui::pos2(if right { rect.right() - 2.0 } else { rect.left() + 2.0 }, rect.center().y);
        painter.text(pos, if right { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, shown, self.font.clone(), color);
    }

    fn draw_row(&self, ui: &mut egui::Ui, rect: Rect, cells: &[CellDraw], view: &mut ResultsView, row: usize, skip: usize) -> Option<egui::Response> {
        let mut response = None;
        for (col, (rect, cell)) in self.cells(rect).zip(cells).enumerate() {
            let at = (row, col + skip);
            if let Some(editing) = view.editing.as_mut().filter(|edit| edit.at == at) {
                let id = ui.scope_id().with(("sql-cell", at));
                let resp = ui.put(rect.shrink(1.0), egui::TextEdit::singleline(&mut editing.buffer)
                    .id(id).font(self.font.clone()).margin(Margin::symmetric(4, 3)));
                editing.changed |= resp.changed();
                if !editing.shown {
                    resp.request_focus();
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0), egui::text::CCursor::new(editing.buffer.chars().count()),
                        )));
                        egui::TextEdit::store_state(ui.ctx(), id, state);
                    }
                    editing.shown = true;
                }
                response = Some(resp);
            } else {
                let color = if cell.null { self.pal.text_dim } else if cell.changed { self.pal.accent } else { self.pal.text };
                let right = cell.text.starts_with(|ch: char| ch.is_ascii_digit() || ch == '-');
                self.draw_text(ui, rect, if cell.null { "NULL" } else { &cell.text }, color, right);
                if cell.changed {
                    ui.painter().hline(rect.x_range().shrink(2.0), rect.bottom() - 2.0, egui::Stroke::new(1.5, self.pal.accent));
                }
            }
            if view.selected == Some(at) {
                ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.5, self.pal.accent), egui::StrokeKind::Middle);
            }
        }
        response
    }
}

/// How wide each column wants to be: the longest of its header and its values, in the grid's monospace. The first
/// `skip` columns (the hidden `rowid`) do not count.
fn column_widths(ui: &egui::Ui, set: &ResultSet, view: &ResultsView, skip: usize, font: &FontId) -> Vec<f32> {
    let width = |text: &str| ui.painter().layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
    let mut widths: Vec<_> = set.columns.iter().enumerate().skip(skip).map(|(i, c)| {
        let mut longest = width(c);
        for row in set.rows.iter().take(500) {
            if let Some(v) = row.get(i) {
                longest = longest.max(width(&v.render()));
            }
        }
        (longest + 24.0).clamp(60.0, 420.0)
    }).collect();
    let changed = view.changes.iter().map(|(&(_, col), value)| (col, value))
        .chain(view.added.iter().flat_map(|row| row.iter().map(|(&col, value)| (col, value))));
    for (col, value) in changed {
        let cell = &mut widths[col - skip];
        *cell = cell.max((width(value.as_deref().unwrap_or("NULL")) + 24.0).min(420.0));
    }
    widths
}

/// Narrow the columns into the width the grid has, never below a least width each.
fn fit_widths(widths: &mut [f32], available: f32) {
    let total: f32 = widths.iter().sum::<f32>() + 4.0 + widths.len() as f32 * 2.0;
    if total <= available || widths.is_empty() {
        return;
    }
    let over = total - available;
    let shrinkable: f32 = widths.iter().map(|w| (w - 60.0).max(0.0)).sum();
    if shrinkable >= over {
        let k = over / shrinkable;
        for w in widths.iter_mut() {
            *w -= (*w - 60.0).max(0.0) * k;
        }
    } else {
        for w in widths.iter_mut() {
            *w = (*w * available / total).max(48.0);
        }
    }
}
