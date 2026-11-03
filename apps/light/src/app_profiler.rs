//! The profiler: a run command of a module that profiles (Node.js with `--cpu-prof`) writes the profile into a folder
//! made for the run; when it ends the profile is read ([`ide_core::profile`]) and the Profiler window draws it - a flame
//! graph of the calls (a click zooms into a function, a double click opens its code) and the functions that took the
//! most time.

use std::path::PathBuf;

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Vec2};
use ide_core::profile::{self, Frame, Hot, Profile};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::widgets::{self, ROW_H};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProfileView {
    #[default]
    Flame,
    Hot,
}

struct Pending {
    key: String,
    format: String,
    report: PathBuf,
    report_dir: PathBuf,
    title: String,
}

#[derive(Default)]
pub struct ProfilerState {
    pub profile: Option<Profile>,
    pub title: String,
    pub view: ProfileView,
    /// The function zoomed into: the indices of the children from the root down.
    pub zoom: Vec<usize>,
    pending: Option<Pending>,
    /// Where the boxes of the flame graph and the rows of the table were drawn (the UI tests click them): name -> rect.
    pub rows: Vec<(String, Rect)>,
}

/// A warm colour of its own for every function, so the same function looks the same everywhere in the graph.
fn flame_color(name: &str, dark: bool) -> Color32 {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619));
    let hue = 8.0 + (h % 42) as f32; // red to yellow
    let (s, l) = if dark { (0.55, 0.42 + (h >> 8 & 15) as f32 / 120.0) } else { (0.75, 0.62 + (h >> 8 & 15) as f32 / 120.0) };
    // HSL to RGB
    let c = (1.0 - (2.0 * l - 1.0f32).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if hue < 60.0 { (c, x, 0.0) } else { (x, c, 0.0) };
    Color32::from_rgb(((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8)
}

fn node_at<'a>(root: &'a Frame, path: &[usize]) -> &'a Frame {
    path.iter().fold(root, |f, i| f.children.get(*i).unwrap_or(f))
}

impl App {
    /// Make the folder for the profile of a run and fill `{report}` and `{report_dir}` of its command line.
    pub fn prepare_profile(&mut self, key: &str, format: &str, line: &str, title: &str) -> Option<String> {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("light-profile-{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.toast(i18n::fmt("Cannot profile: {}", &[&e]), Sev::Error);
            return None;
        }
        let report = dir.join(if format == "cpuprofile" { "profile.cpuprofile" } else { "profile.folded" });
        let t = self.wsl_target();
        let line = line.replace("{report_dir}", &crate::app_run::shell_path(&dir, t.as_ref())).replace("{report}", &crate::app_run::shell_path(&report, t.as_ref()));
        self.profiler.pending = Some(Pending { key: key.to_string(), format: format.to_string(), report, report_dir: dir, title: title.to_string() });
        Some(line)
    }

    /// The active file profiled with the first run command of its language that profiles.
    pub fn run_with_profiler(&mut self) {
        let Some(path) = self.doc().filter(|d| d.diff.is_none()).and_then(|d| d.path.clone()) else {
            self.toast(tr("Open a file to profile first"), Sev::Weak);
            return;
        };
        match self.available_module_runs().into_iter().find(|(_, d)| !d.profile.is_empty()) {
            Some((_, def)) => self.run_language_command(&def, &path),
            None => self.toast(tr("No module that is on profiles files of this language"), Sev::Weak),
        }
    }

