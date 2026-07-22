# Shared Workflow Consistency Contract

## Key Ideas

- **One Coordination Boundary**: every cross-process mutation of workflow meaning uses one shared consistency boundary.
- **Narrow Authorities**: each capability owns its truth and connects it to other truths through stable identities.
- **Evidence Before State**: evidence is durable before the state whose correctness depends on it.
- **Conflicts Preserve Decisions**: a mutation based on stale authority conflicts instead of overwriting newer work.
- **Crash-Repairable Commitments**: an acknowledged decision survives interruption and can be completed without deciding again.
- **Judgment Inside Integrity**: agents retain adaptable judgment while Refine deterministically protects identity, authority, durability, and coordination.
- **Thin Surfaces**: every human or agent surface delegates these rules to the shared capabilities.

## Purpose

Refine has multiple processes and replaceable surfaces acting on the same local work. Durable files make that work inspectable, but independent read-modify-write paths can still publish contradictory decisions. This contract defines the minimum consistency required across those actors without prescribing storage, serialization, or recovery machinery.

The contract belongs to Workflow as the coordinator of state movement. It does not collapse Foundation, Capabilities, and Surfaces into one owner. Product intent, constitution, guidance, and Goal instructions remain context for agent judgment; Refine coordinates the integrity of decisions made from that context.

## Authority And Identity

Authority stays narrow:

- Foundation owns stable target-app identity and its relationship to the current canonical root.
- Workflow owns Goals, their round history, workflow decisions, and claims to advance a specific Goal round.
- Process owns operations and managed-process facts; capacity owns admission leases.
- Git owns the repository, worktree, ref, commit, and integration facts it observes.
- Activity and evidence own the durable account of decisions and execution, not the decisions themselves.
- Projections and surfaces own no authoritative workflow state.

These authorities are connected only as needed by stable identities for the target app, Goal and round, claim, operation, process, capacity lease, Git observation, and evidence. A relationship is recorded rather than inferred later from a path, label, timestamp, process identifier, selected app, branch name, or copied status. One authority may use another as evidence without taking ownership of its truth.

## Required Invariants

Every mutation that can change workflow meaning across processes crosses the same shared coordination boundary, independent of its caller. The boundary acts on the current authoritative relationships and the state on which the caller's decision depends. If either has changed, the mutation conflicts; convenience, recency, or a retry cannot turn a stale observation into authority.

Evidence required to justify a workflow decision is durable before the state that relies on it. Logs, process outcomes, Git observations, reviews, and governance results remain evidence linked to the relevant identities; none independently becomes a workflow verdict.

Once Refine acknowledges a decision, both the decision and enough identity to repair its incomplete consequences are durable. After interruption, recovery preserves that decision and reconciles authorities or derived views without silently choosing a different outcome or repeating consequential work as a new decision.

Agents remain responsible for interpreting intent, weighing evidence, and making implementation, quality, review, and recovery judgments. Refine is responsible for deterministic integrity around those judgments: authority, identity, conflicts, durable acknowledgement, and evidence linkage.

Browser, desktop, CLI, API, MCP, and agent tools remain thin adapters. They submit intent to shared capabilities and present the resulting authoritative state; they do not duplicate coordination rules, write competing workflow truth, or infer success from a request, process, or local projection.

The implementation may use serialization, optimistic concurrency, journaling, or another design. It satisfies this contract only when every process and surface observes the same authority relationships and the invariants above remain true across interruption and concurrency.

## Future Direction

Coordination machinery may evolve with scale. The durable architecture is the boundary: narrow authorities connected by stable identity, evidence preceding the state it proves, stale mutations conflicting, acknowledged decisions remaining repairable, adaptable agents exercising judgment, and replaceable surfaces sharing one truth.
