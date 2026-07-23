# Workflow

## Key Ideas

- **Always-On Automation**: workflow is state movement, not a user-facing scheduler.
- **Goal Lifecycle**: work advances through explicit states from backlog to done or cancelled.
- **Agents As Tools**: agents participate in workflow steps; they do not own the meaning of workflow.
- **Shared Semantics**: CLI, browser, API, and agent surfaces should use the same workflow rules.
- **Shared Consistency**: cross-process mutations use one coordination boundary so state, execution, and evidence cannot contradict one another.
- **Recoverable Progress**: claims, executions, failures, retries, and pauses should be visible and resumable.

## Purpose

Workflow exists to move software work forward without turning each Goal into an ad hoc chat session. It gives Refine the ability to promote, claim, implement, quality-check, integrate, optionally rebuild, review, retry, pause, resume, and recover work.

The point is not scheduling for its own sake. The point is durable state advancement. Refine should know what can happen next, why it can happen, and which actor is responsible for doing it.

## Expected Role

The workflow capability should be the primary engine of Refine's agentic behavior. It coordinates work across model state, process execution, Git worktrees, quality checks, merge behavior, provider invocation, node ownership, and user review.

The workflow lifecycle is:

- backlog: captured work waits until it is ready for action;
- todo: actionable work becomes eligible for claiming;
- in-progress: a node owns the active attempt and agents or processes act;
- qa: checks gather evidence against the isolated candidate before integration or the integrated target after rebuild, according to the round's pinned Quality timing;
- ready-merge: the merger-owned queue serializes exact candidate integration into the recorded target branch;
- build: the integrated target app is rebuilt when configured, or records an explicit skip;
- review: evidence and judgment accept or decline the already-integrated result;
- done: the intended outcome is complete and evidence remains inspectable;
- failed: the attempt did not complete, but evidence supports recovery;
- cancelled: the work is intentionally stopped.

Current implementation details that matter to intent:

- `WorkflowEngine` owns workflow-state advancement.
- Workflow policy tracks limits by global, node, provider, and target app scope.
- Claims record which Goal is being worked, by which provider and node, for which target app.
- Pause controls can stop agents, target-app work, or all automation.
- Goal state rules distinguish manual transitions from automated transitions.
- Feature ordering is respected so ordered Goals advance without losing Feature intent.
- While agents are running, the engine periodically discovers newly eligible queued Goals and
  uses available capacity without waiting for the current agents to finish. Each replenishment
  applies the same priority and Feature-order eligibility rules as the initial claim pass.
- Review is a meaningful boundary: a Goal in review can unblock later ordered Feature work.

Workflow should not be reimplemented in page-local JavaScript, one-off CLI commands, or provider-specific scripts. Those surfaces should call the shared capability.

The [Shared Workflow Consistency Contract](11-consistency-contract.md) defines the authority, identity, evidence, conflict, durability, recovery, and thin-surface invariants shared by every workflow state.

## Future Direction

Future workflow should support fleets of agents composing software at scale. That requires richer dependency reasoning, better claim negotiation, stronger retry semantics, multi-agent coordination, evidence-aware review, and merge orchestration.

The long-term design can be compressed to workflow plus persistence plus orchestration. If future AI systems discover better internal machinery, they should still preserve explicit work state, recoverable progress, shared semantics, and inspectable evidence.
