//! SQLite, the engine the SQL console of the IDE runs on: opening a database of the project in a thread of its own,
//! running what a console file asks statement by statement, and reading the schema back (the tables, their columns)
//! for the database tree and the completion of names.
//!
//! The statements of a file are cut apart here, too: on the `;` that ends each one, with strings, comments and the
//! `BEGIN … END` bodies of triggers kept whole.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::{Batch, Connection, TransactionBehavior, params_from_iter};
use rusqlite::types::ValueRef;

/// A ceiling on what one result set holds, so that a `SELECT` over an unbounded table cannot take all memory.
const MAX_ROWS: usize = 50_000;

/// One value of a row, in the types SQLite keeps.
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl DbValue {
    /// What the grid of results shows for the value.
    pub fn render(&self) -> String {
        match self {
            DbValue::Null => "NULL".to_string(),
            DbValue::Int(n) => n.to_string(),
            DbValue::Real(x) => format!("{x:?}"),
            DbValue::Text(s) => s.clone(),
            DbValue::Blob(b) => format!("<{} B>", b.len()),
        }
    }

    fn of_ref(v: ValueRef<'_>) -> DbValue {
        match v {
            ValueRef::Null => DbValue::Null,
            ValueRef::Integer(n) => DbValue::Int(n),
            ValueRef::Real(x) => DbValue::Real(x),
            ValueRef::Text(t) => DbValue::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => DbValue::Blob(b.to_vec()),
        }
    }
}

/// What one statement came back with: the columns and rows of a `SELECT`, or how many rows a change touched.
#[derive(Clone, Debug, Default)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<DbValue>>,
    /// Rows the statement changed (`INSERT`, `UPDATE`, `DELETE`, `CREATE`), 0 for a `SELECT`.
    pub affected: usize,
    /// The row ceiling was reached and the rest was left unread.
    pub truncated: bool,
}

impl ResultSet {
    /// What to call the set: `42 rows` or `7 rows changed`.
    pub fn label(&self) -> String {
        if !self.columns.is_empty() {
            format!("{}{} rows", self.rows.len(), if self.truncated { "+" } else { "" })
        } else {
            format!("{} rows changed", self.affected)
        }
    }

    /// The rows as CSV, the way spreadsheets read it (`NULL` an empty field, a blob its size).
    pub fn to_csv(&self) -> String {
        self.to_csv_from(0)
    }

    pub fn to_csv_from(&self, skip: usize) -> String {
        let mut out = String::new();
        let field = |v: &DbValue, out: &mut String| {
            let text = match v {
                DbValue::Null => String::new(),
                DbValue::Blob(b) => format!("<{} B>", b.len()),
                other => other.render(),
            };
            if text.contains([',', '"', '\n', '\r']) {
                out.push('"');
                out.push_str(&text.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&text);
            }
        };
        for (i, c) in self.columns.iter().skip(skip).enumerate() {
            if i > 0 {
                out.push(',');
            }
            field(&DbValue::Text(c.clone()), &mut out);
        }
        out.push('\n');
        for row in &self.rows {
            for (i, v) in row.iter().skip(skip).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                field(v, &mut out);
            }
            out.push('\n');
        }
        out
    }
}

/// A column of a table, as `PRAGMA table_info` tells it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DbColumn {
    pub name: String,
    /// The declared type (`INTEGER`, `TEXT`, …), as written in the table.
    pub ty: String,
    pub notnull: bool,
    /// Part of the primary key (the position for compound keys is not kept).
    pub pk: bool,
}

/// A table or a view of the database.
#[derive(Clone, Debug, Default)]
pub struct DbTable {
    pub name: String,
    pub is_view: bool,
    pub columns: Vec<DbColumn>,
    pub rowid: Option<&'static str>,
    pub generated: Vec<String>,
}

impl DbTable {
    /// `table` or `view`, for the tree of the database.
    pub fn kind(&self) -> &'static str {
        if self.is_view { "view" } else { "table" }
    }
}

/// The tables and views of a database, tables first, both in name order.
#[derive(Clone, Debug, Default)]
pub struct DbSchema {
    pub tables: Vec<DbTable>,
}

