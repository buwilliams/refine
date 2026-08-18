# Node

## Key Ideas

- **Node As Owner**: synchronized Goal assignment makes responsibility explicit.
- **Local First, Multi-Node Ready**: one machine stays simple while the same model can scale to a fleet.
- **Durable Semantic Ownership**: node ownership belongs on Goals and Features, not transient workers.
- **Recoverable Handoffs**: branches, Rounds, and evidence let another node continue.

## Purpose

Node explains how Refine grows from one local agent into coordinated agents across machines. Parallel work needs a durable answer to which node may advance each Goal, while process execution remains local to the machine doing the work.

The durable node id is the authoritative ownership key stored on synchronized Goals and Features. A display name is only a label. Active-node selection is runtime-local and project-scoped; the node registry is shared project state. Attach, switch, synchronization, and restart resolve the local selection against that registry.

`default` is the reserved single-node compatibility identity with canonical label `Default`. Explicit renames remain valid. Ambiguous legacy labels remain inspectable and require an operator choice rather than a silent rewrite.

## Expected Role

- Goals are the smallest schedulable units and Features preserve larger order.
- A worker schedules only Goals whose `node_id` matches its active node.
- Status, node, and Round are reread before workflow transitions and consequential effects.
- Live process records describe current local execution but do not grant synchronized authority.
- Git worktrees and branches isolate concurrent changes; logs and semantic evidence support handoff.
- If queued reassignment races with automated start, the start on the previously authoritative node wins during synchronization; ambiguous lifecycle races conflict.

Independent Goals may proceed in parallel while Feature order and review boundaries constrain dependent work. Nodes coordinate through synchronized state and Git without requiring a central service.

## Future Direction

Nodes should gain better capability matching, dependency placement, work stealing, health, provenance, and recovery while keeping Goal assignment authoritative and worker identity transient.
