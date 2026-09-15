# Shared Workflow Consistency Contract

## Key Ideas

- **Stability Under Current Intent**: every accepted action leaves the daemon responsible for restoring usable execution, including after interruption.
- **Execution Is Local**: workers and process identifiers are transient observations on one node.
- **Decisions Supersede Execution**: humans, Skills, and agents use the shared workflow capability; superseded workers and historical evidence cannot veto a later decision.
- **Conflicts Preserve Newer Decisions**: stale workers cannot overwrite reassignment, cancellation, a new Round, or another transition.
- **Settlement Is Occurrence-Fenced**: failure changes Goal status and originating Round metadata together only while that workflow step occurrence remains current.
- **Receipt-Based Restart**: replacing a process preserves accepted results; failed verdicts require an explicit workflow decision before another attempt, while unfinished work remains scheduled under current workflow authority.

## Purpose

Multiple nodes and replaceable surfaces can act on the same synchronized Goals. This contract keeps those actors consistent while avoiding a second durable state machine for locks, reservations, or worker identity.

Surfaces record actions through Application capabilities. The daemon completes downstream cleanup and execution without depending on the lifetime of the request thread. Existing generated state is reusable when coherent; it is not an obligation that permanently prevents progress. These consistency checks enforce the current decision.

## Authority And Identity

Workflow owns Goal status, node assignment, Round history, and workflow decisions. Process owns node-local operations and managed-process facts. Git owns repository, worktree, ref, commit, and integration facts. Activity and evidence record what happened. Projections and surfaces own no authoritative workflow state.

Stable synchronized relationships use target-app, Goal, Round, Feature, and Git identities. Node-local operation, process, and session identifiers support control and may be cited as execution provenance, but they never determine workflow eligibility or act as a resumable workflow checkpoint.

The Goal's existing `event_generation`, current Round, status and node identify the current step occurrence. Every effective status change, new Round, current request change, explicit retry, redirect, cancellation, reopening or reassignment supersedes the previous occurrence at the Goal mutation boundary. Assigning the current step is a no-op: no write, new occurrence, lifecycle dispatch, evidence invalidation, or process stop. Retry and Integrate are distinct operations. Returning to a step after an intervening decision does not restore its earlier occurrence. Evidence and ordinary metadata writes change `workflow_revision` for record compare-and-swap without creating another occurrence.

An explicit selection is persisted before termination. Its existing occurrence records that it supersedes execution; daemon maintenance compares this boundary with each managed process's launch occurrence and stops obsolete processes, including the submitting Skill. Normal automated phase progression does not stop its enclosing execution. Cleanup runs independently of admission and pause, including for parked and terminal Goals, retries interrupted termination, and preserves replacement processes through exact registration and OS identity checks. Unconfirmed old writers prevent shared workspace reuse without rolling back the selected step.

A claim records an execution's observed occurrence, claim-time record revision and timestamp. Claiming does not create workflow authority, and replacement claims do not supersede a workflow decision. Previous claims remain in the Round and claim history. Workers carry the current occurrence into every result write; a successful transition returns the destination occurrence under the same lock. Generic evidence writers used by a worker are bound to that occurrence as well. No lifecycle command must erase claims to make its decision effective.

## Required Invariants

A worker may start only after reading an eligible Goal assigned to its node. A
zero-Round Goal is never eligible. Execution preparation atomically rereads status, node,
exact authored Round count, non-empty request, and record revision before the
Plan write and before Git or agent side effects. A mismatched observation
stops that worker without rewriting the Goal.

Concurrent execution must reuse accepted semantic evidence or reject a superseded transition. Replacing a worker recovers execution while preserving completed verdicts. Restart preserves completed step receipts and exact-candidate evidence; a failed verdict enters Error handling unless an explicit workflow decision starts another occurrence. An unfinished step continues in its existing occurrence after complete prior execution exit is established. Missing or unverified process-scope exit still prevents overlapping worker replacement.

Shutdown preserves that distinction by quiescing the worker and its workload descendants together and retaining their process guardians until exit receipts are complete. Shared process ownership recovery can use a verified enclosing scope's original exit receipt to close a lost nested guardian only when exact isolated launch identities and retained witnesses prove containment. It never manufactures a receipt, treats a shared incarnation as proof, or replaces a completed failed verdict with a successful one.