    /// A run ended: when it profiled, the profile is read.
    pub fn profile_finished(&mut self, _code: Option<i32>) {
        if self.profiler.pending.as_ref().is_none_or(|p| p.key != self.run_key) {
            return;
        }
        let Some(p) = self.profiler.pending.take() else { return };
        // the profile is the report, or else the one file of its kind the tool named itself
        let file = if p.report.is_file() {
            Some(p.report.clone())
        } else {
            std::fs::read_dir(&p.report_dir).ok().and_then(|d| d.flatten().map(|e| e.path()).filter(|f| f.extension().is_some_and(|x| x == p.format.as_str())).max_by_key(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok()))
        };
        let text = file.and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&p.report_dir);
        if text.trim().is_empty() {
            self.toast(tr("The run wrote no profile"), Sev::Error);
            return;
        }
        match profile::parse(&p.format, &text) {
            Ok(prof) => {
                self.profiler.profile = Some(prof);
                self.profiler.title = p.title;
                self.profiler.zoom.clear();
                self.bottom_tool = Some(crate::state::BottomTool::Profiler);
            }
            Err(e) => self.toast(i18n::fmt("Cannot read the profile: {}", &[&e]), Sev::Error),
        }
    }

    /// The code of a function of the profile: its file at its line.
    fn open_profiled(&mut self, file: &str, line: usize) {
        if file.is_empty() || file.starts_with("node:") {
            self.toast(tr("This function is the runtime's own: there is no code of the project to show"), Sev::Weak);
            return;
        }
        let raw = file.strip_prefix("file://").unwrap_or(file);
        // file:///E:/p/x.js -> E:/p/x.js
        let raw = if raw.len() > 3 && raw.starts_with('/') && raw.as_bytes()[2] == b':' { &raw[1..] } else { raw };
        let decoded = raw.replace("%20", " ");
        let path = match self.wsl_target() {
            Some(t) if decoded.starts_with('/') => lisp_runtime::wsl::to_windows(&decoded, &t),
            _ => PathBuf::from(decoded),
        };
        if self.open_file(&path)
            && let Some(d) = self.doc_mut()
        {
            let o = d.ed.line_start(line.saturating_sub(1).min(d.ed.line_count().saturating_sub(1)));
            d.ed.set_caret(o, false);
            d.view.scroll_to_caret = true;
        }
    }

    /// The Profiler window.
    pub fn draw_profiler(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let Some(prof) = self.profiler.profile.clone() else {
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new(tr("No profile yet. Run a file with Run > Profile, or choose a run command that profiles (Node: Profile file).")).color(pal.text_dim));
            });
            return;
        };
        let total = prof.root.total.max(1);
        let ms = |samples: u64| prof.sample_ms * samples as f64;
        let mut open: Option<(String, usize)> = None;
        let mut zoom_to: Option<Vec<usize>> = None;
        let mut rows = Vec::new();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new(&self.profiler.title).font(fonts::semibold(13.0)));
            let length = if prof.sample_ms > 0.0 { i18n::fmt("{0} ms, {1} samples", &[&(ms(prof.root.total).round() as u64), &prof.root.total]) } else { i18n::fmt("{} samples", &[&prof.root.total]) };
            ui.label(RichText::new(length).color(pal.text_dim).size(12.5));
            ui.add_space(12.0);
            for (v, label) in [(ProfileView::Flame, tr("Flame Graph")), (ProfileView::Hot, tr("Hot Functions"))] {
                let active = self.profiler.view == v;
                let g = ui.painter().layout_no_wrap(label.to_string(), fonts::ui(12.5), pal.text);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(g.size().x + 18.0, 22.0), Sense::click());
                if active {
                    ui.painter().rect_filled(r, egui::CornerRadius::same(11), pal.tab_active_bg);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, egui::CornerRadius::same(11), pal.hover);
                }
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, label, fonts::ui(12.5), if active { pal.text } else { pal.text_dim });
                rows.push((format!("view:{v:?}").to_lowercase(), r));
                if resp.clicked() {
                    self.profiler.view = v;
                }
            }
            if self.profiler.view == ProfileView::Flame && !self.profiler.zoom.is_empty() {
                ui.add_space(8.0);
                let b = widgets::compact_button(ui, &pal, tr("Reset Zoom"), false);
                rows.push(("reset".into(), b.rect));
                if b.clicked() {
                    zoom_to = Some(vec![]);
                }
                // where the zoom is: the functions down to it
                let mut names = Vec::new();
                let mut f = &prof.root;
                for i in &self.profiler.zoom {
                    if let Some(c) = f.children.get(*i) {
                        names.push(c.name.clone());
                        f = c;
                    }
                }
                ui.label(RichText::new(names.join("  ›  ")).color(pal.text_dim).size(12.0));
            }
        });
        ui.add_space(4.0);
        match self.profiler.view {
            ProfileView::Flame => {
                let zoom = self.profiler.zoom.clone();
                let top = node_at(&prof.root, &zoom);
                let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width() - 16.0, ui.available_height() - 6.0), Sense::hover());
                let area = area.translate(Vec2::new(8.0, 0.0));
                let painter = ui.painter_at(area);
                let row_h = 20.0;
                let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| area.contains(*p));
                let (clicked, double) = ui.input(|i| (i.pointer.primary_clicked(), i.pointer.button_double_clicked(egui::PointerButton::Primary)));
                let mut tip: Option<String> = None;
                // the graph from the function zoomed into, its callers stacked as one bar above it
                let mut stack: Vec<(&Frame, f32, f32, usize, Vec<usize>)> = vec![(top, area.min.x, area.width(), 0, zoom.clone())];
                while let Some((f, x, w, depth, path)) = stack.pop() {
                    let y = area.min.y + depth as f32 * row_h;
                    if y > area.max.y || w < 0.5 {
                        continue;
                    }
                    let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, row_h - 1.0));
                    let hovered = hover.is_some_and(|p| r.contains(p));
                    let fill = if depth == 0 && path.is_empty() { pal.text_dim.gamma_multiply(0.35) } else { flame_color(&f.name, pal.dark) };
                    painter.rect_filled(r.shrink2(Vec2::new(0.5, 0.0)), egui::CornerRadius::same(3), if hovered { fill.gamma_multiply(1.25) } else { fill });
                    if w > 28.0 {
                        let label = if depth == 0 && path.is_empty() { tr("all").to_string() } else { f.name.clone() };
                        painter.with_clip_rect(r.shrink(3.0)).text(Pos2::new(r.min.x + 5.0, r.center().y), egui::Align2::LEFT_CENTER, label, fonts::ui(12.0), if pal.dark { Color32::from_rgb(20, 16, 12) } else { Color32::from_rgb(40, 24, 10) });
                    }
                    rows.push((format!("flame:{}", f.name), r));
                    if hovered {
                        let percent = f.total as f64 * 100.0 / total as f64;
                        let place = if f.file.is_empty() { String::new() } else { format!("\n{}:{}", f.file.rsplit('/').next().unwrap_or(&f.file), f.line) };
                        let time = if prof.sample_ms > 0.0 { format!(", {} ms", ms(f.total).round() as u64) } else { String::new() };
                        tip = Some(format!("{}{place}\n{percent:.1}% ({} {}{time}); {} {}", f.name, f.total, tr("samples"), f.own, tr("in itself")));
                        if double && !f.file.is_empty() {
                            open = Some((f.file.clone(), f.line));
                        } else if clicked && !(depth == 0 && path == zoom) {
                            zoom_to = Some(path.clone());
                        }
                    }
                    let mut cx = x;
                    for (i, c) in f.children.iter().enumerate() {
                        let cw = w * c.total as f32 / f.total.max(1) as f32;
                        let mut p = path.clone();
                        p.push(i);
                        stack.push((c, cx, cw, depth + 1, p));
                        cx += cw;
                    }
                }
                if let Some(t) = tip {
                    egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), egui::Id::unique("flame_tip"), egui::PopupAnchor::Pointer).show(|ui| {
                        ui.label(t);
                    });
                }
            }
            ProfileView::Hot => {
                let hot: Vec<Hot> = profile::hot(&prof.root);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let w = ui.available_width();
                    for (label, x) in [(tr("Function"), 0.0), (tr("Self"), w - 330.0), (tr("Total"), w - 230.0), (tr("Where"), w - 130.0)] {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(0.0, 18.0), Sense::hover());
                        ui.painter().text(Pos2::new(r.min.x + x, r.center().y), egui::Align2::LEFT_CENTER, label, fonts::semibold(12.0), pal.text_dim);
                    }
                });
                egui::ScrollArea::vertical().id_salt("hot_functions").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for h in hot.iter().take(300) {
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H - 4.0), Sense::click());
                        widgets::row_background(ui, &pal, r, false, resp.hovered());
                        let share = h.own as f32 / total as f32;
                        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(r.min.x + 14.0, r.max.y - 3.0), Vec2::new((r.width() - 360.0).max(0.0) * share, 2.0)), egui::CornerRadius::same(1), flame_color(&h.name, pal.dark));
                        ui.painter().with_clip_rect(Rect::from_min_max(r.min, Pos2::new(r.max.x - 340.0, r.max.y))).text(Pos2::new(r.min.x + 14.0, r.center().y), egui::Align2::LEFT_CENTER, &h.name, egui::FontId::monospace(12.5), pal.text);
                        let fmt = |n: u64| format!("{:.1}%", n as f64 * 100.0 / total as f64);
                        ui.painter().text(Pos2::new(r.max.x - 316.0, r.center().y), egui::Align2::LEFT_CENTER, fmt(h.own), fonts::semibold(12.5), pal.text);
                        ui.painter().text(Pos2::new(r.max.x - 216.0, r.center().y), egui::Align2::LEFT_CENTER, fmt(h.total), fonts::ui(12.5), pal.text_dim);
                        let place = if h.file.is_empty() { String::new() } else { format!("{}:{}", h.file.rsplit('/').next().unwrap_or(&h.file), h.line) };
                        ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(r.max.x - 120.0, r.min.y), r.max)).text(Pos2::new(r.max.x - 116.0, r.center().y), egui::Align2::LEFT_CENTER, place, fonts::ui(12.0), pal.text_dim);
                        rows.push((format!("hot:{}", h.name), r));
                        if resp.clicked() && !h.file.is_empty() {
                            open = Some((h.file.clone(), h.line));
                        }
                    }
                });
            }
        }
        self.profiler.rows = rows;
        if let Some(z) = zoom_to {
            self.profiler.zoom = z;
        }
        if let Some((file, line)) = open {
            self.open_profiled(&file, line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_function_has_the_same_warm_colour_everywhere() {
        let a = flame_color("parse", true);
        assert_eq!(a, flame_color("parse", true));
        assert_ne!(a, flame_color("render", true));
        assert!(a.r() >= a.g() && a.g() >= a.b(), "red to yellow: {a:?}");
        let root = Frame { children: vec![Frame { name: "a".into(), children: vec![Frame { name: "b".into(), ..Default::default() }], ..Default::default() }], ..Default::default() };
        assert_eq!(node_at(&root, &[0, 0]).name, "b");
        assert_eq!(node_at(&root, &[5]).name, "", "a path that is not there stays at the root");
    }
}
