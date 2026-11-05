//! The Tasks window: the tasks the files of the project define - the `scripts` of a package.json, the targets of a
//! makefile, whatever the modules that are on say where to look (see [`TaskDef`]) - each run with a click in a Run tab
//! of its own.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};
use ide_core::modules::TaskDef;

use crate::app::App;
use crate::app_run::{RunRequest, shell_launch, shell_path};
use crate::fonts;
use crate::i18n::{self, tr};
use crate::icons::{self, Icon};
use crate::widgets::{self, ROW_H};

/// A file of the project that holds tasks.
#[derive(Clone, Debug)]
pub struct TaskFile {
    pub path: PathBuf,
    /// The path relative to the project, `/` separated.
    pub rel: String,
    pub def: TaskDef,
    pub tasks: Vec<String>,
    modified: Option<SystemTime>,
}

#[derive(Default)]
pub struct TasksState {
    pub files: Vec<TaskFile>,
    /// What the list was made from (the project, its files, the modules); another key reads the files again.
    key: String,
    /// When the files were last asked whether they changed.
    checked_at: f64,
    /// Groups the user folded in the window (paths of their files).
    pub folded: std::collections::HashSet<PathBuf>,
    /// Where the task rows were drawn last (file relative path, task, rectangle); the UI tests click them.
    pub rows: Vec<(String, String, Rect)>,
}

