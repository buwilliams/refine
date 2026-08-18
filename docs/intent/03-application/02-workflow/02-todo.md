# Todo

## Key Ideas

- **Ready For Work**: todo means a Goal is eligible to advance.
- **Shared Queue**: assigned nodes select todo work from synchronized Goal state.
- **Ordering Aware**: selection respects Feature order, priority, pause state, and soft capacity.

## Purpose

Todo separates captured work from actionable work. It tells Refine that a Goal has enough context to be picked up by automation for planning, recovery, or imported-candidate validation.

## Expected Role

The scheduler reads todo Goals assigned to its active node. When policy permits, it rereads the Goal, advances it to plan, and starts a node-local worker. No durable reservation or repository copy is created merely because a Goal is queued. The implementation worktree is materialized only after the authored Round has crossed into Plan.

If a Goal cannot proceed, it remains visible as actionable work. Live-process counts and runner-local reservations are soft efficiency controls and may be rebuilt after restart.

Todo execution is admitted only for an existing non-empty latest Round. Before
any branch, worktree, or process is created, Refine locks the Goal record and
atomically rechecks Todo status, active-node ownership, exact Round count,
request, and authored workflow revision while moving to plan. A changed
authoring commitment remains Todo and produces no execution side effects.

A Quality- or Governance-finding recovery Round continues on the source
Round's retained candidate instead of a fresh repository copy. After the same
atomic Todo admission, Refine verifies the retained worktree still names the
exact recorded candidate and is clean, then creates the new Round branch at
that commit in the same worktree — preserving its warm build state. Any failed
precondition falls back to the ordinary fresh materialization, and
integration-race recoveries always take the fresh path because their candidate
itself is stale.

## Future Direction

Todo selection should gain better dependency, risk, capability, node-health, and expected-impact reasoning while remaining understandable as the point where work becomes actionable.
