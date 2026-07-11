# Processes

## Key Ideas

- **Visible Runtime Work**: users should see the processes Refine owns.
- **Actionable Status**: process rows should explain state, owner, context, logs, and available controls.
- **Shared Supervisor Truth**: the UI should reflect supervisor-managed state, not guess from browser state.
- **Scoped Loading**: process views should load only when needed.

## Purpose

The Processes surface exists to make runtime work inspectable. Refine launches daemons, target-app commands, agent turns, quality checks, imports, maintenance jobs, terminal sessions, and helpers. Users need to see those processes when diagnosing or supervising automation.

It answers: what is running, what owns it, what is it attached to, and what can I do about it?

## Expected Role

Processes should appear under the system/node management area and should connect process state back to product concepts like Goals, sessions, workflow, and target-app activity.

Current implementation details that matter to intent:

- `/api/processes` exposes managed process state;
- process rows can include owner, pid, state, label, details, output availability, resource labels, and actions;
- chat, workflow, runner, target-app, and UI contexts should be surfaced where available;
- process pause/resume controls affect background work and agents;
- settings/processes views should avoid overfetching unrelated settings data.

Processes should be a debugging and supervision surface, not a hidden implementation detail.

## Future Direction

Future process views should represent agent fleet execution: queues, claims, remote nodes, resource pressure, isolation, cancellation, retry, and provenance.

As Refine scales, process visibility should become one of the main ways people and agents understand the live orchestration layer.
