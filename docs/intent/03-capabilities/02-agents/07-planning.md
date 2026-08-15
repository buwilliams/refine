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

Plan Mode is distinct from the Goal workflow's Plan state. Plan Mode explores intent before a Goal exists or before a user chooses to persist work. Governed implementation planning occurs after one exact Goal Round has entered Plan: Workflow asks fresh installed native agents to propose, criticize, and revise an execution plan against pinned product, constitution, rules, guidance, past Rounds, current Round, and repository context before a fresh implementation agent may change it. That plan guides one attempt; it does not redefine the Goal, approve code, or replace later Implement, Quality, Governance, and Review judgments.

Governed implementation plans are succinct execution guides, not exhaustive design documents. They begin with one plain-language paragraph that explains, top down, what will change and why. Any number of stable checklist items may follow when the work needs them, but each item is one concise line whose connection to the plan is obvious. Items capture only behavior, decisions, risks, or failure boundaries that cannot safely be ignored; detailed rationale, file inventories, exact commands, routine workflow mechanics, and verification transcripts belong in the pinned request, repository, or later execution evidence. Planning completion signals carry the typed result in a top-level `planning_result` JSON object rather than embedding or stringifying it in a human message.

Proposal, criticism, and revision use one bounded structured-output transport and repair policy. The transport accepts one direct, fenced, mixed-text, completion-wrapped, or recursively stringified JSON value within fixed payload, nesting, and unwrapping limits; multiple distinct candidates remain invalid. The selected value is deserialized into the canonical typed phase artifact with path-aware schema diagnostics. Revision resolutions canonically use `criticism_id` and `resolution`; the transport also normalizes the documented equivalent identifier spellings `criticismId`, `finding_id`, and resolution-local `id`, while ambiguous, missing, duplicate, unknown, incomplete, or structurally different resolutions remain invalid. Each unreadable or invalid raw response and its exact parse or validation diagnostic is persisted before a follow-up invocation receives that diagnostic. Repair resumes the same phase from the last valid durable artifact; exhaustion is an output-contract failure distinct from a provider failure. Summary and item bounds remain defensive transport limits rather than a narrow 600-character product rule: current one-line planning summaries may contain up to 20,000 characters.

Planning should actively find important unknowns instead of polishing assumptions. An agent may follow blind-spot paths through adjacent code and behavior, build a small prototype to test a risky idea, or interview the user when product intent cannot be inferred from available evidence. These strategies should narrow consequential uncertainty without turning every plan into a long questionnaire or research exercise.

Draft Feature should convert the selected plan into ordinary Refine work. It should produce one Feature plus implementation-ready Goals in the plan's natural build order. When dependency order is clear, the drafts should reflect it. When the work is exploratory, visual, research-heavy, or prototype-oriented, the drafts should be the smallest useful implementation slices rather than forced architecture categories.

Draft Goal should offer the narrower conversion boundary for a user who wants help shaping one independently actionable Goal rather than decomposing the plan into a Feature. It should extract exactly one reviewable Goal and must not create or imply a Feature.

Current implementation details that matter to intent:

- browser Plan Mode opens a new managed terminal instance of the configured native agent harness for each request, with planning context and an optional starting prompt;
- the planning agent uses the ordinary Refine CLI or API to create a Feature and naturally ordered Goals when the user chooses to persist the plan;
- Refine does not parse the native harness transcript or maintain browser-only Draft Goal and Draft Feature controls;
- CLI, HTTP API, MCP, and import surfaces may still expose structured drafting and review-before-persist where that interaction model is useful;
- Plan and spec-like extraction should use architecture-aware drafting;
- simple CSV, issue-list, and direct import flows should remain direct and not become planning exercises;
- review-before-persist should remain the boundary before creating durable work.
- every workflow-owned Goal Round uses a separate execution-time proposal, independent criticism, and revision pipeline; its typed evidence belongs to the Round and is never Plan Mode draft state.

## Future Direction

Future planning should improve the context and workflow tools available to native agent harnesses rather than recreating their interaction UX. Agents may propose questions, alternatives, prototypes, source evidence, dependency graphs, and implementation slices. They should use improving model capabilities to widen the ambition of feasible work, including outcomes previously dismissed as too slow or expensive. They should still preserve the core posture: help the user think clearly, then turn selected intent into reviewable work without narrowing the user's design space.
