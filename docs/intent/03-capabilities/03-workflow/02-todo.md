# Todo

## Key Ideas

- **Ready For Work**: todo means a Goal is eligible to advance.
- **Shared Queue**: assigned nodes select todo work from synchronized Goal state.
- **Ordering Aware**: selection respects Feature order, priority, pause state, and soft capacity.

## Purpose

Todo separates captured work from actionable work. It tells Refine that a Goal has enough context to be considered for implementation, recovery, or agent action.

## Expected Role

The scheduler reads todo Goals assigned to its active node. When policy permits, it rereads the Goal, advances it to in-progress, and starts a node-local worker. No durable reservation or repository copy is created merely because a Goal is queued. The implementation worktree is materialized only when implementation needs it.

If a Goal cannot proceed, it remains visible as actionable work. Live-process counts and runner-local reservations are soft efficiency controls and may be rebuilt after restart.

## Future Direction

Todo selection should gain better dependency, risk, capability, node-health, and expected-impact reasoning while remaining understandable as the point where work becomes actionable.
