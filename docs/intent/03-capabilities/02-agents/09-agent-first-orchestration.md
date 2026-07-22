# Agent-First Orchestration

## Key Ideas

- **Agents Do Adaptable Work**: use native agents for reasoning, implementation, testing, diagnosis, and recovery instead of rebuilding those abilities as brittle product logic.
- **Refine Owns Coordination**: Goals, Features, workflow, Nodes, Git synchronization and isolation, process supervision, durable evidence, quality, governance, and recovery are Refine's orchestration substrate.
- **Shared Surfaces**: UI, CLI, API, MCP, and agent conversations expose the same capabilities; no surface should become a separate product.
- **Agent-First Is Not Agent-Only**: first-class interfaces remain valuable when they improve access, understanding, control, speed, or safety for people and agents.
- **Deterministic Integrity**: agent judgment complements, but does not replace, deterministic state transitions, persistence, identity, protocol, Git, and process-lifecycle guarantees.

## Purpose

Native agents can inspect repositories, reason about intent, edit code, choose and run tests, diagnose failures, and operate tools. Refine should not encode a second, narrower version of those abilities merely to make adaptable work look deterministic. It should give agents the context, tools, boundaries, and evidence needed to use their improving capabilities directly.

Refine exists because capable agents alone do not provide a coherent software-delivery system. Refine turns their work into understandable Goals and Features, advances it through durable workflow, coordinates it across Nodes, isolates and synchronizes it with Git, supervises processes, and preserves evidence for quality, governance, review, and recovery. Its value is the operating layer around agent work, presented consistently through replaceable surfaces.

## Expected Role

When adding behavior, first ask whether an installed agent can perform it reliably from existing context and tools. Adaptable work such as planning details, selecting verification methods, interpreting failures, proposing recovery, and summarizing review should normally remain agent work. Refine should expose the necessary context and shared capability instead of adding a specialized wizard, rule engine, or surface-only control for each case.

First-class Refine behavior is justified when it supplies durable product value that individual agent turns cannot provide consistently:

- shared state, workflow semantics, ownership, ordering, or Node coordination;
- Git isolation, synchronization, convergence, or recoverability;
- supervised execution, cancellation, progress, failure feedback, or audit evidence;
- stable quality, governance, review, and approval boundaries;
- cross-surface consistency, accessibility, performance, or a materially clearer interaction;
- deterministic integrity required to keep the system legible and safe to operate.

A user interface is not a violation merely because an agent could invoke the same capability. A useful surface can make state visible, make frequent actions faster, or give people direct control. The violation is duplicating agent reasoning or orchestration in bespoke product paths when shared context, tools, and an ordinary agent interaction would provide a stronger and more general result.

Quality illustrates the boundary. Refine stores the project's business requirements, instructions, plain-language tests, timing, workflow state, and evidence contract. A Quality agent determines how to perform those tests for a Goal and reports structured results. Refine supervises the work, records the verdict, and advances or fails the Goal. Governance follows the same split between agent judgment and durable enforcement.

The Supervisor agent should make this model feel continuous. A user can state intent conversationally; the Supervisor can create the appropriate work, monitor workflow health, resolve routine failures, and approve evidence-backed results. Refine supplies the durable state and authority boundaries that make those actions observable and recoverable.

## Future Direction

As native agents improve, Refine should remove scaffolding that duplicates their capabilities and invest more deeply in orchestration, evidence, synchronization, and shared interaction. Existing deterministic features should be revisited when agents can perform the adaptable part more effectively, while stable integrity and user-control boundaries remain explicit.

The test is not simply whether an agent *can* do something. The test is whether product code adds durable shared value beyond what an agent can already do with Refine's context and tools.
