# Planning

## Key Ideas

- **Purpose Before Shape**: planning should start with what the user is trying to make and why it matters.
- **Architecture As Lenses**: persistence, logic, surfaces, and integrations are ways to think, not required sections.
- **Natural Work Order**: drafted work should follow the order implied by the plan, dependencies, and domain.
- **Reviewable Decomposition**: plans should become Features and Goals only after users can inspect and adjust them.
- **Loose Fundamentals**: good architecture gives agents a starting point without becoming a rule system.
- **Map Meets Territory**: the initial request guides discovery, but the actual product, code, runtime, and user needs decide what the plan must address.

## Purpose

Planning exists because useful software work often starts as an uncertain idea. Refine should help users explore that idea, select a direction, and describe the product, feature, or app clearly enough that agents can later implement it.

Plan Mode should capture the whole picture of the work: purpose, audience, success criteria, constraints, major behavior, important surfaces, and the implementation concerns that are actually relevant. It should use architecture fundamentals with an open hand. The goal is clarity about the system, not compliance with a fixed template.

## Expected Role

Planning should sit before durable work creation. It should help users and agents understand:

- what is being made and why;
- who or what will use it;
- what needs to persist, if anything;
- what logic or organization makes the system understandable;
- which human, agent, CLI, API, browser, or other surfaces matter;
- which integrations, runtime processes, events, or recovery paths matter;
- what tradeoffs should be preserved for future implementers.

These are lenses, not mandatory headings. If a concern is irrelevant, temporary, collapsed into another concern, or premature, a good plan may omit it or mention the tradeoff briefly.

Planning should actively find important unknowns instead of polishing assumptions. An agent may follow blind-spot paths through adjacent code and behavior, build a small prototype to test a risky idea, or interview the user when product intent cannot be inferred from available evidence. These strategies should narrow consequential uncertainty without turning every plan into a long questionnaire or research exercise.

Draft Feature should convert the selected plan into ordinary Refine work. It should produce one Feature plus implementation-ready Goals in the plan's natural build order. When dependency order is clear, the drafts should reflect it. When the work is exploratory, visual, research-heavy, or prototype-oriented, the drafts should be the smallest useful implementation slices rather than forced architecture categories.

Draft Goal should offer the narrower conversion boundary for a user who wants help shaping one independently actionable Goal rather than decomposing the plan into a Feature. It should extract exactly one reviewable Goal and must not create or imply a Feature.

Current implementation details that matter to intent:

- Plan Mode is a chat mode that drafts product and implementation intent;
- Draft Feature extracts a Feature and Goals from a Plan transcript through shared import extraction;
- Draft Goal extracts exactly one standalone Goal through the same shared import-extraction capability;
- browser, CLI, HTTP API, and MCP adapters all expose Plan-to-Goal drafting without persisting before review;
- Plan and spec-like extraction should use architecture-aware drafting;
- simple CSV, issue-list, and direct import flows should remain direct and not become planning exercises;
- review-before-persist should remain the boundary before creating durable work.

## Future Direction

Future planning should become less transcript-bound and more structured. Agents may propose questions, alternatives, prototypes, source evidence, dependency graphs, and implementation slices. They should use improving model capabilities to widen the ambition of feasible work, including outcomes previously dismissed as too slow or expensive. They should still preserve the core posture: help the user think clearly, then turn selected intent into reviewable work without narrowing the user's design space.
