//! The SQL console: a run button next to every statement of a `.sql` file (the language of a module whose engine is
//! `sql`), statements run on the SQLite database of the project in a thread of their own, and what they came back
//! with shown in the Results tab as a grid. The schema of the database is kept for the completion of names and the
//! database tree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ide_core::db::{self, DbEvent, DbSchema, DbValue, DbWorker, GridMutation, ResultSet, Statement};

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
    pub sets: Vec<Arc<ResultSet>>,
    /// Which set the grid shows when there are several.
    pub active: usize,
    /// The statement did not run: the database's message.
    pub error: Option<String>,
    /// How long the statements took.
    pub ms: u128,
    /// Which database the statements ran on (its name, for the header).
    pub database: String,
    pub table: bool,
    /// Set when the grid reads one table and can be written back to (the Database window opens such grids).
    pub edit: Option<EditableTable>,
    /// Cell changes waiting for the commit: grid row and column (column 0 is the hidden `rowid`), the new value
    /// (`None`: NULL).
    pub changes: BTreeMap<(usize, usize), Option<String>>,
    /// Rows of the grid marked for deletion (by grid row).
    pub deleted: BTreeSet<usize>,
    /// Rows added below the last row the table has: their value per column (`None`: NULL).
    pub added: Vec<BTreeMap<usize, Option<String>>>,
    /// The cell the keyboard works on (grid row, column 0 being `rowid`).
    pub selected: Option<(usize, usize)>,
    /// The cell being edited, if one is.
    pub editing: Option<Editing>,
}

/// One cell of the grid while its value is being typed over.
#[derive(Clone, Debug, Default)]
pub struct Editing {
    /// The cell: grid row, and column (0 being the hidden `rowid`).
    pub at: (usize, usize),
    pub buffer: String,
    /// The text field has been on screen once (its caret asked for the focus).
    pub shown: bool,
    pub changed: bool,
}

/// A grid that writes back: the table it reads and the read that fills it (`rowid` first), re-run after a commit.
#[derive(Clone, Debug, Default)]
pub struct EditableTable {
    pub name: String,
    pub query: String,
    pub rowid: &'static str,
    pub generated: Vec<String>,
}

impl ResultsView {
    /// The set the grid draws.
    pub fn set(&self) -> Option<&ResultSet> {
        self.sets.get(self.active).map(Arc::as_ref)
    }

    /// How many changes wait for the commit.
    pub fn pending(&self) -> usize {
        self.changes.len() + self.deleted.len() + self.added.len()
    }

    pub fn can_edit_cell(&self, row: usize, col: usize) -> bool {
        let (Some(edit), Some(set)) = (&self.edit, self.set()) else { return false };
        col > 0
            && set.columns.get(col).is_some_and(|name| !edit.generated.contains(name))
            && !self.deleted.contains(&row)
            && row < set.rows.len() + self.added.len()
    }

    pub fn cell_text(&self, row: usize, col: usize) -> String {
        let Some(set) = self.set() else { return String::new() };
        if row >= set.rows.len() {
            return self.added.get(row - set.rows.len()).and_then(|r| r.get(&col)).cloned().flatten().unwrap_or_default();
        }
        if let Some(value) = self.changes.get(&(row, col)) {
            return value.clone().unwrap_or_default();
        }
        match set.rows.get(row).and_then(|r| r.get(col)) {
            Some(DbValue::Null | DbValue::Blob(_)) | None => String::new(),
            Some(value) => value.render(),
        }
    }

    pub fn set_cell(&mut self, row: usize, col: usize, value: Option<String>) {
        if !self.can_edit_cell(row, col) {
            return;
        }
        let set = self.set().expect("editable cell has a result set");
        let base = set.rows.len();
        if row >= base {
            self.added[row - base].insert(col, value);
        } else if match (&set.rows[row][col], &value) {
            (DbValue::Null, None) => true,
            (DbValue::Text(old), Some(new)) => old == new,
            _ => false,
        } {
            self.changes.remove(&(row, col));
        } else {
            self.changes.insert((row, col), value);
        }
    }
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
    request: Option<SqlRequest>,
    /// The tables whose columns the database tree shows.
    pub expanded: Vec<String>,
}

