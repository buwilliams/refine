# Todo

## Key Ideas

- **Ready For Work**: todo means a Goal is eligible to advance.
- **Shared Queue**: assigned nodes select todo work from synchronized Goal state.
- **Ordering Aware**: selection respects Feature order, priority, pause state, and soft capacity.
- **Target-Derived Base**: a Round's base and its branch both come from the configured merge target, never from whatever branch the shared checkout happens to have open.

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

Before the base is pinned, Refine brings the local merge target up to its
remote, so a Round starts from what the fleet has actually published rather than
from whatever a rarely-pulled checkout still holds. That refresh is advisory and
fast-forward only: an absent or unreachable remote leaves the ref untouched and
the Round proceeds on it, and a local target carrying commits its remote does
not is left for integration to merge under its own evidence. The fetch runs
outside the repository lock, which is held only for the ref advance, so a slow
remote never wedges another Goal's repository work. When that advance moves a
branch the human checkout is on, the checkout is brought along with it, and a
sync the working tree blocks or an interruption cuts short stays recorded until
a later pass repairs it.

The Round branch is then created at that pinned base. Whichever branch a human
left checked out never decides where a Round begins: users should not have to
park their checkout on the merge target, or pull it, for their Goals to be
integrable, and a branch born anywhere else would leave the recorded base a
non-ancestor of the candidate and fail integration as stale through no fault of
the work. Resumption follows the same rule — a Round branch that has gone
missing is recreated at the Goal's recorded base, while one still carrying an
interrupted Round's commits is reused exactly as it stands.

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