impl DbSchema {
    pub fn table(&self, name: &str) -> Option<&DbTable> {
        self.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }

    pub fn table_names(&self) -> Vec<&str> {
        self.tables.iter().map(|t| t.name.as_str()).collect()
    }
}

/// A name put into SQL as an identifier: quoted, with the quotes of the name doubled.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The schema of an open database.
fn read_schema(conn: &Connection) -> rusqlite::Result<DbSchema> {
    let mut stmt = conn.prepare("SELECT name, type, wr FROM pragma_table_list WHERE schema = 'main' AND substr(name, 1, 7) <> 'sqlite_' AND type <> 'shadow'")?;
    let mut rows = stmt.raw_query();
    let mut tables = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let without_rowid: bool = row.get(2)?;
        match read_table(conn, name.clone(), &kind, without_rowid) {
            Ok(table) => tables.push(table),
            // An unresolved view or virtual table should not hide the rest of the schema.
            Err(_) if kind == "view" || kind == "virtual" => {
                tables.push(DbTable { name, is_view: kind == "view", ..Default::default() });
            }
            Err(error) => return Err(error),
        }
    }
    tables.sort_by(|a, b| a.is_view.cmp(&b.is_view).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(DbSchema { tables })
}

fn read_table(conn: &Connection, name: String, kind: &str, without_rowid: bool) -> rusqlite::Result<DbTable> {
    let mut table = DbTable { name, is_view: kind == "view", ..Default::default() };
    let mut stmt = conn.prepare("SELECT name, type, \"notnull\", pk, hidden FROM pragma_table_xinfo(?1, 'main')")?;
    let mut rows = stmt.query([&table.name])?;
    while let Some(row) = rows.next()? {
        let hidden: u8 = row.get(4)?;
        if hidden == 1 {
            continue; // virtual table columns excluded by SELECT *
        }
        let name: String = row.get(0)?;
        if hidden > 1 {
            table.generated.push(name.clone());
        }
        table.columns.push(DbColumn { name, ty: row.get(1)?, notnull: row.get(2)?, pk: row.get(3)? });
    }
    if kind == "table" && !without_rowid {
        table.rowid = ["rowid", "_rowid_", "oid"].into_iter()
            .find(|alias| !table.columns.iter().any(|column| column.name.eq_ignore_ascii_case(alias)));
    }
    Ok(table)
}

pub struct GridMutation {
    pub sql: String,
    pub values: Vec<DbValue>,
    /// The row as it was read, including the hidden row identifier. Inserts have no prior row.
    pub before: Option<Vec<DbValue>>,
}

impl From<&DbValue> for rusqlite::types::Value {
    fn from(value: &DbValue) -> Self {
        match value {
            DbValue::Null => Self::Null,
            DbValue::Int(n) => Self::Integer(*n),
            DbValue::Real(n) => Self::Real(*n),
            DbValue::Text(s) => Self::Text(s.clone()),
            DbValue::Blob(bytes) => Self::Blob(bytes.clone()),
        }
    }
}

/// What the console asked the worker to do.
enum DbCommand {
    /// Run SQL and report under this key (the file and offset it came from).
    Exec { key: String, sql: String },
    SaveGrid { table: String, rowid: &'static str, query: String, changes: Vec<GridMutation> },
    /// Read the schema again (after the console changed it).
    Schema,
    Stop,
}

/// What the worker reported back.
#[derive(Debug)]
pub enum DbEvent {
    Opened { path: PathBuf, schema: DbSchema },
    OpenFailed { path: PathBuf, message: String },
    Results { key: String, sets: Vec<ResultSet>, ms: u128, schema: DbSchema },
    Failed { key: String, message: String, schema: DbSchema },
    Schema(DbSchema),
}

/// Run the SQL of one string, statement by statement, collecting a result set per `SELECT` and per change.
fn exec_sql(conn: &Connection, sql: &str) -> Result<Vec<ResultSet>, String> {
    let mut sets = Vec::new();
    let mut batch = Batch::new(conn, sql);
    while let Some(mut stmt) = batch.next().map_err(|e| e.to_string())? {
        if stmt.column_count() == 0 {
            // a change, not a read; the rows it touched is the answer
            let before = conn.total_changes();
            let affected = stmt.raw_execute().map_err(|e| e.to_string())?;
            let affected = if conn.total_changes() == before { 0 } else { affected };
            let changed = ResultSet { affected, ..Default::default() };
            sets.push(changed);
        } else {
            let columns: Vec<String> = (0..stmt.column_count()).map(|i| stmt.column_name(i).unwrap_or("").to_string()).collect();
            let mut set = ResultSet { columns, ..Default::default() };
            {
                let mut rows = stmt.raw_query();
                while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                    if set.rows.len() == MAX_ROWS {
                        set.truncated = true;
                        break;
                    }
                    set.rows.push(read_values(row).map_err(|e| e.to_string())?);
                }
            }
            sets.push(set);
        }
    }
    Ok(sets)
}