/// Does `rel` (a `/` separated path) go through one of the folders `skip` names?
fn in_skipped(rel: &str, skip: &[String]) -> bool {
    rel.split('/').rev().skip(1).any(|part| skip.iter().any(|s| s.eq_ignore_ascii_case(part)))
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl App {
    /// How the enabled modules find tasks.
    pub fn task_defs(&self) -> Vec<TaskDef> {
        self.mods.installed.iter().filter(|m| m.enabled).flat_map(|m| m.manifest.tasks.iter().cloned()).collect()
    }

    /// Bring the task list up to date: when the project, its files or the modules change, and (every second, when
    /// `cheap` allows it) when one of the files holding tasks was written.
    pub fn refresh_tasks(&mut self, force: bool) {
        let defs = self.task_defs();
        let Some(project) = self.project.as_ref() else {
            self.tasks.files.clear();
            self.tasks.key.clear();
            return;
        };
        let key = format!("{}|{}|{}", project.root.display(), project.files_gen, serde_json::to_string(&defs).unwrap_or_default());
        if !force && key == self.tasks.key {
            if self.now - self.tasks.checked_at < 1.0 {
                return;
            }
            self.tasks.checked_at = self.now;
            if self.tasks.files.iter().all(|f| modified(&f.path) == f.modified) {
                return;
            }
        }
        self.tasks.key = key;
        self.tasks.checked_at = self.now;
        let mut files = Vec::new();
        for (path, rel) in project.files.iter().zip(project.files_rel.iter()) {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            for def in defs.iter().filter(|d| d.holds(&name) && !in_skipped(rel, &d.skip)) {
                let text = match self.docs.iter().find(|d| d.path.as_deref() == Some(path.as_path())) {
                    Some(doc) => doc.ed.text().to_string(),
                    None => std::fs::read_to_string(path).unwrap_or_default(),
                };
                files.push(TaskFile { path: path.clone(), rel: rel.replace('\\', "/"), def: def.clone(), tasks: def.tasks_in(&text), modified: modified(path) });
            }
        }
        // the files near the root first, then by path
        files.sort_by(|a, b| (a.rel.matches('/').count(), &a.rel, &a.def.name).cmp(&(b.rel.matches('/').count(), &b.rel, &b.def.name)));
        self.tasks.files = files;
    }

    /// Run `task` of the file `path` in a Run tab of its own, in the file's folder.
    pub fn run_task(&mut self, def: &TaskDef, path: &Path, task: &str) {
        self.save_all();
        let text=self.docs.iter().find(|doc|doc.path.as_deref()==Some(path)).map(|doc|doc.ed.text().to_string()).or_else(||std::fs::read_to_string(path).ok()).unwrap_or_default();
        if !def.tasks_in(&text).iter().any(|name|name==task) {self.toast(tr("This run target changed or its module was disabled"),crate::editor_view::Sev::Weak);return}
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let t = self.wsl_target();
        let root = self.project_root().map(Path::to_path_buf).unwrap_or_else(|| dir.clone());
        let line = def.command_line(task, &shell_path(path, t.as_ref()), &shell_path(&dir, t.as_ref()), &shell_path(&root, t.as_ref()), cfg!(windows)&&t.is_none());
        let launch = shell_launch(&line, &dir, t.as_ref(), &[]);
        let title = format!("{} {task}", def.name);
        if self.start_run(format!("task:{}:{}:{task}", def.name, path.display()), title.clone(), launch, &title, dir, t) {
            self.run_request = Some(RunRequest::Task(def.clone(), path.to_path_buf(), task.to_string()));
        }
    }

    /// The Tasks window: a group for every file with tasks, a row for every task (a click runs it).
    pub fn draw_tasks_panel(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let header = self.panel_header(ui, "Tasks");
        if self.header_action(ui, header, 0, Icon::Refresh, "Refresh") {
            self.refresh_tasks(true);
        }
        self.refresh_tasks(false);
        if self.project.is_none() || self.tasks.files.iter().all(|f| f.tasks.is_empty()) {
            ui.add_space(12.0);
            let text = if self.project.is_none() { tr("No project is open") } else { tr("No tasks: the files of the project define none that the modules that are on know of (package.json scripts, make targets).") };
            ui.horizontal_wrapped(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new(text).color(pal.text_dim));
            });
            return;
        }
        let running: Vec<String> = self.run_tabs().into_iter().filter(|(_, _, status)| *status == crate::app_run::RunStatus::Running).map(|(key, _, _)| key).collect();
        let mut run: Option<(TaskDef, PathBuf, String)> = None;
        let mut open: Option<PathBuf> = None;
        let mut fold: Option<PathBuf> = None;
        let mut rows = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for file in self.tasks.files.iter().filter(|f| !f.tasks.is_empty()) {
                let folded = self.tasks.folded.contains(&file.path);
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                widgets::row_background(ui, &pal, r, false, resp.hovered());
                let x = r.min.x + 14.0;
                icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(x, r.center().y - 6.0), Vec2::splat(12.0)), if folded { Icon::ChevronRight } else { Icon::ChevronDown }, pal.text_dim);
                icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(x + 18.0, r.center().y - 8.0), Vec2::splat(16.0)), Icon::Package, pal.builtin);
                let end = ui.painter().text(Pos2::new(x + 42.0, r.center().y), egui::Align2::LEFT_CENTER, &file.rel, fonts::semibold(13.0), pal.text).max.x;
                ui.painter().text(Pos2::new(end + 8.0, r.center().y), egui::Align2::LEFT_CENTER, &file.def.name, fonts::ui(12.0), pal.text_dim);
                let resp = resp.on_hover_text(file.path.display().to_string());
                if resp.clicked() {
                    fold = Some(file.path.clone());
                }
                if resp.double_clicked() {
                    open = Some(file.path.clone());
                }
                if folded {
                    continue;
                }
                for task in &file.tasks {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
                    widgets::row_background(ui, &pal, r, false, resp.hovered());
                    let key = format!("task:{}:{}:{task}", file.def.name, file.path.display());
                    let busy = running.contains(&key);
                    let icon = if busy { Icon::Spinner } else { Icon::Play };
                    icons::paint(ui.painter(), Rect::from_min_size(Pos2::new(x + 36.0, r.center().y - 7.0), Vec2::splat(14.0)), icon, if busy { pal.accent } else { pal.ok });
                    ui.painter().text(Pos2::new(x + 58.0, r.center().y), egui::Align2::LEFT_CENTER, task, fonts::ui(13.5), pal.text);
                    rows.push((file.rel.clone(), task.clone(), r));
                    let resp = resp.on_hover_text(i18n::fmt("Run '{}'", &[task]));
                    if resp.clicked() {
                        run = Some((file.def.clone(), file.path.clone(), task.clone()));
                    }
                }
            }
        });
        self.tasks.rows = rows;
        if let Some(path) = fold
            && !self.tasks.folded.remove(&path)
        {
            self.tasks.folded.insert(path);
        }
        if let Some(path) = open {
            self.open_file(&path);
        }
        if let Some((def, path, task)) = run {
            self.run_task(&def, &path, &task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_folders_are_any_folder_on_the_way_not_the_file() {
        let skip = vec!["node_modules".to_string()];
        assert!(in_skipped("node_modules/a/package.json", &skip));
        assert!(in_skipped("web/node_modules/b/package.json", &skip));
        assert!(!in_skipped("web/package.json", &skip));
        assert!(!in_skipped("node_modules", &skip), "a file named like the folder is not in it");
    }

    #[test]
    fn the_tasks_of_the_project_files_are_listed_and_follow_edits() {
        let dir = std::env::temp_dir().join(format!("light-tasks-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join("web/node_modules/dep")).unwrap();
        std::fs::write(dir.join("web/package.json"), r#"{"scripts":{"dev":"vite","build":"vite build"}}"#).unwrap();
        std::fs::write(dir.join("web/node_modules/dep/package.json"), r#"{"scripts":{"postinstall":"x"}}"#).unwrap();
        let mut app = App::new(ide_core::settings::Settings::default());
        let files = vec![dir.join("web/package.json"), dir.join("web/node_modules/dep/package.json")];
        let files_rel = files.iter().map(|f| ide_core::workspace::relative(&dir, f)).collect();
        app.project = Some(crate::state::Project { root: dir.clone(), name: "tasks".into(), tree_ver: 0, tree_cache: None, ignore: ide_core::workspace::IgnoreMatcher::new(&dir), expanded: Default::default(), selected: None, dir_cache: Default::default(), files, files_rel, files_gen: 1, index: Default::default(), index_rx: None, branch: None, hidden: Default::default() });
        let def = TaskDef { name: "npm".into(), files: vec!["package.json".into()], json: "scripts".into(), command: "npm run {task}".into(), skip: vec!["node_modules".into()], ..Default::default() };
        let manifest = ide_core::modules::Manifest { id: "javascript".into(), tasks: vec![def], ..Default::default() };
        app.mods.installed.push(ide_core::modules::Installed { manifest, dir: PathBuf::new(), enabled: true });
        app.refresh_tasks(true);
        let listed: Vec<(String, Vec<String>)> = app.tasks.files.iter().map(|f| (f.rel.clone(), f.tasks.clone())).collect();
        assert_eq!(listed, [("web/package.json".to_string(), vec!["dev".to_string(), "build".to_string()])]);
        // an open, edited file is read from the editor
        app.open_file(&dir.join("web/package.json"));
        app.docs[app.active].ed.select_all();
        app.docs[app.active].ed.insert_text_raw(r#"{"scripts":{"test":"vitest"}}"#);
        app.refresh_tasks(true);
        assert_eq!(app.tasks.files[0].tasks, ["test"]);
        // a module switched off takes its tasks along
        app.mods.installed.clear();
        app.refresh_tasks(false);
        assert!(app.tasks.files.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
