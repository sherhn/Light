//! Application state and behaviour (no drawing code here).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;
use ide_core::commands::{self, Keymap};
use ide_core::index::{self, ProjectIndex};
use ide_core::search::{self, TextQuery};
use ide_core::settings::Settings;
use ide_core::workspace::{self, IgnoreMatcher};
use lisp_runtime::compiler;
use lisp_runtime::{Implementation, ReplEvent, ReplSession, RunEvent, runner};
use lisp_syntax::outline::outline;
use lisp_syntax::{TokenKind, symbols};

use crate::doc::Doc;
use crate::i18n::{self, tr};
use crate::editor_view::{Marker, Sev};
use crate::state::*;
use crate::theme::Palette;

pub use crate::app_run::RunConfig;

/// Seconds a file has to stay untouched before it is saved automatically.
pub const AUTOSAVE_DELAY: f64 = 1.0;

/// Read the project's files and their Lisp definitions in the background, leaving out what it hides.
fn read_project_files(root: &Path, hidden: &workspace::Hidden) -> mpsc::Receiver<(ProjectIndex, Vec<PathBuf>)> {
    let (tx, rx) = mpsc::channel();
    let (root, hidden) = (root.to_path_buf(), hidden.clone());
    std::thread::spawn(move || {
        let files = workspace::list_project_files(&root, &hidden, 50_000);
        let idx = ProjectIndex::build_from(files.iter().cloned());
        let _ = tx.send((idx, files));
    });
    rx
}

pub struct App {
    pub settings: Settings,
    pub pal: Palette,
    pub keymap: Keymap,
    pub project: Option<Project>,
    pub project_filter: String,
    pub docs: Vec<Doc>,
    pub active: usize,
    pub split: Option<Split>,
    /// Screen rectangles of the two panes of a split, first pane first (for the UI tests).
    pub pane_rects: Vec<egui::Rect>,
    pub recent_files: Vec<PathBuf>,
    pub left_tool: Option<LeftTool>,
    pub bottom_tool: Option<BottomTool>,
    pub repl: ReplState,
    pub run: RunState,
    pub run_key: String,
    pub run_request: Option<crate::app_run::RunRequest>,
    pub other_runs: Vec<crate::app_run::RunTab>,
    pub compile: Option<CompileJob>,
    pub compiler_problems: Vec<ProblemRow>,
    pub find_results: FindResults,
    pub popup: Popup,
    pub find: FindBar,
    pub completion: Completion,
    pub toasts: Vec<Toast>,
    pub hint: Option<(String, f64)>,
    /// The documentation popup of the name the pointer rests on.
    pub hover: crate::hover::HoverState,
    /// The syntax checks of the languages of modules, running in the background.
    pub checks: crate::app_check::CheckState,
    /// UI tests: the pointer rests on this offset of the active document, wherever the real pointer is.
    pub fake_hover: Option<usize>,
    pub status: String,
    pub editor_focus: bool,
    pub nav_back: Vec<(PathBuf, usize)>,
    pub nav_fwd: Vec<(PathBuf, usize)>,
    pub lisp: Option<Implementation>,
    pub untitled_counter: usize,
    pub last_edit: Option<(PathBuf, usize)>,
    pub last_title: String,
    pub now: f64,
    pub last_disk_check: f64,
    pub window_focused: bool,
    pub exit_now: bool,
    pub editor_rect: Option<egui::Rect>,
    pub caret_screen: Option<egui::Pos2>,
    /// The gutter of the active editor (screen coordinates) and the open old-code popup, if any.
    pub gutter_rect: Option<egui::Rect>,
    pub hunk_popup: Option<HunkPopup>,
    /// Screen rectangles of the file rows of the Commit window (path, row, checkbox), for the UI tests.
    pub commit_rects: Vec<(String, egui::Rect, egui::Rect)>,
    pub pending_close_others: bool,
    pub git: GitState,
    pub logo: Option<eframe::egui::TextureHandle>,
    pub term: crate::app_terminal::TerminalState,
    /// The Linux (WSL) settings page: what is installed and what is being set up.
    pub wsl: crate::app_wsl::WslState,
    /// The GitHub session of the "Get from Version Control" dialog.
    pub github: crate::app_github::GithubState,
    pub run_config: crate::app_run::RunConfig,
    /// The scenario that is running (its other commands are still to start).
    pub scenario: Option<crate::app_scenario::ScenarioRun>,
    /// Where the open project is built and run (its settings in the IDE profile).
    pub project_cfg: crate::app_env::ProjectConfig,
    /// The list of hidden folders as it is being typed on the Project page of the settings (one per line).
    pub hidden_draft: Option<String>,
    /// Where the files of the project were left, the latest first (kept in its session).
    pub file_positions: Vec<(PathBuf, ide_core::settings::FilePos)>,
    /// Ask whether the open (Unix-style) project should be built in Linux.
    pub env_banner: bool,
    /// The open project has a makefile, a configure script or shell scripts at its top.
    pub unix_project: bool,
    /// Modules that are on and whose languages the open project has files of (they bring tools for Linux).
    pub project_modules: Vec<String>,
    /// The question about Linux was closed with "Not now" (until the project is opened again).
    pub env_banner_dismissed: bool,
    /// The "build tools are missing" banner was closed (until the project is opened again or a command is not found).
    pub tools_banner_dismissed: bool,
    /// The modules (languages and their tools): what is installed, what the registry offers.
    pub mods: crate::modules::ModulesState,
    /// Modules the open project asks for or looks like it needs that are not on yet (the banner offers them).
    pub needed_modules: Vec<String>,
    /// The "this project needs modules" banner was closed.
    pub modules_banner_dismissed: bool,
    /// Targets of the project's makefile, offered as run configurations.
    pub make_targets: Vec<String>,
    /// The running/last make: where it ran, to find the files of its compiler messages.
    pub make_run: Option<crate::app_run::MakeRun>,
    pub script: Option<crate::script::Script>,
    /// Longest frame seen so far (ms) and the worst offender's frame number, for freeze diagnostics.
    pub max_frame_ms: f32,
    pub find_rx: Option<mpsc::Receiver<(String, Vec<search::FileHit>)>>,
    pub frame_no: u64,
    /// Double-Shift detection: (shift currently down, another key was used meanwhile, time of the last clean tap).
    pub shift_tap: (bool, bool, f64),
}

pub fn lisp_dialect_of(p: &Path) -> lisp_syntax::Dialect {
    lisp_syntax::Dialect::from_path(p).unwrap_or_default()
}

impl App {
    pub fn new(settings: Settings) -> App {
        let pal = Palette::for_settings(&settings);
        crate::i18n::set(settings.language);
        let lisp = crate::app_wsl::resolve_lisp(&settings);
        let mut app = App {
            pal,
            keymap: Keymap::default_keymap(),
            project: None,
            docs: vec![],
            active: 0,
            split: None,
            pane_rects: vec![],
            recent_files: vec![],
            left_tool: if settings.show_project_panel { Some(LeftTool::Project) } else { None },
            bottom_tool: None,
            repl: ReplState { package: "COMMON-LISP-USER".into(), stick_to_bottom: true, ..Default::default() },
            run: RunState::default(),
            run_key: String::new(),
            run_request: None,
            other_runs: Vec::new(),
            compile: None,
            compiler_problems: vec![],
            find_results: FindResults::default(),
            project_filter: String::new(),
            popup: Popup::None,
            find: FindBar::default(),
            completion: Completion::default(),
            toasts: vec![],
            hint: None,
            hover: Default::default(),
            checks: Default::default(),
            fake_hover: None,
            status: String::new(),
            editor_focus: true,
            nav_back: vec![],
            nav_fwd: vec![],
            lisp,
            untitled_counter: 0,
            last_edit: None,
            last_title: String::new(),
            now: 0.0,
            last_disk_check: 0.0,
            window_focused: true,
            exit_now: false,
            editor_rect: None,
            caret_screen: None,
            gutter_rect: None,
            hunk_popup: None,
            commit_rects: vec![],
            pending_close_others: false,
            git: GitState::default(),
            logo: None,
            term: Default::default(),
            wsl: Default::default(),
            github: Default::default(),
            run_config: Default::default(),
            scenario: None,
            project_cfg: Default::default(),
            hidden_draft: None,
            file_positions: vec![],
            env_banner: false,
            unix_project: false,
            project_modules: vec![],
            env_banner_dismissed: false,
            tools_banner_dismissed: false,
            mods: Default::default(),
            needed_modules: vec![],
            modules_banner_dismissed: false,
            make_targets: vec![],
            make_run: None,
            script: None,
            max_frame_ms: 0.0,
            find_rx: None,
            frame_no: 0,
            shift_tap: (false, false, -10.0),
            settings,
        };
        if !cfg!(test) { app.modules_init(); }
        app
    }

    // ------------------------------------------------------------------ small helpers

    pub fn toast(&mut self, text: impl Into<String>, sev: Sev) {
        let text = text.into();
        self.toasts.push(Toast { text, sev, until: self.now + 4.5 });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn doc(&self) -> Option<&Doc> {
        self.docs.get(self.active)
    }

    pub fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.docs.get_mut(self.active)
    }

    pub fn project_root(&self) -> Option<&Path> {
        self.project.as_ref().map(|p| p.root.as_path())
    }

