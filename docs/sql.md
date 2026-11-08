# SQL

The SQL module is part of the program: `.sql` files light up as soon as Light starts. Statements run on **SQLite**, built into the IDE — a project database opens out of the box, with nothing to install.

## The console

- A run button stands next to every statement in the gutter. Its menu also runs **the whole file**.
- **F5** runs the statement under the caret; what is **selected** runs as it stands.
- Statements are cut on the `;` that ends each one; strings, comments and `BEGIN … END` trigger bodies stay whole.
- Several statements answer with several result sets — the Results tab switches between them.
- When a statement fails, the Results tab shows the database's own message; the statements after it do not run.

## The database

Consoles run on the project's database: **the one chosen for the project**, else the only `.db`/`.sqlite`/`.sqlite3`/`.db3` file in it (two levels deep), else Light asks for a file when a statement first runs and remembers the choice for the project (`Change…` in the Database window picks another). An empty file is a valid empty database — the first `CREATE TABLE` fills it.

## The Results tab

A grid of the rows the last statement came back with: numbers to the right, `NULL` quiet, long values cut with an ellipsis, up to 50 000 rows kept reading smoothly. **Export CSV** writes the selected result set. In a table grid, save or discard edits before exporting. A change (`INSERT`, `UPDATE`, `DELETE`, `CREATE`) reports how many rows it touched.

## Editing the rows of a table

Open a table from the Database window to edit its rows:

- **double-click a cell** (or select it and press Enter) and type: the first keystroke replaces the value, Enter keeps what was typed, Escape drops it;
- **Delete** on a selected cell sets it to `NULL`;
- **Add Row** appends an empty row below the last one. Untouched fields use the column defaults. **Delete Row** marks the selected row for deletion or removes a newly added row;
- every change stands out in the grid: edited cells carry an accent underline, rows to delete are tinted with the error colour, added rows with the success one, and the buttons count what is not saved yet;
- **Save** applies the edits in one transaction and refreshes the grid. If a constraint fails or a selected row changed outside the grid, the database rolls back the batch and the grid keeps your edits. **Discard** clears pending edits. Save or discard before switching databases or replacing the results.

Views, virtual tables and tables without an accessible SQLite row identifier open read-only. Generated columns are read-only within an editable table.

## Names from the schema

While you type, the editor offers what the database holds:

- after `FROM`, `JOIN`, `INTO`, `UPDATE`, `TABLE` — the tables and views first;
- anywhere else — the tables among the language's own words, and the columns of the tables the statement reads;
- after a dot — the columns of the table the alias before it names (`FROM users u` … `u.` → the columns of `users`; a table also answers to its own name).

## The Database window

**Alt+D** (or the tool strip) shows the schema: tables and views with their columns, types and key columns. A click on a table shows its first 500 rows in the Results tab.
