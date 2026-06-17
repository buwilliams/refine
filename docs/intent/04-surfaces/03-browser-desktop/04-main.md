# Main

## Key Ideas

- **Work Surface**: `main` is where the current route's primary work happens.
- **Context Preservation**: detail views should preserve the user's underlying list or dashboard context when possible.
- **Route-Backed State**: filters and views should be shareable, reloadable, and recoverable through URL state.
- **No Hidden Product Logic**: main content should render shared state and call shared capability.

## Purpose

The main area exists to show the selected product surface: Dashboard, Features, Gaps, Changes, Logs, Settings, Node, Project, import, Plan, and related modal flows.

It should give the user enough room to inspect and act without losing the global shell, toolbar, Guide, or operating context.

## Expected Role

The main surface should support dense, repeatable operational work. Lists should be filterable and sortable. Detail views should expose the state, evidence, logs, and actions needed to move work forward. Empty and detached states should be explicit rather than confusing.

Current implementation details that matter to intent:

- hash routing determines the active main view;
- Gap and Feature details open as modal overlays over the current context;
- list filters use URL-backed state so views can be refreshed or shared;
- leaving list routes clears selection state where stale bulk selection would be dangerous;
- detached/no-app state is a first-class UI mode.

The main surface should not treat every feature as a bespoke page. Shared list, modal, workflow, and error patterns should make the app easier to understand.

## Future Direction

Future main views should increasingly organize around agentic state: active work, blocked work, evidence, review, risk, and composition plans.

As AI handles more execution, the main surface should help people understand what the system believes, what it is doing, and where human judgment is most valuable.
