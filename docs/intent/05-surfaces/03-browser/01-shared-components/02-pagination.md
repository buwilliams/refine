# Pagination

## Key Ideas

- **Scale Without Surprise**: large lists should not load or render everything at once.
- **Shared Controls**: pagination should look and behave consistently across surfaces.
- **Real Totals When Needed**: First and Last controls require trustworthy page metadata.
- **URL State**: page and limit should be part of shareable list state.

## Purpose

Pagination exists to keep operational surfaces fast and understandable as work grows. Refine should work for small projects and for large work queues without changing interaction models.

It also protects agent and browser performance. Large Goals, Features, logs, changes, and metrics should be navigable without exhausting the UI.

## Expected Role

Pagination should be shared wherever a list can expose page metadata. It should support standard limits such as 50, 100, 250, 500, and 1000 when appropriate.

Current implementation details that matter to intent:

- shared primitives include `renderPaginationControls` and `bindPaginationControls`;
- Goals, Features, Logs, Changes, Performance, and Feature modal Goal lists use pagination patterns;
- boundary controls are used when the backend provides enough metadata;
- list filters reset page to 1 when the filter context changes.

Pagination should be boring, consistent, and reliable.

## Future Direction

Future pagination may evolve into cursor-based paging, streaming, or agent-prioritized slices. That should be done to support scale, not to hide work.

The core outcome remains: users and agents can inspect large workspaces predictably.
