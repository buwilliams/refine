# Table

## Key Ideas

- **Dense Inspection**: tables are the default shape for repeated operational items.
- **Shared Behavior**: list views should reuse sort, filter, selection, mobile-card, and row-action patterns.
- **Readable At Scale**: tables should handle many Gaps, Features, logs, changes, processes, and metrics without becoming decorative cards.
- **URL-Backed Controls**: filters and sorts should survive refresh and sharing.

## Purpose

Tables exist because Refine is an operational product. Users need to scan, compare, sort, filter, select, and act on many work items.

Cards may be useful for summaries, but repeated work surfaces should prefer dense, consistent tables.

## Expected Role

Tables should provide consistent interaction across Gaps, Features, Logs, Changes, Processes, and Performance. Users should not need to relearn list behavior on every page.

Current implementation details that matter to intent:

- Gaps, Features, Changes, Logs, and Performance use shared table-like list patterns;
- sortable headers use consistent active/arrow behavior;
- filters live above tables in collapsible shells where appropriate;
- mobile-card table styling preserves readability on narrow screens;
- bulk selection appears only when the filter shell is open where that reduces visual noise.

Tables should avoid one-off custom list designs unless the data genuinely requires a different shape.

## Future Direction

Future tables should become more evidence-aware and agent-aware. They may show confidence, risk, dependency, node ownership, claim state, or review readiness.

The principle should remain: dense, predictable, operational scanning over decorative layout.
