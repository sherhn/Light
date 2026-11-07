//! What the SQL console shows: the Results tab, a grid of the rows the last statement came back with.

use eframe::egui::{self, Align2, CornerRadius, FontId, Margin, Rect, Sense, Vec2};

use ide_core::db::{DbValue, ResultSet};

use crate::app::App;
use crate::i18n::{self, tr};

const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 30.0;

impl App {
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
