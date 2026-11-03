# SQL Desk browser update

Continue inline under the user's existing direct-development authorization. No tests executed; build and replace the same SqlDesk.exe.

- [x] Connection editor: explicit New, Save new, Save changes, Delete. Editing tracks the original profile index, so renaming edits the saved connection rather than creating a duplicate. Reject duplicate names; preserve Windows credentials.
- [x] Explorer: saved physical connection, database, schema. Persist last successful selection and restore/reconnect at startup. Keep model and selected index updates together.
- [x] Objects: table, V, MV tabs, separate name search, matching counts, bounded metadata cache. Use database-specific classification and field queries.
- [x] Context menu on the clicked object: structure dialog, LIMIT 100 SQL, GROUP BY selected column with COUNT(*), descending count, LIMIT 100. Generate SQL into the editor; no answer to optional preference, retaining stated manual-execution default.
- [x] SQL syntax highlighting: Slint text renderer over native Slint input, bounded/debounced background lexer; plain text fallback above 64 KiB or at least 500 newline characters. Preserve original SQL and clipboard text.
- [x] Remove header branding and connection address. Connection indicator at bottom left, red/green dot with accessible status text. Keep language selection.
- [x] Review error paths and build; no application launch or database connections.

Independent review: fixed final highlighting request dropped on full queue (latest-value slot plus bounded wake channel), and overlapping rendered text during IME composition (disable styled layer during preedit). Redshift uses SVV_ALL_TABLES, SVV_MV_INFO and SVV_ALL_COLUMNS; local schemas still use pg_namespace. PostgreSQL uses pg_class/pg_attribute to include materialized views.
