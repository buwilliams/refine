# Guidance

## Key Ideas

- **Context For Agents**: agents need durable guidance in addition to concise, turn-specific prompts.
- **Human Editable**: guidance should be understandable and maintainable by the people responsible for the project.
- **Governance As Direction**: rules should shape work and review without reducing Refine to a permission system.
- **Project Specific**: guidance should live near the target app so it reflects the actual product.
- **Future Readability**: stronger future agents should be able to infer intent from guidance without asking the UI.

## Purpose

Guidance exists because AI agents do better work when they have explicit product context, quality expectations, governance concerns, and local operating instructions. Refine should not rely on every agent turn rediscovering the same constraints.

Guidance turns product intent into durable material that agents can use. It should help answer: what kind of software is this, what matters, what should not be broken, what standards govern work, and how should uncertain tradeoffs be handled?

Guidance is a capability because Refine uses it to shape action. It should affect planning, implementation, quality, review, import, generated target-app configuration, and future agent-native workflows.

## Expected Role

Guidance should sit between raw user instruction and automated action. It should be reusable by workflow, tools, quality, governance, import, surfaces, and agents.

The current implementation has explicit services and surfaces for settings, guidance, governance, reporters, quality settings, target-app lifecycle instructions, deterministic checks, and generated project rules. Internal agent prompts are Markdown templates grouped under `src/application/agent_io/prompts/<feature>/` and use one shared prompt engine. Those are implementation details, but they protect an important separation: prompt templates define the small reusable task frame, while guidance supplies durable target-app context that workflow and tools can reuse.

Guidance should not become a wall of policy text that agents ignore or a reason to expand every system prompt. It should be specific, current, structured enough to retrieve, and close enough to the work that it changes outcomes. Prompt changes should prefer less prescription when repository discovery, a prototype, or a focused user interview can close the gap more reliably.

For Goal workflow phases, Refine snapshots every enabled guidance candidate into the current Round before launching the Goal Agent. The agent-facing specification gives each pinned candidate a zero-based Completion Index and omits its stable configuration id. The same agent decides which candidates apply and reports only those integer indexes with its completion signal; display names and stable configuration ids are not signal identifiers. Refine validates and records the applied and skipped candidates without launching a separate guidance-classification turn.

Guidance persistence assigns every entry a stable id and keeps a collection revision. Stable ids belong to configuration mutation, display names belong to human-facing recognition, and positional Completion Indexes belong only to one pinned Goal Agent completion contract. Legacy array files are normalized and migrated in place under the existing record coordination boundary. Browser and CLI item mutations identify one entry and submit the revision they observed; compatibility whole-list replacement is revision-fenced too. A stale writer receives a conflict and must refresh, so adding, editing, enabling, disabling, or removing one entry cannot silently erase an unrelated concurrent change.

## Future Direction

As agents improve, guidance should become more active. It should help agents select strategies, evaluate tradeoffs, choose tools, explain risks, and coordinate with other agents.

The long-term direction is an intent layer for software composition. Refine should preserve guidance as durable context that can be read by humans, used by present-day agents, and compressed by future AI systems into higher-level plans without losing the product's purpose.
