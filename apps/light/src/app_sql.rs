//! The SQL console: a run button next to every statement of a `.sql` file (the language of a module whose engine is
//! `sql`), statements run on the SQLite database of the project in a thread of their own, and what they came back
//! with shown in the Results tab as a grid. The schema of the database is kept for the completion of names and the
//! database tree.

use std::path::{Path, PathBuf};

use ide_core::db::{self, DbEvent, DbSchema, DbWorker, ResultSet, Statement};

use crate::app::App;
use crate::doc::Doc;
use crate::editor_view::{GutterIcon, Sev};
use crate::gutter::GutterAction;
use crate::i18n::{self, tr};

/// What the Results tab shows: the sets of the last thing that ran.
#[derive(Clone, Debug, Default)]
pub struct ResultsView {
    /// What ran (the first words of the statement, or the file's name).
    pub title: String,
    pub sets: Vec<ResultSet>,
    /// Which set the grid shows when there are several.
    pub active: usize,
    /// The statement did not run: the database's message.
    pub error: Option<String>,
    /// How long the statements took.
    pub ms: u128,
    /// Which database the statements ran on (its name, for the header).
    pub database: String,
}

/// The SQL work of the IDE: the database of the project open in its own thread, and its schema.
#[derive(Default)]
pub struct SqlState {
    pub worker: Option<DbWorker>,
    pub schema: Option<DbSchema>,
    /// The database the worker has open.
    pub db_path: Option<PathBuf>,
    /// Why the database could not be opened (shown while it stands).
    pub open_error: Option<String>,
    /// Statements still running (the interface keeps asking for frames while they do).
    pub running: usize,
    /// What runs now (named in the Results tab when it answers).
    pub running_key: String,
}

/// The file extensions a project database has.
const DB_EXTENSIONS: &[&str] = &["db", "sqlite", "sqlite3", "db3"];

impl App {
    /// Is document `idx` a SQL console: its language is one of a module with the SQL engine?
    pub fn is_sql_doc(&self, idx: usize) -> bool {
        let Some(doc) = self.docs.get(idx).filter(|d| d.diff.is_none() && d.page.is_none()) else { return false };
        let Some(language) = doc.ed.language() else { return false };
        self.mods.installed.iter().any(|m| m.enabled && m.manifest.builtin == "sql" && m.manifest.languages.iter().any(|l| l.id == language.id()))
    }

    fn sql_statements(&self, idx: usize) -> Vec<Statement> {
        if !self.is_sql_doc(idx) {
            return vec![];
        }
        db::split_statements(self.docs[idx].ed.text())
    }

    /// The run buttons: one on the first line of every statement of the console.
    pub fn sql_icons(&mut self, idx: usize) -> Vec<GutterIcon> {
        let pal = self.pal;
        self.sql_statements(idx).into_iter().map(|s| GutterIcon {
            line: self.docs[idx].ed.line_of(s.start),
            icon: crate::icons::Icon::Play,
            color: pal.ok,
            tip: i18n::fmt("Run '{}'", &[&s.title()]),
        }).collect()
    }

    /// What the menu of a statement's run button offers.
    pub fn sql_actions(&mut self, idx: usize, line: usize) -> Vec<(crate::icons::Icon, String, GutterAction)> {
        let statements = self.sql_statements(idx);
        let path = self.docs[idx].path.clone().unwrap_or_default();
        let mut items = vec![];
        for s in &statements {
            if self.docs[idx].ed.line_of(s.start) == line {
                items.push((crate::icons::Icon::Play, i18n::fmt("Run '{}'", &[&s.title()]), GutterAction::RunSql(path.clone(), s.start)));
            }
        }
        if !statements.is_empty() {
            items.push((crate::icons::Icon::File, tr("Run the whole file").to_string(), GutterAction::RunSqlFile(path)));
        }
        items
    }

