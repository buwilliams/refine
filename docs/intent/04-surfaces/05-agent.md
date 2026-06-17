# Agent

## Key Ideas

- **First-Class Reader**: agents should be able to understand Refine from files, intent docs, state, and shared capabilities.
- **Installed Agents Over Platform APIs**: Refine should use local agent CLIs and host tools where possible instead of forcing a remote integration model.
- **Guided Autonomy**: agents should have enough context to act independently while leaving evidence and recovery paths.
- **Tools, Not Owners**: agents execute workflow steps and use tools; they should not redefine workflow semantics on their own.
- **Future Dominance**: over time, agent-native interaction may become the most important surface.

## Purpose

The agent surface exists because Refine is designed for AI-assisted and eventually AI-led software work. Agents need a way to read project intent, inspect work state, run shared capabilities, produce changes, and hand work back for quality, review, merge, or further automation.

Refine should not assume agents are only chatbots. They are workers, reviewers, planners, importers, diagnosers, and future orchestrators.

## Expected Role

Agents should use the same product concepts as people: Gap, Feature, workflow state, guidance, governance, logs, changes, and processes. Their outputs should become durable work evidence, not disappear into provider transcripts.

Current implementation details that matter to intent:

- provider configuration is treated as settings and diagnostics, not hardcoded behavior.
- workflow invokes agents as part of shared state advancement.
- chat sessions can attach to Gaps or run standalone.
- standalone sessions can work in Git worktrees and submit ready-merge Gaps.
- guidance, governance, quality, and target-app settings provide context for agent work.

Agents should be powerful enough to get real work done and observable enough that people and other agents can understand what happened.

## Future Direction

The future agent surface should support fleets of agents composing software at scale. That includes planning, decomposition, implementation, quality, review, merge, migration, monitoring, and repair.

As AI systems become stronger, they may compress Refine into workflow, persistence, and orchestration. That is acceptable if they preserve the intent: durable state, inspectable work, local ownership, shared capability, and enough guidance for future agents to understand why the system exists.