enum SqlRequest {
    Console(String),
    Table { name: String, edit: Option<EditableTable> },
    Save(EditableTable),
}

impl SqlState {
    pub fn running(&self) -> bool {
        self.request.is_some()
    }

    pub fn opening(&self) -> bool {
        self.worker.is_some() && self.schema.is_none()
    }
}

/// The file extensions a project database has.
const DB_EXTENSIONS: &[&str] = &["db", "sqlite", "sqlite3", "db3"];

impl App {
    /// Is the SQL engine of the IDE on: its module is installed and enabled?
    pub fn sql_enabled(&self) -> bool {
        self.mods.installed.iter().any(|m| m.enabled && m.manifest.builtin == "sql")
    }

    /// What the database offers where a word is being written: the tables after `FROM`/`JOIN`/`INTO`/`UPDATE`/
    /// `TABLE`, the columns of the table an alias names after its dot, and elsewhere every table plus the columns
    /// of the tables the statement reads. `word_start` is where the word begins in `text`.
    pub fn sql_completions(&self, text: &str, word_start: usize) -> Vec<(String, &'static str, u8)> {
        let Some(schema) = &self.sql.schema else { return vec![] };
        let before = &text[..word_start];
        // `u.`: the columns of the table `u` names
        if let Some(alias) = alias_before_dot(before) {
            let statement = statement_around(text, word_start);
            let named = sql_aliases(&statement).into_iter().find(|(_table, name)| alias == *name).map(|(table, _)| table);
            if let Some(table) = named.and_then(|t| schema.table(&t).map(|t| t.to_owned())) {
                return table.columns.iter().map(|c| (c.name.clone(), "v", 0u8)).collect();
            }
            return vec![];
        }
        // where a table belongs: after the words that ask for one
        if matches!(ends_with_word(before).as_deref(), Some("FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE")) {
            return schema.tables.iter().map(|t| (t.name.clone(), "C", 1u8)).collect();
        }
        // anywhere else: the tables, and the columns of what the statement reads
        let statement = statement_around(text, word_start);
        let tables: Vec<&ide_core::db::DbTable> = sql_aliases(&statement).into_iter().filter_map(|(t, _)| schema.table(&t)).collect();
        let mut out: Vec<(String, &'static str, u8)> = schema.tables.iter().map(|t| (t.name.clone(), "C", 3u8)).collect();
        for t in tables {
            for c in &t.columns {
                out.push((c.name.clone(), "v", 2u8));
            }
        }
        out
    }

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
    pub fn sql_ensure_worker(&mut self) -> bool {
        if self.sql.worker.is_some() {
            return true;
        }
        let Some(path) = self.sql_db_choice() else { return false };
        self.sql_open(&path)
    }

    fn sql_open(&mut self, path: &Path) -> bool {
        self.sql_reset();
        if let Some(view) = self.sql_results_view_mut() {
            *view = ResultsView::default();
        }
        self.sql.worker = Some(DbWorker::open(path));
        self.sql.db_path = Some(path.to_path_buf());
        self.sql.open_error = None;
        true
    }

    /// Ask for a database file, remember it for the project, and open it.
    pub fn sql_pick_database(&mut self) {
        if !self.sql_can_replace_results() {
            return;
        }
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
        if !self.sql_can_replace_results() {
            return;
        }
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
        self.sql.request = Some(SqlRequest::Console(title.clone()));
        worker.exec("query", sql);
        self.status = i18n::fmt("Running {}", &[&title]);
    }

    /// Collect what the database thread answered: results into the Results tab, the schema into the tree and the
    /// completion. Called every frame.
    pub fn poll_sql(&mut self) {
        let Some(worker) = self.sql.worker.as_ref() else { return };
        let events: Vec<DbEvent> = std::iter::from_fn(|| worker.event()).collect();
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                DbEvent::Opened { path, schema } => {
                    self.sql.schema = Some(schema);
                    self.sql.db_path = Some(path);
                    self.sql.open_error = None;
                }
                DbEvent::OpenFailed { path, message } => {
                    self.sql.worker = None;
                    self.sql.request = None;
                    self.status.clear();
                    self.sql.db_path = Some(path);
                    self.sql.open_error = Some(message.clone());
                    self.toast(i18n::fmt("Cannot open the database: {}", &[&message]), Sev::Error);
                }
                DbEvent::Results { sets, ms, schema, .. } => {
                    self.sql.schema = Some(schema);
                    let Some(request) = self.sql.request.take() else { continue };
                    let activate = matches!(request, SqlRequest::Table { .. });
                    let table = !matches!(request, SqlRequest::Console(_));
                    let (title, edit) = match request {
                        SqlRequest::Console(title) => (title, None),
                        SqlRequest::Table { name, edit } => (name, edit),
                        SqlRequest::Save(edit) => {
                            self.toast(tr("Changes saved"), Sev::Weak);
                            (edit.name.clone(), Some(edit))
                        }
                    };
                    let database = self.sql.db_path.as_deref().map(file_stem).unwrap_or_default();
                    let active = sets.iter().rposition(|s| !s.columns.is_empty()).unwrap_or(0);
                    self.show_sql_results(ResultsView { title, sets: sets.into_iter().map(Arc::new).collect(), active, ms, database, table, edit, ..Default::default() });
                    if activate && let Some(i) = self.docs.iter().position(|d| d.page == Some(crate::doc::Page::Results)) {
                        self.active = i;
                        self.editor_focus = false;
                        self.completion.active = false;
                    }
                    self.status.clear();
                }
                DbEvent::Failed { message, schema, .. } => {
                    self.sql.schema = Some(schema);
                    let Some(request) = self.sql.request.take() else { continue };
                    self.status.clear();
                    let table = matches!(request, SqlRequest::Table { .. });
                    let title = match request {
                        SqlRequest::Console(title) => title,
                        SqlRequest::Table { name, .. } => name,
                        SqlRequest::Save(_) => {
                            let message = match message.as_str() {
                                "The table has changed since it was opened" => tr("The table has changed since it was opened"),
                                "A row has changed since the table was opened" => tr("A row has changed since the table was opened"),
                                _ => &message,
                            };
                            self.toast(i18n::fmt("Cannot save the changes: {}", &[&message]), Sev::Error);
                            continue;
                        }
                    };
                    let database = self.sql.db_path.as_deref().map(file_stem).unwrap_or_default();
                    self.show_sql_results(ResultsView { title, error: Some(message), database, table, ..Default::default() });
                }
                DbEvent::Schema(schema) => self.sql.schema = Some(schema),
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
        self.sql = SqlState::default();
    }

    pub fn sql_can_replace_results(&mut self) -> bool {
        let message = if self.sql.running() {
            "Wait for the SQL operation to finish"
        } else if self.sql_results_view_mut().is_some_and(|v| v.pending() > 0 || v.editing.is_some()) {
            "Save or discard the table edits first"
        } else {
            return true;
        };
        self.toast(tr(message), Sev::Weak);
        false
    }

    /// Show the first rows of a table or view from the Database window.
    pub fn sql_show_table(&mut self, name: &str) {
        if !self.sql_can_replace_results() || !self.sql_ensure_worker() {
            return;
        }
        let Some(table) = self.sql.schema.as_ref().and_then(|s| s.table(name)) else { return };
        let qualified = format!("main.{}", db::quote_ident(name));
        let edit = table.rowid.map(|rowid| EditableTable {
            name: name.to_string(),
            query: format!("SELECT {rowid} AS light_rowid, * FROM {qualified} LIMIT 500"),
            rowid,
            generated: table.generated.clone(),
        });
        let query = edit.as_ref().map_or_else(|| format!("SELECT * FROM {qualified} LIMIT 500"), |e| e.query.clone());
        self.sql.request = Some(SqlRequest::Table { name: name.to_string(), edit });
        if let Some(w) = &self.sql.worker {
            w.exec("query", &query);
        }
    }

    /// The Results tab (when it shows a table grid): add an empty row below the last one.
    pub fn sql_add_row(&mut self) {
        let Some(view) = self.sql_results_view_mut() else { return };
        view.added.push(BTreeMap::new());
        let row = view.set().map(|s| s.rows.len()).unwrap_or(0) + view.added.len() - 1;
        view.selected = Some((row, 1));
    }

    /// The Results tab: the selected row goes (a row of the table) or stops being new (a row just added).
    pub fn sql_delete_row(&mut self) {
        let Some(view) = self.sql_results_view_mut() else { return };
        let Some((row, _)) = view.selected else { return };
        let base = view.set().map(|s| s.rows.len()).unwrap_or(0);
        if row >= base {
            let at = row - base;
            if at < view.added.len() {
                view.added.remove(at);
            }
            view.selected = None;
        } else {
            view.deleted.insert(row);
            view.changes.retain(|&(r, _), _| r != row);
        }
    }

    /// The Results tab: every change, addition and deletion goes.
    pub fn sql_revert(&mut self) {
        let Some(view) = self.sql_results_view_mut() else { return };
        view.changes.clear();
        view.deleted.clear();
        view.added.clear();
        view.selected = None;
        view.editing = None;
    }

    /// Save the changes and refresh the grid in one worker operation.
    pub fn sql_commit(&mut self) {
        if self.sql.running() {
            return;
        }
        self.sql_editing_finish(true);
        let Some(view) = self.sql_results_view_mut() else { return };
        let (Some(edit), Some(set)) = (view.edit.clone(), view.set()) else { return };
        let changes = grid_mutations(&edit, set, &view.changes, &view.deleted, &view.added);
        if changes.is_empty() {
            return;
        }
        let Some(worker) = &self.sql.worker else { return };
        worker.save_grid(edit.name.clone(), edit.rowid, edit.query.clone(), changes);
        self.sql.request = Some(SqlRequest::Save(edit));
        self.status = tr("Saving changes...").into();
    }

    /// The Results tab of the active document, if it is the one shown.
    fn sql_results_view_mut(&mut self) -> Option<&mut ResultsView> {
        self.docs.iter_mut().find(|d| d.page == Some(crate::doc::Page::Results)).and_then(|d| d.results.as_deref_mut())
    }

    /// Enter or a click away keeps the current text. Escape drops it.
    pub fn sql_editing_finish(&mut self, keep: bool) {
        let Some(view) = self.sql_results_view_mut() else { return };
        let Some(editing) = view.editing.take() else { return };
        let Editing { at: (row, col), buffer, changed, .. } = editing;
        if keep && changed {
            view.set_cell(row, col, Some(buffer));
        }
    }
}

/// Bind values separately from SQL. SQLite applies each destination column's affinity.
fn grid_mutations(edit: &EditableTable, set: &ResultSet, changes: &BTreeMap<(usize, usize), Option<String>>, deleted: &BTreeSet<usize>, added: &[BTreeMap<usize, Option<String>>]) -> Vec<GridMutation> {
    let table = format!("main.{}", db::quote_ident(&edit.name));
    let names: Vec<String> = set.columns.iter().map(|name| db::quote_ident(name)).collect();
    let rowid = edit.rowid;
    let mut mutations = Vec::new();
    for &row in deleted {
        mutations.push(GridMutation { sql: format!("DELETE FROM {table} WHERE {rowid} = ?"), values: vec![], before: Some(set.rows[row].clone()) });
    }
    let value = |v: &Option<String>| v.as_ref().map_or(DbValue::Null, |s| DbValue::Text(s.clone()));
    let mut updates = changes.iter().peekable();
    while let Some((&(row, col), v)) = updates.next() {
        if deleted.contains(&row) {
            continue;
        }
        let mut assignments = vec![format!("{} = ?", names[col])];
        let mut values = vec![value(v)];
        while let Some(((_, col), v)) = updates.next_if(|((r, _), _)| *r == row) {
            assignments.push(format!("{} = ?", names[*col]));
            values.push(value(v));
        }
        mutations.push(GridMutation { sql: format!("UPDATE OR ABORT {table} SET {} WHERE {rowid} = ?", assignments.join(", ")), values, before: Some(set.rows[row].clone()) });
    }
    for row in added {
        let sql = if row.is_empty() {
            format!("INSERT OR ABORT INTO {table} DEFAULT VALUES")
        } else {
            let columns = row.keys().map(|col| names[*col].as_str()).collect::<Vec<_>>().join(", ");
            let params = vec!["?"; row.len()].join(", ");
            format!("INSERT OR ABORT INTO {table} ({columns}) VALUES ({params})")
        };
        mutations.push(GridMutation { sql, values: row.values().map(value).collect(), before: None });
    }
    mutations
}

fn file_stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The words of SQL text, upper-case, with the byte range each came from. Quoted identifiers count as words
/// (their quotes are taken away).
fn sql_words(text: &str) -> Vec<(String, usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            out.push((text[start..i].to_ascii_uppercase(), start, i));
        } else if c == b'"' || c == b'`' {
            // a quoted identifier: one word without its quotes
            let start = i;
            i += 1;
            while i < b.len() && b[i] != c {
                i += 1;
            }
            i = (i + 1).min(b.len());
            out.push((text[start + 1..(i - 1).min(text.len())].to_ascii_uppercase(), start, i));
        } else {
            i += 1;
        }
    }
    out
}