fn read_values(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vec<DbValue>> {
    (0..row.as_ref().column_count()).map(|i| row.get_ref(i).map(DbValue::of_ref)).collect()
}

fn save_grid(conn: &mut Connection, name: String, rowid: &str, query: &str, changes: Vec<GridMutation>) -> Result<Vec<ResultSet>, String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let (kind, without_rowid) = tx.query_row(
        "SELECT type, wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
        [&name], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
    ).map_err(|e| e.to_string())?;
    let table = read_table(&tx, name, &kind, without_rowid).map_err(|e| e.to_string())?;
    if table.rowid != Some(rowid) {
        return Err("The table has changed since it was opened".into());
    }
    {
        let sql = format!("SELECT {rowid}, * FROM main.{} WHERE {rowid} = ?", quote_ident(&table.name));
        let mut read = tx.prepare(&sql).map_err(|e| e.to_string())?;
        for change in changes {
            let mut values = change.values;
            if let Some(before) = change.before {
                let Some(DbValue::Int(id)) = before.first() else { return Err("Invalid grid row identifier".into()) };
                let mut rows = read.query([id]).map_err(|e| e.to_string())?;
                let current = rows.next().and_then(|row| row.map(read_values).transpose()).map_err(|e| e.to_string())?;
                if current.as_ref() != Some(&before) {
                    return Err("A row has changed since the table was opened".into());
                }
                values.push(DbValue::Int(*id));
            }
            let count = tx.execute(&change.sql, params_from_iter(values.iter().map(rusqlite::types::Value::from))).map_err(|e| e.to_string())?;
            if count != 1 {
                return Err("A row has changed since the table was opened".into());
            }
        }
    }
    let sets = exec_sql(&tx, query)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(sets)
}

/// The database of the project open in a thread of its own, so that no query holds up the interface. Commands go in
/// one by one (they run in order); the answers come back on [`DbWorker::event`].
pub struct DbWorker {
    tx: Option<Sender<DbCommand>>,
    handle: Option<JoinHandle<()>>,
    events: Receiver<DbEvent>,
}

impl DbWorker {
    /// Open (or create) a database file and read its schema. The first event on the channel says whether that worked.
    pub fn open(path: &Path) -> DbWorker {
        let (tx, rx) = channel::<DbCommand>();
        let (ex, events) = channel::<DbEvent>();
        let path = path.to_path_buf();
        let handle = std::thread::Builder::new()
            .name("light-db".into())
            .spawn(move || worker(path, rx, ex))
            .expect("spawn light-db");
        DbWorker { tx: Some(tx), handle: Some(handle), events }
    }