    /// The database the project's consoles run on: the one the project chose, or the only database file in it.
    fn sql_db_choice(&self) -> Option<PathBuf> {
        let root = self.project_root()?.to_path_buf();
        if let Some(chosen) = &self.project_cfg.sql_database {
            let p = root.join(chosen);
            if p.is_file() {
                return Some(p);
            }
        }
        let mut found: Vec<PathBuf> = vec![];
        collect_databases(&root, &mut found, 0);
        if found.len() == 1 {
            return found.pop();
        }
        None
    }

    /// Make sure the database is open in its thread. Returns false when the project has none to offer: the caller
    /// asks for a file in that case.
    fn sql_ensure_worker(&mut self) -> bool {
        if self.sql.worker.is_some() {
            return true;
        }
        let Some(path) = self.sql_db_choice() else { return false };
        self.sql_open(&path)
    }

    fn sql_open(&mut self, path: &Path) -> bool {
        self.sql.worker = Some(DbWorker::open(path));
        self.sql.db_path = Some(path.to_path_buf());
        self.sql.open_error = None;
        true
    }

    /// Ask for a database file, remember it for the project, and open it.
    pub fn sql_pick_database(&mut self) {
        let start = self.db_choice_dir();
        if let Some(p) = rfd::FileDialog::new()
            .set_title(tr("Open database"))
            .add_filter("SQLite database", DB_EXTENSIONS)
            .set_directory(start)
            .pick_file()
        {
            if let Some(root) = self.project_root().map(Path::to_path_buf) {
                let rel = p.strip_prefix(&root).map(|r| r.to_path_buf()).unwrap_or_else(|_| p.clone());
                self.project_cfg.sql_database = Some(rel.to_string_lossy().replace('\\', "/"));
                let _ = crate::app_env::save_config(&root, &self.project_cfg);
            }
            self.sql_open(&p);
            // the tree and the completion start from what this database holds
            if let Some(w) = &self.sql.worker {
                w.schema();
            }
        }
    }

    /// Where the database picker starts: the project's root, or the folder of the last database.
    fn db_choice_dir(&self) -> PathBuf {
        self.sql.db_path.clone()
            .or_else(|| self.project_root().map(Path::to_path_buf))
            .unwrap_or_default()
    }

    /// Run the statement at `offset` of the console (the document when the path is empty).
    pub fn sql_run_at(&mut self, path: &Path, offset: usize) {
        let idx = if path.as_os_str().is_empty() { Some(self.active) } else { self.docs.iter().position(|d| d.path.as_deref() == Some(path)) };
        let Some(idx) = idx.filter(|&i| self.is_sql_doc(i)) else {
            self.toast(tr("There is no SQL statement there any more"), Sev::Weak);
            return;
        };
        let statements = self.sql_statements(idx);
        let Some(statement) = statements.iter().find(|s| offset >= s.start && offset <= s.start + s.sql.len()).cloned() else {
            self.toast(tr("There is no SQL statement there any more"), Sev::Weak);
            return;
        };
        self.sql_exec_text(&statement.sql, statement.title());
    }

    /// Run the statement the caret is in (or what is selected), the way the run button of its line does.
    pub fn sql_run_statement(&mut self) {
        if !self.is_sql_doc(self.active) {
            return;
        }
        let (text, caret) = {
            let d = &self.docs[self.active];
            (d.ed.text().to_string(), d.ed.caret())
        };
        let (from, to) = self.docs[self.active].ed.selection();
        if from != to {
            // what is selected runs as it stands
            let sql = text[from..to].to_string();
            if !sql.trim().is_empty() {
                let title = sql.trim().to_string();
                self.sql_exec_text(&sql, title);
                return;
            }
        }
        let statements = db::split_statements(&text);
        let Some(statement) = statements.iter().find(|s| caret >= s.start && caret <= s.start + s.sql.len()).cloned() else {
            self.toast(tr("The caret is not in a SQL statement"), Sev::Weak);
            return;
        };
        self.sql_exec_text(&statement.sql, statement.title());
    }

