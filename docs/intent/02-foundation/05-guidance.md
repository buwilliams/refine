# Guidance

## Key Ideas

- **Context For Agents**: agents need durable guidance, not only prompts embedded in code or UI.
- **Human Editable**: guidance should be understandable and maintainable by the people responsible for the project.
- **Governance As Direction**: rules should shape work and review without reducing Refine to a permission system.
- **Project Specific**: guidance should live near the target app so it reflects the actual product.
- **Future Readability**: stronger future agents should be able to infer intent from guidance without asking the UI.

## Purpose

Guidance exists because AI agents do better work when they have explicit product context, quality expectations, governance concerns, and local operating instructions. Refine should not rely on every agent turn rediscovering the same constraints.

Guidance turns product intent into durable material that agents can use. It should help answer: what kind of software is this, what matters, what should not be broken, what standards govern work, and how should uncertain tradeoffs be handled?

## Expected Role

Guidance should sit between raw user instruction and automated action. It should inform planning, implementation, QA, review, import extraction, generated target-app configuration, and future agent-native workflows.

The current implementation has explicit services and surfaces for settings, guidance, governance, reporters, quality settings, target-app commands, and generated project rules. Those are implementation details, but they show the intended role: guidance is first-class target-app context that workflow and tools can reuse.

Guidance should not become a wall of policy text that agents ignore. It should be specific, current, structured enough to retrieve, and close enough to the work that it changes outcomes.

## Future Direction

As agents improve, guidance should become more active. It should help agents select strategies, evaluate tradeoffs, choose tools, explain risks, and coordinate with other agents.

The long-term direction is an intent layer for software composition. Refine should preserve guidance as durable context that can be read by humans, used by present-day agents, and compressed by future AI systems into higher-level plans without losing the product's purpose.
