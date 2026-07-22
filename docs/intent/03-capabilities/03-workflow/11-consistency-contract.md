# Shared Workflow Consistency Contract

## Key Ideas

- **One Coordination Boundary**: every workflow mutation crosses one shared, cross-process consistency boundary, regardless of which surface or agent requested it.
- **Narrow Authorities**: Goals, claims, operations, processes, leases, Git, and activity each own one kind of truth and relate through stable identities rather than copied status.
- **Evidence Before Advancement**: a Goal cannot move ahead of the durable evidence required to justify its next state.
- **Versioned Decisions**: mutations validate the state the actor observed; stale actors receive a conflict instead of overwriting newer work.
- **Crash-Repairable Progress**: an acknowledged decision survives interruption, and incomplete projections can be deterministically repaired without repeating consequential work.
- **Agents Decide, Refine Coordinates**: agents interpret product intent, constitution, quality, and recovery choices; Refine deterministically protects identity, ordering, ownership, persistence, and evidence.

## Purpose

Refine has several processes and replaceable surfaces acting on the same local work. A daemon can schedule a Goal while a runner completes it, a user cancels an operation, a Supervisor investigates failure, and Git or another node changes the durable state. Flat files and local ownership make that work inspectable, but they do not make overlapping read-modify-write sequences consistent by themselves.

This contract defines the minimum architecture that keeps those actors from publishing mutually impossible truths. It belongs to Workflow because Workflow coordinates state movement. It does not move Foundation state, Process state, Git evidence, or logs into Workflow; each capability retains its authority and Workflow records their relationships.

The contract protects the product and constitution without encoding their judgment into a state machine. The effective product, constitution, guidance, and Goal round remain agent context. Workflow preserves which durable context an attempt used and enforces the deterministic boundary around what the attempt may change.

## Authoritative Ownership And Relationships

No record becomes authoritative merely because it is newest, easiest for a surface to read, or copied into a projection.

| Concept | Authoritative owner | Required relationships |
| --- | --- | --- |
| Target app | Foundation's target-app registry owns stable target-app identity and its current canonical root. A path, selected UI app, or process working directory is not identity. | Every Goal execution, claim, operation, process, lease, Git record, and evidence item names the target-app identity. Commands resolve and validate its root at the consistency boundary. |
| Product and constitution | Durable target-app state owns the effective product, constitution, guidance, and governance context. | An execution identifies the context revision it received when that context can affect implementation, quality, review, or recovery evidence. Runtime copies are not authority. |
| Goal and rounds | The durable Goal aggregate owns Goal identity, Feature membership and order, current workflow state, node assignment, and its ordered rounds. A round is an append-only instruction revision within one Goal, not an execution retry. | Claims and evidence identify both Goal and round. Earlier rounds and their reports are not rewritten when a new round is appended. A retry creates a new execution attempt against the same round unless instructions changed and a new round was explicitly added. |
| Workflow transition | Workflow owns the durable decision to accept or reject one state-changing command, including actor, idempotency identity, source revisions, validated relationships, and legal from/to state. | A committed transition names every authoritative record it coordinates. It explains why related authorities change but does not replace their own records. |
| Workflow claim | Workflow owns the exclusive right for one node and execution to advance one Goal round. | A claim binds target app, Goal, round, node, execution, and the revisions on which eligibility was decided. At most one active claim may advance a Goal unless an explicit future multi-agent mode defines cooperative ownership. |
| Capacity lease | The shared capacity capability owns admission against global, node, provider, and target-app limits. | A lease names its holder and associated claim or agent session. It is acquired with the right to start work, is never inferred from a Goal or PID count, and is released exactly once when its protected consumption ends. |
| Operation | Process owns the durable lifecycle of a requested unit of work: accepted, active, cancelling, and one terminal outcome with progress, result, or error. | A workflow-owned operation names its target app, Goal, round, claim, and execution. An operation result is evidence for Workflow; it does not independently move a Goal. |
| Managed process | Process supervision owns observed child lifecycle, process identity, ownership, output locations, and exit information. | A managed child names the operation or execution that launched it. A PID or exit code is process evidence, not proof of workflow success, and PID reuse must not establish identity. |
| Git branch and worktree evidence | The Git capability owns repository identity, worktree location, branch/ref identity, base and candidate commits, dirty state, and integration result observed at a point in time. | Git evidence names target app, Goal, round, and execution. Workflow persists exact refs and commit identities used for a handoff or merge; it does not reconstruct them later from a branch name alone. |
| User-visible logs and evidence | Activity and evidence own the append-only explanation of accepted decisions, execution output, failures, reviews, and recovery. | Entries name the transition or idempotency identity plus applicable target app, Goal, round, claim, operation, process, and Git evidence. Logs explain authority but do not replace its records. |
| Projections and caches | No projection is authoritative. | UI summaries, workflow counts, Supervisor health, and search indexes derive from the records above and carry a source revision or freshness marker. They may be discarded and rebuilt. |