Implementation planning persists accepted plans and checklist evidence bound to Goal, Round, context digest, branch, target branch, and base commit. Plan Skills produce independent final plans with namespaced checklist IDs; historical proposals and criticisms remain readable. Invocation records pin the Event and Skill definitions, parameter values, occurrence identity, results, invalid responses, diagnostics, and process references. Invalid output emits Error without provider repairs. Restart retains accepted results and completed receipts; unfinished work without a completion receipt continues under a currently admitted owner after prior execution is proven stopped, preserving interrupted launch history and existing workspace changes. A later workflow entry receives a new generation. Process receipts aid inspection and never grant workflow authority.

Governance acquires the integrated-target lease before final target revalidation and rereads originating Round, current step occurrence, node, status, and current decision before every consequential boundary. The lease covers a provable clean rebase, atomic old/replacement identity persistence, exact replacement-candidate Quality and Governance, publication, integration, and settlement. Within the lease, repository-lock holds cover only Git mutations; the verdict, replacement Quality, and conflicted-refresh resolution run between holds, every rebase continue and the integration itself first re-prove the exact target tip the refresh observed, and a moved tip refreshes again for a bounded number of passes before queueing integration recovery. Cancellation or reassignment before a boundary prevents the next action. Once publication or integration begins it may finish and persist exact integration evidence even if cancellation arrives; cancellation remains the Goal's terminal status and no unsafe rollback is attempted.

Already-merged reconciliation is admitted only by the authoritative current Round's own integration identity, never by candidate reachability alone. A retained revert record makes that integration historical: Git ancestry survives a revert, so preparation archives the old execution and reproduces from the current target without undoing the revert. The automatic workflow and operator action share one resolver. It observes exact local and required published ancestry under repository coordination, then revalidates the originating Round, current step occurrence, candidate, integration, candidate-bound Governance, and isolated Quality proof while holding the Goal mutation lock. Quality proof is retained, normalized only from a complete legacy identity, or regenerated in a clean managed checkout of the exact candidate; a merged-target descendant cannot supply approval. The terminal mutation records the proof disposition and moves Quality to Review atomically. Repeated or concurrent callers return the existing Review outcome. Missing, mismatched, failed, or unavailable admitted proof remains explicit and fails closed; the separate destructive revert capability retains its exact target-snapshot fence for genuine failed verdicts.

Target advancement is distinct from provenance failure. All executable steps use one workspace preparation capability. It retains a coherent base, branch and candidate together, reconstructs missing work from verified identities where possible, and never attaches an advanced target observation as a new base to an unchanged retained candidate. Missing or inconsistent generated inputs can cause a recorded recovery to Plan on a fresh execution branch under the same authored Round. Old work and reports remain historical; the shared invalidation policy removes their current applicability and the replacement work receives its configured checks. Selecting Plan does not itself author a new Round.

Candidate refresh and integration retain their exact Git preconditions: resolving identities, base ancestry, the expected branch tip, and an unambiguous delta where required. Failed refresh or integration retains the original identities and conflict evidence and emits Error. Preparation recovery does not convert a completed failed verdict into success or retry arbitrary provider/Git errors. Forced integration is separately audited against pinned inputs and the existing Git compare-and-swap boundary; forced Done changes status only. A superseded integration completion records actual effects without overwriting the later decision.

The selected Round index and request remain the execution axis through
Plan, Implement, Quality, and Governance.
Workflow-owned evidence may advance the record revision, but it may not silently
switch to another Round. Restart recovery never synthesizes a generic Round:
zero-Round Plan, Implement, Quality, or Governance Goals are preserved, diagnosed,
and skipped while valid siblings continue.

Preparation and behavior failures use one Goal-record mutation that verifies the originating Round and step occurrence under the Goal lock, then writes both `failed` status and failure metadata onto that exact Round. If cancellation is undone or bulk-moved to todo, the reopened intent wins immediately; after either same-Round retry decision or new-Round decision, an old worker may retain logs, process records, branches, and worktrees but cannot settle or contaminate the active Round's failure fields.

Synchronization preserves a valid one-sided node transfer when it races with automated start on the old node. Independent conflicting decisions remain visible through existing state-sync conflict handling; local process observations or timestamps do not invent an ordering.

Browser, CLI, API, MCP, and agent tools submit Goal or process intent through shared Application behavior. They do not expose a parallel workflow-execution resource or infer authority from a local process identifier.

