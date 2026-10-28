//! Plain data types of the application state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui::Pos2;

use ide_core::diff::DiffLine;
use ide_core::git::{Branch, Change, Commit, Repo};
use ide_core::index::ProjectIndex;
use ide_core::localvcs::LocalVcs;
use ide_core::workspace::IgnoreMatcher;
use ide_core::search::FileHit;
use ide_core::workspace::Entry;
use lisp_runtime::{ReplSession, RunHandle};

use crate::editor_view::{Sev, ViewState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftTool {
    Project,
    Structure,
    /// The Commit window: changed files, commit message, buttons.
    Commit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomTool {
    Repl,
    Run,
    Problems,
    Find,
    Git,
    Terminal,
}

/// The editor area divided into two panes. The focused pane shows `docs[active]`; the other one shows the
/// document whose id is `other` (ids, not indices, so closing tabs cannot repoint it).
pub struct Split {
    /// Panes side by side (true) or stacked (false).
    pub vertical: bool,
    pub other: u64,
    /// The focused pane is the right / bottom one.
    pub second_focused: bool,
    /// Scroll state of the unfocused pane while both panes show the same document.
    pub view: ViewState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitTab {
    Log,
    Local,
}

pub struct GitSnapshot {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub changes: Vec<Change>,
    pub branches: Vec<Branch>,
    pub log: Vec<Commit>,
    pub truncated: bool,
}

pub struct GitState {
    pub repo: Option<Repo>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub changes: Vec<Change>,
    pub branches: Vec<Branch>,
    pub log: Vec<Commit>,
    /// Lane layout of `log` (the branch graph).
    pub graph: Vec<ide_core::graph::GraphRow>,
    pub rx: Option<Receiver<GitSnapshot>>,
    pub last_refresh: f64,
    pub commit_msg: String,
    pub checked: HashSet<String>,
    pub busy: Option<String>,
    pub op_rx: Option<Receiver<(String, Result<String, String>)>>,
    pub tab: GitTab,
    /// Absolute path -> change kind for tree colouring; directories containing changes.
    pub kinds: HashMap<PathBuf, ide_core::git::ChangeKind>,
    pub dirty_dirs: HashSet<PathBuf>,
    pub available: bool,
    /// Local commits (checkpoints kept outside git).
    pub local: Option<LocalVcs>,
    pub changes_truncated: bool,
    /// Commit window: collapsed groups / directories (see `ide_core::changes::collapse_key`).
    pub collapsed: HashSet<String>,
    /// Commit window: show files under their directories.
    pub by_dir: bool,
    /// Put the keyboard focus into the commit message on the next frame.
    pub focus_msg: bool,
}

impl Default for GitState {
    fn default() -> Self {
        GitState {
            repo: None,
            branch: None,
            head: None,
            changes: vec![],
            branches: vec![],
            log: vec![],
            graph: vec![],
            rx: None,
            last_refresh: -100.0,
            commit_msg: String::new(),
            checked: HashSet::new(),
            busy: None,
            op_rx: None,
            tab: GitTab::Log,
            kinds: HashMap::new(),
            dirty_dirs: HashSet::new(),
            available: true,
            local: None,
            changes_truncated: false,
            collapsed: HashSet::new(),
            by_dir: true,
            focus_msg: false,
        }
    }
}

#[derive(Clone)]
pub struct TreeRow {
    pub path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub name: String,
    pub ignored: bool,
}

pub struct Project {
    /// Bumped whenever the visible tree may have changed (expand/collapse, refresh).
    pub tree_ver: u64,
    pub tree_cache: Option<(u64, Vec<TreeRow>)>,
    pub ignore: IgnoreMatcher,
    pub root: PathBuf,
    pub name: String,
    pub expanded: HashSet<PathBuf>,
    pub selected: Option<PathBuf>,
    pub dir_cache: HashMap<PathBuf, Vec<Entry>>,
    pub files: Vec<PathBuf>,
    /// Project-relative paths parallel to `files` (computed once, used by Go to File).
    pub files_rel: Vec<String>,
    /// Bumped every time `files` is read anew (the definitions of the other languages are read again then).
    pub files_gen: u64,
    pub index: ProjectIndex,
    pub index_rx: Option<Receiver<(ProjectIndex, Vec<PathBuf>)>>,
    pub branch: Option<String>,
    /// The folders and files the project does not show (its settings in the IDE profile).
    pub hidden: ide_core::workspace::Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    Input,
    Output,
    Value,
    Error,
    Info,
    Trace,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub kind: LogKind,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum Pending {
    Complete { start: usize, prefix: String },
    Doc { title: String },
    ParamInfo { name: String },
    Macroexpand,
    Definition { name: String },
    /// What the hover popup shows for a name.
    Hover { cache_key: String, name: String },
}

#[derive(Default)]
pub struct ReplState {
    pub session: Option<ReplSession>,
    pub starting: bool,
    pub ready: bool,
    pub version: String,
    pub package: String,
    pub log: Vec<LogLine>,
    pub input: String,
    pub history: Vec<String>,
    pub hist_pos: Option<usize>,
    pub busy: HashSet<u64>,
    pub pending: HashMap<u64, Pending>,
    pub stick_to_bottom: bool,
    pub focus_input: bool,
}

#[derive(Default)]
pub struct RunState {
    pub handle: Option<RunHandle>,
    pub title: String,
    pub log: Vec<LogLine>,
    pub running: bool,
    pub exit_code: Option<i32>,
}

pub struct CompileJob {
    pub handle: RunHandle,
    pub fasl: PathBuf,
    pub file: PathBuf,
    pub output: String,
}

#[derive(Clone, Debug)]
pub struct ProblemRow {
    pub path: Option<PathBuf>,
    pub start: usize,
    pub end: usize,
    pub sev: Sev,
    pub message: String,
    pub source: &'static str,
}

#[derive(Default)]
pub struct FindResults {
    pub title: String,
    pub hits: Vec<FileHit>,
}

// ------------------------------------------------------------------ popups

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    Everywhere,
    Actions,
    Files,
    Symbols,
    Recent,
    Structure,
    Choose,
}

impl PaletteMode {
    pub fn title(self) -> &'static str {
        match self {
            PaletteMode::Everywhere => "Search Everywhere",
            PaletteMode::Actions => "Find Action",
            PaletteMode::Files => "Go to File",
            PaletteMode::Symbols => "Go to Symbol",
            PaletteMode::Recent => "Recent Files",
            PaletteMode::Structure => "File Structure",
            PaletteMode::Choose => "Choose Declaration",
        }
    }
}

#[derive(Clone, Debug)]
pub enum PTarget {
    Command(String),
    File(PathBuf),
    Symbol { path: PathBuf, offset: usize },
    Offset(usize),
    Branch(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub detail: String,
    pub badge: String,
    pub section: &'static str,
    pub target: PTarget,
    /// Matched char positions in `label` for highlighting.
    pub hits: Vec<usize>,
    pub shortcut: String,
}

pub struct PaletteState {
    pub mode: PaletteMode,
    pub query: String,
    pub items: Vec<Item>,
    pub selected: usize,
    pub built_for: Option<(PaletteMode, String)>,
    pub fixed: Option<Vec<Item>>,
    pub just_opened: bool,
}

#[derive(Default)]
pub struct FindBar {
    pub open: bool,
    pub replace_mode: bool,
    pub query: String,
    pub replace: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub matches: Vec<(usize, usize)>,
    pub current: Option<usize>,
    pub built_for: Option<(usize, u64, String, bool, bool)>,
    pub focus: bool,
}


#[derive(Clone, Debug)]
pub struct CItem {
    pub text: String,
    pub kind: String,
    pub source: u8,
}

#[derive(Default)]
pub struct Completion {
    pub active: bool,
    pub items: Vec<CItem>,
    pub selected: usize,
    pub start: usize,
    pub prefix: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    CloseDoc(usize),
    CloseAll,
    OpenProject(PathBuf),
    CloseProject,
    Exit,
    Delete(PathBuf),
    Rollback(String),
    RestoreLocal(u64),
    /// Put one file back into its state in a local commit: (commit id, relative path).
    RestoreLocalFile(u64, String),
    DeleteLocal(u64),
    /// A language server's rename that leaves the name in files it does not see (the plan waits in the LSP state).
    PartialRename,
}

/// Messages from the background clone thread.
pub enum CloneMsg {
    Line(String),
    /// Where the clone goes, once known (a Linux home folder is looked up first).
    Dest(PathBuf),
    Done(Result<(), String>),
}

pub struct CloneRun {
    pub rx: std::sync::mpsc::Receiver<CloneMsg>,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub progress: f32,
    pub status: String,
    pub dest: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneTab {
    /// Type or paste a repository URL.
    Url,
    /// Pick one of your GitHub repositories.
    GitHub,
}

/// The "Get from Version Control" dialog.
pub struct CloneState {
    pub tab: CloneTab,
    /// Full name (`owner/repo`) of the repository picked on the GitHub tab.
    pub gh_selected: Option<String>,
    pub url: String,
    /// The user edited the directory by hand: stop deriving it from the repository name.
    pub dir_touched: bool,
    pub parent: String,
    pub branch: String,
    pub shallow: bool,
    pub error: Option<String>,
    pub just_opened: bool,
    pub run: Option<CloneRun>,
    /// Clone into a folder of a WSL distribution (git of the distribution, Linux file system).
    pub linux: bool,
    pub linux_distro: String,
    /// Parent folder inside the distribution (`~/projects`).
    pub linux_dir: String,
    /// Cloning into a Windows folder: build the project in Linux (Linux line endings, Linux environment).
    pub build_in_linux: bool,
    /// Modules to connect to the project once it is here (downloaded first if they are not installed).
    pub modules: Vec<String>,
    pub show_modules: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalViewMode {
    /// The diff of the selected file against the state before the commit.
    Changes,
    /// The whole file as it was in the commit.
    Code,
}

/// The local commit browser: files of one commit, their diff or full code, and navigation between commits.
pub struct LocalViewState {
    pub id: u64,
    /// What the commit changed (relative to the previous local commit, or HEAD for the first one).
    pub files: Vec<Change>,
    pub sel: usize,
    pub mode: LocalViewMode,
    /// The (commit, file index) the cached `diff` / `code` belong to.
    pub loaded: Option<(u64, usize)>,
    pub diff: Vec<DiffLine>,
    pub code: Vec<String>,
    pub scroll_to: Option<usize>,
}

/// The small window next to the gutter that shows the old code of a changed block.
#[derive(Clone, Debug)]
pub struct HunkPopup {
    /// Gutter line (0-based) that was clicked.
    pub line: usize,
    /// Editor revision / scroll position the popup was made for: it closes when either changes.
    pub rev: u64,
    pub scroll_y: f32,
    pub hunk: ide_core::diff::Hunk,
    pub anchor: Pos2,
    pub doc_path: PathBuf,
    pub base_label: String,
    /// The file does not exist in the base at all (untracked / new).
    pub is_new_file: bool,
}

pub enum Popup {
    None,
    Palette(PaletteState),
    GotoLine { input: String, just_opened: bool },
    Rename { old: String, input: String, just_opened: bool },
    NewFile { dir: PathBuf, input: String, extension: String, is_dir: bool, just_opened: bool },
    /// `modules`: the ones to connect; `template`: `module/Template` or empty for none (`@auto`: the first one);
    /// `creating`: waiting for the modules to be downloaded before making the project.
    NewProject { parent: String, name: String, just_opened: bool, modules: Vec<String>, template: String, creating: bool, location: crate::project_location::ProjectLocation },
    FindInFiles { query: String, replace: String, case_sensitive: bool, whole_word: bool, just_opened: bool, replace_mode: bool },
    Settings { lisp_path: String, detected: String, filter: String, page: usize },
    Clone(Box<CloneState>),
    ProjectBrowser(Box<crate::ui_project_browser::ProjectBrowser>),
    LocalView(Box<LocalViewState>),
    About,
    /// Opening another project while one is open: this window, a new window or cancel.
    OpenProject { path: PathBuf },
    Info { title: String, text: String },
    Confirm { kind: ConfirmKind, message: String },
    NewBranch { input: String, just_opened: bool },
    LocalCommit { input: String, just_opened: bool },
    RenameFile { path: PathBuf, input: String, just_opened: bool },
    /// The editor of the project's run scenarios.
    Scenarios(Box<ScenariosState>),
}

/// The scenarios being edited: a working copy, and the commands of each as one text.
pub struct ScenariosState {
    pub list: Vec<crate::app_env::Scenario>,
    pub texts: Vec<String>,
    pub selected: usize,
    pub just_opened: bool,
}

impl Popup {
    pub fn is_open(&self) -> bool {
        !matches!(self, Popup::None)
    }
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub sev: Sev,
    pub until: f64,
}
