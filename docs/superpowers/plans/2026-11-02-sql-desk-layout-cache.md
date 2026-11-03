# SQL Desk layout and metadata cache

Continue direct implementation. Do not run tests or launch app; build same EXE for user testing.

- [x] Edge-aligned Explorer and SQL/result panels with a draggable sidebar boundary and editor/result divider.
- [x] Explorer list fills remaining sidebar height; show column panel only when populated. Fixed object tab labels: Table, View, MView.
- [x] Move language and connection settings into Settings menu. Connection settings become a labelled form dialog.
- [x] Remove bottom page-size help; preserve pagination controls, total count and errors.
- [x] Cache metadata per endpoint, engine, user and database; share the database list across databases. First access populates cache, reuse for 7 days, then update on first access. Manual refresh invalidates current connection's metadata. Persistent disposable cache capped at 8 MiB/128 entries; lazy loading preserves memory/CPU bounds.
- [x] CSV null exports as literal NULL; source values and other exports retained.
- [x] Review, build release and replace SqlDesk.exe. No tests executed.

Read-only independent review found no actionable bugs. Fixed Slint menu/parent dimension dependency loop; subsequent release build has no warnings. Added focused cache isolation/expiry assertions but did not execute them. Visual drag/menu behavior and real database requests remain for user testing.
