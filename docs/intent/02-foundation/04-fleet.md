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

Symmetric sync never infers authority. A missing baseline remains a fail-closed
whole-side recovery. Unequal edits from a valid baseline produce an atomic,
complete node-local conflict report while health and activity expose only its
stable id, count, location, and guidance. The corresponding stale-fenced
preview supports a default side and explicit per-path overrides. Recovery
preserves one-sided and schema-proven changes, retains pre-recovery and target
refs, compare-and-swaps the remote head, and resumes only its exact manifest.
Rounds and other identity-free ordered arrays remain atomic; compatible Goal
merges are limited to object members and stable keyed collections. The shared
`nodes.json` registry is reconciled by canonical node id: records are retained
as a union, one-sided changes survive, and concurrent versions use the later
comparable record timestamp. Absence never implies node deletion. Invalid or
duplicate identities, malformed timestamps, and equal-timestamp disagreement
remain conflicts, and an unrelated unresolved path withholds the entire
prepared reconciliation.

Sync health is bound to the serving daemon's active target and node and remains outside `refine/state`. Monotonic attempt ids and sources fence overlapping settlements: a late result cannot relabel a newer attempt, and a neutral lock deferral cannot clear an active failure episode or its report pointer. A daemon reports its active node from local evidence and reports other nodes as unknown unless it has direct evidence from those daemons. A stale or failed local reconciliation means that daemon's fleet-wide counts are a local projection, not an authoritative fleet total; surfaces must label that boundary instead of presenting divergent arithmetic as fact. Ordinary per-node heartbeat churn is semantically reconciled and does not by itself stale-fence synchronization or downgrade otherwise authoritative fleet counts.

Infrastructure and credentials remain outside Refine's core. The manage-fleet runbook guides agents through provisioning and authentication without placing secrets in shared state.

## Future Direction

Fleet policy may add dependency-aware placement, health, conflict prediction, and capacity awareness. Those remain richer uses of distribute and synchronized Goal authority, not a separate durable scheduler or worker-lock system.
