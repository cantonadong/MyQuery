# SQL Desk editing and browsing update

Continue direct development. No tests or app launches; overwrite the fixed EXE after building.

- [x] Inline connection/database/schema labels, shared live search and all-category counts only when searching; Refresh after MView; remove Insert table name button.
- [x] Stable tab hover and object list while metadata is loading. Replay latest field selection after completion; clear columns on metadata context changes.
- [x] Context actions generate SELECT * LIMIT 10, or selected column + COUNT(*) GROUP BY and ORDER BY COUNT(*) DESC.
- [x] Query timeout moves to Settings. Latest steering replaces horizontal text toolbar with five small vertical SVG icon buttons: Run, Cancel, Save, Import, Export, with localized tooltips.
- [x] Synchronous bounded highlighting uses one styled paragraph, keeping input geometry stable. Single editor persists through busy-state changes. Source SQL remains unchanged; large documents retain plain display.
- [x] Multiple SQL documents: + adds, x closes; modified marker, remembered file path, save/discard/cancel prompts for tab/application close. Import opens/activates document; Export writes a copy. Maximum 32 documents. Query results remain a single bounded result store; clear displayed results when switching documents.
- [x] Read-only independent reviews: fixed dropped field selections/stale metadata. Document/save/close review found no concrete issues. Saved-snapshot regression assertions added but not executed.
- [x] Final release build and artifact verification. No runtime claims; user verifies rendering, hover and save dialogs.

Final release: 12,140,032 bytes; SHA256 07DE307B28FAA0FA8DD82278DB7CAE8FD7ED968D69B06FB9EF35F69E35E1430D. Formatting check passed. Build artifact and fixed user EXE hashes match. No tests executed. Document save/close paths reviewed independently with no concrete findings; cancellation/error paths keep dirty contents. Active tabs scroll into view; dirty marker precedes long names so it remains visible.