    /// Run SQL; the answer arrives as a `DbEvent::Results` with the same key.
    pub fn exec(&self, key: &str, sql: &str) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(DbCommand::Exec { key: key.to_string(), sql: sql.to_string() });
        }
    }

    pub fn save_grid(&self, table: String, rowid: &'static str, query: String, changes: Vec<GridMutation>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(DbCommand::SaveGrid { table, rowid, query, changes });
        }
    }

    /// Ask for the schema as it is now.
    pub fn schema(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(DbCommand::Schema);
        }
    }

    /// The next answered event, if one is here (the caller polls every frame).
    pub fn event(&self) -> Option<DbEvent> {
        self.events.try_recv().ok()
    }

    /// Wait for the next event (tests and startup).
    pub fn wait_event(&self, timeout: Duration) -> Option<DbEvent> {
        self.events.recv_timeout(timeout).ok()
    }

    /// Stop the thread after queued commands have finished.
    pub fn stop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(DbCommand::Stop);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for DbWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker(path: PathBuf, rx: Receiver<DbCommand>, ex: Sender<DbEvent>) {
    let open = Connection::open(&path).and_then(|conn| read_schema(&conn).map(|schema| (conn, schema)));
    let (mut conn, schema) = match open {
        Ok(opened) => opened,
        Err(e) => {
            let _ = ex.send(DbEvent::OpenFailed { path, message: e.to_string() });
            return;
        }
    };
    if ex.send(DbEvent::Opened { path: path.clone(), schema }).is_err() {
        return;
    }
    while let Ok(cmd) = rx.recv() {
        let started = Instant::now();
        let (key, result) = match cmd {
            DbCommand::Exec { key, sql } => (key, exec_sql(&conn, &sql)),
            DbCommand::SaveGrid { table, rowid, query, changes } => ("edit".into(), save_grid(&mut conn, table, rowid, &query, changes)),
            DbCommand::Schema => {
                let schema = read_schema(&conn).unwrap_or_default();
                if ex.send(DbEvent::Schema(schema)).is_err() {
                    break;
                }
                continue;
            }
            DbCommand::Stop => break,
        };
        let ms = started.elapsed().as_millis();
        let schema = read_schema(&conn).unwrap_or_default();
        let event = match result {
            Ok(sets) => DbEvent::Results { key, sets, ms, schema },
            Err(message) => DbEvent::Failed { key, message, schema },
        };
        if ex.send(event).is_err() {
            break;
        }
    }
}

/// One statement of a console file: where it starts in the whole text and its SQL.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// Offset of the statement's first non-space character in the text.
    pub start: usize,
    pub sql: String,
}

impl Statement {
    /// The first words of the statement, to name it by (`CREATE TABLE users`, `SELECT * FROM …`). Leading comments
    /// are skipped: they are not what the statement does.
    pub fn title(&self) -> String {
        let sql = strip_lead_comments(&self.sql);
        let mut title = String::new();
        for word in sql.split_whitespace() {
            if !title.is_empty() {
                title.push(' ');
            }
            title.push_str(word);
            if title.len() >= 24 {
                break;
            }
        }
        title
    }
}

/// The SQL with the comments at its start taken away.
fn strip_lead_comments(sql: &str) -> &str {
    let b = sql.as_bytes();
    let mut i = 0;
    loop {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if b.get(i) == Some(&b'-') && b.get(i + 1) == Some(&b'-') {
            i = skip_until(b, i + 2, b'\n');
        } else if b.get(i) == Some(&b'/') && b.get(i + 1) == Some(&b'*') {
            i = skip_block_comment(b, i + 2);
        } else {
            return &sql[i.min(sql.len())..];
        }
    }
}

/// Cut console text into statements on the `;` that ends each one. Strings (`'…'`, with `''` an escaped quote),
/// quoted identifiers (`"…"`, `` `…` ``, `[…]`) and the comments (`-- …`, `/* … */`) stay whole, and so do the
/// `BEGIN … END` bodies of triggers and `CASE … END` expressions (their `;` belong to the statement).
pub fn split_statements(text: &str) -> Vec<Statement> {
    let b = text.as_bytes();
    let mut out: Vec<Statement> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i = match c {
            b'\'' | b'"' | b'`' => skip_quoted(b, i + 1, c, true),
            b'[' => skip_quoted(b, i + 1, b']', false),
            b'-' if b.get(i + 1) == Some(&b'-') => skip_until(b, i + 2, b'\n'),
            b'/' if b.get(i + 1) == Some(&b'*') => skip_block_comment(b, i + 2),
            _ => {
                if c == b';'
                    && let Ok(sql) = CString::new(&b[start..=i])
                {
                    // SQLite recognizes trigger bodies and transaction statements using its own tokenizer.
                    // The buffer stays alive and NUL-terminated for the duration of the call.
                    if unsafe { rusqlite::ffi::sqlite3_complete(sql.as_ptr()) } != 0 {
                        push_statement(&mut out, text, &mut start, i);
                    }
                }
                i + 1
            }
        };
    }
    push_statement(&mut out, text, &mut start, b.len());
    out
}

