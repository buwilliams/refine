# In Progress

## Key Ideas

- **Node-Owned Work**: in-progress means the synchronized Goal is active on its assigned node.
- **Observable Execution**: local processes, agent sessions, logs, and evidence remain inspectable.
- **Replaceable Workers**: interruption may start the same semantic attempt again.

## Purpose

In-progress makes active work explicit. A Goal should not disappear into an agent turn, terminal command, or hidden process once work begins.

## Expected Role

The Goal's status, node assignment, and current Round authorize advancement. Local managed processes record which worker is currently acting, but their identifiers do not enter synchronized Goal or Round evidence.

Before implementation, Workflow pins the Round context and advances the typed Plan -> Criticize -> Revise -> Implement pipeline. Each completed artifact is persisted before its consumer starts and updates compare against the complete prior planning value plus Goal, Round, context digest, branch, target branch, and base commit.

Planning agents must leave the repository unchanged. The implementation agent receives the final checklist and reports semantic checklist and verification evidence. Guidance selection occurs inside the implementation turn. Post-implementation governance evaluates the pinned governance snapshot.

The Goal Agent runs as an observable managed PTY so browser and CLI can attach without creating another session. If user input is genuinely required, the same local process may wait for it. A daemon restart does not promise to restore that process; a replacement worker consumes the same synchronized context and preserved artifacts.

Every transition rereads status, node, and Round. Reassignment, cancellation, a new Round, or another state transition makes a stale worker stop. Duplicate idempotent work can occur during restart or delayed synchronization, but only a worker with current Goal authority may publish the next state.

On success, implementation produces a reviewable candidate and advances toward QA or Ready Merge. On failure, semantic evidence is preserved and the unchanged active Goal moves to failed where appropriate.

## Future Direction

In-progress should support cooperative agents, better progress evidence, and remote-node observability while keeping worker lifetime separate from synchronized Goal authority.
