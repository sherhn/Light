//! What the SQL console shows: the Results tab, a grid of the rows the last statement came back with.

use eframe::egui::{self, Align2, CornerRadius, FontId, Margin, Rect, Sense, Vec2};

use ide_core::db::{DbValue, ResultSet};

use crate::app::App;
use crate::i18n::{self, tr};

const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 30.0;

impl App {
    /// The Database window: the tables of the project's database, their columns; a click on a table shows its rows.
    pub fn draw_database_panel(&mut self, ui: &mut egui::Ui) {
        use crate::icons::{Icon, paint as icon};
        let pal = self.pal;
        // open the project's database when the window first shows itself
        if self.sql.worker.is_none() && self.sql.open_error.is_none() {
            self.sql_ensure_worker();
            if let Some(w) = &self.sql.worker {
                w.schema();
            }
        }
        let name = self.sql.db_path.as_deref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let schema = self.sql.schema.clone();

        // the header: which database, and the button that changes it
        ui.horizontal(|ui| {
            let r = ui.allocate_exact_size(Vec2::new(16.0, 16.0), Sense::hover()).0;
            icon(ui.painter(), r, Icon::Database, pal.accent);
            let label = if name.is_empty() { tr("No database").to_string() } else { name.clone() };
            ui.label(egui::RichText::new(label).strong());
            if self.sql.running > 0 {
                ui.label(egui::RichText::new("…").weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(tr("Change…")).clicked() {
                    self.sql_pick_database();
                    if let Some(w) = &self.sql.worker {
                        w.schema();
                    }
                }
            });
        });
        if let Some(error) = self.sql.open_error.clone() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(error).color(pal.error).monospace());
        }
        ui.add_space(4.0);

        let Some(schema) = schema else {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tr("The SQL consoles of this project run on its SQLite database: the one chosen for it, or the only database file in it.").to_string()).weak());
            return;
        };
        let mut show: Option<String> = None;
        let mut toggle: Option<String> = None;
        let expanded = self.sql.expanded.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let groups = [
                (tr("Tables"), false, Icon::Table),
                (tr("Views"), true, Icon::Eye),
            ];
            for (label, views, group_icon) in groups {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(format!("{} ({})", label, schema.tables.iter().filter(|t| t.is_view == views).count())).weak().small());
                for table in schema.tables.iter().filter(|t| t.is_view == views) {
                    let open = expanded.contains(&table.name);
                    let row = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::click()).1;
                    let arrow = Rect::from_min_size(row.rect.left_top() + Vec2::new(2.0, 4.0), Vec2::splat(16.0));
                    if ui.interact(arrow, ui.scope_id().with(("db", &table.name)), Sense::click()).clicked() {
                        toggle = Some(table.name.clone());
                    }
                    icon(ui.painter(), arrow, if open { Icon::ChevronDown } else { Icon::ChevronRight }, pal.text_dim);
                    let mark = Rect::from_min_size(row.rect.left_top() + Vec2::new(22.0, 4.0), Vec2::splat(16.0));
                    icon(ui.painter(), mark, group_icon, pal.accent);
                    let label_rect = Rect::from_min_max(egui::pos2(row.rect.min.x + 44.0, row.rect.min.y), egui::pos2(row.rect.max.x, row.rect.max.y));
                    ui.painter_at(label_rect).text(egui::pos2(label_rect.min.x, row.rect.center().y), Align2::LEFT_CENTER, &table.name, egui::FontId::default(), if row.hovered() { pal.text } else { pal.text_dim });
                    // the name is the button: the rows of the table, in the Results tab
                    if row.clicked() {
                        show = Some(table.name.clone());
                    }
                    if open {
                        for c in &table.columns {
                            let sub = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover()).1;
                            let label = if c.pk { format!(" {} {}", c.name, tr("(key)")) } else { format!("  {}", c.name) };
                            ui.painter().text(egui::pos2(sub.rect.min.x + 44.0, sub.rect.center().y), Align2::LEFT_CENTER, label, egui::FontId::default(), pal.text_dim);
                            ui.painter().text(egui::pos2(sub.rect.max.x - 8.0, sub.rect.center().y), Align2::RIGHT_CENTER, c.ty.clone(), egui::FontId::proportional(11.0), pal.text_dim);
                        }
                    }
                }
            }
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

    /// The Results tab: what ran, what it answered with, as a grid.
    pub fn draw_results(&mut self, ui: &mut egui::Ui, idx: usize) {
        let pal = self.pal;
        let Some(view) = self.docs[idx].results.clone() else { return };
        let mut exported: Option<std::path::PathBuf> = None;
        let mut active = view.active;

        // the header: what ran, on which database, how long it took, and the sets when there are several
        egui::Frame::new().fill(pal.panel_bg).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&view.title).strong());
                if !view.database.is_empty() {
                    let r = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover()).0;
                    crate::icons::paint(ui.painter(), r, crate::icons::Icon::Database, pal.text_dim);
                    ui.label(egui::RichText::new(&view.database).weak());
                }
                if view.error.is_none() && view.ms > 0 {
                    ui.label(egui::RichText::new(format!("{} ms", view.ms)).weak());
                }
            });
            if view.sets.len() > 1 {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (i, set) in view.sets.iter().enumerate() {
                        let label = if set.columns.is_empty() { set.label() } else { format!("{} · {}", i + 1, set.label()) };
                        if ui.selectable_label(i == active, label).clicked() {
                            active = i;
                        }
                    }
                });
            }
        });
        if active != view.active {
            if let Some(v) = self.docs[idx].results.as_mut() {
                v.active = active.min(v.sets.len().saturating_sub(1));
            }
        }

        if let Some(error) = &view.error {
            ui.add_space(16.0);
            egui::Frame::new().fill(pal.error.gamma_multiply(0.12)).inner_margin(Margin::same(12)).corner_radius(CornerRadius::same(10)).show(ui, |ui| {
                ui.set_max_width(720.0);
                ui.label(egui::RichText::new(tr("The statement did not run").to_string()).color(pal.error).strong());
                ui.label(egui::RichText::new(error.clone()).monospace());
                ui.label(egui::RichText::new(tr("The database stopped at the first statement it could not run.").to_string()).weak());
            });
            return;
        }
        let Some(set) = view.sets.get(active).cloned() else { return };

        if set.columns.is_empty() {
            // a change, not a read: there is nothing to tabulate
            ui.add_space(16.0);
            ui.label(egui::RichText::new(set.label()).strong());
            return;
        }

        ui.add_space(8.0);
        let font = FontId::monospace(12.5);
        let mut widths = column_widths(ui, &set, &font);
        fit_widths(&mut widths, ui.available_width() - 8.0);

        // the header row of the columns stays above the scrolling rows
        let head = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_H), Sense::hover()).0;
        ui.painter().rect_filled(head, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, pal.panel_bg);
        draw_cells(ui, head, &widths, &set.columns.iter().map(|c| (c.clone(), true)).collect::<Vec<_>>(), &font, &pal, true);

        let rows = set.rows.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
            let base = ui.available_rect_before_wrap();
            let width = base.width();
            for i in range {
                let rect = Rect::from_min_size(egui::pos2(base.left(), base.top() + i as f32 * ROW_H), Vec2::new(width, ROW_H));
                if i % 2 == 1 {
                    ui.painter().rect_filled(rect, 0.0, pal.panel_bg.gamma_multiply(0.35));
                }
                let cells: Vec<(String, bool)> = rows[i].iter().map(|v| (v.render(), matches!(v, DbValue::Null))).collect();
                draw_cells(ui, rect, &widths, &cells, &font, &pal, false);
            }
        });

        // the summary and the export sit under the grid
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(set.label()).weak());
            if set.truncated {
                ui.label(egui::RichText::new(tr("the row ceiling was reached").to_string()).weak());
            }
            if ui.button(tr("Export CSV")).clicked() {
                exported = rfd::FileDialog::new().set_file_name("results.csv").save_file();
            }
        });
        if let Some(path) = exported {
            match std::fs::write(&path, set.to_csv()) {
                Ok(()) => self.toast(i18n::fmt("Saved {}", &[&path.display().to_string()]), crate::editor_view::Sev::Weak),
                Err(e) => self.toast(i18n::fmt("Cannot save {}: {}", &[&path.display().to_string(), &e.to_string()]), crate::editor_view::Sev::Error),
            }
        }
    }
}

