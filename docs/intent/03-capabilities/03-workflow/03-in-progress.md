# In Progress

## Key Ideas

- **Claimed Work**: in-progress means active work is owned by a node.
- **Observable Execution**: the work should expose process, agent, logs, and evidence as it advances.
- **Recoverable Interruption**: interruption should leave enough state to retry, resume, or reassign.

## Purpose

In-progress exists to make active work explicit. A Goal should not disappear into an agent turn, terminal command, or hidden process once work begins.

## Expected Role

In-progress should connect the Goal to node ownership, workflow claims, process execution, agent activity, target-app context, and evidence. Other nodes and agents should be able to see that the work is already active and avoid duplicating it.

If an in-progress run fails or is interrupted, Refine should preserve what happened and decide whether the Goal returns to todo, moves to failed, or follows a recovery path.

## What Happens

When a Goal is in-progress:

- A node owns the active attempt.
- Refine records the workflow claim, provider or actor, target app, and execution context.
- Agents use guidance, governance, tools, files, terminal/process execution, and target-app lifecycle context to act on the work.
- Process output, logs, changed files, agent output, and intermediate evidence should remain observable.
- Other nodes should not silently duplicate the same active work.
- On success, the work should produce a reviewable handoff and move toward ready-merge or another appropriate next state.
- On interruption or failure, Refine should preserve evidence and route the Goal to retry, failed, or recovery.

## Future Direction

Future in-progress behavior should support richer concurrency controls, leases, partial progress, cooperative multi-agent work, and stronger recovery semantics while preserving clear ownership.
