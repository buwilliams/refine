# Fleet

## Key Ideas

- **Fleet As Composed Nodes**: nodes coordinate through shared Goal state and Git remotes.
- **Distribute Is The Mechanism**: assignment, rebalance, and handoff use one explicit operation.
- **Ephemeral Workers, Durable Evidence**: a worker can be rebuilt from Git and synchronized state.
- **Symmetric Sync**: every node publishes and pulls the same control branch.
- **Node-Local Sync Health**: each daemon reports its own reconciliation freshness; it cannot infer another daemon's health from shared state.
- **Rolling Upgrades**: nodes upgrade one at a time, in any order; mixed versions converge on the shared control branch, and a node whose turn has not come reports pending upgrade rather than failing the fleet.
- **Judgment Converges**: implementation may run anywhere while review converges where judgment occurs.

## Purpose

Fleet explains how work moves across nodes and returns for review. One machine is a fleet of one; larger fleets add parallel execution without changing the Goal ownership model.

## Expected Role

Distribute reassigns eligible queued Goals among enabled, healthy nodes. It does not move a Goal already in an automated state. Reassignment of queued work is the explicit transfer of semantic ownership; local workers and processes are not transferred.

Convergence is distribution pointed toward the review node. Strategies remain inspectable: spread evenly, fill observed capacity, or match provider capability while respecting priority and Feature order.

State synchronizes symmetrically on `refine/state`; application branches remain separate. Goal records carry target branch, base commit, candidate branch, and exact candidate commit. Implementation, quality, and governance evidence are produced where work runs, and review and integration consume that durable evidence.

How divergent `refine/state` heads converge is the persistence-sync
Application's persistence-sync policy, not Fleet's
(`docs/intent/03-application/04-persistence-sync.md`). Fleet keeps only the
principles that outlast any mechanism — and they are more than prose:
persistence sync hands this doctrine verbatim to the resolving agent as its
guidance, and a test pins that quote to this document so code and intent
cannot drift. Goal ownership changes only through supported transfer
surfaces. A valid one-sided node_id change is an explicit transfer and remains
authoritative while every other member is reconciled. Different concurrent
transfers, or missing or malformed operands, remain ambiguous: automatic
resolution must not delete the Goal or choose its owner. Timestamps, recency,
current host, merge location, retry order, and merge-base ownership decide
nothing. Round evidence and the workflow authority that produced it (status,
assignment, branch) move as one coupled unit: Rounds and other identity-free
ordered arrays are atomic and never split from that authority. Nothing is
silently destroyed: every losing side is retained as a ref before
publication. When resolution genuinely needs help, escalation carries a
domain-terms question an operator can answer in one read, never a bare fence.
The agent resolves ordinary contested members first; the daemon's automatic
recovery remains available afterward under its own opt-out, but it overlays a
proven transferred `node_id` on its whole-record choice and leaves ambiguous
ownership unchanged for explicit sync authority or a supported Goal transfer.
The deterministic structural driver merges what it can prove disjoint,
including the `nodes.json` registry, whose records union by canonical node id
so absence never implies node deletion.

A fleet is upgraded node by node, in any order, and is never required to
upgrade at once. A node's CLI and daemon are one binary, so no node is ever
internally mixed; between nodes there are exactly two shared surfaces. The
first is `refine/state`, where an upgraded node's merges and a
not-yet-upgraded node's linear rebase-and-push are each just commits to the
other side, so the branch keeps converging throughout a rollout and neither
kind of node loses or deletes the other's records. The second is the daemon
API: a node still on the previous contract version rejects a newer node's
request, and fleet surfaces report that as that node's pending-upgrade status
— a per-node condition that leaves the rest of the fleet syncing, keeps the
node eligible for work because it is still a working node, and clears when
that node's turn comes. Upgrading a node also retires whatever the previous
build left on it, without losing a record.

What one node observes of another's daemon — reachable, upgraded, erroring —
belongs to the pass that observed it and is reported there. It is never
written into the synchronized node registry, because it is a fact about the
link between those two nodes rather than shared truth, and because a node's
recorded health is its provisioning verdict: the one signal that withholds
work from a node, owned by the bootstrap that produced it. No answer to a
synchronization request may create that verdict or erase it.

Sync health is bound to the serving daemon's active target and node and remains outside `refine/state`. A daemon reports its active node from local evidence and reports other nodes as unknown unless it has direct evidence from those daemons. A stale or failed local reconciliation means that daemon's fleet-wide counts are a local projection, not an authoritative fleet total; surfaces must label that boundary instead of presenting divergent arithmetic as fact.

Repeated unchanged background failures back off under a cap; any change in
context or a successful sync resets suppression, and an explicit `sync`
always remains immediately available.

Infrastructure and credentials remain outside Refine's core. The manage-fleet runbook guides agents through provisioning and authentication without placing secrets in shared state.

## Future Direction

Fleet policy may add dependency-aware placement, health, conflict prediction, and capacity awareness. Those remain richer uses of distribute and synchronized Goal authority, not a separate durable scheduler or worker-lock system.
