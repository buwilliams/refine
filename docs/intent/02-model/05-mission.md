# Mission

## Key Ideas

- **System-Level Outcome**: a Mission owns one governed outcome that is larger than any single Goal, expressed as intent, success criteria, and durable artifacts.
- **Composition, Not Inheritance**: a Mission is not a bigger Goal record. It composes ordinary Goal workflows and stays a separate concept from Feature grouping.
- **Frozen Context In, Evidence Out**: Goals receive a pinned, scoped context capsule and return typed findings and evidence. Parallel Goals never mutate shared Mission state.
- **Reconciliation Is the Only Reducer**: one fenced behavior turns parallel Goal evidence into the next immutable Mission snapshot. Nothing else promotes accepted knowledge.
- **Derived Knowledge, Stored History**: what the Mission knows lives in immutable snapshots. Supersession, invalidation, and contested state are computed from that history like any other projection, never stored as mutable graphs.
- **Honest Verification**: deterministic checks prove provenance and machine-checkable claims only. Judgment belongs to criticized agent reasoning, and design authority stays behind human gates.
- **Append-Only Recovery**: a wrong-but-valid reduction is repaired by appending a correction snapshot, never by editing history.

## Purpose

Large outcomes fail between Goals, not inside them. Individual Goals can each succeed while the combined result misses the intent, contradicts itself, or loses what earlier work learned. Mission exists to preserve system-level intent across many local Goals, adapt later work when earlier work reveals new facts, and judge the combined result rather than equating child completion with outcome success.

Mission also keeps learned context durable. Research, models, decisions, risks, and contradictions become evidence-backed records that later Goals and later Missions can build on, instead of evaporating with each agent conversation.

## Expected Role

Mission sits above Goal and Feature as the third work concept: Goal executes, Feature groups and orders, Mission governs a system-level outcome. A Goal may belong to one Mission and one Feature at once, and standalone Goals remain fully valid.

The central loop is investigation, planning in waves, Goal execution against pinned context capsules, reconciliation at wave boundaries, synthesis, and an immutable published Outcome that later Missions may consume exactly.

The division of responsibility follows Refine's agent-first posture. Refine owns durable identity, provenance, verification of machine-checkable claims, authority gates, fencing, budgets, and the computed invalidation closure. Agents draft reductions, criticize them, and propose resolutions; their text can never mint authority or workflow transitions. People approve plans, material amendments, Decisions, and Directives, and resolve open contradictions.

Current implementation details that matter to intent:

- Mission snapshots are immutable, digest-addressed manifests; accepted knowledge is addressed by stable assertion ids inside them.
- Reconciliation preserves dissent and contested members verbatim rather than averaging them away; contested load-bearing knowledge blocks dependent admission instead of resolving silently.
- A GoalRound is affected by an invalidation only if its pinned capsule actually included the invalidated assertion.
- Wave boundaries should produce near-zero human interrupts; recurring decision volume is a plan-quality signal, not a world problem.
- Late evidence is never lost: it carries to the next boundary, and a mandatory sweep precedes synthesis.

Future work should preserve these properties while deeper execution phases (agent-invoked investigation, synthesis, fleet distribution) are built out.

## Future Direction

As agents improve, Missions should scale toward software composition at scale: many waves, cross-Mission reuse of exact published Outcomes, and richer knowledge lineages. The scaffolding should stay honest about what models can take over — judgment, reduction, and criticism are agent work — while Refine keeps the durable substrate: identity, evidence, authority, invalidation, and recovery.

The direction is a system where a Mission's full reasoning history is inspectable and repairable by both people and stronger future agents, and where the boundary between deterministic guarantees and judged knowledge remains explicit rather than implicit in prompts.
