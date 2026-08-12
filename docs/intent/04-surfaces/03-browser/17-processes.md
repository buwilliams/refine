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

The Goal terminal and Processes view use the same shared process capability. The Processes pause control reflects canonical `workflow_paused` state: pause blocks new Goal admission and quiesces automatic Git sync and inactive-worktree cleanup at safe boundaries, while already active Goal executions continue unless stopped separately. Resume makes admission and those repository workers eligible again. The browser does not relabel live Agents or unrelated active operations as paused, terminate Agents, or create browser-specific pause state.

## Future Direction

Process views should show resource pressure, isolation, remote-node summaries, retry, and provenance while clearly distinguishing node-local execution from synchronized Goal ownership.
