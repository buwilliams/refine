# Activity And Evidence

## Key Ideas

- **Durable Trail**: important actions should leave inspectable activity, logs, or evidence.
- **Operational Memory**: Refine should remember what happened well enough to support recovery and review.
- **Evidence Links Work To Reality**: logs, processes, diffs, checks, chat output, and user actions should connect back to Goals and Features.
- **Surface Shared**: browser System, Logs, CLI output, API responses, and agents should draw from shared activity semantics.
- **No Silent Failure**: failures, retries, blocked work, and risky actions should be visible.
- **Evidence Follows Authority**: activity explains committed decisions and execution facts without becoming a competing source of workflow truth.

## Purpose

Activity and evidence exist because agentic software work must be explainable after the fact. Without a durable trail, Refine would become a collection of chat messages and shell side effects that are hard to trust or recover.

Evidence turns work from assertion into something inspectable. A Goal should not merely say it is done; Refine should preserve what changed, what ran, what failed, who or what acted, and what can be reviewed.

## Expected Role

Activity and evidence should support every major capability:

- workflow state changes should explain why they happened;
- processes should expose status, output, owner, and exit information;
- quality checks should attach results to the work they verify;
- merge and review should preserve diffs and decisions;
- System should show immediate local notices;
- Logs should preserve deeper audit history;
- chat and standalone sessions should produce durable outputs when they affect work.

Activity should not be only a UI feed. It is product memory. Future agents should be able to inspect it to understand the current state, diagnose failures, and decide what should happen next.

Evidence is correlated by stable target-app, Goal, round, transition, claim, operation, process, and Git identities where they apply. Required evidence is durable before the workflow state it justifies; user notices and derived activity views may be projected afterward using the committed transition identity so retries do not duplicate them. The [Shared Workflow Consistency Contract](../03-workflow/11-consistency-contract.md) defines that ordering and makes missing or late evidence repairable instead of allowing a log entry to overwrite authority.

## Future Direction

Future evidence should become richer and more structured: screenshots, traces, dependency graphs, provenance, risk classifications, test artifacts, review summaries, and agent reasoning summaries.

As Refine moves toward fleets of agents, activity and evidence will become the audit layer that lets autonomous work remain understandable. The system should make it easy to ask: what happened, why did it happen, what changed, what proof exists, and what should happen next?