Identifiers are stable and never reassigned. Relationships are recorded when the records are created; later code must not guess them from labels, timestamps, branch-name patterns, current app selection, or error text.

## The Consistency Boundary

Every command that can change workflow meaning across a Goal, round, claim, operation, managed execution, capacity lease, Git handoff, or required evidence is coordinated by one shared Workflow command path. A component-local mutation that cannot affect workflow meaning remains with its owning capability, using that authority's own version and idempotency rules. The workflow boundary is cross-process: a process-local mutex, browser request ordering, or a single runner loop is insufficient.

A mutation carries a stable command or idempotency identity and the expected revision of every authoritative record on which its decision depends. A revision may be a generation, content identity, or another comparable durable version; it must change when relevant authority changes. The coordinator resolves the target-app identity to its canonical root, reads the authoritative records, and validates:

- the command has not already committed;
- expected revisions still match;
- Goal, round, target app, node, claim, operation, process, lease, and Git relationships agree;
- the actor still owns the action and the requested state movement is legal;
- Feature ordering, pause state, capacity, and other eligibility inputs used by the decision are still current.

The implementation may serialize a complete transition or use optimistic compare-and-commit. If all authoritative records cannot share one physical transaction, it must first durably commit one transition decision and then apply the remaining record changes as idempotent projections of that decision. The decision is the commit point: before it, no consequential external effect is authorized; after it, recovery completes the same decision rather than choosing again.

A command that requires an external effect has prepared and settled phases under the same command identity. The prepared decision durably authorizes the exact launch, cancellation, quality action, or Git action before it occurs. The settled decision consumes the correlated result and required evidence before advancing the Goal. Recovery reconciles a prepared action by identity; it does not blindly perform the effect again or pretend that preparation was success.

Concurrent commands against the same authority have a deterministic outcome. The first valid terminal decision wins. A late completion, cancellation, retry, recovery, or surface write is retained as evidence when useful but cannot overwrite that decision. Repeating a committed command returns its existing outcome. A different command based on an old revision fails with a conflict that identifies what must be refreshed.

## Legal Transition Ordering

The workflow states and actor permissions defined by the Workflow documents remain the legal graph. The normal automated path is `todo` to `in-progress` to a reviewable handoff, then applicable build and QA work, review, and `done`; `failed` and `cancelled` are explicit exits. Backlog promotion, review revision, undo, retry, and reopening occur only through their documented shared operations. This contract adds ordering invariants, not shortcut edges.

### Starting work

1. Resolve the target app and current Goal round, then validate eligibility against their expected revisions.
2. Commit the active claim and required capacity lease as one logical transition with the Goal's move to `in-progress`. None may become visible as active without the others.
3. Commit an operation or launch intent correlated to that claim before starting external work.
4. Start work through Process supervision so the child is discoverable under the same execution identity before it can publish workflow mutations.

If launch fails, the same execution becomes failed or interrupted with evidence; it does not silently return to an unclaimed state.

### Advancing successfully

1. Persist the operation result and required process, agent, quality, governance, review, and Git evidence for the exact execution.
2. Revalidate the Goal round, claim ownership, target-app mapping, and evidence revisions at the consistency boundary.
3. Commit the claim's terminal outcome, the Goal transition, its evidence links, and capacity disposition as one logical transition.
4. Publish activity, projections, and user notices from the committed decision.

Process exit zero alone never advances a Goal. `ready-merge` requires an inspectable candidate identity. `done` requires the review or approval decision and exact integration evidence required by the workflow, not merely the absence of a running process.

