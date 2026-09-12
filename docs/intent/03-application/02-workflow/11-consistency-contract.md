# Shared Workflow Consistency Contract

## Key Ideas

- **Goal State Is Authority**: synchronized Goal status, node assignment, Round, and semantic evidence decide what work may advance.
- **Execution Is Local**: workers and process identifiers are transient observations on one node.
- **Human Decisions Supersede Automation**: explicit assignments can bypass automated requirements; stopped and superseded workers cannot veto them.
- **Conflicts Preserve Newer Decisions**: stale workers cannot overwrite reassignment, cancellation, a new Round, or another transition.
- **Settlement Is Occurrence-Fenced**: failure changes Goal status and originating Round metadata together only while that workflow step occurrence remains current.
- **Receipt-Based Restart**: replacing a process preserves accepted results; failed or interrupted work requires an explicit workflow decision before another attempt.

## Purpose

Multiple nodes and replaceable surfaces can act on the same synchronized Goals. This contract keeps those actors consistent while avoiding a second durable state machine for locks, reservations, or worker identity.

## Authority And Identity

Workflow owns Goal status, node assignment, Round history, and workflow decisions. Process owns node-local operations and managed-process facts. Git owns repository, worktree, ref, commit, and integration facts. Activity and evidence record what happened. Projections and surfaces own no authoritative workflow state.

Stable synchronized relationships use target-app, Goal, Round, Feature, and Git identities. Node-local operation, process, and session identifiers support control and may be cited as execution provenance, but they never authorize a workflow mutation or act as a resumable workflow checkpoint.

The Goal's existing `event_generation`, current Round, status and node identify the authorized step occurrence. Every status change, new Round, current request change, explicit same-step retry, redirect, cancellation, reopening or reassignment supersedes the previous occurrence at the Goal mutation boundary. Returning to the same named step or node does not restore an earlier occurrence. Evidence and ordinary metadata writes change `workflow_revision` for record compare-and-swap without creating another occurrence.

A claim records an execution's observed occurrence, claim-time record revision and timestamp. Claiming does not create workflow authority, and replacement claims do not supersede a workflow decision. Previous claims remain in the Round and claim history. Workers carry the authorized occurrence into every result write; a successful transition returns the destination occurrence under the same lock. Generic evidence writers used by a worker are bound to that occurrence as well. No lifecycle command must erase claims to make its decision effective.

## Required Invariants

A worker may start only after reading an eligible Goal assigned to its node. A
zero-Round Goal is never eligible. Todo start atomically rereads status, node,
exact authored Round count, non-empty request, and record revision before the
Plan write and before Git or agent side effects. A mismatched observation
stops that worker without rewriting the Goal.

Concurrent execution must reuse accepted semantic evidence or reject a superseded transition. Replacing a worker is process recovery, not authorization to retry failed work. Restart preserves completed step receipts and exact-candidate evidence; an interrupted or failed step enters Error handling unless an explicit workflow decision authorizes another occurrence. Missing or unverified process-scope exit still prevents overlapping worker replacement.

Implementation planning persists accepted plans and checklist evidence bound to Goal, Round, context digest, branch, target branch, and base commit. Plan Skills produce independent final plans with namespaced checklist IDs; historical proposals and criticisms remain readable. Invocation records pin the Event and Skill definitions, parameter values, occurrence identity, results, invalid responses, diagnostics, and process references. Invalid output emits Error without provider repairs. Restart retains accepted results and completed receipts; interrupted work without a completion receipt requires an explicit decision. A later workflow entry receives a new generation. Process receipts aid inspection and never grant workflow authority.

Governance acquires the integrated-target lease before final target revalidation and rereads originating Round, authorized step occurrence, node, status, and cancellation authority before every consequential boundary. The lease covers a provable clean rebase, atomic old/replacement identity persistence, exact replacement-candidate Quality and Governance, publication, integration, and settlement. Within the lease, repository-lock holds cover only Git mutations; the verdict, replacement Quality, and conflicted-refresh resolution run between holds, every rebase continue and the integration itself first re-prove the exact target tip the refresh observed, and a moved tip refreshes again for a bounded number of passes before queueing integration recovery. Cancellation or reassignment before a boundary prevents the next action. Once publication or integration begins it may finish and persist exact integration evidence even if cancellation arrives; cancellation remains the Goal's terminal status and no unsafe rollback is attempted.

Already-merged reconciliation is admitted only by the authoritative current Round's own integration identity, never by candidate reachability alone. The automatic workflow and operator action share one resolver. It observes exact local and required published ancestry under repository coordination, then revalidates the originating Round, authorized step occurrence, candidate, integration, candidate-bound Governance, and isolated Quality proof while holding the Goal mutation lock. Quality proof is retained, normalized only from a complete legacy identity, or regenerated in a clean managed checkout of the exact candidate; a merged-target descendant cannot supply approval. The terminal mutation records the proof disposition and moves Quality to Review atomically. Repeated or concurrent callers return the existing Review outcome. Missing, mismatched, failed, or unavailable admitted proof remains explicit and fails closed; the separate destructive revert capability retains its exact target-snapshot fence for genuine failed verdicts.

