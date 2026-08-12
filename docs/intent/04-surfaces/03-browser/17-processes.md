# Processes

## Key Ideas

- **Visible Runtime Work**: users should see processes Refine owns on this node.
- **Actionable Status**: rows explain owner, context, logs, and controls.
- **Shared Supervisor Truth**: the browser reflects managed-process state.

## Purpose

Processes makes daemons, target-app commands, agents, quality checks, imports, maintenance, terminals, and helpers inspectable. It answers what is running locally, what owns it, and what can be done about it.

## Expected Role

`/api/processes` exposes managed local process state. Rows may include owner, pid, state, label, Goal, Round, workflow state, output, resources, and actions. Node-local process identifiers support observation and Stop, not workflow authority.

Stop confirms process exit and conditionally returns an unchanged linked Goal to todo. It retains workflow worktrees and branches. If the Goal is already cancelled or changed, Stop preserves that newer state. Goal cancellation is a separate Goal action that commits terminal intent first.

The Goal terminal and Processes view use the same shared process capability. Pause and resume controls affect supported background work without creating browser-specific state.

## Future Direction

Process views should show resource pressure, isolation, remote-node summaries, retry, and provenance while clearly distinguishing node-local execution from synchronized Goal ownership.
