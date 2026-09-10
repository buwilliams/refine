# Workflow

## Key Ideas

- **Goal Lifecycle**: synchronized Goal state expresses what work may happen next.
- **Always-On Automation**: workflow is state movement, not a user-facing scheduler.
- **Agents As Tools**: agents perform steps; they do not own workflow meaning.
- **Transient Execution**: local workers are replaceable and may run work at least once.
- **Hard Sequencing**: Feature order and priority enforce when Goals may start.
- **Shared Semantics**: CLI, browser, API, MCP, and agent surfaces use the same rules.

## Purpose

Workflow moves software work forward without turning each Goal into an ad hoc chat session. It plans, implements, quality-checks, governs, integrates, reviews, retries, pauses, resumes, and recovers work through explicit Goal states.

The point is durable semantic advancement. Refine should know what can happen next, why it can happen, and which node owns the Goal without persisting a second execution-ownership state machine.

## Expected Role

The lifecycle is:

- backlog: captured work waits until it is ready;
- todo: actionable work is eligible on its assigned node;
- plan: configured Plan Skills produce accepted implementation plans from pinned project and Goal context;
- implement: a fresh agent changes the isolated candidate using the finalized plan;
- quality: a fresh agent reviews the plan and implementation, writes or selects appropriate tests, corrects the candidate, and proves the checks pass;
- governance: an independent review verifies project intent and applicable Skills before exact candidate integration;
- review: evidence and judgment accept or decline the integrated result;
- done: the intended outcome is complete;
- failed: the attempt stopped with inspectable evidence;
- cancelled: the work is intentionally terminal.

Workflow policy applies soft global, node, provider, and target-app limits based on observed live processes. Feature order and priority enforce start ordering. For ordered Goals in a Feature, Goal N+1 cannot start until Goal N reaches review, done, or cancelled. A failed ordered Goal does not release the barrier and blocks the rest of that Feature sequence.

Priority is also a hard gate across the active workflow lifecycle. A lower-priority authored Todo Goal is ineligible while a higher-priority authored and Feature-eligible Goal on the same node is Todo, plan, implement, quality, or governance. This includes Todo work already tracked by a runner. Already-active Goals continue; the lower-priority Todo becomes eligible only after the higher-priority work exits that lifecycle.

These gates are currently evaluated per node, not across the fleet. Users who need an ordered sequence enforced today must assign its Goals to one node. Global enforcement across all nodes is the intended direction.

Linear Feature order plus priority is Refine's canonical dependency model. User-authored “after X” placement and imported `depends_on` relationships compile to the Feature's integer order; Refine deliberately has no separate DAG or blocked-by graph.

The existing admission cycle remains active while child Goal executions run. It materializes pending occurrences and alternates admission opportunities between eligible Goals and standalone Skills. Both use shared transient reservations and observed supervised processes for global, node, provider, and app capacity; one execution is counted once. Busy checkouts and malformed pending records do not prevent unrelated eligible work from being considered. Pending-record reads are bounded and rotate through the queue. An in-memory active set avoids duplicate launches in one runner, while synchronized Goal status, node assignment, and Round remain authoritative across nodes.

Workers persist semantic artifacts and reread Goal authority at transitions and consequential boundaries. A restart may schedule the same nonterminal Goal again. Preserved planning, Git, quality, governance, integration, logs, branches, and worktrees make that repetition idempotent and explainable. A valid Quality or Governance finding may draft a fresh recovery Round and atomically return the Goal to todo; both stages share one bounded retry counter. Provider, parsing, Git, harness, and infrastructure failures do not consume that automatic recovery budget.

Preparation and non-retryable failures move an unchanged active Goal to failed. Retryable local failures use in-memory backoff and do not create durable delay records. Pause controls suppress new work and quiesce supported processes.

Bulk status correction protects automated states from generic replacement. Explicit cancellation is the lifecycle exception: it writes `cancelled` as Goal intent and then performs best-effort local cleanup per Goal.

The [Shared Workflow Consistency Contract](11-consistency-contract.md) and [Execution Ownership](../03-execution-ownership.md) define the authority and recovery rules.

Scheduling contains each Goal's preparation, Skill execution, result construction and failure settlement, including panics. Preparation and settlement occupy the Goal's local slot until their owning task ends. Observed live child groups continue to occupy capacity after a task or group leader exits. Finished thread handles provide completion evidence even if result delivery fails. A transient admission error is retried on the next poll while completions continue to drain; it does not suppress admission for the rest of a long-running pass.

The scheduling thread publishes port-local ticks at least once a second, including pause and app detachment. A tick records its worker incarnation, managed registration and OS identity, canonical runtime and target, node, sequence, last completed admission cycle, active attempts and failure context. Thirty seconds without a valid tick or completed cycle is unhealthy; startup has its own thirty-second bound. Long-running Goals retain fresh scheduling cycles. A target switch or detachment stops new admission to the old target while owned attempts drain with fresh ticks. Missing evidence is explicit, and disabled automation is a policy state.

Independent daemon supervision replaces a stalled worker only after fencing launches, stopping the exact old incarnation, rescanning its registered groups and proving their exit. A replacement is accepted after its own matching scheduler tick. Restart delay grows from one second to a five-minute ceiling. Ambiguous identity, failed termination or unreadable recovery evidence remains unhealthy with inspection and recovery guidance; it never permits overlapping replacement work. HTTP reachability and shutdown remain available throughout.

## Future Direction

Workflow should support richer agent selection, multi-agent composition, evidence-aware review, and merge orchestration while preserving explicit Goal state, cheap restart, shared semantics, and inspectable evidence.
