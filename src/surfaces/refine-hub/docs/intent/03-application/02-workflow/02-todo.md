# Todo

## Key Ideas

- **Ready For Work**: todo means a Goal is eligible to advance.
- **Shared Queue**: assigned nodes select todo work from synchronized Goal state.
- **Ordering Aware**: selection respects Feature order, priority, pause state, and soft capacity.
- **Shared Preparation**: all executable steps retain coherent lineage or regenerate work through the same capability; new work starts from the configured target.

## Purpose

Todo separates captured work from actionable work. It tells Refine that a Goal has enough context to be picked up by automation for planning, recovery, or imported-candidate validation.

## Expected Role

The scheduler reads todo Goals assigned to its active node. When policy permits, it rereads the Goal, advances it to plan, and starts a node-local worker. No durable reservation or repository copy is created merely because a Goal is queued. The implementation worktree is materialized only after the authored Round has crossed into Plan.

If a Goal cannot proceed, the shared health and admission assessment reports its actual current blocking reason. Live-process counts and runner-local reservations are soft efficiency controls and may be rebuilt after restart.

Todo execution is admitted only for an existing non-empty latest Round. Before
any implementation branch, worktree, or process is created, Refine locks the Goal record and
atomically rechecks Todo status, active-node ownership, exact Round count,
request, and authored workflow revision while moving to plan. A changed
authoring commitment remains Todo and produces no execution side effects.

Configured executable Backlog and Todo Enter/Exit Skills may need to run before that admission. Each such invocation owns a separate managed linked checkout pinned from the configured target, with Goal, current Round/request, node and transition authority (a pending user transition or the scheduler's current Todo occurrence) or a durable occurrence. Its workspace and output never set the implementation branch, base or candidate. Ordinary queueing and Backlog promotion remain state-only; disabled and context-only bindings create no checkout. Lifecycle process launch still obeys capacity and pause controls, and cancellation supersedes pending transition authority immediately. A cancelled, incomplete or invalid blocking Todo Entry invocation prevents transition approval and implementation materialization, including after dispatch retries or restart. Before settlement, Refine rereads every required Entry and Exit invocation and binding, requiring successful completion under the current authority and the original workspace registration even after later Skills have finished.

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

New work receives a branch at the selected target base. Retained work keeps its verified base, branch and candidate together; observing a newer target never replaces only the base field. The shared preparation capability can reconstruct a missing checkout or branch from verified identities. It applies equally when Todo is reentered or another executable step is selected directly.

If generated lineage or required inputs cannot be reused, preparation records recovery on the existing authored Round and selects Plan with a fresh branch such as `round-1-execution-UUID`. It leaves old work and reports as history and invalidates their current applicability. Selecting Plan does not itself append a Round. An absent authored request remains a visible blocker. See the [shared consistency contract](11-consistency-contract.md) for the uniform recovery rules and exceptions.

Lifecycle dispatch and scheduler admission consume the same occurrence-pinned Entry configuration, even when Skill definitions change before either worker runs. A completed blocking requirement can permit advancement while a background failure remains visible; cancellation or missing/invalid blocking evidence cannot.

## Future Direction

Todo selection should gain better dependency, risk, capability, node-health, and expected-impact reasoning while remaining understandable as the point where work becomes actionable.