### Failure and cancellation

Failure first preserves the best available error, output, and repository state, then commits the operation or claim failure, the legal Goal transition, evidence links, and capacity disposition as one logical transition. If complete evidence cannot be collected, the durable failure record states what is missing rather than fabricating certainty.

Cancellation commits a cancellation decision for the exact execution before requesting external termination. That decision prevents a late worker from publishing success. Managed processes may remain visibly `stopping` until their actual exit is observed. A capacity lease is released only when its defined protected consumption has ended; logical cancellation may release it earlier only if the remaining process is fenced from consequential work and the capacity policy explicitly treats it as non-consuming. Repeated cancellation and release are idempotent.

Adding a round, retrying, or recovering never reopens a terminal claim. It creates a new execution identity after the previous attempt is terminal. A new round is appended before it becomes current and before a new claim can reference it.

## Durability, Failure, And Recovery

An acknowledged commit must survive daemon, runner, or host-process interruption. Authoritative records are published as complete old or complete new versions, never torn or partially encoded state. Required evidence is durable before a transition that depends on it is acknowledged. Incremental process output should be preserved as it is produced so a crash does not erase the only useful diagnosis.

The transition decision and prerequisite evidence are authoritative durability; cache refreshes, UI notices, duplicated activity views, and other projections may follow. Their writers use the transition identity for idempotency. A crash between the commit point and projection completion therefore causes temporary incompleteness, not a contradictory second decision.

Recovery replays committed decisions and reconciles related authorities. It may rebuild projections, finish an idempotent write, confirm process liveness, terminate an orphaned child, release a proven-abandoned lease, or expose an interrupted attempt. It must not infer success from an old timestamp, missing PID, absent cache entry, branch name, or partially written log. When authority or liveness cannot be proven, recovery preserves the records and raises an actionable interrupted or attention state.

Automated or Supervisor-led recovery uses the same command boundary, expected revisions, permissions, and evidence rules as any other actor. Diagnosis and bounded correction may be agent judgment; committing the recovery decision is deterministic. Recovery cannot silently change target app, node ownership, Goal instructions, product, constitution, Git base, or destructive-action authority. An explicit retry is observable and gets a new execution identity.

## Compatibility And Migration

The contract applies during upgrades, not only after all records have the newest shape.

- Readers accept supported legacy records without treating missing relationships as proof. Deterministic relationships may be derived only when unambiguous; otherwise the record remains observable and needs attention.
- The first mutation of a legacy aggregate establishes its baseline revision and missing stable relationships inside the same consistency boundary. Migration preserves unknown fields, Goal and round history, logs, operation results, and Git evidence.
- Migrations and projection rebuilds are idempotent and restartable. They record their outcome and never advance workflow merely because a schema was upgraded.
- Legacy runtime operations, processes, or leases without safe correlation remain visible as legacy or orphaned evidence until reconciled. They are not attached to whichever target app is currently selected.
- During mixed-version operation, a writer that cannot participate in the shared boundary must be rejected or restricted to reads. Compatibility must not reintroduce last-write-wins mutation through an older CLI, API, daemon, or direct file path.

## Thin-Surface Obligations

Browser, desktop, CLI, API, MCP, and agent tools are adapters. They may gather intent and present results, but they do not own transition policy, revisions, retries, capacity, process truth, Git truth, or evidence ordering.

A mutating surface sends the stable actor and target-app identity, command/idempotency identity, expected revision from the state being acted on (or an explicit create/no-prior-state precondition), and the requested shared operation. It does not directly edit durable files, preemptively update authoritative UI state, infer completion from a request ending, or emit a success log before the shared command commits.

The shared response distinguishes accepted, committed, conflicted, and still-running work. It returns the committed identities and revisions needed to refresh Goal, claim, operation, process, lease, Git, and evidence views. Conflicts are normal coordination outcomes: surfaces show the newer truth and safe retry choices rather than hiding them behind generic failure or automatically resubmitting a stale decision.

## Future Direction

The mechanism may evolve from local files and processes to stronger journals, replication, or distributed consensus as fleet scale demands. The invariant should remain small: one versioned decision boundary coordinates narrow authorities, evidence precedes the state it proves, agents retain adaptable judgment, and every surface observes the same recoverable truth.