Failure settlement distinguishes an authoritative failed attempt, an already verified originating outcome, a superseded attempt and evidence that could not be persisted. It retries transient persistence faults at most three times, with bounded record-lock acquisition and backoff. Each retry rereads node ownership, originating Round, current step occurrence and cancellation under the Goal mutation lock. Newer intent is never overwritten to settle an old failure. Originating Goal, error, failure stage, failure timestamp, Round and current step occurrence are captured before settlement. Settlement panics become explicit unpersisted-evidence outcomes before the common runtime evidence finalizer. Serialization, storage and reporting faults cannot escape attempt completion. If runtime storage also fails, the fallible report contains the complete originating evidence, settlement outcome and write fault, and explicitly denies runtime persistence. Cancellation and superseding workflow decisions remain untouched. Explicit integration recovery remains subject to exact provenance and revision fencing.

Admission control delegates blocking discovery, Event dispatch, preparation and settlement while continuing to observe completions and emit scheduler ticks. In-flight preparation and settlement count against observed capacity, deduplicated against Skill reservations and managed child observations. A live or unverified owned group cannot release its slot merely because its leader or task ended. None of these runtime observations is synchronized Goal authority.

Derived scheduling indexes and pending Event dispatch files are reconstructible from durable Goal records. New Event deliveries retain their pinned dispatch descriptors on the Goal; legacy queue-only deliveries cannot be reconstructed if both the queue file and its pinned inputs are unavailable. Interrupted index updates remain discoverable; malformed per-Goal and per-dispatch data is reported without stopping valid siblings. A damaged authored Goal record is isolated and retried when repaired; recovery does not choose a potentially older synchronized copy as the current decision. Failed startup recovery remains pending for another pass. These mechanisms do not invent missing authored requests or decisions, or silently reopen Backlog, Cancelled, Done, or a completed failed verdict.

## Future Direction

Coordination may become more distributed, but stronger machinery should be added only when duplicate idempotent work is materially more expensive than maintaining it. The default remains synchronized semantic state, transient local execution, strict stale-write rejection, and observable recovery.

## Unresolved Outcomes And Compatibility

Admission, independent health and next-action diagnostics use the same workflow blocking assessment. Pending transition gates, pending Error handling, unresolved current-step settlement, authored Round requirements and ordering each expose their actual reason. Process liveness, capacity and local reservations remain observations; repository and workspace locks protect concrete side effects.

Failure settlement retains immutable runtime evidence even when the synchronized Goal cannot be written. Recovery retries settlement only, never the provider or Git work. Until settlement or an explicit superseding decision, the shared assessment exposes the unresolved outcome of that exact current occurrence. Later occurrences do not inherit it. If both Goal and runtime storage fail, the process retains the outcome and reports the complete evidence and write fault; persistence across process loss cannot be guaranteed when no storage accepted it.

Legacy `workflow_attempt_authority` fields remain readable provenance. Legacy `workflow-failure-fences` are retained evidence inputs, not an independent authority store: recovery relates the original Round and recorded revision to synchronized occurrence receipts, with recorded failure time as a fallback for older history. New occurrence receipts also retain the existing record revision so a late legacy result or skewed clock cannot outrank a later workflow decision. A bare legacy marker can identify only its unchanged post-claim record; copying or restoring its file cannot grant authority through a filesystem timestamp. A superseded Round or later workflow decision makes them historical, including after restart. No claim or failed-attempt evidence is deleted to release new work. Record revisions still protect compare-and-swap writes of requested transition snapshots; candidate identities and Git leases remain independent exact-side-effect preconditions.

## Judgment and coordination

The AI decides when to stop and whether work succeeds, using the Goal, context, and applicable Skill instructions. Refine requires a current response identity and a reported decision, not a proof of satisfactory work. References to evidence in this contract mean retained execution history, accepted decisions, or identities protecting concrete Git actions. They do not introduce required supporting lists, test commands, checklist coverage, or user-authored acceptance criteria. Quality and Governance Skills may express such expectations for the AI to interpret; Refine does not grade them independently.

Explicit Round deletion is a human exception to automatic history retention. Its shared capability confirms current process exit, fences stale results, removes the Round and associated records, and compacts remaining references. Git retains recoverable history. A cleanup journal may exist only while removal is incomplete; success removes it.
