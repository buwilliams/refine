# Fleet

## Key Ideas

- **Fleet As Composed Nodes**: nodes coordinate through shared Goal state and Git remotes.
- **Distribute Is The Mechanism**: assignment, rebalance, and handoff use one explicit operation.
- **Ephemeral Workers, Durable Evidence**: a worker can be rebuilt from Git and synchronized state.
- **Symmetric Sync**: every node publishes and pulls the same control branch.
- **Node-Local Sync Health**: each daemon reports its own reconciliation freshness; it cannot infer another daemon's health from shared state.
- **Judgment Converges**: implementation may run anywhere while review converges where judgment occurs.

## Purpose

Fleet explains how work moves across nodes and returns for review. One machine is a fleet of one; larger fleets add parallel execution without changing the Goal ownership model.

## Expected Role

Distribute reassigns eligible queued Goals among enabled, healthy nodes. It does not move a Goal already in an automated state. Reassignment of queued work is the explicit transfer of semantic ownership; local workers and processes are not transferred.

Convergence is distribution pointed toward the review node. Strategies remain inspectable: spread evenly, fill observed capacity, or match provider capability while respecting priority and Feature order.

State synchronizes symmetrically on `refine/state`; application branches remain separate. Goal records carry target branch, base commit, candidate branch, and exact candidate commit. Implementation, quality, and governance evidence are produced where work runs, and review and integration consume that durable evidence.

How divergent `refine/state` heads converge is the persistence-sync
capability's policy, not Fleet's
(`docs/intent/03-capabilities/05-persistence-sync.md`). Fleet keeps only the
principles that outlast any mechanism. Reconciliation never guesses a winner
from circumstance: timestamps, recency, and which node happens to run the
merge decide nothing. Ownership is declared doctrine, never circumstance:
the node that owned a record at the last agreed baseline is authoritative
for contested members, and staleness alone never discards work only the
owning node could produce — a stale local understanding is not a wrong one.
Round evidence and the workflow authority that produced it (status,
assignment, branch) move as one coupled unit: Rounds and other identity-free
ordered arrays are atomic and never split from that authority. Nothing is
silently destroyed: every losing side is retained as a ref before
publication. When resolution genuinely needs help, escalation carries a
domain-terms question an operator can answer in one read, never a bare
fence. The deterministic structural driver merges what it can prove
disjoint, including the `nodes.json` registry, whose records union by
canonical node id so absence never implies node deletion.

Sync health is bound to the serving daemon's active target and node and remains outside `refine/state`. A daemon reports its active node from local evidence and reports other nodes as unknown unless it has direct evidence from those daemons. A stale or failed local reconciliation means that daemon's fleet-wide counts are a local projection, not an authoritative fleet total; surfaces must label that boundary instead of presenting divergent arithmetic as fact.

Repeated unchanged background failures back off under a cap; any change in
context or a successful sync resets suppression, and an explicit project sync
always remains immediately available.

Infrastructure and credentials remain outside Refine's core. The manage-fleet runbook guides agents through provisioning and authentication without placing secrets in shared state.

## Future Direction

Fleet policy may add dependency-aware placement, health, conflict prediction, and capacity awareness. Those remain richer uses of distribute and synchronized Goal authority, not a separate durable scheduler or worker-lock system.
