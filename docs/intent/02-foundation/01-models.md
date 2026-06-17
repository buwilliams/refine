# Models

## Key Ideas

- **Gap**: the smallest useful unit of work, framed as the difference between what exists and what should exist.
- **Feature**: an organized set of Gaps that together produce a larger product outcome.
- **Workflow State**: the current position of a Gap in the system's work lifecycle.
- **Node**: the local or distributed actor that owns active work.
- **Projection**: a fast, queryable view over durable flat-file state.
- **Plain Objects**: model data should stay simple enough for people and agents to read directly.

## Purpose

Models give Refine a shared language for software work. They let people, agents, the workflow engine, tools, and surfaces agree on what exists without depending on one screen or command.

The central product model is the Gap. A Gap records the actual behavior, desired target behavior, notes, rounds, status, priority, reporter, assignee, Feature membership, node ownership, and logs needed to understand and advance work. The model is intentionally ordinary: it should be possible to open the file, read it, and understand what the work is.

Features exist to preserve intent across multiple Gaps. A Feature should not replace the Gap model; it groups Gaps, preserves ordering when order matters, and lets the system explain larger outcomes without losing the smaller work units that agents can execute.

## Expected Role

Models should be the stable center of the system. Surfaces may rename controls, workflows may gain new steps, and tools may change providers, but the model should preserve the meaning of work.

Current implementation details that matter to intent:

- `Gap` and `Feature` are explicit Rust model types.
- Gap statuses are named workflow states: backlog, todo, in-progress, qa, ready-merge, build, review, done, failed, and cancelled.
- Gaps can belong to Features with an order, letting Refine advance ordered work without forcing every Gap into a sequence.
- Active work is owned by a node so distributed or multi-instance operation can be reasoned about explicitly.
- Projections exist so the system can stay fast without replacing flat files as the source of truth.

Future model changes should preserve these properties: human readability, agent readability, stable workflow meaning, and clear node ownership for active work.

## Future Direction

As AI agents improve, models should become more expressive without becoming more obscure. Refine may need richer representations of design intent, dependency graphs, quality evidence, governance decisions, agent capabilities, review provenance, and composition plans.

The direction should be toward a model that a superintelligent software system can use to compose large systems from many smaller changes while still leaving people an understandable audit trail. New model fields should answer real questions: what is the work, why does it matter, which node owns active progress, what evidence supports it, what can safely happen next, and what changed as a result.