/// Words that cannot be the alias of a table in `FROM t [AS] a`.
const NOT_AN_ALIAS: &[&str] = &[
    "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION", "INTERSECT", "EXCEPT", "INNER", "LEFT",
    "RIGHT", "FULL", "OUTER", "CROSS", "NATURAL", "JOIN", "ON", "USING", "SET", "VALUES", "SELECT", "AS",
];

/// The tables a statement's `FROM`/`JOIN` clauses name: each table under its alias, and under its own name.
fn sql_aliases(sql: &str) -> Vec<(String, String)> {
    let words = sql_words(sql);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let (word, _, end) = &words[i];
        if (word == "FROM" || word == "JOIN") && *end < sql.len()
            && let Some((table, _, _)) = words.get(i + 1)
        {
            let table = table.clone();
            if let Some((alias, _, _)) = words.get(i + 2) {
                let alias = if alias == "AS" { words.get(i + 3).map(|w| w.0.clone()) } else { Some(alias.clone()) };
                if let Some(alias) = alias.filter(|a| !NOT_AN_ALIAS.contains(&a.as_str())) {
                    out.push((table.clone(), alias));
                }
            }
            // a table also answers to its own name, with or without an alias
            out.push((table.clone(), table));
        }
        i += 1;
    }
    out
}

