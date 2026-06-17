# Gap

## Key Ideas

- **Atomic Work Unit**: a Gap is the basic unit of meaningful software change.
- **Actual To Target**: every Gap should preserve what exists now and what should exist next.
- **Round-Based Work**: repeated attempts, recovery, and follow-up instructions should be durable.
- **Modal Detail**: detail should preserve the user's surrounding context.
- **Workflow Actions**: available actions should follow shared Gap status rules.

## Purpose

The Gap surface exists to create, inspect, update, discuss, implement, retry, review, and verify individual work items.

It should make the work understandable to both humans and agents: what is wrong or missing, what should be true, what has been tried, what happened, and what can happen next.

## Expected Role

The Gap UI should expose identity, status, priority, reporter, assignee, Feature membership, node ownership, notes, rounds, logs, governance, quality, chat, and workflow actions.

Current implementation details that matter to intent:

- Gaps list uses URL-backed filters for status, reporter, assignee, Feature, rounds, node, severity, category, actor, sort, and page;
- Gap details open as a modal over the current page;
- failed Feature-blocking Gaps should explain what they block and log user-visible notices to System;
- new rounds can be submitted for failed or review states where shared rules allow it;
- bulk operations should use shared work item behavior and preserve node/Feature constraints.

The Gap surface should keep agent work concrete. A Gap that cannot be explained as actual-to-target behavior is too vague for reliable automation.

## Future Direction

Future Gaps should carry richer evidence: screenshots, test output, design rationale, dependency traces, risk assessments, and agent reasoning summaries.

As AI systems improve, the Gap surface may become less about manual editing and more about approving, redirecting, and auditing autonomous work.