/// How wide each column wants to be: the longest of its header and its values, in the grid's monospace.
fn column_widths(ui: &mut egui::Ui, set: &ResultSet, font: &FontId) -> Vec<f32> {
    let text_w = |ui: &mut egui::Ui, s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE).size().x;
    set.columns.iter().enumerate().map(|(i, c)| {
        let mut longest = text_w(ui, c);
        for row in set.rows.iter().take(500) {
            if let Some(v) = row.get(i) {
                longest = longest.max(text_w(ui, &v.render()));
            }
        }
        (longest + 24.0).clamp(60.0, 420.0)
    }).collect()
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

/// One row of the grid: the header's bold labels above, the values in their cells below (numbers to the right,
/// `NULL` quiet, a long value cut with an ellipsis).
fn draw_cells(ui: &mut egui::Ui, rect: Rect, widths: &[f32], cells: &[(String, bool)], font: &FontId, pal: &crate::theme::Palette, header: bool) {
    let mut x = rect.left() + 4.0;
    for (w, (text, is_null)) in widths.iter().zip(cells) {
        let cell = Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2((x + w - 4.0).min(rect.right() - 4.0), rect.bottom()));
        let color = if header { pal.text } else if *is_null { pal.text_dim } else { pal.text };
        let right = !header && text.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-');
        let full = ui.painter().layout_no_wrap(text.clone(), font.clone(), color);
        let shown = if full.size().x > cell.width() {
            let keep = ((cell.width() / full.size().x) * text.chars().count() as f32).floor() as usize;
            let mut cut: String = text.chars().take(keep.saturating_sub(1)).collect();
            cut.push('…');
            cut
        } else {
            text.clone()
        };
        let pos = if right { egui::pos2(cell.right() - 2.0, rect.center().y) } else { egui::pos2(cell.left() + 2.0, rect.center().y) };
        ui.painter().text(pos, if right { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, shown, font.clone(), color);
        if header {
            ui.painter().line_segment([egui::pos2(x, rect.bottom() - 1.0), egui::pos2(x + w - 8.0, rect.bottom() - 1.0)], egui::Stroke::new(1.0, pal.border));
        }
        x += w + 2.0;
    }
}