    pub fn theme_changed(&mut self) {
        self.pal = Palette::for_settings(&self.settings);
    }

    pub fn save_settings(&mut self) {
        self.settings.open_files = self.docs.iter().filter_map(|d| d.path.as_ref().map(|p| p.to_string_lossy().into_owned())).collect();
        self.settings.active_file = self.doc().and_then(|d| d.path.as_ref()).map(|p| p.to_string_lossy().into_owned());
        for i in 0..self.docs.len() {
            self.remember_position(i);
        }
        if let Some(project) = &self.project {
            let positions = self.file_positions.iter().filter(|(p, _)| p.starts_with(&project.root)).map(|(p, pos)| (p.to_string_lossy().into_owned(), *pos)).collect();
            self.settings.project_sessions.insert(project.root.to_string_lossy().into_owned(), ide_core::settings::ProjectSession {
                files: self.settings.open_files.clone(), active: self.settings.active_file.clone(),
                expanded: project.expanded.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                positions,
            });
        }
        self.settings.show_project_panel = self.left_tool.is_some();
        if !cfg!(test) && let Err(e) = self.settings.save() {
            eprintln!("could not save settings: {e}");
        }
    }

    // ------------------------------------------------------------------ project

    /// Open a project the way the user asked for it: with another project already open, ask whether it goes into
    /// this window or a new one (as JetBrains IDEs do).
    pub fn request_open_project(&mut self, path: &Path) {
        if self.project.as_ref().is_some_and(|p| p.root != path) {
            self.popup = Popup::OpenProject { path: path.to_path_buf() };
        } else {
            self.open_project(path);
        }
    }

