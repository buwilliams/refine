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
- routine state-sync health lives as a compact Healthy/Unhealthy label in Toolbar > System; timestamps, attempt IDs, errors, and report paths do not occupy the Dashboard;
- all system diagnostics, including worker health, count freshness, synchronization errors, and explicit conflict recovery, belong in Toolbar > System, not Dashboard alerts or content;
- health events refresh System independently of the current page and log-tail setting;
- workflow visualization is shared with the Goals screen;
- an intentionally paused workflow is neutral operating context, not a runtime-worker failure needing attention;
- target-app and agent status are part of the operating context;
- detached/no-app mode should render a clear setup path rather than raw errors.

The Dashboard should stay compact and practical. Its job is orientation and routing, not detailed editing.

Toolbar > System is the central place for system health, diagnostics, and remedies. Dashboard attention is reserved for work such as failed Goals and pending reviews. A failed Dashboard read shows a brief availability message pointing to System, without diagnostic internals.

## Future Direction

Future Dashboard views should summarize agent fleets, composition plans, blocked dependencies, pending approvals, and risk. As automation grows, it should become the user's high-level mission control for software work.

The best future Dashboard should make a complex autonomous system understandable at a glance.