/// The statement `offset` is in (the whole text when it is in none).
fn statement_around(text: &str, offset: usize) -> String {
    for s in db::split_statements(text) {
        if offset >= s.start && offset <= s.start + s.sql.len() {
            return s.sql;
        }
    }
    text.to_string()
}

/// The last identifier `text` ends with (after whitespace), if it ends with one.
fn ends_with_word(text: &str) -> Option<String> {
    let t = text.trim_end();
    let cut = t.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$')).map_or(0, |p| p + 1);
    (cut < t.len()).then(|| t[cut..].to_ascii_uppercase())
}

/// Does `text` end with `alias.` (the word the columns would belong to)?
fn alias_before_dot(text: &str) -> Option<String> {
    let t = text.trim_end();
    t.ends_with('.').then(|| {
        let head = &t[..t.len() - 1];
        let cut = head.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$')).map_or(0, |p| p + 1);
        head[cut..].to_ascii_uppercase()
    }).filter(|w| !w.is_empty())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ide_core::db::DbTable;

    fn schema() -> DbSchema {
        DbSchema {
            tables: vec![
                DbTable { name: "users".into(), is_view: false, columns: vec![
                    ide_core::db::DbColumn { name: "id".into(), ty: "INTEGER".into(), notnull: true, pk: true },
                    ide_core::db::DbColumn { name: "name".into(), ty: "TEXT".into(), notnull: false, pk: false },
                ], ..Default::default() },
                DbTable { name: "orders".into(), is_view: false, columns: vec![
                    ide_core::db::DbColumn { name: "user_id".into(), ty: "INTEGER".into(), notnull: false, pk: false },
                ], ..Default::default() },
            ],
        }
    }

    #[test]
    fn aliases_of_from_and_join() {
        let named = sql_aliases("SELECT * FROM users u JOIN orders o ON o.user_id = u.id WHERE u.name = 'x'");
        assert!(named.contains(&("USERS".to_string(), "U".to_string())));
        assert!(named.contains(&("USERS".to_string(), "USERS".to_string())));
        assert!(named.contains(&("ORDERS".to_string(), "O".to_string())));
        // `AS` counts, and a keyword is not an alias
        assert!(sql_aliases("FROM users AS u2").contains(&("USERS".to_string(), "U2".to_string())));
        assert!(!sql_aliases("FROM users WHERE").iter().any(|(_, n)| n == "WHERE"));
    }

    #[test]
    fn words_after_a_dot_and_before_the_caret() {
        assert_eq!(alias_before_dot("SELECT u."), Some("U".into()));
        assert_eq!(alias_before_dot("SELECT name FROM "), None);
        assert_eq!(ends_with_word("select * from "), Some("FROM".into()));
        assert_eq!(ends_with_word("select * "), None);
    }

    #[test]
    fn the_dot_offers_the_columns_of_the_aliased_table() {
        let mut app = App::new(ide_core::settings::Settings::default());
        app.sql.schema = Some(schema());
        let text = "SELECT u. FROM users u";
        let at = "SELECT u.".len();
        let offered: Vec<String> = app.sql_completions(text, at).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(offered, vec!["id", "name"]);
        // a table's own name works as its alias
        let text = "SELECT users. FROM users";
        let offered: Vec<String> = app.sql_completions(text, "SELECT users.".len()).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(offered, vec!["id", "name"]);
    }

    #[test]
    fn after_from_the_tables_the_others_after_the_columns() {
        let mut app = App::new(ide_core::settings::Settings::default());
        app.sql.schema = Some(schema());
        // right after FROM: the tables, first
        let after_from = app.sql_completions("SELECT * FROM ", "SELECT * FROM ".len());
        assert_eq!(after_from.first().map(|(n, _, s)| (n.as_str(), *s)), Some(("users", 1)));
        // anywhere else: the tables and the columns of what the statement reads
        let anywhere = app.sql_completions("SELECT * FROM users WHERE ", 8);
        let names: Vec<&str> = anywhere.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"orders"));
        assert!(names.contains(&"name"));
        assert!(names.contains(&"id"));
        // what the statement does not read is not among its columns
        assert!(!names.contains(&"user_id"));
    }

    struct TestDb {
        app: App,
        path: PathBuf,
    }

    impl TestDb {
        fn new(sql: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("light-sql-{}-{}.db", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_file(&path);
            let mut app = App::new(ide_core::settings::Settings::default());
            app.sql_open(&path);
            let mut test = Self { app, path };
            test.exec(sql);
            assert!(test.view().error.is_none(), "{:?}", test.view().error);
            test
        }

        fn settle(&mut self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                self.app.poll_sql();
                if !self.app.sql.running() {
                    return;
                }
                assert!(std::time::Instant::now() < deadline, "SQL operation timed out");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        fn exec(&mut self, sql: &str) {
            self.app.sql_exec_text(sql, "test".into());
            self.settle();
        }

        fn open(&mut self, name: &str) {
            self.app.sql_show_table(name);
            self.settle();
            assert_eq!(self.view().title, name);
        }

        fn save(&mut self) {
            self.app.sql_commit();
            self.settle();
        }

        fn view(&mut self) -> &mut ResultsView {
            self.app.sql_results_view_mut().unwrap()
        }

        fn rows(&mut self) -> &[Vec<DbValue>] {
            &self.view().set().unwrap().rows
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            self.app.sql_reset();
            let _ = std::fs::remove_file(&self.path);
        }
    }

    #[test]
    fn grid_saves_key_and_text_together_and_uses_insert_defaults() {
        let mut db = TestDb::new("CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT 'new', generated TEXT AS (name || '!')); INSERT INTO users VALUES (1, 'old');");
        db.open("users");
        assert!(!db.view().can_edit_cell(0, 3));
        db.view().set_cell(0, 1, Some("3".into()));
        db.view().set_cell(0, 2, Some("007'\0".into()));
        db.app.sql_add_row();
        db.app.sql_commit();
        db.app.sql_commit(); // a second click must not add a second row
        db.settle();
        assert_eq!(db.view().pending(), 0);
        assert_eq!(db.rows().len(), 2);
        assert_eq!(db.rows()[0][1..], [DbValue::Int(3), DbValue::Text("007'\0".into()), DbValue::Text("007'\0!".into())]);
        assert_eq!(db.rows()[1][2], DbValue::Text("new".into()));
    }

    #[test]
    fn constraint_failure_rolls_back_every_change_and_preserves_edits() {
        let mut db = TestDb::new("CREATE TABLE users(name TEXT NOT NULL); INSERT INTO users VALUES ('first'), ('second');");
        db.open("users");
        db.view().selected = Some((1, 1));
        db.app.sql_delete_row();
        db.view().set_cell(0, 1, None);
        db.save();
        assert_eq!(db.view().pending(), 2);
        assert_eq!(db.rows().len(), 2);
        db.app.sql_revert();
        db.exec("SELECT name FROM users ORDER BY rowid");
        assert_eq!(db.rows(), &[vec![DbValue::Text("first".into())], vec![DbValue::Text("second".into())]]);
    }

    #[test]
    fn rowid_columns_cannot_shadow_the_grid_key() {
        let mut db = TestDb::new("CREATE TABLE users(rowid INTEGER, name TEXT); INSERT INTO users VALUES (9, 'first'), (9, 'second'); CREATE TABLE no_id(id INTEGER PRIMARY KEY) WITHOUT ROWID; INSERT INTO no_id VALUES (1); CREATE TABLE shadowed(rowid, _rowid_, oid);");
        db.open("users");
        assert_eq!(db.view().edit.as_ref().unwrap().rowid, "_rowid_");
        db.view().selected = Some((1, 1));
        db.app.sql_delete_row();
        db.save();
        assert_eq!(db.rows().len(), 1);
        assert_eq!(db.rows()[0][2], DbValue::Text("first".into()));
        db.open("no_id");
        assert!(db.view().edit.is_none());
        assert_eq!(db.rows(), &[vec![DbValue::Int(1)]]);
        db.open("shadowed");
        assert!(db.view().edit.is_none());
    }

    #[test]
    fn external_row_changes_do_not_get_overwritten() {
        let mut db = TestDb::new("CREATE TABLE users(name TEXT); INSERT INTO users VALUES ('old');");
        db.open("users");
        db.view().set_cell(0, 1, Some("grid".into()));
        let worker = DbWorker::open(&db.path);
        let timeout = std::time::Duration::from_secs(5);
        assert!(matches!(worker.wait_event(timeout), Some(DbEvent::Opened { .. })));
        worker.exec("external", "UPDATE users SET name = 'external'");
        assert!(matches!(worker.wait_event(timeout), Some(DbEvent::Results { .. })));
        db.save();
        assert_eq!(db.view().pending(), 1);
        db.app.sql_revert();
        db.open("users");
        assert_eq!(db.rows()[0][1], DbValue::Text("external".into()));
    }

    #[test]
    fn results_keep_their_request_and_database() {
        let mut db = TestDb::new("CREATE TABLE first(name TEXT); CREATE TABLE second(name TEXT); INSERT INTO first VALUES ('first');");
        db.app.sql_show_table("first");
        db.app.sql_show_table("second");
        db.settle();
        assert_eq!(db.view().title, "first");
        db.view().set_cell(0, 1, Some("grid".into()));
        db.app.request_close_doc(db.app.active);
        db.open("first"); // a new query cannot replace a grid with edits
        assert_eq!(db.view().pending(), 1);
        let other = TestDb::new("CREATE TABLE first(name TEXT); INSERT INTO first VALUES ('other');");
        assert!(!db.app.sql_can_replace_results());
        db.app.sql_open(&other.path); // replacing the worker invalidates its old grid
        db.app.sql_commit();
        db.exec("SELECT name FROM first");
        assert_eq!(db.rows(), &[vec![DbValue::Text("other".into())]]);
        db.app.sql_reset();
    }
}
