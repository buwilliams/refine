# Goal

## Key Ideas

- **Atomic Work Unit**: a Goal is the basic unit of meaningful software change.
- **Prompt-Driven**: every Goal should preserve a direct, actionable instruction for the agent.
- **Round-Based Work**: repeated attempts, recovery, and follow-up instructions should be durable.
- **Modal Detail**: detail should preserve the user's surrounding context.
- **Human Control**: explicit workflow actions may override automated transition rules.

## Purpose

The Goal surface exists to create, inspect, update, discuss, implement, retry, review, and verify individual work items.

It should make the work understandable to both humans and agents: what should be accomplished, what has been tried, what happened, and what can happen next.

## Expected Role

Each Round shows one read-only Implementation Plan card sourced from the
authoritative Goal detail. The active phase and final checklist are primary;
the original proposal and independent criticism remain expandable history.
Phase timestamps, failures, verification, and per-item implementation outcomes
make discrepancies inspectable without turning the browser into a planning
state machine. Expanded Round and planning-history sections remain open across
SSE-driven redraws.

The browser never orchestrates or polls implementation planning. Existing Goal
SSE and reconnect reconciliation deliver the same Round object observed by CLI,
API, MCP, and exports, and Open Agent attaches to the active workflow phase.
Between phase-process registrations it reports that Workflow is between phases
and never launches a diagnostic substitute for a Goal active in Plan or Implement.

The Goal UI should expose identity, status, priority, reporter, assignee, Feature membership, node ownership, notes, rounds, implementation reports, logs, governance, quality, chat, and workflow actions.

Current implementation details that matter to intent:

- Goals list uses URL-backed filters for status, reporter, assignee, Feature, rounds, node, severity, category, actor, sort, and page;
- Goal details open as a modal over the current page;
- a Goal owned by another node should offer **Transfer to my node** in the modal
  action menu when the browser has an authoritative active-node context. The
  action uses the shared single-item transfer capability; server-side workflow
  and Feature-membership constraints remain authoritative, and rejected
  transfers are surfaced without changing ownership;
- failed Feature-blocking Goals should explain what they block and log user-visible notices to System;
- new rounds can be submitted for failed or review states where shared rules allow it;
- bulk operations should use shared work item behavior and preserve node/Feature constraints;
- confirmed bulk actions should immediately acknowledge that Refine is working on them asynchronously, before the authoritative outcome is available;
- the primary workflow action uses the same split-button layout as Open Agent: a Todo button and a separate arrow menu, with every workflow step available from every Goal state. Choosing a step invokes the shared human override, stops active Goal agents, and selects that exact step on the existing Round;
- every Round has a subdued gray, accessible trash-can button. Confirmation describes full record deletion and stopping current work. After deletion the Goal is in Backlog; the user can select Todo to resubmit the remaining Round;
- the bulk status picker includes Backlog, Todo, Plan, Implement, Quality, Governance, Review, Done, Failed, and Cancelled. Explicit assignment applies to active and terminal Goals and reports per-Goal failures. Automation continues to follow normal rules;
- each implemented round should retain a timestamped, plain-language report of what changed, why, and the deterministic verification outcomes; the report should be visible with that round when the Goal opens.
- Goals bulk actions should export all or a selected subset as Jira-importable SOC 2 evidence containing each request, implementation reports, review outcomes, notes, and exact commit range without requiring users to reconstruct delivery history manually. Each row should fit Jira by preserving Goal identity and commit traceability first, compacting repeated machine verdict payloads, and visibly marking any lower-priority evidence shortened at a valid character boundary. One Goal's verbose history should not abort the other selected rows. The export should run as a visible, cancellable operation that survives page reloads and can recover after daemon interruption.

The Goal surface should keep agent work concrete. A Goal without an actionable prompt is too vague for reliable automation.

Workflow controls expose explicit step changes with reason, context, expected revision, and request identity. The Goal shows pending error handling and retained decision history. Forced Done is clearly status-only; forced integration is a separate action against the retained candidate. Node switches and late responses preserve the selected-node boundary.

## Future Direction

Future Goals should carry richer evidence: screenshots, test output, design rationale, dependency traces, risk assessments, and agent reasoning summaries.

As AI systems improve, the Goal surface may become less about manual editing and more about approving, redirecting, and auditing autonomous work.

The Goal status tag is the sole top-level failure summary. Do not repeat it in generic Goal failure banners. Each Round owns its status and history section, containing its failure explanation, quality and governance results, workflow decisions, and logs. These details expand within that Round instead of competing with the Goal title, prompt, and primary actions.

Workflow visualization cards subtly highlight the selected status filter. Multiple selected statuses each receive the same highlight; an all-status selection is visually neutral. Selection must survive filtered-table refreshes and route reloads.

Round plan display reads both recorded Plan Skill results and legacy implementation-plan records. Moving to a Skill-driven workflow must not hide an already recorded plan.
