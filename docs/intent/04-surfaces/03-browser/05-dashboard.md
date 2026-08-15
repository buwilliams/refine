# Dashboard

## Key Ideas

- **Operational Overview**: the Dashboard should show the current health and shape of work.
- **First Landing**: it should orient users after opening Refine or switching apps.
- **Workflow Summary**: status distribution, attention areas, and agent activity should be visible quickly.
- **Jump Surface**: dashboard elements should lead to filtered work, logs, settings, or processes.

## Purpose

The Dashboard exists to answer "what is happening?" It should summarize work state, active automation, recent activity, target-app status, agent status, and anything needing attention.

It is not a marketing home page. It is the first operational view of the attached app.

## Expected Role

The Dashboard should provide broad context without replacing deeper surfaces. Users should be able to land there, see whether Refine is healthy, and jump to the exact place that needs attention.

Current implementation details that matter to intent:

- dashboard data comes from daemon-backed projection and diagnostic summaries;
- state-sync health shows the serving node's correlated attempt id and source,
  last successful reconciliation, failure start, stale boundary, redacted
  bounded error, and complete conflict-report id and local location when one
  exists;
- failed or stale sync is prominent needs-attention state, and all-node counts
  keep one compact explicit non-authoritative label while degraded; the
  Dashboard does not load or render the complete path set;
- only the typed missing-baseline condition opens the daemon-backed recovery preview. The Dashboard presents complete target, repository, head, snapshot, count, bounded-conflict, and evidence identity without recommending or preselecting authority; live or remote authority and exact-preview confirmation are separate deliberate actions;
- Git-busy recovery rejection retains the preview but invalidates confirmation before retry. Stale-preview rejection clears the preview and choice until a fresh preview is reviewed. Successful recovery retains its audit ref, manifest, resulting heads, authority, counts, detail, and evidence while authoritative state-sync health refreshes and shows the failure cleared;
- typed state-sync-health SSE updates refresh Dashboard, Nodes, and Logs on failure, stale-threshold crossing, recovery, initial connection, and reconnect;
- workflow visualization is shared with the Goals screen;
- an intentionally paused workflow is neutral operating context, not a runtime-worker failure needing attention;
- target-app and agent status are part of the operating context;
- detached/no-app mode should render a clear setup path rather than raw errors.

The Dashboard should stay compact and practical. Its job is orientation and routing, not detailed editing.

## Future Direction

Future Dashboard views should summarize agent fleets, composition plans, blocked dependencies, pending approvals, and risk. As automation grows, it should become the user's high-level mission control for software work.

The best future Dashboard should make a complex autonomous system understandable at a glance.
