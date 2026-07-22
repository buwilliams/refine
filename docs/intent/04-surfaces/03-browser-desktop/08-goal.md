# Goal

## Key Ideas

- **Atomic Work Unit**: a Goal is the basic unit of meaningful software change.
- **Prompt-Driven**: every Goal should preserve a direct, actionable instruction for the agent.
- **Round-Based Work**: repeated attempts, recovery, and follow-up instructions should be durable.
- **Modal Detail**: detail should preserve the user's surrounding context.
- **Workflow Actions**: available actions should follow shared Goal status rules.

## Purpose

The Goal surface exists to create, inspect, update, discuss, implement, retry, review, and verify individual work items.

It should make the work understandable to both humans and agents: what should be accomplished, what has been tried, what happened, and what can happen next.

## Expected Role

The Goal UI should expose identity, status, priority, reporter, assignee, Feature membership, node ownership, notes, rounds, implementation reports, logs, governance, quality, chat, and workflow actions.

Current implementation details that matter to intent:

- Goals list uses URL-backed filters for status, reporter, assignee, Feature, rounds, node, severity, category, actor, sort, and page;
- Goal details open as a modal over the current page;
- failed Feature-blocking Goals should explain what they block and log user-visible notices to System;
- new rounds can be submitted for failed or review states where shared rules allow it;
- bulk operations should use shared work item behavior and preserve node/Feature constraints.
- each implemented round should retain a timestamped, plain-language report of what changed, why, and the deterministic verification outcomes; the report should be visible with that round when the Goal opens.
- Goals bulk actions should export all or a selected subset as Jira-importable SOC 2 evidence containing each request, implementation reports, review outcomes, notes, and exact commit range without requiring users to reconstruct delivery history manually. The export should run as a visible, cancellable operation that survives page reloads and can recover after daemon interruption.

The Goal surface should keep agent work concrete. A Goal without an actionable prompt is too vague for reliable automation.

## Future Direction

Future Goals should carry richer evidence: screenshots, test output, design rationale, dependency traces, risk assessments, and agent reasoning summaries.

As AI systems improve, the Goal surface may become less about manual editing and more about approving, redirecting, and auditing autonomous work.
