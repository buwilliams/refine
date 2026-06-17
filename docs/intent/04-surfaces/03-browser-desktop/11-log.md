# Log

## Key Ideas

- **Durable Evidence**: logs explain what happened across workflow, UI, agents, processes, and system operations.
- **Filterable Activity**: users should be able to search and narrow logs by severity, category, actor, Gap, and time.
- **Shared Table Pattern**: Logs should align with other list surfaces.
- **UI Errors Matter**: browser-visible failures should become activity, not disappear after a toast.

## Purpose

The Log surface exists to make Refine auditable. It lets users and agents inspect system activity, workflow events, UI errors, process messages, and other evidence after the fact.

Logs should reduce mystery. When a Gap failed, an import completed, a UI action errored, or a background job started, the system should leave a trail.

## Expected Role

The Logs UI should be a dense investigation surface. It should use filters, facets, sorting, pagination, details expansion, and visualization to help users find relevant evidence quickly.

Current implementation details that matter to intent:

- Logs read activity through `/api/activity`;
- filters include severity, category, actor, Gap ID, search text, period, limit, page, sort, and direction;
- UI errors can be recorded into activity;
- server-provided facets populate filter controls;
- boundary pagination supports large log sets.

Logs should complement the Toolbar System log. System is for immediate operational notices; Logs are for durable investigation and audit.

## Future Direction

Future logs should support agent-readable provenance. Agents should be able to summarize evidence, find root causes, compare attempts, and explain why a workflow moved or stopped.

The Log surface should become a high-trust evidence layer for autonomous software composition.