fn push_statement(out: &mut Vec<Statement>, text: &str, start: &mut usize, end: usize) {
    let sql = &text[*start..end];
    let trimmed = sql.trim();
    if !strip_lead_comments(trimmed).trim().is_empty() {
        out.push(Statement { start: *start + (sql.len() - sql.trim_start().len()), sql: trimmed.to_string() });
    }
    *start = end + 1;
}

/// Skip to just after the closing quote. A doubled quote inside is an escaped one; a line comment-style identifier
/// (`[…]`) has no escapes.
fn skip_quoted(b: &[u8], mut i: usize, quote: u8, escapes: bool) -> usize {
    while i < b.len() {
        if b[i] == quote {
            if escapes && b.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        if b[i] == b'\n' && quote != b'`' {
            return i; // a string does not span lines; cutting there is safe anyway
        }
        i += 1;
    }
    b.len()
}

fn skip_until(b: &[u8], mut i: usize, stop: u8) -> usize {
    while i < b.len() && b[i] != stop {
        i += 1;
    }
    i
}

fn skip_block_comment(b: &[u8], mut i: usize) -> usize {
    while i + 1 < b.len() {
        if b[i] == b'*' && b[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("light-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    fn wait(worker: &DbWorker) -> DbEvent {
        worker.wait_event(Duration::from_secs(10)).expect("the database answered")
    }

    #[test]
    fn statements_are_cut_on_semicolons_only() {
        let ss = split_statements("SELECT 1; SELECT ';' AS c;\n-- a; comment\nSELECT 2");
        assert_eq!(ss.len(), 3);
        assert_eq!(ss[0].sql, "SELECT 1");
        assert_eq!(ss[1].sql, "SELECT ';' AS c");
        // the comment belongs to the statement that follows it
        assert_eq!(ss[2].sql, "-- a; comment\nSELECT 2");
        assert_eq!(ss[2].start, "SELECT 1; SELECT ';' AS c;\n".len());
        assert_eq!(ss[2].title(), "SELECT 2");
    }

    #[test]
    fn escaped_quotes_and_comments_stay_whole() {
        let ss = split_statements("INSERT INTO t VALUES ('it''s; fine'); /* x ; y */ SELECT 2;");
        assert_eq!(ss.len(), 2);
        assert_eq!(ss[0].sql, "INSERT INTO t VALUES ('it''s; fine')");
        assert_eq!(ss[1].sql, "/* x ; y */ SELECT 2");
    }

    #[test]
    fn quoted_identifiers_keep_their_semicolons() {
        let ss = split_statements("SELECT \"a;b\", [c;d], `e;f` FROM t; SELECT 5;");
        assert_eq!(ss.len(), 2);
        assert_eq!(ss[1].sql, "SELECT 5");
    }

    #[test]
    fn trigger_bodies_keep_their_inner_semicolons() {
        let sql = "CREATE TRIGGER t AFTER INSERT ON x BEGIN UPDATE y SET a = 1; UPDATE y SET b = 2; END;\nSELECT 3;";
        let ss = split_statements(sql);
        assert_eq!(ss.len(), 2);
        assert!(ss[0].sql.starts_with("CREATE TRIGGER"));
        assert!(ss[0].sql.ends_with("END"));
        assert_eq!(ss[1].sql, "SELECT 3");
    }

    #[test]
    fn case_expressions_do_not_eat_the_separator() {
        let ss = split_statements("SELECT CASE WHEN a THEN 1 ELSE 2 END FROM t; SELECT 4;");
        assert_eq!(ss.len(), 2);
        assert_eq!(ss[1].sql, "SELECT 4");
    }

    #[test]
    fn transactions_and_comment_only_statements_follow_sqlite() {
        let sql = "CREATE TABLE users(name TEXT); BEGIN; INSERT INTO users VALUES ('a'); COMMIT; -- trailing";
        let statements = split_statements(sql);
        assert_eq!(statements.len(), 4);
        assert_eq!(statements[1].sql, "BEGIN");
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(exec_sql(&conn, sql).unwrap().len(), 4);
        assert!(conn.is_autocommit());
        let sets = exec_sql(&conn, "UPDATE users SET name = 'b' WHERE 0; SELECT * FROM users").unwrap();
        assert_eq!(sets[0].affected, 0);
        assert_eq!(sets[1].rows, vec![vec![DbValue::Text("a".into())]]);
    }

    #[test]
    fn unresolved_views_keep_the_rest_of_the_schema_available() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE users(name TEXT); CREATE VIEW missing AS SELECT * FROM nonexistent").unwrap();
        let schema = read_schema(&conn).unwrap();
        assert!(schema.table("users").unwrap().rowid.is_some());
        assert!(schema.table("missing").unwrap().is_view);
    }

    #[test]
    fn a_non_sqlite_file_reports_open_failure() {
        let path = temp_db("not-sqlite");
        std::fs::write(&path, "This is not a SQLite database. ".repeat(10)).unwrap();
        let worker = DbWorker::open(&path);
        assert!(matches!(wait(&worker), DbEvent::OpenFailed { .. }));
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_title_names_the_statement() {
        let s = Statement { start: 0, sql: "SELECT * FROM users WHERE id = 1".into() };
        assert_eq!(s.title(), "SELECT * FROM users WHERE");
    }

    #[test]
    fn schema_and_results_of_a_real_database() {
        let path = temp_db("schema.db");
        let mut worker = DbWorker::open(&path);
        assert!(matches!(wait(&worker), DbEvent::Opened { .. }));
        worker.exec("k", "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL); INSERT INTO users (name) VALUES ('light'), ('sql');");
        match wait(&worker) {
            DbEvent::Results { key, sets, .. } => {
                assert_eq!(key, "k");
                assert_eq!(sets.len(), 2);
                assert_eq!(sets[1].affected, 2);
            }
            other => panic!("unexpected {other:?}"),
        }
        worker.schema();
        let schema = match wait(&worker) {
            DbEvent::Schema(s) => s,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(schema.table_names(), vec!["users"]);
        let users = schema.table("users").unwrap();
        assert_eq!(users.kind(), "table");
        assert_eq!(users.columns.len(), 2);
        assert_eq!(users.columns[0].name, "id");
        assert!(users.columns[0].pk);
        assert!(users.columns[1].notnull);
        assert!(schema.table("USERS").is_some()); // names match without case

        worker.exec("q", "SELECT id, name FROM users ORDER BY id");
        let sets = match wait(&worker) {
            DbEvent::Results { key, sets, .. } if key == "q" => sets,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(sets[0].columns, vec!["id", "name"]);
        assert_eq!(sets[0].rows, vec![
            vec![DbValue::Int(1), DbValue::Text("light".into())],
            vec![DbValue::Int(2), DbValue::Text("sql".into())],
        ]);
        assert_eq!(sets[0].label(), "2 rows");
        worker.stop();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_bad_statement_reports_its_message() {
        let path = temp_db("error.db");
        let mut worker = DbWorker::open(&path);
        assert!(matches!(wait(&worker), DbEvent::Opened { .. }));
        worker.exec("k", "SELECT no_such_table");
        match wait(&worker) {
            DbEvent::Failed { key, message, .. } => {
                assert_eq!(key, "k");
                assert!(!message.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        worker.stop();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn csv_quotes_fields_that_need_it() {
        let set = ResultSet {
            columns: vec!["a".into(), "b".into()],
            rows: vec![vec![DbValue::Text("x,y".into()), DbValue::Null], vec![DbValue::Int(7), DbValue::Text("he said \"hi\"".into())]],
            ..Default::default()
        };
        assert_eq!(set.to_csv(), "a,b\n\"x,y\",\n7,\"he said \"\"hi\"\"\"\n");
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_ident("users"), "\"users\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }
}
