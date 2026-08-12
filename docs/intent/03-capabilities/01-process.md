# Process

## Key Ideas

- **Local Observation**: process records describe work observed on one node; they are not synchronized workflow authority.
- **Supervisor Ownership**: Refine should know which processes it owns and why they exist.
- **Recoverable Work**: stopping or losing a process should preserve Goal, Round, branch, worktree, and evidence so work can be started again cheaply.
- **Soft Capacity**: admission uses observed live processes and configured limits, not durable reservations.
- **Shared Control**: CLI, browser, API, and agents use the same process capability.

## Purpose

Refine runs target apps, agents, quality checks, imports, maintenance tasks, terminals, and background operations. The Process capability makes that local execution visible and controllable without making process identity part of synchronized product state.

A user or agent should be able to answer what is running on this node, why it was started, where its output is, whether it is alive, and whether it can be stopped.

## Expected Role

Managed processes record local facts such as owner, pid, state, label, output paths, limits, start time, exit code, Goal, Round, workflow state, provider, and target app. Node-local identifiers may connect local logs, sessions, operations, and child processes, and evidence may cite them as provenance. They do not grant permission to mutate a Goal or act as resumable workflow state.

Goal status and node assignment determine whether automated work is still authorized. A worker rereads those fields before a workflow transition or consequential side effect. Process records help avoid needless duplicate work on one node and provide controls; they are advisory when deciding capacity or recovery.

Stopping a Goal worker confirms its exit, retains its branch and worktree, and conditionally returns the same Goal attempt to `todo` only if the Goal status, Round, update, and node assignment still match the worker's starting observation. Explicit Goal cancellation is monotonic: the Goal becomes `cancelled` first and local process termination follows as best-effort cleanup. Neither path rolls synchronized Goal state back because local cleanup failed.

After daemon restart, live-process recovery terminates stale owned workers and removes retired execution-coordination files. Any nonterminal Goal remains eligible for a new idempotent worker. Planning artifacts, Git observations, and semantic outputs remain durable; a prior process identity is only provenance, not ownership that must be recovered.

Worktree cleanup is separate from Stop. It may hibernate a clean inactive worktree when no live local process or operation uses it, while dirty, ambiguous, standalone, and state worktrees remain protected. Candidate branches retain their own exact-SHA integration safeguards.

## Future Direction

Process management should gain better resource observation, isolation, health checks, remote-node visibility, and provenance without turning node-local runtime facts into synchronized locks. Scaling should preserve the cheap-restart model: durable semantic work, transient workers, and clear Goal authority.
