# SQL Desk compact controls and execution context

User requests icon from design/MyQuery_icon.png, faster cached navigation, daily refresh, inline SQL-tab close/new controls, compact DBeaver-like proportions, context menu copy/structure/quick SQL, and Redshift relation-not-found investigation.

- [x] Embed supplied PNG in window and Windows EXE; build derives multi-size ICO and compiles resources with installed LLVM-MinGW.
- [x] Cache entries expire by local calendar day. Cached database switching updates browser selection without logging in again; metadata misses and SQL execution ensure the actual database session matches the selected profile/database.
- [x] Commit schema selection before metadata loading; clear stale context on selection failure/connection change. Before execution, set the selected search_path; permit ROLLBACK through an aborted transaction. Keep SQL unchanged and show database/schema context for 42P01. Actual failing SQL/selected schema requested asynchronously; no credentials supplied.
- [x] 16px toolbar icons, 24px controls, 22px virtualized list/table rows with keyboard selection and column resizing. Existing color palette retained.
- [x] Plus after last tab, inline close target, double-click empty tab area to create. Toolbar order Run/Stop/Save/Export/Import; right-click structure/copy name/LIMIT10/GROUPBY.
- [x] Independent code review found schema-selection failure could leave prior execution schema; synchronized selection before fetch and clear prior data. Also update stored connection label on database selection.
- [x] Final build and static artifact verification; user preference prohibits tests and app launch.

Final release: 12,244,480 bytes; SHA256 45F149DB9A59376A5D5E5647D82EE9683ECBF9907C5E70121FCD4E2B8D7C6611 matches fixed SqlDesk.exe. Formatting check passed. PE contains ICON/GROUP_ICON resources; embedded PNG matches supplied design asset. No tests/app/database queries run. Relation existence, interactive sizing and real connection latency remain unverified.
