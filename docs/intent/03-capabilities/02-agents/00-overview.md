# Agents

## Key Ideas

- **Agents Turn Intent Into Work**: agents read context, use tools, make changes, and leave evidence.
- **Guided Autonomy**: agents should have enough guidance, governance, and target-app context to act well without redefining product semantics.
- **Tools With Accountability**: agents should use powerful local tools through shared behavior and leave recoverable traces.
- **Evidence Before Trust**: quality checks, logs, diffs, review, and activity should explain why work is ready.
- **Reviewable Handoff**: agent work should move through review, merge, and Git worktree boundaries without losing context.
- **Shared Capability**: browser, CLI, API, desktop, and agent-native surfaces should call the same agent behavior.

## Purpose

Agents exist because Refine is designed for software work that is increasingly performed by AI systems. The agent capability is how Refine gives agents context, tools, operating boundaries, verification paths, and handoff semantics.

This capability folds together the concerns that make agent work useful instead of merely powerful:

- guidance gives agents product context and local operating instructions;
- governance shapes risk, judgment, and review pressure;
- tools let agents act on the target app, files, Git, commands, imports, diagnostics, and work items;
- quality checks produce evidence before confidence;
- activity and evidence preserve what happened and why;
- Git worktrees, review, and merge make changes inspectable and recoverable.

Agents should not own the meaning of workflow. Workflow decides how work advances. Process runs commands and long-lived execution. Agents use both to produce useful changes with durable evidence.

The child documents in this section describe the supporting capabilities agents need. They are grouped under Agents because their primary purpose is to help agents understand work, act on the target app, prove what happened, and hand changes back safely. Import belongs under Tools because it is a tool-backed way to turn unstructured source material into ordinary Refine work.

## Expected Role

The Agents capability should sit between Refine's intent and the outside world. It should help agents:

- read target-app context, guidance, governance, settings, and existing work;
- select tools appropriate to the work;
- create or refine Gaps and Features from imports, chats, plans, and source material;
- implement changes in an isolated Git branch or worktree when appropriate;
- run or request target-app commands, tests, diagnostics, and quality checks;
- attach logs, diffs, quality output, source context, and reasoning summaries to work;
- prepare changes for review and merge without bypassing workflow;
- recover from failures with enough evidence for another agent, node, or person to continue.

Current implementation details that matter to intent:

- provider configuration is settings and diagnostics, not hardcoded behavior;
- agents should prefer installed local CLIs and host tools where possible;
- chat and standalone sessions are agent behavior, not browser-only behavior;
- import extraction and draft review should use shared work item persistence;
- quality, governance, logs, activity, and System notices should be reusable agent evidence;
- worktrees isolate agent output and preserve merge handoff.

The Agents capability should remain powerful. Refine's safety posture is mitigation greater than prevention: use Git, logs, governance, quality checks, review, process visibility, and durable state to make powerful actions recoverable and accountable.

## Future Direction

Future agents should become the main actors in Refine. They may decompose Features, import plans, implement Gaps, run quality checks, review other agents, resolve conflicts, prepare merges, update guidance, and coordinate across nodes.

As agents improve, the capability should become less transcript-bound and more evidence-aware. Agents should produce structured plans, source links, tests, risk summaries, dependency graphs, review notes, merge summaries, and recovery proposals.

The long-term direction is fleets of agents composing software at scale. Refine should give those agents enough shared context, tools, evidence, and handoff semantics to work in parallel without losing the product's purpose.
