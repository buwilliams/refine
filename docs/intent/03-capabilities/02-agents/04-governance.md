# Governance

## Key Ideas

- **Direction Over Permission**: governance should guide work, risk, and review without turning Refine into an authorization system.
- **Project Intent**: governance captures what the project cares about beyond a single Goal.
- **Agent Context**: agents should receive governance concerns before they act, not after they cause avoidable damage.
- **Review Pressure**: governance should shape which changes need evidence, escalation, or human judgment.
- **Durable And Editable**: governance should be stored as target-app context that people and agents can inspect.
- **Capability With Accountability**: governance should protect consequential boundaries without hobbling agents with preferences, hypothetical risks, or inherited trade-offs.

## Purpose

Governance exists because software work has values, risks, and boundaries that are not always captured by tests. A project may care about accessibility, data handling, migration safety, API compatibility, performance, user trust, or operational reliability.

Refine should make those concerns explicit. Governance helps agents choose better plans, reviewers ask better questions, and workflow decide where caution is needed.

## Expected Role

Governance should act as a durable layer of project judgment. It should inform import extraction, Goal creation, planning, implementation, quality, review, merge readiness, and future automation.

Governance should not become broad capability denial. Refine's design favors mitigation greater than prevention: powerful tools remain available, but governance should increase visibility, evidence, and review pressure where the project says risk matters.

Governance should judge actual changes against explicit project intent. It should not turn a familiar implementation pattern into a rule, report imagined violations, or require agents to accept a standard good-fast-cheap trade-off. An ambitious or unconventional solution is valid when it preserves the project's real boundaries and supplies evidence proportional to its risk.

Governance also applies to Refine's own agent design. Internal prompts remain reviewable Markdown templates under `src/prompts`; they should be concise enough to use current model capability overhang and should direct agents toward repository evidence, blind-spot discovery, prototypes, and focused user interviews. Safety-critical limits still belong in the relevant prompt and shared capability boundary, not in repetitive general instruction.

Current implementation details that matter to intent:

- governance lives near settings, guidance, reporters, and quality configuration;
- agents and workflow should be able to reuse governance context;
- governance concerns should be visible when they affect work;
- governance should preserve human-editable project intent rather than hiding policy inside code;
- governance verdicts should cite actual rule violations rather than preferences or hypothetical risks.

## Future Direction

Future governance should become more active and contextual. Agents may classify risk, map changes to governance concerns, ask for approvals, propose safer plans, or explain why a change is outside project norms.

The long-term direction is not a rule wall. It is an intent-aware judgment layer that helps autonomous software composition preserve the project's purpose while still allowing novelty and breakthroughs.