Target advancement is distinct from provenance failure. Candidate refresh requires an exactly resolving base and candidate, base ancestry of both candidate and target, a clean candidate branch still naming the candidate, and a linear unambiguous delta. Failed refresh or integration preserves original candidate identities, handoff, gates, target observations, and conflict evidence. The originating attempt emits Error; no recovery Round is appended automatically. Subsequent work requires an explicit workflow control request with revision and ownership checks. Forced integration is separately audited against real pinned inputs and the existing Git compare-and-swap boundary; forced Done changes status only.

The selected Round index and request remain the execution axis through
Plan, Implement, Quality, and Governance.
Workflow-owned evidence may advance the record revision, but it may not silently
switch to another Round. Restart recovery never synthesizes a generic Round:
zero-Round Plan, Implement, Quality, or Governance Goals are preserved, diagnosed,
and skipped while valid siblings continue.

Preparation and behavior failures use one Goal-record mutation that verifies the originating Round and step occurrence under the Goal lock, then writes both `failed` status and failure metadata onto that exact Round. If cancellation is undone or bulk-moved to todo, the reopened intent wins immediately; after either same-Round retry decision or new-Round decision, an old worker may retain logs, process records, branches, and worktrees but cannot settle or contaminate the active Round's failure fields.

Synchronization resolves one narrow ownership race: when a queued Goal is reassigned concurrently with automated work starting on its previously authoritative node, the start wins and the reassignment request is discarded. Other competing lifecycle changes remain conflicts.

Browser, CLI, API, MCP, and agent tools submit Goal or process intent through shared Application behavior. They do not expose a parallel workflow-execution resource or infer authority from a local process identifier.

Failure settlement distinguishes an authoritative failed attempt, an already verified originating outcome, a superseded attempt and evidence that could not be persisted. It retries transient persistence faults at most three times, with bounded record-lock acquisition and backoff. Each retry rereads node ownership, originating Round, authorized step occurrence and cancellation under the Goal mutation lock. Newer intent is never overwritten to settle an old failure. Originating Goal, error, failure stage, failure timestamp, Round and authorized step occurrence are captured before settlement. Settlement panics become explicit unpersisted-evidence outcomes before the common runtime evidence finalizer. Serialization, storage and reporting faults cannot escape attempt completion. If runtime storage also fails, the fallible report contains the complete originating evidence, settlement outcome and write fault, and explicitly denies runtime persistence. Cancellation and superseding workflow decisions remain untouched. Explicit integration recovery remains subject to exact provenance and revision fencing.

Admission control delegates blocking discovery, promotion, Event dispatch, preparation and settlement while continuing to observe completions and emit scheduler ticks. In-flight preparation and settlement count against observed capacity, deduplicated against Skill reservations and managed child observations. A live or unverified owned group cannot release its slot merely because its leader or task ended. None of these runtime observations is synchronized Goal authority.

## Future Direction

Coordination may become more distributed, but stronger machinery should be added only when duplicate idempotent work is materially more expensive than maintaining it. The default remains synchronized semantic state, transient local execution, strict stale-write rejection, and observable recovery.

## Unresolved Outcomes And Compatibility

Admission, independent health and next-action diagnostics use the same workflow blocking assessment. Pending transition gates, pending Error handling, unresolved current-step settlement, authored Round requirements and ordering each expose their actual reason. Process liveness, capacity and local reservations remain observations; repository and workspace locks protect concrete side effects.

Failure settlement retains immutable runtime evidence even when the synchronized Goal cannot be written. Recovery retries settlement only, never the provider or Git work. Until settlement or an explicit superseding decision, the shared assessment exposes the unresolved outcome of that exact current occurrence. Later occurrences do not inherit it. If both Goal and runtime storage fail, the process retains the outcome and reports the complete evidence and write fault; persistence across process loss cannot be guaranteed when no storage accepted it.

Legacy `workflow_attempt_authority` fields remain readable provenance. Legacy `workflow-failure-fences` are retained evidence inputs, not an independent authority store: recovery relates the original Round and recorded revision to synchronized occurrence receipts, with recorded failure time as a fallback for older history. New occurrence receipts also retain the existing record revision so a late legacy result or skewed clock cannot outrank a later workflow decision. A bare legacy marker can identify only its unchanged post-claim record; copying or restoring its file cannot grant authority through a filesystem timestamp. A superseded Round or later workflow decision makes them historical, including after restart. No claim or failed-attempt evidence is deleted to release new work. Record revisions still protect compare-and-swap writes of requested transition snapshots; candidate identities and Git leases remain independent exact-side-effect preconditions.

## Judgment and coordination

The AI decides when to stop and whether work succeeds, using the Goal, context, and applicable Skill instructions. Refine requires a current response identity and a reported decision, not a proof of satisfactory work. References to evidence in this contract mean retained execution history, accepted decisions, or identities protecting concrete Git actions. They do not introduce required supporting lists, test commands, checklist coverage, or user-authored acceptance criteria. Quality and Governance Skills may express such expectations for the AI to interpret; Refine does not grade them independently.

Explicit Round deletion is a human exception to automatic history retention. Its shared capability confirms current process exit, fences stale results, removes the Round and associated records, and compacts remaining references. Git retains recoverable history. A cleanup journal may exist only while removal is incomplete; success removes it.
