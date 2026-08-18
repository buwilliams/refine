# Agents

## Key Ideas

- **Agents Turn Intent Into Work**: agents read context, use tools, make changes, and leave evidence.
- **Guided Autonomy**: agents should have enough guidance, governance, and target-app context to act well without redefining product semantics.
- **Operations With Accountability**: agents should use powerful Application behavior and Infrastructure mechanisms while leaving recoverable traces.
- **Evidence Before Trust**: quality checks, logs, diffs, review, and activity should explain why work is ready.
- **Reviewable Handoff**: agent work should move through review, merge, and Git worktree boundaries without losing context.
- **Shared Application**: browser, CLI, API, MCP, and agent-native Surfaces should call the same agent behavior.
- **Unhobbled By Default**: prompts should be concise enough to leave room for the spiky intelligence and capability overhang of current models.

## Purpose

Agents exist because Refine is designed for software work that is increasingly performed by AI systems. The Agents area of Application gives agents context, operations, authority boundaries, verification paths, and handoff semantics while Infrastructure handles provider and process mechanics.

This Application area folds together the concerns that make agent work useful instead of merely powerful:

- guidance gives agents product context and local operating instructions;
- governance shapes risk, judgment, and review pressure;
- operations let agents act on the target app, files, Git, commands, imports, diagnostics, and work items without creating a generic Tools layer;
- quality checks produce evidence before confidence;
- planning turns uncertain ideas into reviewable product and implementation intent;
- activity and evidence preserve what happened and why;
- Git worktrees, review, and merge make changes inspectable and recoverable.

Agents should not own the meaning of workflow. Workflow decides how work advances. Process runs commands and long-lived execution. Agents use both to produce useful changes with durable evidence.

The child documents in this section describe the Application behavior agents need. They are grouped under Agents because their primary purpose is to help agents understand work, act on the target app, prove what happened, and hand changes back safely. Planning belongs here because it helps users and agents shape intent before work exists. Import belongs under Agent Operations because it turns unstructured source material into ordinary Refine work through shared Application behavior.

## Expected Role

The Agents Application should sit between Refine's intent and the outside world. It should help agents:

- read target-app context, guidance, governance, settings, and existing work;
- explore ideas and shape selected plans into reviewable Features and Goals;
- select tools appropriate to the work;
- create or refine Goals and Features from imports, chats, plans, and source material;
- implement changes in an isolated Git branch or worktree when appropriate;
- run or request target-app lifecycle work, tests, diagnostics, and quality checks;
- attach logs, diffs, quality output, source context, and reasoning summaries to work;
- prepare changes for review and merge without bypassing workflow;
- recover from failures with enough evidence for another agent, node, or person to continue.

Current implementation details that matter to intent:

- provider configuration is settings and diagnostics, not hardcoded behavior;
- one provider-aware host capability prepares every interactive and
  noninteractive agent launch. Codex can preserve its native stdin contract;
  Claude, Gemini, Copilot, smoke-ai, and configured generic CLIs use bounded
  inline prompts or the shared secure prompt-file bootstrap. The decision uses
  Process's final effective environment, and the prepared launch carries that
  same environment to the managed-process or PTY boundary. Surfaces do not
  choose thresholds, merge environments, or create prompt files.
- prompt-file payloads live under the selected runtime root, outside the source
  worktree, with private ownership, exact UTF-8 byte and SHA-256 evidence, and
  explicit failure when the handoff is missing, changed, unreadable, or cannot
  fit the provider sandbox.
- every stateful provider launch receives the owning checkout's exact
  `run/<port>` root from daemon, workflow, CLI, or durable operation context.
  Runtime-free detect, configure, authentication, and diagnostics remain
  stateless; invoke, resume, interactive, and managed launches fail closed
  instead of inventing HOME, XDG, platform support, or CWD state.
- every internal agent prompt is a Markdown template under `src/application/agent_io/prompts/<feature>/`, loaded through the shared prompt engine rather than embedded in consumer code;
- agents should prefer installed local CLIs and host tools where possible;
- chat and standalone sessions are agent behavior, not browser-only behavior;
- import extraction and draft review should use shared work item persistence;
- quality, governance, logs, activity, and System notices should be reusable agent evidence;
- worktrees isolate agent output and preserve merge handoff.
- a workflow-owned Goal Agent runs in one native CLI terminal per active Goal;
  supported surfaces attach to that process instead of launching another agent.

The Agents Application should remain powerful. Refine's safety posture is mitigation greater than prevention: use Git, logs, Governance, Quality, review, process visibility, and durable state to make powerful actions recoverable and accountable.

An agent should treat a specification or prompt as a map and the repository, runtime, user need, and evidence as the territory. It should inspect before assuming, follow blind-spot paths, and prototype when a small experiment resolves uncertainty faster than more prose. When a consequential unknown belongs to the user, the agent should interview the user with focused questions. When the evidence is available, it should resolve the unknown itself.

This posture is intentionally ambitious. Refine should not teach agents to accept the usual good-fast-cheap trade-off before they try. Current agents can search, implement, and verify quickly enough to pursue all three, while governance and review still require evidence for the result.

## Future Direction

Future agents should become the main actors in Refine. They may decompose Features, import plans, implement Goals, run quality checks, review other agents, resolve conflicts, prepare merges, update guidance, and coordinate across nodes.

As agents improve, the Application should become less transcript-bound and more evidence-aware. Agents should produce structured plans, source links, tests, risk summaries, dependency graphs, review notes, merge summaries, and recovery proposals.

The long-term direction is fleets of agents composing software at scale. Refine should give those agents enough shared context, tools, evidence, and handoff semantics to work in parallel without losing the product's purpose.
