# Chat

## Key Ideas

- **Contextual Conversation**: chat should attach to work when work exists.
- **Durable Sessions**: messages, output, interruptions, and recovery should not depend on a single browser moment.
- **Gap-Aware Priming**: Gap chat should provide enough context for agents to help with that work.
- **Toolbar Native**: chat belongs in the persistent dock so it can support any main surface.
- **Transcript To Work**: useful chat should be able to become structured Refine work where appropriate.
- **Planning As Exploration**: Plan Mode should help users find the shape of an idea before work is created.

## Purpose

The Chat surface exists to let users collaborate with agents while staying inside Refine's product model. Chat can help clarify work, draft rounds, inspect failures, discuss implementation, and convert vague intent into concrete Gaps.

Chat should not replace durable work state. It should support it.

## Expected Role

Chat should support standalone sessions, Plan Mode, and Gap-attached sessions. Gap chat should know which Gap it is helping with and should preserve the user's place in the broader UI. Plan Mode should help users explore purpose, audience, constraints, success criteria, major behavior, and relevant architecture concerns without forcing a fixed template.

Current implementation details that matter to intent:

- toolbar state holds one permanent standalone tab plus one tab per opened Gap chat;
- Plan Mode uses the toolbar chat surface and can later be drafted into a Feature;
- chat sessions have server-side identifiers and output queues;
- Gap chat eligibility depends on shared Gap status semantics;
- session recovery should handle daemon restarts and interrupted turns;
- standalone chat has its own worktree and handoff behavior, documented separately.

Chat should stay connected to Refine's durable model: Gaps, rounds, logs, guidance, governance, and workflow state.

## Future Direction

Future chat should become more structured and less transcript-bound. Agents may summarize, propose actions, draft Gaps, attach evidence, and route conversation into workflow automatically.

As AI improves, chat may become one of several agent interaction modes rather than the center of the system.
