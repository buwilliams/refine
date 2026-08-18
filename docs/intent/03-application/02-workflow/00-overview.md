# Workflow

## Key Ideas

- **Goal Lifecycle**: synchronized Goal state expresses what work may happen next.
- **Always-On Automation**: workflow is state movement, not a user-facing scheduler.
- **Agents As Tools**: agents perform steps; they do not own workflow meaning.
- **Transient Execution**: local workers are replaceable and may run work at least once.
- **Shared Semantics**: CLI, browser, API, MCP, and agent surfaces use the same rules.

## Purpose

Workflow moves software work forward without turning each Goal into an ad hoc chat session. It plans, implements, quality-checks, governs, integrates, reviews, retries, pauses, resumes, and recovers work through explicit Goal states.

The point is durable semantic advancement. Refine should know what can happen next, why it can happen, and which node owns the Goal without persisting a second execution-ownership state machine.

## Expected Role

The lifecycle is:

- backlog: captured work waits until it is ready;
- todo: actionable work is eligible on its assigned node;
- plan: independent agents propose, critique, and finalize an implementation plan from pinned project and Goal context;
- implement: a fresh agent changes the isolated candidate using the finalized plan;
- quality: a fresh agent reviews the plan and implementation, writes or selects appropriate tests, corrects the candidate, and proves the checks pass;
- governance: an independent review verifies product, constitution, rules, and guidance before exact candidate integration;
- review: evidence and judgment accept or decline the integrated result;
- done: the intended outcome is complete;
- failed: the attempt stopped with inspectable evidence;
- cancelled: the work is intentionally terminal.

Workflow policy applies soft global, node, provider, and target-app limits based on observed live processes. Feature order and priority shape selection. An in-memory active set avoids duplicate launches in one runner, while synchronized Goal status, node assignment, and Round remain authoritative across nodes.

Workers persist semantic artifacts and reread Goal authority at transitions and consequential boundaries. A restart may schedule the same nonterminal Goal again. Preserved planning, Git, quality, governance, integration, logs, branches, and worktrees make that repetition idempotent and explainable. A valid Quality or Governance finding may draft a fresh recovery Round and atomically return the Goal to todo; both stages share one bounded retry counter. Provider, parsing, Git, harness, and infrastructure failures do not consume that automatic recovery budget.

Preparation and non-retryable failures move an unchanged active Goal to failed. Retryable local failures use in-memory backoff and do not create durable delay records. Pause controls suppress new work and quiesce supported processes.

Bulk status correction protects automated states from generic replacement. Explicit cancellation is the lifecycle exception: it writes `cancelled` as Goal intent and then performs best-effort local cleanup per Goal.

The [Shared Workflow Consistency Contract](11-consistency-contract.md) and [Execution Ownership](../03-execution-ownership.md) define the authority and recovery rules.

## Future Direction

Workflow should support richer dependency reasoning, agent selection, multi-agent composition, evidence-aware review, and merge orchestration while preserving explicit Goal state, cheap restart, shared semantics, and inspectable evidence.