    /// Start another Light process for `path`.
    pub fn open_project_in_new_window(&mut self, path: &Path) {
        let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg(path).spawn());
        match spawned {
            Ok(_) => self.toast(i18n::fmt("Opened {} in a new window", &[&path.display()]), Sev::Weak),
            Err(e) => self.toast(i18n::fmt("Cannot start a new window: {}", &[&e]), Sev::Error),
        }
    }

    pub fn open_project(&mut self, path: &Path) {
        if self.project.as_ref().is_some_and(|p| p.root != path) {
            self.flush_saves();
            if self.has_unsaved() {
                self.popup = Popup::Confirm { kind: ConfirmKind::OpenProject(path.to_path_buf()), message: tr("Save changes before opening another project?").into() };
                return;
            }
        }
        self.finish_open_project(path);
    }

    pub(crate) fn finish_open_project(&mut self, path: &Path) {
        if !path.is_dir() {
            self.toast(i18n::fmt("{} is not a folder", &[&path.display()]), Sev::Error);
            return;
        }
        if self.project.is_some() { self.save_settings(); }
        let root = path.to_path_buf();
        let session = self.settings.project_sessions.get(&root.to_string_lossy().into_owned()).cloned();
        self.project_filter.clear();
        // another project in this window: nothing of the old one stays (tabs, search results, history, problems...)
        if self.project.as_ref().is_some_and(|p| p.root != root) {
            self.leave_project();
        }
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.to_string_lossy().into_owned());
        let hidden = workspace::Hidden::new(&root, &crate::app_env::load_config(&root).hidden);
        let rx = read_project_files(&root, &hidden);
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        if let Some(session) = &session {
            expanded = session.expanded.iter().map(PathBuf::from).filter(|p| p.starts_with(&root) && p.is_dir()).collect();
        }
        let branch = workspace::git_branch(&root);
        self.project = Some(Project { tree_ver: 0, tree_cache: None, ignore: IgnoreMatcher::new(&root), root: root.clone(), name, expanded, selected: None, dir_cache: HashMap::new(), files: vec![], files_rel: vec![], files_gen: 0, index: ProjectIndex::default(), index_rx: Some(rx), branch, hidden });
        self.settings.push_recent_project(&root.to_string_lossy());
        self.left_tool = Some(LeftTool::Project);
        self.compiler_problems.clear();
        self.git_attach();
        self.load_project_env();
        // restart the REPL in the new working directory (and environment) on next use
        if self.repl.session.is_some() {
            self.repl_restart();
        }
        self.file_positions = session.as_ref().map(|s| s.positions.iter().map(|(p, pos)| (PathBuf::from(p), *pos)).collect()).unwrap_or_default();
        if let Some(session) = session {
            for file in session.files { let path = PathBuf::from(file); if path.is_file() { self.open_file(&path); } }
            if let Some(active) = session.active { let path = PathBuf::from(active); if path.is_file() { self.open_file(&path); } }
        }
        self.save_settings();
    }

    /// Leave the open project for another: its files are saved and everything that belongs to it goes - the tabs, the
    /// search results, the back/forward history, problems, what ran and the terminal.
    fn leave_project(&mut self) {
        self.cancel_scenario();
        if let Some(h) = self.run.handle.take() {
            h.kill();
        }
        self.stop_other_runs();
        self.run_key.clear();
        self.run_request = None;
        self.run = RunState::default();
        if let Some(c) = self.compile.take() {
            c.handle.kill();
            let _ = std::fs::remove_file(&c.fasl);
        }
        self.docs.clear();
        self.active = 0;
        self.split = None;
        self.pane_rects.clear();
        self.recent_files.clear();
        self.nav_back.clear();
        self.nav_fwd.clear();
        self.last_edit = None;
        self.find_results = FindResults::default();
        self.find.matches.clear();
        self.find.current = None;
        self.find.built_for = None;
        self.compiler_problems.clear();
        self.make_run = None;
        self.make_targets.clear();
        self.run_config = RunConfig::CurrentFile;
        self.completion.active = false;
        self.hunk_popup = None;
        self.hint = None;
        self.hide_hover();
        self.needed_modules.clear();
        self.modules_banner_dismissed = false;
        self.tools_banner_dismissed = false;
        // the terminal starts again in the new project's folder
        self.terminal_stop();
        self.term.log.clear();
        self.term.cwd = PathBuf::from(".");
        self.editor_focus = false;
    }

    pub fn close_project(&mut self) {
        self.flush_saves();
        if self.has_unsaved() {
            self.popup = Popup::Confirm { kind: ConfirmKind::CloseProject, message: tr("Save changes before closing the project?").into() };
            return;
        }
        self.finish_close_project();
    }

    pub(crate) fn finish_close_project(&mut self) {
        self.save_settings();
        self.leave_project();
        self.project = None;
        self.load_project_env();
        self.docs.clear();
        self.active = 0;
        self.compiler_problems.clear();
        self.git = GitState::default();
    }

    pub fn poll_index(&mut self) {
        if let Some(p) = self.project.as_mut()
            && let Some(rx) = &p.index_rx
            && let Ok((idx, files)) = rx.try_recv()
        {
            p.index = idx;
            p.files_rel = files.iter().map(|f| workspace::relative(&p.root, f)).collect();
            p.files = files;
            p.files_gen += 1;
            p.index_rx = None;
            p.tree_ver += 1;
            self.refresh_needed_modules();
        }
    }

    pub fn dir_entries(&mut self, dir: &Path) -> Vec<workspace::Entry> {
        let Some(p) = self.project.as_mut() else { return vec![] };
        if let Some(v) = p.dir_cache.get(dir) {
            return v.clone();
        }
        let mut v = workspace::list_dir(dir).unwrap_or_default();
        v.retain(|e| !p.hidden.is_hidden(&e.path, e.is_dir));
        p.dir_cache.insert(dir.to_path_buf(), v.clone());
        v
    }

    pub fn refresh_project(&mut self) {
        let Some(root) = self.project.as_ref().map(|p| p.root.clone()) else { return };
        if let Some(p) = self.project.as_mut() {
            p.dir_cache.clear();
            p.tree_ver += 1;
            p.ignore.reset();
            p.branch = workspace::git_branch(&root);
            if p.index_rx.is_none() {
                p.index_rx = Some(read_project_files(&root, &p.hidden));
            }
        }
    }

    /// The project's hidden folders changed: keep them, and read the tree, the file list and the indexes again.
    pub fn set_hidden_folders(&mut self, patterns: Vec<String>) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else { return };
        self.project_cfg.hidden = patterns;
        if let Err(e) = crate::app_env::save_config(&root, &self.project_cfg) {
            self.toast(i18n::fmt("Cannot save the project settings: {}", &[&e]), Sev::Error);
        }
        if let Some(p) = self.project.as_mut() {
            p.hidden = workspace::Hidden::new(&root, &self.project_cfg.hidden);
            // a list that is being read was read with the old rules
            p.index_rx = None;
        }
        self.refresh_project();
    }

    pub fn reindex_open_docs(&mut self) {
        if let Some(p) = self.project.as_mut() {
            for d in &self.docs {
                if let Some(path) = &d.path {
                    p.index.update_file(path, d.ed.text(), d.ed.dialect());
                }
            }
        }
    }

    // ------------------------------------------------------------------ documents

    pub fn open_file(&mut self, path: &Path) -> bool {
        if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(path)) {
            self.show_doc(i);
            self.editor_focus = true;
            return true;
        }
        match std::fs::metadata(path) {
            Ok(m) if m.len() > 8 * 1024 * 1024 => {
                self.toast(i18n::fmt("{0} is too large to open ({1} MB)", &[&path.display(), &(m.len() / 1_048_576)]), Sev::Error);
                return false;
            }
            _ => {}
        }
        match Doc::open(path) {
            Ok(mut d) => {
                self.assign_language(&mut d);
                if let Some(p) = self.project.as_mut() {
                    p.index.update_file(path, d.ed.text(), d.ed.dialect());
                }
                self.docs.push(d);
                self.active = self.docs.len() - 1;
                self.restore_position(self.active);
                // a file that was just opened is checked too
                self.schedule_check(self.active);
                self.editor_focus = true;
                self.recent_files.retain(|p| p != path);
                self.recent_files.insert(0, path.to_path_buf());
                self.recent_files.truncate(30);
                true
            }
            Err(e) => {
                self.toast(i18n::fmt("Cannot open {0}: {1}", &[&path.display(), &e]), Sev::Error);
                false
            }
        }
    }

    /// Open a file and put the caret at `offset` (a byte offset).
    pub fn goto(&mut self, path: &Path, offset: usize, record_nav: bool) {
        if record_nav {
            self.push_nav();
        }
        if !self.open_file(path) {
            return;
        }
        if let Some(d) = self.doc_mut() {
            let o = offset.min(d.ed.text().len());
            let o = (0..=o).rev().find(|&i| d.ed.text().is_char_boundary(i)).unwrap_or(0);
            d.ed.set_caret(o, false);
            d.view.scroll_to_caret = true;
            d.view.last_input_time = 0.0;
        }
        self.editor_focus = true;
    }

    pub fn goto_range(&mut self, path: &Path, start: usize, end: usize, record_nav: bool) {
        self.goto(path, start, record_nav);
        if let Some(d) = self.doc_mut() {
            let n = d.ed.text().len();
            d.ed.select_range(start.min(n), end.min(n));
        }
    }

    pub fn push_nav(&mut self) {
        if let Some(d) = self.doc()
            && let Some(p) = &d.path
        {
            let entry = (p.clone(), d.ed.caret());
            if self.nav_back.last() != Some(&entry) {
                self.nav_back.push(entry);
                if self.nav_back.len() > 100 {
                    self.nav_back.remove(0);
                }
            }
            self.nav_fwd.clear();
        }
    }

    pub fn navigate_back(&mut self) {
        let Some((p, o)) = self.nav_back.pop() else { return };
        if let Some(d) = self.doc()
            && let Some(cur) = &d.path
        {
            self.nav_fwd.push((cur.clone(), d.ed.caret()));
        }
        self.goto(&p, o, false);
    }

    pub fn navigate_forward(&mut self) {
        let Some((p, o)) = self.nav_fwd.pop() else { return };
        if let Some(d) = self.doc()
            && let Some(cur) = &d.path
        {
            self.nav_back.push((cur.clone(), d.ed.caret()));
        }
        self.goto(&p, o, false);
    }

    /// A new scratch file. It lives in the settings folder and is saved automatically like any other file, so
    /// there is never an unsaved document to lose; an empty scratch file is deleted when its tab is closed.
    pub fn new_untitled(&mut self) {
        if let Some(dir) = Settings::config_dir().map(|d| d.join("scratches"))
            && std::fs::create_dir_all(&dir).is_ok()
        {
            loop {
                self.untitled_counter += 1;
                let path = dir.join(format!("scratch-{}.txt", self.untitled_counter));
                if !path.exists() {
                    if std::fs::write(&path, "").is_ok() {
                        self.open_file(&path);
                        return;
                    }
                    break;
                }
            }
        }
        self.untitled_counter += 1;
        let mut d = Doc::untitled(self.untitled_counter);
        self.assign_language(&mut d);
        self.docs.push(d);
        self.active = self.docs.len() - 1;
        self.editor_focus = true;
    }

    /// Save one document; untitled documents ask for a location.
    pub fn save_doc(&mut self, idx: usize) -> bool {
        let Some(d) = self.docs.get_mut(idx) else { return false };
        if d.path.is_none() {
            let start = self.project.as_ref().map(|p| p.root.clone());
            let mut dlg = rfd::FileDialog::new().set_file_name(d.name());
            if let Some(s) = start {
                dlg = dlg.set_directory(s);
            }
            match dlg.save_file() {
                Some(path) => {
                    if let Err(e) = d.save_as(&path) {
                        let msg = i18n::fmt("Cannot save: {}", &[&e]);
                        self.toast(msg, Sev::Error);
                        return false;
                    }
                }
                None => return false,
            }
        } else if let Err(e) = d.save() {
            let msg = i18n::fmt("Cannot save {0}: {1}", &[&d.name(), &e]);
            self.toast(msg, Sev::Error);
            return false;
        }
        let (path, text, dialect) = {
            let d = &self.docs[idx];
            (d.path.clone(), d.ed.text().to_string(), d.ed.dialect())
        };
        if let (Some(p), Some(proj)) = (path.as_ref(), self.project.as_mut()) {
            proj.index.update_file(p, &text, dialect);
            // a save may create or change files shown in the tree
            proj.dir_cache.clear();
            proj.tree_ver += 1;
            // a file saved for the first time joins the project's list (Go to File, the definitions of other languages)
            if p.starts_with(&proj.root) && !proj.files.contains(p) && !proj.hidden.is_hidden(p, false) {
                proj.files_rel.push(workspace::relative(&proj.root, p));
                proj.files.push(p.clone());
            }
        }
        if let Some(p) = path.as_ref() {
            self.lang_index_saved(p, &text);
        }
        self.git.last_refresh = -100.0;
        // the language's own check looks at what was just written
        self.schedule_check(idx);
        if path.as_ref().and_then(|p| p.file_name()).is_some_and(|n| { let n = n.to_string_lossy().to_ascii_lowercase(); n == "makefile" || n == "gnumakefile" || n.ends_with(".mk") }) {
            self.refresh_make_targets();
        }
        true
    }

    pub fn save_all(&mut self) {
        let mut saved = 0;
        for i in 0..self.docs.len() {
            if self.docs[i].ed.is_modified() && self.docs[i].path.is_some() && self.save_doc(i) {
                saved += 1;
            }
        }
        if saved > 0 {
            self.status = i18n::fmt("Saved {} file(s)", &[&saved]);
        }
        // untitled documents are saved only by an explicit Save
        if let Some(d) = self.doc()
            && d.path.is_none()
            && d.diff.is_none()
        {
            let a = self.active;
            self.save_doc(a);
        }
    }

    /// Bring document `idx` to the front.
    pub fn show_doc(&mut self, idx: usize) {
        if idx >= self.docs.len() {
            return;
        }
        // the tab of the other pane's document moves the focus there instead of showing it twice
        if idx != self.active && self.split.as_ref().is_some_and(|s| s.other == self.docs[idx].id) {
            self.focus_other_pane();
            return;
        }
        self.active = idx;
    }

    /// Changes still in memory, including files whose automatic save failed.
    pub fn has_unsaved(&self) -> bool {
        self.docs.iter().any(|d| d.diff.is_none() && d.ed.is_modified())
    }

    /// An explicit Save All also saves conflicts and asks where to put untitled documents.
    pub fn save_unsaved_docs(&mut self) -> bool {
        for i in 0..self.docs.len() {
            if self.docs[i].diff.is_none() && self.docs[i].ed.is_modified() && !self.save_doc(i) {
                return false;
            }
        }
        !self.has_unsaved()
    }

    /// Save every modified file right now, silently. Returns how many were written.
    pub fn flush_saves(&mut self) -> usize {
        let mut saved = 0;
        for i in 0..self.docs.len() {
            if self.docs[i].diff.is_none() && self.docs[i].ed.is_modified() {
                self.docs[i].sync_with_disk();
            }
            let d = &self.docs[i];
            if d.diff.is_none() && d.path.is_some() && !d.conflict && d.ed.is_modified() && self.save_doc(i) {
                saved += 1;
            }
        }
        saved
    }

    /// Autosave: a file is written once it has been left alone for a moment.
    fn autosave_tick(&mut self) {
        let now = self.now;
        let mut due = vec![];
        for (i, d) in self.docs.iter_mut().enumerate() {
            let rev = d.ed.revision();
            if rev != d.autosave_rev {
                d.autosave_rev = rev;
                d.autosave_at = now;
            } else if d.diff.is_none() && d.path.is_some() && !d.conflict && d.ed.is_modified() && now - d.autosave_at >= AUTOSAVE_DELAY {
                due.push(i);
            }
        }
        for i in due {
            self.docs[i].sync_with_disk();
            if !self.docs[i].conflict && self.docs[i].ed.is_modified() { self.save_doc(i); }
        }
    }

    /// Remember where `doc` is (its caret and the line at the top), for when its file is opened again.
    fn remember_position(&mut self, idx: usize) {
        let Some(d) = self.docs.get(idx).filter(|d| d.diff.is_none()) else { return };
        let Some(p) = d.path.clone() else { return };
        // a tab that was never shown has not been scrolled yet: it is still where it was put
        let pos = ide_core::settings::FilePos { caret: d.ed.caret(), top_line: d.view.scroll_to_line_f.unwrap_or(d.view.top_line_f) };
        self.file_positions.retain(|(q, _)| *q != p);
        self.file_positions.insert(0, (p, pos));
        self.file_positions.truncate(ide_core::settings::MAX_POSITIONS);
    }

    /// Put a file that was just opened back where it was left.
    fn restore_position(&mut self, idx: usize) {
        let Some(d) = self.docs.get_mut(idx) else { return };
        let Some(path) = d.path.as_ref() else { return };
        let Some((_, pos)) = self.file_positions.iter().find(|(p, _)| p == path) else { return };
        let text = d.ed.text();
        let o = pos.caret.min(text.len());
        let o = (0..=o).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        d.ed.set_caret(o, false);
        d.view.scroll_to_line_f = Some(pos.top_line.max(0.0));
    }

    pub fn close_doc(&mut self, idx: usize) {
        if idx >= self.docs.len() {
            return;
        }
        self.remember_position(idx);
        let doc = self.docs.remove(idx);
        // an empty scratch file is not worth keeping
        if let (Some(p), Some(dir)) = (doc.path.as_ref(), Settings::config_dir().map(|d| d.join("scratches")))
            && p.starts_with(&dir)
            && doc.ed.text().trim().is_empty()
            && !doc.ed.is_modified()
        {
            let _ = std::fs::remove_file(p);
        }
        if self.active >= self.docs.len() {
            self.active = self.docs.len().saturating_sub(1);
        } else if idx < self.active {
            self.active -= 1;
        }
        self.find.built_for = None;
        self.completion.active = false;
    }

    pub fn request_close_doc(&mut self, idx: usize) {
        if let Some(doc) = self.docs.get_mut(idx) && doc.diff.is_none() && doc.ed.is_modified() {
            doc.sync_with_disk();
        }
        match self.docs.get(idx) {
            Some(d) if d.diff.is_none() && d.ed.is_modified() && (d.path.is_none() || d.conflict) => {
                self.popup = Popup::Confirm { kind: ConfirmKind::CloseDoc(idx), message: i18n::fmt("Save changes to {} before closing?", &[&d.name()]) };
            }
            Some(d) if d.diff.is_none() && d.ed.is_modified() => {
                if self.save_doc(idx) {
                    self.close_doc(idx);
                }
            }
            Some(_) => self.close_doc(idx),
            None => {}
        }
    }

    // ------------------------------------------------------------------ periodic work

    pub fn tick(&mut self, ctx: &egui::Context) {
        self.now = ctx.input(|i| i.time);
        self.frame_no += 1;
        let dt = ctx.input(|i| i.unstable_dt) * 1000.0;
        if self.frame_no > 5 && dt > self.max_frame_ms && dt < 120_000.0 {
            self.max_frame_ms = dt;
        }
        self.toasts.retain(|t| t.until > self.now);
        if self.hint.as_ref().is_some_and(|h| h.1 < self.now) {
            self.hint = None;
        }
        self.poll_index();
        self.poll_repl();
        self.poll_run();
        self.poll_compile();
        if let Some(rx) = &self.find_rx
            && let Ok((title, hits)) = rx.try_recv()
        {
            self.find_results = FindResults { title, hits };
            self.find_rx = None;
        }
        self.git_poll();
        self.poll_terminal();
        self.poll_wsl();
        self.poll_modules(ctx);
        self.poll_checks();
        self.poll_github();
        self.git_update_bases();

        // external file changes (once a second)
        if self.now - self.last_disk_check > 1.0 {
            self.last_disk_check = self.now;
            let mut reloaded = vec![];
            for d in self.docs.iter_mut() {
                if d.sync_with_disk() {
                    reloaded.push(d.name());
                }
            }
            for n in reloaded {
                self.toast(i18n::fmt("{} reloaded from disk", &[&n]), Sev::Weak);
            }
        }
        // autosave: shortly after the last edit, and at once when the window loses focus
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if self.window_focused && !focused {
            self.flush_saves();
        }
        self.autosave_tick();
        // coming back to the window: the file system may have changed meanwhile
        if focused && !self.window_focused
            && let Some(p) = self.project.as_mut()
        {
            p.dir_cache.clear();
            p.tree_ver += 1;
            p.ignore.reset();
        }
        self.window_focused = focused;
    }

    // ------------------------------------------------------------------ REPL

    pub fn ensure_repl(&mut self) -> bool {
        if self.repl.session.as_ref().is_some_and(|s| s.is_alive()) {
            return true;
        }
        if !self.lisp_enabled() {
            return false;
        }
        self.repl.session = None;
        if self.lisp.is_none() {
            self.lisp = self.resolve_lisp();
        }
        let Some(imp) = self.lisp.clone() else {
            self.repl.log.push(LogLine { kind: LogKind::Error, text: tr("No Lisp implementation found. Install SBCL (https://www.sbcl.org) or set its path in Settings > Lisp.").into() });
            self.toast(tr("SBCL not found - set the path in Settings"), Sev::Error);
            return false;
        };
        let cwd = self.project.as_ref().map(|p| p.root.clone());
        match ReplSession::start(&imp, cwd.as_deref()) {
            Ok(s) => {
                self.repl.session = Some(s);
                self.repl.starting = true;
                self.repl.ready = false;
                self.repl.busy.clear();
                self.repl.pending.clear();
                self.repl.log.push(LogLine { kind: LogKind::Info, text: i18n::fmt("Starting {} ...", &[&imp.name]) });
                true
            }
            Err(e) => {
                self.repl.log.push(LogLine { kind: LogKind::Error, text: i18n::fmt("Cannot start {0}: {1}", &[&imp.path.display(), &e]) });
                self.toast(i18n::fmt("Cannot start Lisp: {}", &[&e]), Sev::Error);
                false
            }
        }
    }

    pub fn repl_restart(&mut self) {
        if let Some(s) = self.repl.session.take() {
            s.kill();
        }
        self.repl.ready = false;
        self.repl.starting = false;
        self.repl.busy.clear();
        self.repl.pending.clear();
        self.repl.log.push(LogLine { kind: LogKind::Info, text: tr("REPL restarted").into() });
        self.ensure_repl();
    }

    pub fn repl_eval(&mut self, code: &str, echo: bool) {
        if code.trim().is_empty() || !self.ensure_repl() {
            return;
        }
        if echo {
            self.repl.log.push(LogLine { kind: LogKind::Input, text: code.trim_end().to_string() });
        }
        let res = self.repl.session.as_ref().map(|s| s.eval(code));
        match res {
            Some(Ok(id)) => {
                self.repl.busy.insert(id);
                self.repl.stick_to_bottom = true;
            }
            Some(Err(e)) => self.repl.log.push(LogLine { kind: LogKind::Error, text: i18n::fmt("Cannot send to REPL: {}", &[&e]) }),
            None => {}
        }
    }

    pub fn repl_request(&mut self, kind: &str, payload: &str, pending: Pending) -> bool {
        if !self.repl.ready || self.repl.session.is_none() {
            return false;
        }
        let r = self.repl.session.as_ref().map(|s| s.request(kind, payload));
        if let Some(Ok(id)) = r {
            self.repl.pending.insert(id, pending);
            return true;
        }
        false
    }

    fn log_repl_output(&mut self, text: &str) {
        let text = text.replace('\r', "");
        for seg in text.split_inclusive('\n') {
            match self.repl.log.last_mut() {
                Some(l) if l.kind == LogKind::Output && !l.text.ends_with('\n') => l.text.push_str(seg),
                _ => self.repl.log.push(LogLine { kind: LogKind::Output, text: seg.to_string() }),
            }
        }
    }

    pub fn poll_repl(&mut self) {
        let mut events = Vec::new();
        if let Some(s) = &self.repl.session {
            while let Some(e) = s.try_recv() {
                events.push(e);
                if events.len() > 500 {
                    break;
                }
            }
        }
        for e in events {
            match e {
                ReplEvent::Ready(v) => {
                    self.repl.ready = true;
                    self.repl.starting = false;
                    self.repl.version = v.clone();
                    self.repl.log.push(LogLine { kind: LogKind::Info, text: i18n::fmt("{} ready.", &[&v]) });
                }
                ReplEvent::Output(t) => self.log_repl_output(&t),
                ReplEvent::Value { text, .. } => {
                    for l in text.lines() {
                        self.repl.log.push(LogLine { kind: LogKind::Value, text: l.to_string() });
                    }
                }
                ReplEvent::Error { message, .. } => {
                    for l in message.lines() {
                        self.repl.log.push(LogLine { kind: LogKind::Error, text: l.to_string() });
                    }
                }
                ReplEvent::Trace { text, .. } => {
                    for l in text.lines().take(12) {
                        self.repl.log.push(LogLine { kind: LogKind::Trace, text: l.to_string() });
                    }
                }
                ReplEvent::Done { id, package } => {
                    self.repl.busy.remove(&id);
                    self.repl.package = package;
                }
                ReplEvent::Reply { id, payload } => {
                    if let Some(p) = self.repl.pending.remove(&id) {
                        self.handle_reply(p, &payload);
                    }
                }
                ReplEvent::Exited(code) => {
                    self.repl.log.push(LogLine { kind: LogKind::Info, text: i18n::fmt("REPL process exited (code {})", &[&code.map_or("?".to_string(), |c| c.to_string())]) });
                    self.repl.session = None;
                    self.repl.ready = false;
                    self.repl.starting = false;
                    self.repl.busy.clear();
                    self.repl.pending.clear();
                }
            }
        }
        if self.repl.log.len() > 6000 {
            self.repl.log.drain(..1000);
        }
        if !self.repl.log.is_empty() {
            self.repl.stick_to_bottom = true;
        }
    }

    fn handle_reply(&mut self, p: Pending, payload: &str) {
        match p {
            Pending::Complete { start, prefix, .. } => {
                if self.completion.active && self.completion.start == start && self.completion.prefix == prefix {
                    let mut live: Vec<CItem> = payload
                        .lines()
                        .filter_map(|l| {
                            let (n, k) = l.split_once('\t')?;
                            Some(CItem { text: n.to_string(), kind: k.to_string(), source: 0 })
                        })
                        .collect();
                    let mut seen: HashSet<String> = live.iter().map(|c| c.text.to_lowercase()).collect();
                    for c in self.completion.items.drain(..) {
                        if seen.insert(c.text.to_lowercase()) {
                            live.push(c);
                        }
                    }
                    live.retain(|c| c.text.to_lowercase() != prefix.to_lowercase() || live_len_gt1());
                    live.truncate(80);
                    if !live.is_empty() {
                        self.completion.selected = 0;
                        self.completion.items = live;
                    } else {
                        self.completion.active = false;
                    }
                }
            }
            Pending::Doc { title } => {
                let text = if payload.trim().is_empty() { tr("No documentation available.").to_string() } else { payload.to_string() };
                self.popup = Popup::Info { title, text };
            }
            Pending::ParamInfo { name } => {
                if !payload.trim().is_empty() {
                    self.hint = Some((payload.to_string(), self.now + 6.0));
                } else {
                    self.hint = Some((i18n::fmt("No parameter info for {}", &[&name]), self.now + 3.0));
                }
            }
            Pending::Hover { cache_key, name } => self.hover_reply(&cache_key, &name, payload),
            Pending::Macroexpand => {
                self.popup = Popup::Info { title: tr("Macroexpand").into(), text: payload.to_string() };
            }
            Pending::Definition { name } => {
                let first = payload.lines().next().unwrap_or("");
                if let Some((path, off)) = first.split_once('\t') {
                    // in WSL the Lisp reports Linux paths
                    let p = self.repl.session.as_ref().map_or_else(|| PathBuf::from(path), |s| s.implementation.host_path(path));
                    let char_off: usize = off.trim().parse().unwrap_or(0);
                    let byte = std::fs::read_to_string(&p).ok().map(|t| t.char_indices().nth(char_off).map_or(t.len(), |(b, _)| b)).unwrap_or(0);
                    self.goto(&p, byte, true);
                } else {
                    // the image does not know it either: read the project's text for it
                    self.search_definition_text(&name);
                }
            }
        }
    }

    // ------------------------------------------------------------------ run / compile

    pub fn run_current_file(&mut self) {
        let Some(d) = self.doc() else { return };
        if !d.ed.is_lisp() && d.diff.is_none() {
            // a file of a language a module brings: that module's run command; else the project's make
            let lang = d.ed.language().map(|l| l.id().to_string()).unwrap_or_default();
            if let Some(def) = self.run_def_for(&lang) {
                if d.path.is_none() {
                    let a = self.active;
                    if !self.save_doc(a) {
                        return;
                    }
                }
                self.save_all();
                if let Some(path) = self.doc().and_then(|d| d.path.clone()) {
                    self.run_language_command(&def, &path);
                }
                return;
            }
            match self.make_targets.first().cloned() {
                Some(t) => self.run_make(&t),
                None => self.toast(tr("There is nothing to run for this file: no module with a run command for it, and no makefile"), Sev::Weak),
            }
            return;
        }
        if d.path.is_none() {
            let a = self.active;
            if !self.save_doc(a) {
                return;
            }
        }
        self.save_all();
        let Some(path) = self.doc().and_then(|d| d.path.clone()) else { return };
        let Some(imp) = self.lisp.clone().or_else(|| self.resolve_lisp()) else {
            self.toast(tr("SBCL not found - set the path in Settings"), Sev::Error);
            return;
        };
        match runner::run_file(&imp, &path, self.project_root()) {
            Ok(h) => {
                self.prepare_run(format!("file:{}", path.display()));
                self.run_request = Some(crate::app_run::RunRequest::File(path.clone()));
                self.run = RunState { handle: Some(h), title: format!("Run '{}'", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), log: vec![], running: true, exit_code: None };
                self.run.log.push(LogLine { kind: LogKind::Info, text: format!("{} --non-interactive --load {}", imp.path.display(), path.display()) });
                self.bottom_tool = Some(BottomTool::Run);
            }
            Err(e) => self.toast(i18n::fmt("Cannot run: {}", &[&e]), Sev::Error),
        }
    }

    pub fn stop_run(&mut self) {
        self.cancel_scenario();
        if let Some(h) = &self.run.handle {
            h.kill();
        }
        if self.run.running {
            self.run.log.push(LogLine { kind: LogKind::Info, text: tr("Process terminated by user").into() });
            self.run.running = false;
        }
        if let Some(c) = self.compile.take() {
            c.handle.kill();
            let _ = std::fs::remove_file(&c.fasl);
        }
    }

    pub fn poll_run(&mut self) {
        self.poll_selected_run();
        for index in 0..self.other_runs.len() {
            self.select_run_tab(index);
            self.poll_selected_run();
            self.select_run_tab(index);
        }
    }

    fn poll_selected_run(&mut self) {
        let mut exited = None;
        let mut lines = Vec::new();
        if let Some(h) = &self.run.handle {
            while let Some(ev) = h.try_recv() {
                match ev {
                    RunEvent::Output { text, is_err } => lines.push((text, is_err)),
                    RunEvent::Exited(c) => exited = Some(c),
                }
            }
        }
        for (text, is_err) in lines {
            let text = text.replace('\r', "");
            for seg in text.split_inclusive('\n') {
                let kind = if is_err { LogKind::Error } else { LogKind::Output };
                match self.run.log.last_mut() {
                    Some(l) if l.kind == kind && !l.text.ends_with('\n') => l.text.push_str(seg),
                    _ => self.run.log.push(LogLine { kind, text: seg.to_string() }),
                }
            }
        }
        if let Some(code) = exited {
            self.run.running = false;
            self.run.exit_code = code;
            let msg = i18n::fmt("Process finished with exit code {}", &[&code.map_or("?".to_string(), |c| c.to_string())]);
            self.run.log.push(LogLine { kind: if code == Some(0) { LogKind::Info } else { LogKind::Error }, text: msg });
            self.run.handle = None;
            self.make_finished(code);
            // a scenario goes on with its next command
            self.scenario_after_exit(code);
        }
    }

    pub fn compile_current_file(&mut self) {
        if self.doc().is_none() {
            return;
        }
        if self.doc().is_some_and(|d| d.path.is_none()) {
            let a = self.active;
            if !self.save_doc(a) {
                return;
            }
        }
        self.save_all();
        let Some(path) = self.doc().and_then(|d| d.path.clone()) else { return };
        let Some(imp) = self.lisp.clone().or_else(|| self.resolve_lisp()) else {
            self.toast(tr("SBCL not found - set the path in Settings"), Sev::Error);
            return;
        };
        match compiler::compile_file(&imp, &path) {
            Ok((handle, fasl)) => {
                self.compile = Some(CompileJob { handle, fasl, file: path, output: String::new() });
                self.status = tr("Compiling...").into();
            }
            Err(e) => self.toast(i18n::fmt("Cannot compile: {}", &[&e]), Sev::Error),
        }
    }

    pub fn poll_compile(&mut self) {
        let Some(job) = self.compile.as_mut() else { return };
        let mut exit = None;
        while let Some(ev) = job.handle.try_recv() {
            match ev {
                RunEvent::Output { text, .. } => job.output.push_str(&text),
                RunEvent::Exited(c) => exit = Some(c),
            }
        }
        let Some(code) = exit else { return };
        let job = self.compile.take().unwrap();
        let _ = std::fs::remove_file(&job.fasl);
        let problems = compiler::parse_compiler_output(&job.output);
        let (text, dialect, rev) = match self.docs.iter().find(|d| d.path.as_deref() == Some(job.file.as_path())) {
            Some(d) => (d.ed.text().to_string(), d.ed.dialect(), d.ed.revision()),
            None => (std::fs::read_to_string(&job.file).unwrap_or_default(), lisp_dialect_of(&job.file), 0),
        };
        self.compiler_problems.retain(|p| p.path.as_deref() != Some(job.file.as_path()));
        let mut markers = Vec::new();
        let (mut errors, mut warnings) = (0, 0);
        for p in &problems {
            let loc = compiler::locate(p, &text, dialect);
            let sev = match p.severity {
                compiler::Severity::Error => {
                    errors += 1;
                    Sev::Error
                }
                compiler::Severity::Warning => {
                    warnings += 1;
                    Sev::Warning
                }
                _ => Sev::Weak,
            };
            let (s, e) = loc.unwrap_or((0, 0));
            if loc.is_some() {
                markers.push(Marker { start: s, end: e.max(s + 1), sev, msg: p.message.lines().next().unwrap_or("").to_string() });
            }
            self.compiler_problems.push(ProblemRow { path: Some(job.file.clone()), start: s, end: e, sev, message: p.message.lines().next().unwrap_or("").to_string(), source: "compiler" });
        }
        if let Some(d) = self.docs.iter_mut().find(|d| d.path.as_deref() == Some(job.file.as_path())) {
            d.compiler_markers = markers;
            d.compiled_rev = rev;
        }
        let name = job.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if code == Some(0) && errors == 0 {
            self.status = i18n::fmt("Compiled {0}: {1} warning(s)", &[&name, &warnings]);
            self.toast(i18n::fmt("Compiled {0} successfully ({1} warnings)", &[&name, &warnings]), Sev::Weak);
        } else {
            self.status = i18n::fmt("Compilation of {} failed", &[&name]);
            self.toast(i18n::fmt("Compilation of {0}: {1} error(s), {2} warning(s)", &[&name, &errors, &warnings]), Sev::Error);
            self.bottom_tool = Some(BottomTool::Problems);
        }
    }

    // ------------------------------------------------------------------ problems

    pub fn all_problems(&self) -> Vec<ProblemRow> {
        let mut v = Vec::new();
        for d in self.docs.iter().filter(|d| d.ed.is_lisp() && d.diff.is_none()) {
            for diag in &d.ed.parsed().diagnostics {
                v.push(ProblemRow {
                    path: d.path.clone(),
                    start: diag.start,
                    end: diag.end,
                    sev: if diag.severity == lisp_syntax::Severity::Error { Sev::Error } else { Sev::Warning },
                    message: diag.message.clone(),
                    source: "syntax",
                });
            }
        }
        for d in self.docs.iter().filter(|d| !d.ed.is_lisp() && d.diff.is_none()) {
            let mut markers = d.language_markers();
            let checked = d.check_rev == d.ed.revision();
            if checked {
                markers.extend(d.check_markers.iter().cloned());
            }
            let n_language = markers.len() - if checked { d.check_markers.len() } else { 0 };
            for (k, m) in markers.into_iter().enumerate() {
                v.push(ProblemRow { path: d.path.clone(), start: m.start, end: m.end, sev: m.sev, message: m.msg, source: if k < n_language { "syntax" } else { "check" } });
            }
        }
        v.extend(self.compiler_problems.iter().cloned());
        v
    }

    // ------------------------------------------------------------------ symbols / navigation

    pub fn symbol_at_caret(&self) -> Option<String> {
        let d = self.doc()?;
        let (s, e) = d.ed.symbol_range_at(d.ed.caret());
        (s < e).then(|| d.ed.text()[s..e].to_string())
    }

    pub fn goto_declaration(&mut self, name: Option<String>) {
        // a language of a module: its definitions come from the project's index, not from the Lisp reader
        if self.doc().is_some_and(|d| d.diff.is_none() && !d.ed.is_lisp()) {
            self.goto_language_declaration(name);
            return;
        }
        let Some(name) = name.or_else(|| self.symbol_at_caret()) else { return };
        self.reindex_open_docs();
        let defs = self.project.as_ref().map(|p| p.index.find_definitions(&name)).unwrap_or_default();
        match defs.len() {
            0 => {
                // a function the project defines in C or assembly
                let near = self.doc().and_then(|d| d.path.clone());
                let hits = self.project_definitions(&name, None, near, None, false);
                if !hits.is_empty() {
                    self.goto_hits(&name, hits);
                    return;
                }
                if !self.repl_request("definition", &name, Pending::Definition { name: name.clone() }) {
                    // no REPL to ask: read the project's text for it
                    self.search_definition_text(&name);
                }
            }
            1 => {
                let d = &defs[0];
                self.goto(&d.path, d.def.name_start, true);
            }
            _ => {
                let root = self.project_root().map(Path::to_path_buf);
                let items: Vec<Item> = defs
                    .iter()
                    .map(|d| Item {
                        label: d.def.name.clone(),
                        detail: root.as_ref().map_or_else(|| d.path.to_string_lossy().into_owned(), |r| workspace::relative(r, &d.path)),
                        badge: d.def.kind.badge().to_string(),
                        section: "",
                        target: PTarget::Symbol { path: d.path.clone(), offset: d.def.name_start },
                        hits: vec![],
                        shortcut: String::new(),
                    })
                    .collect();
                self.popup = Popup::Palette(PaletteState { mode: PaletteMode::Choose, query: String::new(), items: items.clone(), selected: 0, built_for: None, fixed: Some(items), just_opened: true });
            }
        }
    }

    /// A definition of the project, taken apart: what the hover popup and Quick Documentation show.
    pub fn local_definition_parts(&self, name: &str) -> Option<DefinitionParts> {
        let defs = self.project.as_ref()?.index.find_definitions(name);
        let d = defs.first()?;
        let text = self
            .docs
            .iter()
            .find(|x| x.path.as_deref() == Some(d.path.as_path()))
            .map(|x| x.ed.text().to_string())
            .or_else(|| std::fs::read_to_string(&d.path).ok())?;
        let dialect = lisp_dialect_of(&d.path);
        let p = lisp_syntax::parse(&text, dialect);
        let tl = p.top_level();
        let form = tl.iter().find(|f| p.start_of(**f) == d.def.start)?;
        let open = p.partner.get(form.last).copied().flatten()?;
        let kids = p.children(open);
        let is_var = matches!(d.def.operator.to_ascii_lowercase().as_str(), "defvar" | "defparameter" | "defconstant" | "define-constant");
        // (defun name (lambda list) "doc" body...): the string is documentation only when forms follow it
        let doc = kids
            .get(3)
            .filter(|k| p.tokens[k.first].kind == TokenKind::String && (is_var || kids.len() > 4))
            .map(|k| text[p.start_of(*k)..p.end_of(*k)].to_string())
            .map(|s| s.trim().trim_matches('"').replace("\\\"", "\"").replace("\\\\", "\\"));
        // the call as written: (name a b &optional c)
        let signature = if is_var || kids.len() < 3 {
            d.def.name.clone()
        } else {
            let ll = &text[p.start_of(kids[2])..p.end_of(kids[2])];
            let inner = ll.trim().strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or(ll.trim()).split_whitespace().collect::<Vec<_>>().join(" ");
            if inner.is_empty() { format!("({})", d.def.name) } else { format!("({} {})", d.def.name, inner) }
        };
        let line = text[..d.def.start.min(text.len())].matches('\n').count();
        Some(DefinitionParts { signature, doc, kind: d.def.kind.label().to_string(), path: d.path.clone(), line, name_start: d.def.name_start })
    }

    fn local_definition_text(&self, name: &str) -> Option<String> {
        let defs = self.project.as_ref()?.index.find_definitions(name);
        let d = defs.first()?;
        let text = self
            .docs
            .iter()
            .find(|x| x.path.as_deref() == Some(d.path.as_path()))
            .map(|x| x.ed.text().to_string())
            .or_else(|| std::fs::read_to_string(&d.path).ok())?;
        let dialect = lisp_dialect_of(&d.path);
        let p = lisp_syntax::parse(&text, dialect);
        let tl = p.top_level();
        let form = tl.iter().find(|f| p.start_of(**f) == d.def.start)?;
        let open = p.partner.get(form.last).copied().flatten()?;
        let kids = p.children(open);
        let sig_end = kids.iter().take(3).map(|k| p.end_of(*k)).max()?;
        let header = text[d.def.start..sig_end].to_string() + ")";
        let doc = kids.iter().skip(3).find(|k| p.tokens[k.first].kind == TokenKind::String).map(|k| text[p.start_of(*k)..p.end_of(*k)].trim_matches('"').to_string());
        Some(match doc {
            Some(dc) => format!("{header}\n\n{dc}"),
            None => header,
        })
    }

    pub fn show_quick_doc(&mut self) {
        let Some(name) = self.symbol_at_caret() else { return };
        self.reindex_open_docs();
        if let Some(t) = self.local_definition_text(&name) {
            self.popup = Popup::Info { title: name, text: t };
            return;
        }
        if self.repl_request("doc", &name, Pending::Doc { title: name.clone() }) {
            return;
        }
        let bare = symbols::strip_package(&name);
        let text = if symbols::is_keyword(bare) {
            i18n::fmt("{}\n\nCommon Lisp special form / macro.\nStart the REPL (Alt+8) to see the full documentation from the running image.", &[&bare])
        } else if symbols::is_builtin_function(bare) {
            i18n::fmt("{}\n\nCommon Lisp standard function.\nStart the REPL (Alt+8) to see the full documentation from the running image.", &[&bare])
        } else {
            i18n::fmt("No documentation found for '{}'.", &[&name])
        };
        self.popup = Popup::Info { title: name, text };
    }

    pub fn show_param_info(&mut self) {
        // operator of the innermost enclosing list
        let Some(d) = self.doc() else { return };
        let p = d.ed.parsed();
        let Some(open) = p.enclosing_open(d.ed.caret()) else { return };
        let kids = p.children(open);
        let Some(head) = kids.first() else { return };
        let name = d.ed.text()[p.start_of(*head)..p.end_of(*head)].to_string();
        self.reindex_open_docs();
        if let Some(t) = self.local_definition_text(&name) {
            self.hint = Some((t.lines().next().unwrap_or("").to_string(), self.now + 6.0));
            return;
        }
        if !self.repl_request("arglist", &name, Pending::ParamInfo { name: name.clone() }) {
            self.hint = Some((i18n::fmt("{}: start the REPL (Alt+8) for parameter info", &[&name]), self.now + 3.0));
        }
    }

    pub fn macroexpand_form(&mut self) {
        let Some(d) = self.doc() else { return };
        let range = d.ed.sexp_before_caret().or_else(|| d.ed.form_to_eval());
        let Some((s, e)) = range else { return };
        let form = d.ed.text()[s..e].to_string();
        if !self.repl_request("macroexpand", &form, Pending::Macroexpand) {
            self.ensure_repl();
            self.toast(tr("Starting REPL - run Macroexpand again when it is ready"), Sev::Weak);
        }
    }

    pub fn find_usages(&mut self) {
        if self.doc().is_some_and(|d| d.diff.is_none() && !d.ed.is_lisp()) {
            self.find_language_usages();
            return;
        }
        let Some(name) = self.symbol_at_caret() else { return };
        self.find_usages_everywhere(&name);
    }

    pub fn run_find_in_files(&mut self, query: &str, opts: TextQuery) {
        let Some(root) = self.project_root().map(Path::to_path_buf) else {
            self.toast(tr("Open a project first"), Sev::Weak);
            return;
        };
        self.save_all();
        // searching a big project can take seconds: do it off the UI thread
        let (tx, rx) = mpsc::channel();
        let q = query.to_string();
        let title = i18n::fmt("Find '{}'", &[&query]);
        self.find_results = FindResults { title: i18n::fmt("{} - searching...", &[&title]), hits: vec![] };
        self.find_rx = Some(rx);
        let hidden = self.project.as_ref().map(|p| p.hidden.clone()).unwrap_or_default();
        std::thread::spawn(move || {
            let hits = search::find_in_project(&root, &hidden, &q, opts, 3000);
            let _ = tx.send((title, hits));
        });
        self.bottom_tool = Some(BottomTool::Find);
    }

    /// Rename every code occurrence of `old` (project files and open buffers).
    pub fn rename_symbol(&mut self, old: &str, new: &str) {
        if new.is_empty() || new.chars().any(|c| c.is_whitespace() || "()\"';".contains(c)) {
            self.toast(tr("Invalid symbol name"), Sev::Error);
            return;
        }
        let mut count = 0;
        let mut files = 0;
        let mut paths: Vec<PathBuf> = self.project.as_ref().map(|p| p.files.clone()).unwrap_or_default();
        for d in &self.docs {
            if let Some(p) = &d.path
                && !paths.contains(p)
            {
                paths.push(p.clone());
            }
        }
        for path in paths {
            let dialect = lisp_dialect_of(&path);
            if lisp_syntax::Dialect::from_path(&path).is_none() {
                continue;
            }
            if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(path.as_path())) {
                let ranges = index::symbol_ranges(self.docs[i].ed.text(), dialect, old);
                if !ranges.is_empty() {
                    count += ranges.len();
                    files += 1;
                    self.docs[i].ed.replace_ranges(&ranges, new);
                }
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                let ranges = index::symbol_ranges(&text, dialect, old);
                if ranges.is_empty() {
                    continue;
                }
                let mut out = text.clone();
                for &(s, e) in ranges.iter().rev() {
                    out.replace_range(s..e, new);
                }
                if std::fs::write(&path, out).is_ok() {
                    count += ranges.len();
                    files += 1;
                }
            }
        }
        self.refresh_project();
        self.toast(i18n::fmt("Renamed {0} occurrence(s) in {1} file(s)", &[&count, &files]), Sev::Weak);
    }

    // ------------------------------------------------------------------ completion

    pub fn refresh_completion(&mut self, explicit: bool) {
        // the Lisp vocabulary and the REPL are Lisp's; the languages of modules complete from their own words
        if self.docs.get(self.active).is_some_and(|d| d.diff.is_none() && !d.ed.is_lisp()) {
            self.refresh_language_completion(explicit);
            return;
        }
        let Some(d) = self.docs.get(self.active).filter(|d| d.ed.is_lisp()) else {
            self.completion.active = false;
            return;
        };
        let caret = d.ed.caret();
        let (s, _) = d.ed.symbol_range_at(caret);
        let prefix = d.ed.text()[s..caret].to_string();
        if (prefix.is_empty() && !explicit) || (!explicit && prefix.chars().count() < 2) || d.ed.has_selection() {
            self.completion.active = false;
            return;
        }
        if d.ed.parsed().in_line_comment(caret) {
            self.completion.active = false;
            return;
        }
        let low = prefix.to_lowercase();
        let mut seen: HashSet<String> = HashSet::new();
        let mut items: Vec<CItem> = Vec::new();
        let mut push = |text: String, kind: &str, source: u8, items: &mut Vec<CItem>| {
            let l = text.to_lowercase();
            if l.starts_with(&low) && l != low && seen.insert(l) {
                items.push(CItem { text, kind: kind.to_string(), source });
            }
        };
        // definitions in the project
        if let Some(p) = &self.project {
            for sym in p.index.with_prefix(&low, 80) {
                let name = sym.def.name.split(' ').next().unwrap_or("").to_string();
                let k = match sym.def.kind {
                    outline_kind if outline_kind.badge() == "m" => "m",
                    outline_kind if outline_kind.badge() == "v" || outline_kind.badge() == "c" => "v",
                    _ => "f",
                };
                push(name, k, 1, &mut items);
            }
        }
        // identifiers in the current file
        let p = d.ed.parsed();
        // large files: only look at the neighbourhood of the caret
        let (lo, hi) = if d.ed.text().len() > 300_000 { (caret.saturating_sub(100_000), caret + 100_000) } else { (0, usize::MAX) };
        let first_tok = p.tokens.partition_point(|t| t.end < lo);
        for t in p.tokens[first_tok..].iter().take_while(|t| t.start <= hi) {
            if t.kind == TokenKind::Symbol && t.end != caret {
                push(t.text(d.ed.text()).to_string(), "s", 2, &mut items);
            }
        }
        for name in symbols::complete(&low) {
            push(name.to_string(), if symbols::is_keyword(name) { "m" } else { "f" }, 3, &mut items);
        }
        items.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.text.len().cmp(&b.text.len())).then_with(|| a.text.cmp(&b.text)));
        items.truncate(60);
        let started = self.completion.active && self.completion.start == s && self.completion.prefix == prefix;
        self.completion = Completion { active: !items.is_empty() || self.repl.ready, items, selected: if started { self.completion.selected } else { 0 }, start: s, prefix: prefix.clone() };
        if self.repl.ready && !prefix.is_empty() {
            let p2 = prefix.clone();
            self.repl_request("complete", &p2, Pending::Complete { start: s, prefix });
        }
        if self.completion.items.is_empty() && !self.repl.ready {
            self.completion.active = false;
        }
        if self.completion.selected >= self.completion.items.len() {
            self.completion.selected = 0;
        }
    }

    pub fn accept_completion(&mut self) {
        if !self.completion.active {
            return;
        }
        let Some(item) = self.completion.items.get(self.completion.selected).cloned() else {
            self.completion.active = false;
            return;
        };
        let start = self.completion.start;
        if let Some(d) = self.docs.get_mut(self.active) {
            let caret = d.ed.caret();
            if start <= caret {
                d.ed.replace_range(start, caret, &item.text);
                d.view.scroll_to_caret = true;
            }
        }
        self.completion.active = false;
    }

    // ------------------------------------------------------------------ find bar

    pub fn open_find(&mut self, replace: bool) {
        if self.doc().is_none() {
            return;
        }
        self.find.open = true;
        self.find.replace_mode = replace;
        self.find.focus = true;
        if let Some(d) = self.doc()
            && d.ed.has_selection()
            && !d.ed.selected_text().contains('\n')
        {
            self.find.query = d.ed.selected_text().to_string();
        }
        self.find.built_for = None;
    }

    pub fn refresh_find(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        let key = (self.active, d.ed.revision(), self.find.query.clone(), self.find.case_sensitive, self.find.whole_word);
        if self.find.built_for.as_ref() == Some(&key) {
            return;
        }
        self.find.matches = if self.find.open { search::find_in_text(d.ed.text(), &self.find.query, TextQuery { case_sensitive: self.find.case_sensitive, whole_word: self.find.whole_word }) } else { vec![] };
        let caret = d.ed.selection().0;
        self.find.current = if self.find.matches.is_empty() { None } else { Some(self.find.matches.iter().position(|m| m.0 >= caret).unwrap_or(0)) };
        self.find.built_for = Some(key);
    }

    pub fn find_step(&mut self, forward: bool) {
        self.refresh_find();
        if self.find.matches.is_empty() {
            if !self.find.open {
                self.open_find(false);
            }
            return;
        }
        let n = self.find.matches.len();
        let cur = self.find.current.unwrap_or(0);
        // when the current match is selected, move on; otherwise land on the nearest
        let selected_is_current = self.doc().is_some_and(|d| d.ed.selection() == self.find.matches[cur.min(n - 1)]);
        let next = if selected_is_current { if forward { (cur + 1) % n } else { (cur + n - 1) % n } } else { cur };
        self.find.current = Some(next);
        let (s, e) = self.find.matches[next];
        if let Some(d) = self.doc_mut() {
            d.ed.select_range(s, e);
            d.view.scroll_to_caret = true;
        }
    }

    pub fn replace_current(&mut self) {
        self.refresh_find();
        let Some(i) = self.find.current else { return };
        let Some(&(s, e)) = self.find.matches.get(i) else { return };
        let r = self.find.replace.clone();
        if let Some(d) = self.doc_mut() {
            d.ed.replace_range(s, e, &r);
        }
        self.find.built_for = None;
        self.refresh_find();
        self.find_step(true);
    }

    pub fn replace_all(&mut self) {
        self.refresh_find();
        let ms = self.find.matches.clone();
        if ms.is_empty() {
            return;
        }
        let r = self.find.replace.clone();
        if let Some(d) = self.doc_mut() {
            d.ed.replace_ranges(&ms, &r);
        }
        self.toast(i18n::fmt("Replaced {} occurrence(s)", &[&ms.len()]), Sev::Weak);
        self.find.built_for = None;
    }

    // ------------------------------------------------------------------ palettes

    pub fn open_palette(&mut self, mode: PaletteMode) {
        let initial = if let Popup::Palette(p) = &self.popup { Some(p.query.clone()) } else { None };
        let fixed = None;
        self.popup = Popup::Palette(PaletteState { mode, query: initial.unwrap_or_default(), items: vec![], selected: 0, built_for: None, fixed, just_opened: true });
    }

    pub fn open_structure_palette(&mut self) {
        let Some(d) = self.doc() else { return };
        let defs = outline(d.ed.text(), d.ed.dialect());
        let items: Vec<Item> = defs
            .iter()
            .map(|x| Item { label: x.name.clone(), detail: i18n::fmt("{0}  line {1}", &[&x.kind.label(), &(x.line + 1)]), badge: x.kind.badge().into(), section: "", target: PTarget::Offset(x.name_start), hits: vec![], shortcut: String::new() })
            .collect();
        self.popup = Popup::Palette(PaletteState { mode: PaletteMode::Structure, query: String::new(), items: vec![], selected: 0, built_for: None, fixed: Some(items), just_opened: true });
    }

    fn rel(&self, p: &Path) -> String {
        match self.project_root() {
            Some(r) => workspace::relative(r, p),
            None => p.to_string_lossy().into_owned(),
        }
    }

    pub fn build_items(&self, mode: PaletteMode, query: &str, fixed: Option<&Vec<Item>>) -> Vec<Item> {
        let mut out: Vec<Item> = Vec::new();
        let files_items = |limit: usize, section: &'static str| -> Vec<Item> {
            let mut v = Vec::new();
            let recent: Vec<(&PathBuf, String)> = self.recent_files.iter().map(|f| (f, self.rel(f))).collect();
            let recent_set: std::collections::HashSet<&PathBuf> = self.recent_files.iter().collect();
            let project_files: Vec<(&PathBuf, &String)> = self.project.as_ref().map(|p| p.files.iter().zip(p.files_rel.iter()).filter(|(f, _)| !recent_set.contains(f)).collect()).unwrap_or_default();
            let mk = |f: &PathBuf, rel: &str, hits: Vec<usize>| Item {
                label: f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                detail: rel.to_string(),
                badge: "F".into(),
                section,
                target: PTarget::File(f.clone()),
                hits,
                shortcut: String::new(),
            };
            if query.trim().is_empty() {
                for (f, rel) in recent.iter().map(|(f, r)| (*f, r.as_str())).chain(project_files.iter().map(|(f, r)| (*f, r.as_str()))).take(limit) {
                    v.push(mk(f, rel, vec![]));
                }
            } else {
                let mut scored: Vec<(i32, &PathBuf, &str, search::FuzzyMatch)> = Vec::new();
                for (f, rel) in recent.iter().map(|(f, r)| (*f, r.as_str())).chain(project_files.iter().map(|(f, r)| (*f, r.as_str()))) {
                    if let Some(m) = search::fuzzy_match(query, rel) {
                        scored.push((m.score, f, rel, m));
                    }
                }
                scored.sort_by_key(|s| std::cmp::Reverse(s.0));
                for (_, f, rel, m) in scored.into_iter().take(limit) {
                    let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let ni = rel.chars().count() - name.chars().count();
                    let hits: Vec<usize> = search::fuzzy_match(query, &name).map(|m| m.positions).unwrap_or_else(|| m.positions.iter().filter(|&&p| p >= ni).map(|p| p - ni).collect());
                    v.push(mk(f, rel, hits));
                }
            }
            v
        };
        let symbol_items = |limit: usize, section: &'static str| -> Vec<Item> {
            let Some(p) = &self.project else { return vec![] };
            if query.trim().is_empty() {
                return p
                    .index
                    .all()
                    .symbols
                    .iter()
                    .take(limit)
                    .map(|s| Item { label: s.def.name.clone(), detail: self.rel(&s.path), badge: s.def.kind.badge().into(), section, target: PTarget::Symbol { path: s.path.clone(), offset: s.def.name_start }, hits: vec![], shortcut: String::new() })
                    .collect();
            }
            p.index
                .search(query, limit)
                .into_iter()
                .map(|(s, m)| Item { label: s.def.name.clone(), detail: self.rel(&s.path), badge: s.def.kind.badge().into(), section, target: PTarget::Symbol { path: s.path.clone(), offset: s.def.name_start }, hits: m.positions, shortcut: String::new() })
                .collect()
        };
        let action_items = |limit: usize, section: &'static str| -> Vec<Item> {
            let lisp_on = self.mods.lisp;
            let list: Vec<&commands::Command> = commands::COMMANDS.iter().filter(|c| (lisp_on || !commands::is_lisp_command(c.id)) && !c.title.is_empty() && !matches!(c.id, "edit.newline" | "edit.backspace" | "edit.delete" | "app.escape") && !c.id.starts_with("caret.") && !c.id.starts_with("select.") && !c.id.starts_with("scroll.")).collect();
            let mk = |c: &commands::Command, hits: Vec<usize>| Item { label: tr(c.title).to_string(), detail: tr(c.category.title()).to_string(), badge: "A".into(), section, target: PTarget::Command(c.id.to_string()), hits, shortcut: c.shortcuts.first().map(|s| commands::display_shortcut(s)).unwrap_or_default() };
            if query.trim().is_empty() {
                return list.iter().take(limit).map(|c| mk(c, vec![])).collect();
            }
            search::rank(query, &list, |c| tr(c.title).to_string(), limit).into_iter().map(|(i, m)| mk(list[i], m.positions)).collect()
        };
        match mode {
            PaletteMode::Everywhere => {
                out.extend(files_items(6, "Files"));
                out.extend(symbol_items(6, "Symbols"));
                out.extend(action_items(6, "Actions"));
            }
            PaletteMode::Files => out.extend(files_items(60, "")),
            PaletteMode::Symbols => out.extend(symbol_items(60, "")),
            PaletteMode::Actions => out.extend(action_items(60, "")),
            PaletteMode::Recent => {
                let q = query.to_lowercase();
                for f in &self.recent_files {
                    let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if q.is_empty() || name.to_lowercase().contains(&q) {
                        out.push(Item { label: name, detail: self.rel(f), badge: "F".into(), section: "", target: PTarget::File(f.clone()), hits: vec![], shortcut: String::new() });
                    }
                }
            }
            PaletteMode::Structure | PaletteMode::Choose => {
                if let Some(f) = fixed {
                    if query.trim().is_empty() {
                        out.extend(f.iter().cloned());
                    } else {
                        for (i, m) in search::rank(query, f, |it| it.label.clone(), 100) {
                            let mut it = f[i].clone();
                            it.hits = m.positions;
                            out.push(it);
                        }
                    }
                }
            }
        }
        out
    }

    pub fn tick_palette(&mut self) {
        let Popup::Palette(p) = &self.popup else { return };
        let key = (p.mode, p.query.clone());
        if p.built_for.as_ref() == Some(&key) {
            return;
        }
        let items = self.build_items(p.mode, &p.query, p.fixed.as_ref());
        if let Popup::Palette(p) = &mut self.popup {
            p.items = items;
            p.selected = 0;
            p.built_for = Some(key);
        }
    }

    pub fn exit_clean(&mut self) {
        self.save_settings();
        if let Some(s) = self.repl.session.take() {
            s.kill();
        }
        self.stop_run();
        self.exit_now = true;
    }

}

fn live_len_gt1() -> bool {
    true
}

#[cfg(test)]
#[path = "app_saving_tests.rs"]
mod saving_tests;

/// A definition of the project, taken apart (see `App::local_definition_parts`).
pub struct DefinitionParts {
    /// The call as written: `(name a b)`.
    pub signature: String,
    pub doc: Option<String>,
    pub kind: String,
    pub path: PathBuf,
    /// 0-based line of the definition.
    pub line: usize,
    /// Byte offset of the name in the file.
    pub name_start: usize,
}

#[cfg(test)]
#[path = "app_session_tests.rs"]
mod session_tests;
