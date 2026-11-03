//! Code coverage: a run that measures it (a run command or a test framework of a module that says how) writes a report
//! into a folder made for it; when the run ends the report is read ([`ide_core::coverage`]), every file's lines get a
//! bar in the gutter - green ran, red did not - and the Coverage window lists the files with their share of lines.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};
use ide_core::coverage::{self, FileCoverage};

use crate::app::App;
use crate::editor_view::Sev;
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::widgets::{self, ROW_H};

/// A run that measures coverage, until it ends.
#[derive(Clone, Debug)]
struct Pending {
    format: String,
    report: PathBuf,
    report_dir: PathBuf,
    /// Where the run was: relative paths of the report are relative to it.
    cwd: PathBuf,
    title: String,
}

#[derive(Default)]
pub struct CoverageState {
    /// The files of the last report, by their paths on this computer.
    pub files: Vec<(PathBuf, FileCoverage)>,
    pub title: String,
    /// The bars are not drawn in the editor (the report stays in the Coverage window).
    pub hidden: bool,
    pending: HashMap<String, Pending>,
    /// Where the rows of the Coverage window were drawn (the UI tests click them).
    pub rows: Vec<(String, Rect)>,
}

/// A folder of its own for the report of a run.
fn new_report_dir() -> std::io::Result<PathBuf> {
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("light-coverage-{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl App {
    /// Make the folder for the report of a coverage run and fill `{report}` and `{report_dir}` of its command line.
    /// The run is known as `key` until it ends.
    pub fn prepare_coverage(&mut self, key: &str, format: &str, line: &str, cwd: &Path, title: &str) -> Option<String> {
        let dir = match new_report_dir() {
            Ok(d) => d,
            Err(e) => {
                self.toast(i18n::fmt("Cannot measure coverage: {}", &[&e]), Sev::Error);
                return None;
            }
        };
        let file = if format == "lcov" { "lcov.info" } else { "report.json" };
        let report = dir.join(file);
        let t = self.wsl_target();
        let line = line.replace("{report_dir}", &crate::app_run::shell_path(&dir, t.as_ref())).replace("{report}", &crate::app_run::shell_path(&report, t.as_ref()));
        self.coverage.pending.insert(key.to_string(), Pending { format: format.to_string(), report, report_dir: dir, cwd: cwd.to_path_buf(), title: title.to_string() });
        Some(line)
    }

    /// The active file with coverage: all its tests when its test framework measures coverage, or else the first run
    /// command of its language that does.
    pub fn run_with_coverage(&mut self) {
        let Some(path) = self.doc().filter(|d| d.diff.is_none()).and_then(|d| d.path.clone()) else {
            self.toast(tr("Open a file to run with coverage first"), Sev::Weak);
            return;
        };
        if self.tests_of(self.active).is_some_and(|f| !f.def.coverage.is_empty()) {
            self.run_tests_with(&path, None, true);
            return;
        }
        match self.available_module_runs().into_iter().find(|(_, d)| !d.coverage.is_empty()) {
            Some((_, def)) => self.run_language_command(&def, &path),
            None => self.toast(tr("No module that is on measures the coverage of this file"), Sev::Weak),
        }
    }

    /// A run ended: when it measured coverage its report is read.
    pub fn coverage_finished(&mut self, code: Option<i32>) {
        let Some(p) = self.coverage.pending.remove(&self.run_key) else { return };
        let text = std::fs::read_to_string(&p.report).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&p.report_dir);
        if text.trim().is_empty() {
            self.toast(if code == Some(0) { tr("The run wrote no coverage report") } else { tr("The run failed before it wrote a coverage report") }, Sev::Error);
            return;
        }
        let files = match coverage::parse(&p.format, &text) {
            Ok(f) => f,
            Err(e) => {
                self.toast(i18n::fmt("Cannot read the coverage report: {}", &[&e]), Sev::Error);
                return;
            }
        };
        let t = self.wsl_target();
        let root = self.project_root().map(Path::to_path_buf);
        let mut local: Vec<(PathBuf, FileCoverage)> = files.into_iter().filter_map(|f| {
            let path = match &t {
                Some(t) if f.path.starts_with('/') => lisp_runtime::wsl::to_windows(&f.path, t),
                _ if Path::new(&f.path).is_absolute() => PathBuf::from(&f.path),
                _ => p.cwd.join(f.path.replace('\\', "/")),
            };
            let path = std::path::absolute(&path).unwrap_or(path);
            // the files of the project only (not the headers of the system, not node_modules)
            let inside = root.as_ref().is_none_or(|r| path.starts_with(r)) && !path.components().any(|c| c.as_os_str() == "node_modules");
            (inside && path.is_file()).then_some((path, f))
        }).collect();
        local.sort_by(|a, b| a.0.cmp(&b.0));
        let (hit, all) = local.iter().fold((0, 0), |(h, a), (_, f)| { let (x, y) = f.counts(); (h + x, a + y) });
        self.coverage.files = local;
        self.coverage.title = p.title;
        self.coverage.hidden = false;
        let percent = (hit * 100).checked_div(all).unwrap_or(100);
        self.status = i18n::fmt("Coverage: {0}% of the lines ({1} of {2})", &[&percent, &hit, &all]);
        self.bottom_tool = Some(crate::state::BottomTool::Coverage);
    }

    /// The lines of document `idx` the last report knows: (0-based line, ran).
    pub fn coverage_lines(&self, idx: usize) -> Vec<(usize, bool)> {
        if self.coverage.hidden {
            return vec![];
        }
        let Some(path) = self.docs.get(idx).filter(|d| d.diff.is_none()).and_then(|d| d.path.as_ref()) else { return vec![] };
        let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        self.coverage.files.iter().find(|(p, _)| ide_core::dap::same_path(&p.to_string_lossy(), &path.to_string_lossy())).map(|(_, f)| f.lines.iter().filter(|(l, _)| **l > 0).map(|(l, c)| (l - 1, *c > 0)).collect()).unwrap_or_default()
    }

    /// The Coverage window: a row for every file with its share of lines, the lowest first stand out in red.
    pub fn draw_coverage(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.coverage.files.is_empty() {
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new(tr("No coverage yet. Run a test file or a program with coverage (Run > Run with Coverage, or the menu of a test's button).")).color(pal.text_dim));
            });
            return;
        }
        let root = self.project_root().map(Path::to_path_buf);
        let (hit, all) = self.coverage.files.iter().fold((0, 0), |(h, a), (_, f)| { let (x, y) = f.counts(); (h + x, a + y) });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new(&self.coverage.title).font(fonts::semibold(13.0)));
            let percent = (hit * 100).checked_div(all).unwrap_or(100);
            ui.label(RichText::new(i18n::fmt("{0}% of the lines, {1} of {2}", &[&percent, &hit, &all])).color(pal.text_dim).size(12.5));
        });
        ui.add_space(4.0);
        let mut open = None;
        let mut rows = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (path, f) in &self.coverage.files {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                widgets::row_background(ui, &pal, r, false, resp.hovered());
                let rel = root.as_ref().and_then(|root| path.strip_prefix(root).ok()).unwrap_or(path).to_string_lossy().replace('\\', "/");
                let ratio = f.ratio();
                let color = if ratio >= 0.8 { pal.ok } else if ratio >= 0.5 { pal.warning } else { pal.error };
                icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(r.min.x + 16.0, r.center().y - 8.0), Vec2::splat(16.0)), Icon::File, pal.text_dim);
                ui.painter().text(Pos2::new(r.min.x + 40.0, r.center().y), egui::Align2::LEFT_CENTER, &rel, fonts::ui(13.5), pal.text);
                // the bar and the numbers at the right
                let bar = Rect::from_min_size(Pos2::new(r.max.x - 330.0, r.center().y - 3.0), Vec2::new(140.0, 6.0));
                ui.painter().rect_filled(bar, egui::CornerRadius::same(3), pal.hover);
                ui.painter().rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * ratio, bar.height())), egui::CornerRadius::same(3), color);
                let (h, a) = f.counts();
                ui.painter().text(Pos2::new(bar.max.x + 14.0, r.center().y), egui::Align2::LEFT_CENTER, format!("{}%", (ratio * 100.0).round() as u32), fonts::semibold(13.0), color);
                ui.painter().text(Pos2::new(bar.max.x + 64.0, r.center().y), egui::Align2::LEFT_CENTER, i18n::fmt("{0} of {1} lines", &[&h, &a]), fonts::ui(12.5), pal.text_dim);
                rows.push((rel.clone(), r));
                if resp.on_hover_text(path.display().to_string()).clicked() {
                    open = Some(path.clone());
                }
            }
        });
        self.coverage.rows = rows;
        if let Some(p) = open {
            self.open_file(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_becomes_the_lines_of_the_open_files() {
        let dir = std::env::temp_dir().join(format!("light-cov-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.js"), "one\ntwo\nthree\n").unwrap();
        let mut app = App::new(ide_core::settings::Settings::default());
        app.open_file(&dir.join("src/a.js"));
        app.run_key = "test-cov:a".into();
        let line = app.prepare_coverage("test-cov:a", "lcov", "tool --out \"{report}\"", &dir, "Coverage: a.js").unwrap();
        let report = app.coverage.pending["test-cov:a"].report.clone();
        assert!(line.contains("lcov.info") && !line.contains("{report}"));
        std::fs::write(&report, "SF:src\\a.js\nDA:1,2\nDA:2,0\nDA:3,1\nend_of_record\nSF:node_modules/x.js\nDA:1,1\nend_of_record\nSF:/elsewhere/gone.js\nDA:1,1\nend_of_record\n").unwrap();
        app.coverage_finished(Some(0));
        assert_eq!(app.coverage.files.len(), 1, "only the files of the project that exist");
        assert_eq!(app.coverage_lines(app.active), [(0, true), (1, false), (2, true)]);
        assert!(app.status.contains("66%"), "{}", app.status);
        assert!(!report.parent().unwrap().exists(), "the report's folder goes");
        app.coverage.hidden = true;
        assert!(app.coverage_lines(app.active).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
