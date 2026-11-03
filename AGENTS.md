# Project UI rules

- Keep buttons compact and sized to their labels; do not stretch them across available space. Repeated row actions must have equal widths and align.
- Toasts last 3 seconds in total and fade out before disappearing.

- Keep all interface elements visually consistent: reuse the existing palette, typography, control sizes, borders, corner radii, spacing, and hover, focus, and disabled states. This includes chart-rendered controls such as legend reset buttons.
- All new and changed interface text must support Simplified Chinese and English through the shared localization module. The default language follows the browser; users may select Simplified Chinese or English in Settings. Do not translate user-provided data, field names, or report titles.
- Follow the user's global working rules: work directly on main and do not run tests unless explicitly requested.
- Numeric formats are display settings only: integer, decimal, and percentage must preserve the original source values and calculation precision. Decimal display and the default decimal setting allow 1–4 decimal places; integer display uses 0.
- UI terminology: the second sidebar is the toolbar; the right sidebar is the properties sidebar. Filter controls are added from the toolbar, appear on the canvas, and are configured in the properties sidebar after selection.
- Filtering is controlled exclusively through toolbar-added canvas filters. Do not expose per-chart filter modes or apply legacy hidden filter conditions.
- Filter controls can use only text and date dimensions, never numeric fields. This applies to field choices, cascading defaults, and restored saved filters.
