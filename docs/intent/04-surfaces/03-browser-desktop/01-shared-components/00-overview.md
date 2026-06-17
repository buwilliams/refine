# Shared Components

## Key Ideas

- **One Interaction Language**: repeated UI patterns should behave the same wherever they appear.
- **Operational Components**: shared components exist to support work, review, status, and recovery.
- **Dense But Clear**: components should help users scan, compare, and act without decorative noise.
- **State Preservation**: shared components should preserve focus, filters, selection, and context where possible.
- **Accessible By Default**: reusable components should carry labels, predictable controls, and keyboard-friendly behavior.

## Purpose

Shared components exist so Refine feels like one system rather than a pile of pages. Modals, tables, pagination, filter shells, segmented controls, status pills, priority pills, banners, empty states, forms, command buttons, and loading states should teach the user once and then stay consistent.

These components also help future agents reason about the UI. A table should mean sortable, filterable, scan-friendly data. A modal should mean contextual detail or confirmation without losing the underlying page. A status pill should mean workflow state, not arbitrary decoration.

This section is the parent for reusable browser-desktop component intent. Components with enough product significance, such as tables and pagination, get their own child documents so their expected behavior can stay precise without making every page doc repeat the same rules.

## Expected Role

Shared components should carry common behavior across Gaps, Features, Logs, Changes, Settings, Processes, Toolbar, Guide, and future surfaces.

The most important shared components are:

- modals for detail, confirmation, drafting, and review flows;
- tables for repeated operational records;
- pagination for large datasets;
- filter shells for narrowing lists without permanent clutter;
- status and priority pills for fast scanning;
- banners and System notices for user-visible state;
- segmented controls for compact mode or period selection;
- empty states for detached, missing, or zero-result conditions;
- busy buttons and action errors for recoverable mutations.

Shared components should not hide product semantics. They should make the same semantics easier to use everywhere.

## Future Direction

As Refine becomes more agentic, shared components should show richer evidence and risk without becoming visually noisy. Future components may need to represent agent claims, confidence, provenance, dependency impact, approval requirements, and recovery paths.

The design direction should remain consistent: reusable operational patterns over one-off page inventions.