    /// Run every statement of the console (the active document when the path is empty).
    pub fn sql_run_file(&mut self, path: &Path) {
        let idx = if path.as_os_str().is_empty() { Some(self.active) } else { self.docs.iter().position(|d| d.path.as_deref() == Some(path)) };
        let Some(idx) = idx.filter(|&i| self.is_sql_doc(i)) else { return };
        if self.sql_statements(idx).is_empty() {
            self.toast(tr("The file has no SQL statements"), Sev::Weak);
            return;
        }
        let text = self.docs[idx].ed.text().to_string();
        let title = self.docs[idx].name();
        self.sql_exec_text(&text, title);
    }

    fn sql_exec_text(&mut self, sql: &str, title: String) {
        if sql.trim().is_empty() {
            return;
        }
        if !self.sql_ensure_worker() {
            // no database in the project: the user picks one, and the statement runs on it
            self.sql_pick_database();
            if self.sql.worker.is_none() {
                self.toast(tr("No database to run the statement on"), Sev::Weak);
                return;
            }
        }
        let Some(worker) = &self.sql.worker else { return };
        self.sql.running += 1;
        self.sql.running_key = title.clone();
        worker.exec("console", sql);
        self.status = i18n::fmt("Running {}", &[&title]).into();
    }

    /// Collect what the database thread answered: results into the Results tab, the schema into the tree and the
    /// completion. Called every frame.
    pub fn poll_sql(&mut self) {
        let Some(worker) = self.sql.worker.as_ref() else { return };
        let events: Vec<DbEvent> = std::iter::from_fn(|| worker.event()).collect();
        if events.is_empty() {
            return;
        }
        let mut schema_refresh = false;
        for event in events {
            match event {
                DbEvent::Opened { path, schema } => {
                    self.sql.schema = Some(schema);
                    self.sql.db_path = Some(path);
                    self.sql.open_error = None;
                }
                DbEvent::OpenFailed { path, message } => {
                    self.sql.worker = None;
                    self.sql.db_path = Some(path);
                    self.sql.open_error = Some(message.clone());
                    self.toast(i18n::fmt("Cannot open the database: {}", &[&message]), Sev::Error);
                }
                DbEvent::Results { key, sets, ms } => {
                    if key == "console" {
                        self.sql.running = self.sql.running.saturating_sub(1);
                        let database = self.sql.db_path.as_deref().map(file_stem).unwrap_or_default();
                        self.show_sql_results(ResultsView { title: self.sql.running_key.clone(), sets, active: 0, error: None, ms, database });
                        self.status = String::new();
                    }
                    schema_refresh = true;
                }
                DbEvent::Failed { key, message } => {
                    if key == "console" {
                        self.sql.running = self.sql.running.saturating_sub(1);
                        let database = self.sql.db_path.as_deref().map(file_stem).unwrap_or_default();
                        self.show_sql_results(ResultsView { title: self.sql.running_key.clone(), sets: vec![], active: 0, error: Some(message.clone()), ms: 0, database });
                        self.status = String::new();
                    }
                }
                DbEvent::Schema(schema) => {
                    self.sql.schema = Some(schema);
                }
            }
        }
        if schema_refresh {
            if let Some(w) = &self.sql.worker {
                w.schema();
            }
        }
    }

    /// Put the results where the Results tab shows them (opening the tab when nothing is there yet).
    fn show_sql_results(&mut self, view: ResultsView) {
        match self.docs.iter_mut().find(|d| d.page == Some(crate::doc::Page::Results)) {
            Some(doc) => doc.results = Some(Box::new(view)),
            None => {
                let mut doc = Doc::results_tab();
                doc.results = Some(Box::new(view));
                self.docs.push(doc);
            }
        }
    }

    /// The SQL state is the project's: closing the project closes its database.
    pub fn sql_reset(&mut self) {
        if let Some(mut w) = self.sql.worker.take() {
            w.stop();
        }
        self.sql.schema = None;
        self.sql.db_path = None;
        self.sql.open_error = None;
        self.sql.running = 0;
        self.sql.running_key.clear();
    }
}

fn file_stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Collect the database files under `dir` (a short look: two levels down, and not into hidden or vendor folders).
fn collect_databases(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 2 || out.len() > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            collect_databases(&path, out, depth + 1);
        } else if DB_EXTENSIONS.iter().any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e))) {
            out.push(path);
        }
    }
}
