# Target-App Ontology And Ontology-Driven Implementation

## Status

Draft.

## Summary

Refine will add an optional target-app ontology to Governance. The ontology is a
durable, structured description of the application: its entities, properties,
relations, behaviors, rules, and current realization in the repository.

Product, Constitution, and Ontology form the generative semantic specification
for a target app:

```text
Product       defines the intended outcomes and users
Constitution  defines principles every realization must preserve
Ontology      defines what exists, how it behaves, and what remains valid
Source code   is one concrete realization of those semantics
```

Rules become part of the ontology rather than a separate Governance data source.
They reference ontology concepts and declare how Refine should establish
compliance through structural validation, repository observations, behavioral
evidence, Governance judgment, or human Review.

The ontology is useful only when incorporated into implementation. Refine will
replace the current single semantic Goal Agent completion with an optional,
durable implementation sub-workflow:

1. A planning agent receives Product, Constitution, Ontology, Goal history, the
   current Round, and repository access.
2. It proposes an ontology-driven implementation plan in a constrained format.
3. Refine validates and pins that plan.
4. Refine executes one focused implementation step at a time, giving the active
   agent only the relevant ontology slice, rules, prerequisites, and evidence.
5. Refine deterministically observes changes, evaluates obligations, records
   evidence, checkpoints successful steps, and either advances, replans, or
   fails.
6. Existing full Governance, Quality, Ready Merge, build, and human Review
   boundaries remain authoritative.

The web surface will present the ontology visually with a bounded tldraw
application. The canvas is a projection and editing surface over the same shared
ontology capability used by CLI, API, MCP, agents, and Workflow. tldraw state is
never the authoritative ontology.

Refine continues to work normally when no ontology is configured.

## Motivation

Goal Agents currently receive a broad pinned specification and autonomously
inspect, implement, and verify the entire Round before emitting one completion
signal. Refine deterministically controls the worktree, commit, Governance,
Quality, integration, and Review around that probabilistic implementation, but
the internal implementation process remains one semantic step.

This leaves several recurring failure modes:

- the agent reconstructs the target app's conceptual model from prose and code;
- relevant project-wide constraints compete with unrelated prompt context;
- cross-capability and cross-surface effects are discovered late or missed;
- the agent decides its own implementation sequence without durable checkpoints;
- Governance detects violations only after the complete implementation attempt;
- reviewers see code and workflow evidence without a shared semantic account of
  what application concepts changed;
- incorrect or incomplete project understanding is not converted into a
  reviewable target-app model improvement.

Refine's Workflow already demonstrates the desired control pattern. It is an
executable ontology of delivery: Goals, Rounds, processes, candidates, evidence,
states, transitions, and review boundaries have explicit meaning, and agents
cannot bypass the deterministic engine that applies that meaning.

The target-app ontology extends the same pattern to implementation semantics:

```text
Workflow governs how implementation proceeds.
Ontology governs what a valid implementation means.
```

## Design Principles

### Optional Model, Mandatory Use When Present

Ontology configuration is optional. Its absence must preserve current Goal
behavior and must not make Governance incomplete.

When an ontology is present and accepted, Goal Workflow must use it. An agent
cannot opt out of impact classification, omit applicable ontology rules, or
replace the pinned ontology revision with current mutable state.

### Deterministic Outer Engine, Probabilistic Inner Actors

Agents propose ontology content, implementation plans, step results, and
discrepancy classifications. Refine owns:

- schema validation;
- referential integrity;
- revision checks;
- applicable-rule closure;
- step readiness and state transitions;
- process ownership;
- Git observation and checkpoints;
- cancellation and recovery;
- durable evidence;
- Goal workflow transitions.

No agent response independently advances durable state.

### Semantic Model Over Implementation Catalog

The semantic ontology describes durable product meaning independent of the
current framework, file layout, or implementation technique.

Repository paths, symbols, tests, and runtime observations belong to a separate
realization mapping. They explain where the semantics are currently implemented
or demonstrated without making those locations part of the permanent meaning.

### One Authoritative Capability

Ontology behavior belongs to one shared product capability. Web, CLI, API, MCP,
Workflow, background operations, and agents must use that capability rather than
implementing their own parsers, validators, mutation rules, or persistence.

### Reviewable Evolution

Generated or agent-proposed ontology changes are candidates. They never silently
replace accepted ontology state.

An implementation failure cannot be made successful retroactively by changing
the ontology revision that judged it. Ontology corrections apply to new Rounds
or explicit reevaluation against a newly pinned revision.

### Focused Context

Goal Agents receive a relevant ontology slice rather than an unbounded graph.
Refine records why every entity, relation, behavior, and rule was selected.

### Evidence Before Conclusions

Ontology claims, implementation-plan outcomes, rule findings, and ontology
updates cite repository, Git, test, runtime, or human evidence. Explanatory prose
does not substitute for an observed result.

### Mitigation And Accountability

The ontology should increase implementation quality, evidence, and review
pressure without becoming a broad permission wall. Rule effects and enforcement
modes remain explicit and proportional to the represented risk.

## Goals

- Give Refine and target-app agents a stable vocabulary for product semantics.
- Represent entities, typed properties, relations, behaviors, and rules with
  stable IDs.
- Preserve semantic meaning separately from current repository realization.
- Generate and refresh an ontology by inspecting the target app through an
  installed agent provider.
- Validate generated output before it becomes an ontology candidate.
- Present structural diffs and require explicit application of a candidate.
- Let humans, agents, programs, CLI, API, MCP, and web operate on the same model.
- Identify ontology impact during Goal creation or before implementation.
- Pin the exact ontology revision and selected slice to each Round.
- Decompose implementation into a durable ontology-driven plan.
- Execute focused steps with deterministic state, evidence, checkpoints,
  cancellation, recovery, and bounded replanning.
- Evaluate ontology rules against actual implementation evidence.
- Show declared impact, observed impact, discrepancies, plan progress, and
  semantic changes in Review.
- Provide a tldraw visual projection and editing surface without making canvas
  state authoritative.
- Preserve existing behavior when no ontology exists.
- Measure whether ontology-driven implementation materially improves candidate
  quality before completing the full visual product investment.

## Non-Goals

- Guarantee that Product, Constitution, and Ontology can reproduce every detail
  of an existing application in the first version.
- Preserve source code, frameworks, module layout, styling, or algorithms that
  are not part of the semantic contract.
- Replace Git, tests, Quality, Governance judgment, or human Review.
- Treat an LLM-generated ontology or implementation plan as trusted execution
  authority.
- Store arbitrary executable shell commands in generated ontology rules.
- Make a graph database, RDF store, OWL reasoner, or external ontology service a
  runtime dependency.
- Make tldraw snapshots the source of truth.
- Convert the rest of Refine's web surface to React.
- Execute implementation steps concurrently in the same worktree in the first
  version.
- Let an ontology update retroactively alter the evidence or outcome of a pinned
  Round.
- Require small or non-software Goals to use a multi-step implementation plan.

## Governance Model

Governance becomes:

```text
Governance
├── Product
├── Constitution
└── Ontology
    ├── Semantic model
    │   ├── Entities
    │   ├── Property definitions and values
    │   ├── Relations
    │   ├── Behaviors
    │   └── Rules
    └── Realization mapping
        ├── Paths and symbols
        ├── Tests and commands
        ├── Runtime observations
        └── Evidence
```

Product and Constitution remain concise human-editable Markdown.

The ontology is structured JSON with human-readable names, descriptions, and
rule statements. The structured fields support deterministic validation,
selection, mutation, and diffing; the prose fields support agent and human
judgment.

Governance remains configured when Product and Constitution are both present.
Ontology presence does not affect the existing `configured` meaning.

## Terminology

### Accepted Ontology

The current authoritative ontology revision used for new Goal impact
classification and new Round context.

### Ontology Candidate

A validated proposed replacement for the accepted ontology. A candidate records
its base revision, provider, generation mode, raw output, validation result,
computed diff, and evidence. It has no implementation authority until applied.

### Semantic Ontology

The application concepts and rules intended to survive implementation changes.

### Realization Mapping

Evidence-backed links from semantic ontology IDs to the paths, symbols, tests,
interfaces, and runtime behavior that currently realize them.

### Ontology Slice

A bounded selection of ontology records and applicable rules assembled for a
Goal or implementation step. A slice records selection reasons and the accepted
ontology revision and digest.

### Declared Impact

The entities, relations, behaviors, properties, and rules believed to be
affected before implementation.

### Observed Impact

The semantic impact supported by actual changed paths, agent reports, tests,
runtime observations, and realization mappings after work occurs.

### Semantic Discrepancy

Durable evidence that observed behavior and expected ontology semantics do not
align. A discrepancy may indicate an implementation violation, ontology gap,
ontology error, Goal misclassification, realization-mapping drift, or evidence
gap.

### Implementation Plan

A versioned, validated set of dependent implementation steps owned by one Goal
Round and one ontology revision.

### Implementation Step

A focused unit of inspection, change, reconciliation, or verification with an
ontology scope, prerequisites, success conditions, required evidence, attempts,
and durable outcome.

## Canonical Data Model

### Governance Document

`governance.json` remains the authoritative Governance document. Its next schema
adds `ontology` and removes `rules` as an independent write target after
migration:

```json
{
  "product": "Product intent in Markdown.",
  "constitution": "Constitution in Markdown.",
  "ontology": {
    "schema_version": 1,
    "revision": 3,
    "updated_at": "2026-07-27T18:00:00Z",
    "digest": "sha256:...",
    "semantics": {
      "entities": [],
      "property_definitions": [],
      "relations": [],
      "behaviors": [],
      "rules": []
    },
    "realization": {
      "mappings": []
    }
  },
  "configured": true
}
```

`configured` remains derived rather than caller-controlled.

The server owns `schema_version`, `revision`, `updated_at`, and `digest`.
Provider output cannot set authoritative metadata.

### Entity

```json
{
  "id": "capability.workflow",
  "kind": "Capability",
  "name": "Workflow",
  "description": "Coordinates durable Goal state advancement.",
  "properties": {
    "authoritative_for": ["concept.workflow-state"],
    "shared": true
  },
  "aliases": ["workflow engine"]
}
```

Requirements:

- `id` is stable, unique, non-empty, and safe for references.
- `kind`, `name`, and `description` are required.
- property keys refer to declared property definitions unless explicitly marked
  as extension metadata.
- renaming an entity does not change its ID.
- replacing an ID is represented as remove plus add and is visible in the diff.

### Property Definition

```json
{
  "id": "authoritative_for",
  "name": "Authoritative for",
  "description": "Concepts for which the subject owns canonical semantics.",
  "applies_to": ["Capability"],
  "value_type": "entity_ref_list",
  "cardinality": "many",
  "required": false
}
```

Supported first-version value types:

- `string`;
- `string_list`;
- `boolean`;
- `integer`;
- `number`;
- `enum`;
- `entity_ref`;
- `entity_ref_list`;
- `relation_ref`;
- `relation_ref_list`;
- `behavior_ref`;
- `behavior_ref_list`.

The validator rejects unknown referenced IDs, invalid cardinality, and values
that do not match their definitions.

### Relation

```json
{
  "id": "relation.web-delegates-workflow",
  "type": "delegates_to",
  "name": "Web delegates to Workflow",
  "description": "The web surface delegates workflow semantics to the shared capability.",
  "from": "surface.web",
  "to": "capability.workflow",
  "properties": {}
}
```

Relation endpoints must exist. Relation type definitions may be represented as
entities or property-backed kinds in the first version; the validator must still
apply endpoint and cardinality constraints consistently.

### Behavior

```json
{
  "id": "behavior.goal-transition",
  "name": "Advance Goal state",
  "description": "Advance a Goal only when required evidence is durable.",
  "actors": ["capability.workflow"],
  "inputs": ["entity.goal", "entity.round", "entity.evidence"],
  "preconditions": ["rule.evidence-before-transition"],
  "postconditions": ["relation.goal-has-new-state"],
  "observables": ["goal status", "workflow log", "round evidence"]
}
```

Behaviors capture operations, state transitions, causal ordering, and observable
contracts that cannot be represented adequately as static relationships.

### Rule

Rules are ontology records:

```json
{
  "id": "rule.surface-delegation",
  "kind": "invariant",
  "statement": "Surfaces delegate product semantics to shared capabilities.",
  "scope": {
    "entity_ids": ["surface.web", "capability.workflow"],
    "entity_kinds": ["Surface", "Capability"],
    "relation_types": ["delegates_to"],
    "behavior_ids": []
  },
  "effect": "block",
  "enforcement": {
    "mode": "judgment",
    "checker_id": null,
    "required_evidence": [
      "affected adapters",
      "shared capability verification"
    ]
  }
}
```

Supported rule kinds:

- `invariant`: a truth every valid realization must preserve;
- `behavior`: a required transition, causal order, or observable outcome;
- `policy`: an architectural or implementation constraint;
- `evidence`: verification required for a represented risk;
- `escalation`: a condition requiring explicit human judgment;
- `inference`: a derivation Refine may make from ontology facts.

Supported effects:

- `inform`: record and display without changing progression;
- `require_evidence`: block when required evidence is absent;
- `require_review`: surface the finding prominently at human Review;
- `block`: stop automated progression on an actual violation.

Supported enforcement modes:

- `structural`: evaluated by ontology schema and graph validators;
- `repository`: evaluated against changed paths, symbols, Git observations, and
  realization mappings;
- `behavioral`: evaluated through registered Quality checks or runtime evidence;
- `judgment`: evaluated by the Governance agent against actual changes;
- `human`: decided only at Review.

Generated rules may reference only registered checker IDs. Provider output must
not create executable commands or silently register new checkers.

### Realization Mapping

```json
{
  "id": "mapping.workflow-capability",
  "ontology_ids": [
    "capability.workflow",
    "behavior.goal-transition"
  ],
  "paths": ["src/workflow"],
  "symbols": ["WorkflowEngine"],
  "tests": ["workflow unit and full-workflow coverage"],
  "interfaces": ["/api/goals/:id/approve"],
  "evidence": [
    {
      "kind": "repository",
      "reference": "src/workflow",
      "claim": "Contains the shared workflow coordinator."
    }
  ]
}
```

Realization mappings are accepted ontology data but remain explicitly distinct
from semantic meaning. Repository refresh may update mappings without changing
the meaning of referenced entities.

### Evidence Reference

Evidence references use typed, inspectable fields:

```json
{
  "kind": "path",
  "reference": "src/workflow/behaviors/mod.rs",
  "claim": "Workflow owns Goal implementation state advancement.",
  "observed_at": "2026-07-27T18:00:00Z",
  "commit": "..."
}
```

Supported evidence kinds initially include `document`, `path`, `symbol`, `git`,
`test`, `runtime`, `review`, and `human`.

An unavailable evidence reference does not automatically delete an ontology
claim. It produces a realization-mapping discrepancy for review.

## Identity, Revisions, And Digests

- Ontology IDs are stable semantic identities, not array indexes.
- Every accepted mutation increments the ontology revision.
- The digest covers normalized semantic and realization content, excluding
  server-owned timestamps.
- All candidates record `base_revision` and `base_digest`.
- Applying a stale candidate returns a conflict and leaves accepted state
  unchanged.
- A candidate is applied atomically through the shared service.
- Round context records the accepted revision, digest, and exact selected slice.
- Previously pinned Round context remains immutable when the accepted ontology
  changes.
- A new Round is required when an ontology change materially changes the
  implementation contract for a failed or incomplete attempt.

## Persistence And Layout

Authoritative ontology state remains target-app control state outside the
primary source worktree and follows existing Refine state synchronization and
durability behavior.

Visual layout is stored separately from `governance.json`:

```json
{
  "schema_version": 1,
  "ontology_digest": "sha256:...",
  "nodes": {
    "capability.workflow": {"x": 120, "y": 240, "w": 280, "h": 160}
  },
  "groups": {},
  "annotations": []
}
```

Layout is a rebuildable shared projection keyed by ontology IDs. Failure to load
or migrate layout must not make ontology state unavailable.

Camera position, zoom, selection, open panels, and transient drawing state are
browser-local and are not persisted as authoritative project state.

Ontology candidates may be retained as durable operation results or dedicated
candidate records. They are not embedded into the accepted ontology and do not
participate in Goal context until applied.

## Shared Capability Architecture

The exact responsibility boundaries are required even if implementation file
names evolve:

```text
Foundation
  Ontology model, typed records, normalization, validation, revisions

Ontology capability
  Load, query, slice, diff, generate, refresh, candidate review,
  command application, impact reconciliation, rule applicability

Workflow capability
  Implementation-plan state, step readiness, execution, replanning,
  checkpoints, cancellation, recovery, final advancement

Agents capability
  Planner, step executor, Governance evaluator, native session contract

Process capability
  Managed provider processes, transcripts, input, cancellation, ownership

Surfaces
  Thin HTTP, CLI, MCP, web, desktop, and tldraw adapters
```

Suggested source organization:

```text
src/model/ontology.rs
src/tools/product/ontology/
  mod.rs
  validation.rs
  storage.rs
  candidates.rs
  generation.rs
  diff.rs
  commands.rs
  slicing.rs
  impact.rs
src/workflow/implementation_plan/
  mod.rs
  model.rs
  planning.rs
  execution.rs
  recovery.rs
  evidence.rs
```

The shared capability may initially compose the existing Governance file service
rather than require an unrelated Governance migration. Web routes must not own
provider invocation, parsing, fallback generation, validation, or persistence.

## Ontology Generation And Refresh

### Prompt Template

`src/prompts/ontology.md` is the agent-facing template for both initial
generation and refresh.

The prompt remains concise and uses the shared prompt engine. A representative
contract is:

```markdown
Construct or revise the target app ontology from repository evidence. Return
only JSON conforming to schema version {{schema_version}}. Model durable domain
and architectural entities, relations, meaningful properties, behaviors, and
rules; exclude incidental implementation trivia. Preserve stable IDs from the
current ontology when meanings are unchanged. Cite evidence for material claims.
Do not edit files.

Mode: {{mode}}
Product: {{product}}
Constitution: {{constitution}}
Repository: {{target_root}}
Current ontology:
{{current_ontology}}
```

The Rust model and validator define the schema. The prompt does not become a
second schema authority.

### Modes

`generate`:

- requires Product and Constitution;
- runs when no accepted semantic ontology exists;
- asks the provider for one complete candidate;
- does not synthesize a static fallback.

`refresh`:

- requires an accepted ontology;
- supplies the current ontology and repository;
- asks for one complete replacement candidate;
- instructs the provider to preserve stable IDs when meanings are unchanged;
- allows semantic and realization changes;
- relies on Refine, not the provider, to compute the diff.

### Operation Lifecycle

Generation and refresh are durable, cancellable, exclusively owned background
operations:

```text
register operation with base revision
  -> launch supervised provider in target-app context
  -> retain raw output
  -> parse candidate
  -> normalize and validate
  -> compute structural diff
  -> finish with reviewable candidate
```

Only one generation or refresh operation may own an ontology base revision at a
time. Other ontology reads remain available.

Provider failure, authentication failure, malformed output, validation failure,
or cancellation leaves accepted ontology state unchanged. There is no generic
fallback ontology because plausible but false semantics are worse than explicit
absence.

### Candidate Review And Application

Candidate review shows:

- added, changed, and removed entities;
- property-definition and property-value changes;
- relation changes;
- behavior changes;
- rule and enforcement changes;
- realization-mapping changes;
- stable IDs preserved or replaced;
- missing or weak evidence;
- validation warnings;
- provider and base revision.

Apply requires explicit user action and a matching base revision. Discard retains
the operation audit record but removes the candidate from active attention.

## Semantic Discrepancies And Ontology Updates

Humans, Goal Agents, Governance evaluators, verification programs, and observers
may record a semantic discrepancy:

```json
{
  "id": "discrepancy-...",
  "goal_id": "GOAL1",
  "round": 2,
  "ontology_revision": 12,
  "classification": "ontology_gap",
  "expected": "Workflow transitions are evidence-backed.",
  "observed": "A transition occurred before evidence persistence.",
  "ontology_ids": [
    "capability.workflow",
    "rule.evidence-before-transition"
  ],
  "evidence": [],
  "proposed_response": "ontology_candidate"
}
```

Classifications:

- `implementation_violation`: correct the implementation against the same
  pinned ontology;
- `ontology_gap`: propose missing semantic content;
- `ontology_error`: propose a correction to accepted meaning;
- `goal_misclassification`: recompute declared impact or slice selection;
- `realization_drift`: refresh repository mappings;
- `evidence_gap`: gather evidence without changing semantic meaning.

Discrepancy classification is a proposal subject to validation and Review.

An ontology candidate created from a discrepancy records the discrepancy ID.
Applying it never changes the historical Round's ontology revision or verdict.

## Goal Impact And Ontology Slicing

### Declared Impact

When an accepted ontology exists, Goal creation, import, Plan drafting, or
pre-implementation preparation proposes declared impact:

```json
{
  "ontology_revision": 12,
  "entity_ids": ["capability.workflow"],
  "relation_ids": ["relation.workflow-owns-goal-state"],
  "behavior_ids": ["behavior.goal-transition"],
  "property_ids": [],
  "rule_ids": ["rule.evidence-before-transition"],
  "rationale": "The Goal changes workflow state advancement."
}
```

The proposal may be agent-generated, but Refine validates every ID. Goal creation
must not fail solely because automatic impact classification cannot run; it may
record an unresolved classification that must be resolved before ontology-driven
implementation starts.

### Slice Selection

Refine computes a focused slice from:

- declared impact;
- direct relations between impacted entities;
- required endpoint entities;
- referenced property definitions;
- relevant behaviors;
- rules whose scope intersects selected concepts;
- constitutional or project-wide rules;
- realization mappings needed to locate current code and evidence;
- an explicit token or record budget.

Each included record carries one or more selection reasons.

The agent may propose additional relevant IDs. Refine validates and expands the
slice deterministically. The agent cannot remove rules that Refine determines
are applicable.

### Round Pinning

The Round records:

- ontology revision and digest;
- declared impact;
- exact selected ontology slice;
- selection reasons;
- unresolved classification warnings;
- assembly time and context schema version.

No accepted ontology is represented explicitly as absent context rather than an
empty but configured ontology.

## Ontology-Driven Implementation Planning

### Placement In Workflow

The implementation plan is a subordinate state machine inside the existing
`InProgress` Workflow behavior.

It does not introduce new public Goal statuses. Only the existing Workflow may
transition the Goal to QA, Ready Merge, Failed, Cancelled, or any later state.

One Goal Round owns one active implementation plan. Distinct Goals may continue
to execute concurrently. Steps for one plan execute sequentially in the first
version.

### Planning Agent

The planning agent receives:

- Product;
- Constitution;
- current accepted ontology revision;
- declared Goal impact and initial ontology slice;
- Goal identity and history;
- all previous Rounds;
- current Round request;
- Workflow and Review boundaries;
- target-app repository access;
- the implementation-plan output schema.

Planning is read-only. Provider adapters should enforce read-only planning when
supported. Refine verifies that planning did not mutate the worktree; an
unexpected mutation fails planning and is retained as evidence rather than
silently discarded.

The planner may inspect the repository to reconcile semantics with the current
realization. Product, Constitution, and Ontology are the semantic minimum, not a
substitute for repository evidence.

### Plan Model

```json
{
  "schema_version": 1,
  "id": "plan-...",
  "revision": 1,
  "status": "active",
  "goal_id": "GOAL1",
  "round": 2,
  "base_commit": "...",
  "ontology_revision": 12,
  "ontology_digest": "sha256:...",
  "provider": "codex",
  "created_at": "2026-07-27T18:00:00Z",
  "steps": []
}
```

Server-owned plan status values:

- `planning`;
- `active`;
- `replanning`;
- `completed`;
- `failed`;
- `cancelled`.

### Step Model

```json
{
  "id": "step.shared-capability",
  "kind": "implement",
  "objective": "Implement the behavior in the shared Workflow capability.",
  "depends_on": [],
  "ontology_scope": {
    "entity_ids": ["capability.workflow"],
    "relation_ids": [],
    "behavior_ids": ["behavior.goal-transition"],
    "rule_ids": ["rule.evidence-before-transition"]
  },
  "expected_areas": ["src/workflow"],
  "success_conditions": [
    "The shared capability owns the behavior.",
    "Existing surfaces use the shared behavior."
  ],
  "required_evidence": [
    "focused tests",
    "changed-path observation"
  ],
  "status": "pending",
  "attempts": [],
  "checkpoint_commit": null
}
```

Step kinds:

- `inspect`: establish repository or runtime facts without intended mutation;
- `implement`: add or change behavior;
- `reconcile`: correct an earlier step, integrate discoveries, or resolve a
  semantic discrepancy;
- `verify`: gather focused behavioral evidence.

`expected_areas` guide investigation and review. They do not prohibit justified
changes elsewhere.

Server-owned step statuses:

- `pending`;
- `ready`;
- `running`;
- `needs_input`;
- `succeeded`;
- `failed`;
- `cancelled`;
- `superseded`.

### Plan Validation

Before execution, Refine validates:

- Goal, Round, base commit, ontology revision, and digest;
- all referenced ontology IDs;
- unique plan and step IDs;
- acyclic step dependencies;
- bounded step count and description sizes;
- declared-impact coverage;
- applicable-rule coverage;
- success conditions and evidence obligations;
- no requested Goal transition, approval, merge, or other unavailable authority;
- at least one verification path for mutating plans.

The planner proposes `ontology_scope.rule_ids`. Refine computes applicable rules
from the selected ontology scope and injects omissions before the plan is pinned.

### Adaptive Granularity

The engine supports one-step plans. A small change should not be forced through
artificial decomposition.

Planning limits prevent both a single unbounded implementation step for a
high-impact Goal and pathological micro-steps. Initial defaults should favor:

- one step for trivial or non-software work;
- two to four steps for ordinary feature work;
- additional explicit verification or reconciliation steps for high-risk,
  cross-capability, persistence, concurrency, process, Git, migration, or
  cross-surface changes.

Granularity is proposed by the planner and bounded by deterministic settings and
validation.

## Dynamic Implementation Engine

### Execution Cycle

```text
select next dependency-ready step
  -> compute focused ontology slice and applicable rules
  -> assemble step prompt
  -> launch one managed native Goal Agent session
  -> receive step completion claim or needs-input signal
  -> observe Git, paths, tests, and process evidence
  -> validate observed ontology impact
  -> evaluate step obligations
  -> checkpoint success, replan, retry, or fail
```

The engine, not the agent, marks a step succeeded.

### Step Context

Each step receives only:

- Goal identity and current Round request;
- the step objective, prerequisites, and success conditions;
- relevant summaries from completed prerequisite steps;
- the focused ontology slice and selection reasons;
- every deterministically applicable rule;
- relevant Guidance candidates;
- realization mappings for likely code and evidence;
- current branch, worktree, base commit, and latest checkpoint;
- the step completion protocol.

The full ontology and full prior transcripts are not repeated unless selected by
an explicit context rule.

### Agent Sessions

Each step uses a fresh managed native provider session by default. This provides
context isolation and prevents the accumulated conversation from defeating
focused slicing.

The user experience remains one logical Goal Agent:

- the CLI and web "Open Agent" actions attach to the currently active step;
- process metadata includes Goal ID, Round, plan ID, plan revision, and step ID;
- step transcripts aggregate under the Round;
- exactly one step session may own a Goal at a time;
- closing a surface detaches without stopping work;
- explicit `needs_input` preserves the same active step session and workflow
  claim until answered.

The engine reuses installed provider CLIs and their native TUI behavior. It does
not rebuild the agent interaction experience.

### Step Completion Contract

A step agent claims completion through a structured signal:

```json
{
  "state": "completed",
  "plan_id": "plan-...",
  "plan_revision": 1,
  "step_id": "step.shared-capability",
  "message": "Implemented the shared behavior and ran focused tests.",
  "observed_impact": {
    "entity_ids": ["capability.workflow"],
    "relation_ids": [],
    "behavior_ids": ["behavior.goal-transition"],
    "rule_ids": ["rule.evidence-before-transition"]
  },
  "verification": [
    {
      "command": "cargo test ...",
      "outcome": "passed",
      "evidence": "..."
    }
  ],
  "guidance_applied": [],
  "discrepancies": [],
  "plan_amendment_requested": false
}
```

The engine rejects wrong plan or step identity, unknown ontology IDs, malformed
evidence, invalid Guidance selection, or a stale plan revision.

`needs_input` remains available only for an impossible missing decision or
authority. Routine uncertainty should produce the best supported implementation
decision or a replan proposal rather than user interruption.

### Observation And Rule Evaluation

After the agent claims completion, Refine observes:

- worktree status;
- changed paths since the latest checkpoint;
- Git diff and candidate commit ancestry;
- registered test and Quality results;
- process outcome and transcript;
- declared versus observed ontology impact;
- realization mappings;
- rule-specific evidence.

Structural and inexpensive repository rules run after every step.

Behavioral and judgment checks may run at semantic milestones when their cost is
proportional to the step risk. Full configured Governance and Quality still run
after the entire plan.

A discrepancy between declared and observed impact does not automatically fail
when it represents a justified discovery. It must be explained, validated, and
either incorporated through replanning or recorded for Review.

### Checkpoints

After a mutating step passes its required obligations, Refine creates a
step-labeled checkpoint commit in the existing implementation branch and records
the exact SHA.

Checkpoint commits:

- remain inside the Round's isolated worktree and branch;
- are never automatically merged or approved;
- give restart recovery an exact durable boundary;
- make completed step evidence auditable;
- allow later steps to build on known state.

Read-only steps record the observed base or latest checkpoint without creating
an empty commit.

The final candidate commit is the last verified checkpoint or a final
reconciliation commit covering remaining verified changes.

### Replanning

A step may request replanning because of:

- an unexpected dependency;
- a newly affected ontology concept;
- an invalid assumption;
- an ontology or realization discrepancy;
- failed required evidence;
- a need for reconciliation.

Replanning receives the original plan, immutable completed-step outcomes, current
worktree and checkpoint state, discrepancies, and remaining Goal obligations.

Plan revisions are append-only. A revision may add, change, reorder, or
supersede pending steps. It cannot rewrite completed steps, their evidence, or
their checkpoint commits.

Refine validates every revision and recomputes applicable rules. Replanning has
bounded attempts; exceeding the configured bound fails the implementation while
preserving all evidence.

### Failure

A failed step records:

- attempt number;
- provider and process identity;
- error and phase;
- worktree observation;
- partial changed paths;
- verification outcomes;
- applicable rules;
- transcript and logs;
- whether retry or replan is available.

Refine never reports plan or Goal success because an agent produced a plausible
completion message.

Partial work remains in the retained implementation worktree. Existing recovery
Round behavior remains available after Goal failure.

### Cancellation

Explicit Goal cancellation is terminal and has precedence over implementation
plan progression.

Cancellation:

- durably records intent before process settlement;
- stops or settles the active step session through the shared Process
  capability;
- marks pending steps cancelled;
- preserves completed steps, attempts, checkpoints, transcripts, and
  discrepancies;
- cannot be reversed into a running plan by a late agent signal;
- retains the worktree according to existing Goal cancellation policy.

### Restart Recovery

On restart, the engine reconciles:

- durable Goal and Round state;
- plan and step revision;
- active workflow claim;
- managed process state;
- checkpoint commit;
- worktree changes since checkpoint;
- cancellation intent;
- provider completion signals.

Recovery must never launch a duplicate active step. If process and durable step
state cannot be reconciled safely, the step becomes interrupted or failed with
evidence rather than being guessed complete.

## Finalization And Existing Workflow

Plan completion is not Goal completion.

After all required steps succeed:

1. Refine verifies that no unexplained worktree changes remain outside the final
   checkpoint.
2. It reconciles declared and observed ontology impact.
3. It records the complete implementation report from step outcomes rather than
   relying on one final agent narrative.
4. It runs full post-implementation Governance against the pinned ontology.
5. It advances to configured Quality timing only when Governance passes.
6. Existing Ready Merge, build, post-build Quality, and human Review behavior
   continues unchanged.

The implementation engine cannot integrate, approve, merge, mark Review
accepted, or move the Goal directly to Done.

## Governance Findings

Ontology-aware findings use stable references:

```json
{
  "rule_id": "rule.surface-delegation",
  "message": "The web surface introduced product semantics outside the shared capability.",
  "ontology_ids": [
    "surface.web",
    "capability.workflow",
    "relation.web-delegates-workflow"
  ],
  "plan_id": "plan-...",
  "step_id": "step.web-adapter",
  "evidence": [
    {
      "kind": "path",
      "reference": "src/surfaces/web/...",
      "claim": "Contains the new eligibility decision."
    }
  ]
}
```

Governance evaluates actual changes against applicable pinned rules. It must not
report preferences, hypothetical risks, or violations of unselected current
ontology revisions.

The engine records:

- rules considered;
- selection reasons;
- enforcement mode;
- checker or provider used;
- evidence;
- pass, fail, review-required, or informational result.

## Review

Review adds semantic evidence alongside the existing code and workflow evidence:

- ontology revision and digest;
- declared Goal impact;
- implementation-plan revisions;
- step statuses and checkpoint commits;
- observed impact;
- intended versus observed semantic diff;
- applicable rules and findings;
- discrepancies and their dispositions;
- proposed ontology changes;
- code diff, Quality, build, Git, and existing Governance evidence.

Review may:

- accept the integrated implementation through the existing approval path;
- request a new implementation Round;
- request or apply a separately reviewed ontology candidate;
- record a follow-up Goal;
- fail or cancel through existing actions.

Accepting an implementation does not implicitly accept an ontology candidate.
Accepting an ontology candidate does not implicitly accept an implementation.

## Web Ontology Surface

### Product Placement

Governance settings retain Product and Constitution fields and add an Ontology
section. Rules are presented within Ontology rather than as an independent
source of truth.

The Ontology section provides:

- empty-state explanation;
- Generate action when absent;
- Refresh action when accepted state exists;
- current revision and validation status;
- structured list/editor fallback;
- candidate diff and explicit Apply or Discard;
- link to a full visual ontology route.

The full visual route presents the model, realization mappings, rules,
discrepancies, and revision history.

### tldraw Projection

tldraw visual mappings:

- entity -> custom node/card shape;
- relation -> typed bound arrow;
- property -> selected-record inspector field;
- behavior -> behavior or state-transition shape;
- rule -> constraint card connected to its scope;
- realization mapping -> optional repository/evidence overlay;
- domain or capability group -> frame;
- candidate addition, change, or removal -> diff overlay.

The ontology-to-tldraw adapter maps stable ontology IDs to stable visual shape
metadata. Generated tldraw record IDs are never used as semantic identity.

### Typed Canvas Commands

Canvas gestures emit typed ontology commands:

```json
{
  "command": "add_relation",
  "base_revision": 12,
  "relation": {
    "id": "relation.web-delegates-workflow",
    "type": "delegates_to",
    "from": "surface.web",
    "to": "capability.workflow",
    "name": "Web delegates to Workflow",
    "description": "..."
  }
}
```

The shared service validates every command. Connecting shapes does not directly
write JSON. Deleting a referenced entity requires an explicit cascade candidate
or fails with the dependent records listed.

Canvas edits remain a local draft until Save submits one atomic command batch
against a base revision. A stale base produces a visible conflict and
authoritative refresh; the client never overwrites newer state.

### React Island Boundary

tldraw is integrated as one compiled React island mounted inside Refine's
existing static web surface.

The island:

- mounts into one stable, morph-preserved host;
- exposes a small JavaScript bridge for load, reconcile, save, selection, and
  teardown;
- receives authoritative ontology data through normal API reads and SSE
  reconciliation;
- emits typed commands only;
- unmounts explicitly on route exit;
- is not recreated during recurring Settings or SSE redraws;
- keeps React and tldraw dependencies inside the bounded visual artifact.

The broader Refine web surface does not migrate to React as part of this work.

The implementation must choose one compatible tldraw version and satisfy its
license and attribution requirements. Existing Harmonic DAG code may inform the
adapter design but must not be copied as an ontology data contract or introduce
multiple tldraw major versions.

### Live Updates

SSE remains the only live browser update transport.

When an agent or program changes accepted ontology state:

1. Refine emits the new revision.
2. The browser performs one authoritative reconciliation read.
3. The adapter updates shapes by ontology ID while preserving compatible layout.
4. Added, changed, and removed semantic records are highlighted.

The canvas does not poll ontology or operation status.

### Accessibility And Non-Visual Parity

The visual graph is not the only way to inspect or edit the ontology.

The web surface also provides keyboard-accessible structured lists and property
forms. CLI, API, and MCP expose the same records and commands. Essential model
meaning must not depend on color, spatial placement, or pointer-only gestures.

## API, CLI, And MCP Surface

Exact naming may follow current command and route conventions, but all surfaces
must expose the same shared operations.

Required capability operations:

- show accepted ontology;
- validate a document or candidate;
- generate;
- refresh;
- show operation and candidate status;
- show structural diff;
- apply candidate with base revision;
- discard candidate;
- apply typed command batch;
- list and show entities, relations, behaviors, rules, mappings, and
  discrepancies;
- query a focused slice;
- classify or amend Goal impact;
- show implementation plan and step evidence.

Representative CLI shape:

```text
refine ontology show
refine ontology validate
refine ontology generate
refine ontology refresh
refine ontology diff <candidate-id>
refine ontology apply <candidate-id>
refine ontology discard <candidate-id>
refine ontology discrepancy list
refine goal plan <goal-id>
```

CLI output supports structured JSON for agents and programs. MCP tools delegate
to the same service and return the same typed contracts.

Surfaces never receive a private bypass that writes `governance.json` directly.

## Prompt Templates

Expected templates:

- `ontology.md`: generate or refresh an ontology candidate;
- `ontology-plan.md`: produce a constrained implementation plan;
- `ontology-step.md`: execute one focused implementation step;
- existing post-implementation Governance template extended with the pinned
  ontology slice and observed impact.

Templates remain concise and task-specific. Large structured context is rendered
as data sections by shared code rather than duplicated instruction prose.

Prompt rendering tests enforce:

- all required variables are used;
- no undeclared variables are accepted;
- output schema identity is explicit;
- Product, Constitution, ontology revision, Goal, Round, plan, and step
  identities are correctly pinned;
- current mutable ontology is never substituted for Round-pinned context.

## Migration And Compatibility

### Existing Governance Rules

Existing top-level Governance rules migrate into `ontology.semantics.rules`:

```json
{
  "id": "rule-9",
  "kind": "policy",
  "statement": "Use SSE as the only mechanism for live browser updates.",
  "scope": {
    "entity_ids": [],
    "entity_kinds": [],
    "relation_types": [],
    "behavior_ids": []
  },
  "effect": "block",
  "enforcement": {
    "mode": "judgment",
    "checker_id": null,
    "required_evidence": []
  },
  "source": "legacy-governance-rule"
}
```

They remain project-wide until linked to more specific ontology scope. Existing
IDs, text, created time, updated time, and source are preserved.

The loader accepts the prior Governance schema and presents an in-memory
normalized ontology view. A durable schema write occurs through an explicit
migration or subsequent Governance mutation, following existing state migration
rules.

### Existing Goals And Rounds

- Existing Goal records remain valid.
- Existing pinned context version 1 remains readable and immutable.
- New ontology-aware Rounds use a new context version.
- A Round without ontology context continues through legacy one-shot
  implementation unless an explicit compatible migration is defined.
- The first implementation rollout may allow a setting-controlled choice
  between one-shot and plan execution for comparison and rollback.

### Existing Surfaces

Product and Constitution fields keep their wire meaning. Legacy `rules` reads may
be provided temporarily as a projection of `ontology.semantics.rules`, but new
writes use ontology commands.

## Security And Trust Boundaries

- Provider output is untrusted input.
- Generation and planning do not gain merge, approval, Goal transition, or
  arbitrary state-write authority.
- Generated checker IDs must resolve to registered capabilities.
- Generated ontology content cannot define executable shell commands.
- Planning is read-only and unexpected worktree mutation is an error.
- Step agents retain normal implementation authority only inside the isolated
  Goal worktree.
- Candidate application requires a matching base revision.
- Raw provider output is retained as potentially sensitive operation evidence
  according to existing redaction and export rules.
- API and remote-browser mutation origin rules apply to ontology mutations.
- tldraw embeds and arbitrary external assets are disabled unless explicitly
  supported by a separate security decision.

## Observability

Ontology and implementation-plan activity emits structured logs with:

- operation, Goal, Round, plan, step, and process identity;
- provider;
- ontology revision and digest;
- candidate and base revision;
- state transition;
- selected ontology IDs and reasons;
- rule checks and evidence;
- checkpoint commits;
- cancellation, interruption, retry, and replan details;
- validation or conflict errors.

Dashboard and Goal detail may summarize active plan progress, but durable Round,
operation, process, Git, and ontology state remain authoritative.

Provider or authentication failure must be visible as such and must not be
reported as an ontology validation failure, plan failure, or workflow-capacity
problem.

## Efficacy Gate

The first implementation stage tests the product hypothesis before building the
generator and tldraw experience.

Create a compact hand-authored Refine ontology and run representative historical
Goals through:

1. current one-shot implementation;
2. ontology-aware one-shot context;
3. ontology-driven planned step execution.

Use the same starting commit, model, provider configuration, authority, and
budget. Repeat conditions enough to distinguish a consistent effect from one
provider sample. Reviewers should evaluate candidates without knowing the
condition.

Primary measures:

- material semantic or architectural defects;
- first-candidate acceptability;
- missed affected capabilities or surfaces;
- corrective Rounds required;
- behavioral and failure-path coverage;
- unnecessary changes.

Secondary measures:

- elapsed time;
- provider invocations;
- token or budget usage where available;
- plan and ontology classification accuracy;
- reviewer effort.

Ontology citations, longer implementation reports, and visually richer Review
are not success measures by themselves.

Proceed to full generation and visual productization only if planned ontology
execution materially reduces defects or rework across repeated runs with an
acceptable execution-cost increase. Record the threshold and exact experiment
results before changing the gate.

## Implementation Order

### Phase 0: Efficacy Prototype

- Hand-author a minimal Refine ontology.
- Add typed model and slice rendering sufficient for experiments.
- Add a feature-gated plan schema and sequential step executor.
- Use existing provider and native session capabilities.
- Run and record the efficacy comparison.
- Stop or revise the approach if implementation outcomes do not improve.

### Phase 1: Canonical Ontology Foundation

- Add typed ontology model, validation, normalization, IDs, revisions, and
  digests.
- Normalize legacy Governance rules into ontology rules.
- Add shared storage, query, command, diff, and slice services.
- Preserve no-ontology behavior and old Round compatibility.
- Add CLI and API read/validate coverage.

### Phase 2: Generation And Candidate Review

- Add `ontology.md`.
- Add durable generation and refresh operations.
- Parse and validate complete provider candidates.
- Add structural diff, stale-base conflict, explicit Apply, and Discard.
- Add discrepancy records and candidate provenance.
- Expose shared behavior through CLI, API, MCP, and basic web forms.

### Phase 3: Goal Impact And Planning

- Add declared-impact classification.
- Pin ontology revisions and slices to new Rounds.
- Add `ontology-plan.md`, plan validation, and durable plan revisions.
- Keep one-step compatibility for small Goals.
- Expose plan state in Goal detail and process metadata.

### Phase 4: Dynamic Step Execution

- Add `ontology-step.md` and structured step completion.
- Launch fresh managed native sessions per step.
- Add deterministic observations, applicable-rule closure, step checkpoints,
  replanning, retry bounds, cancellation, and restart recovery.
- Aggregate step evidence into the implementation report.
- Preserve final Governance, Quality, integration, and Review boundaries.

### Phase 5: Ontology-Aware Governance And Review

- Extend Governance findings with ontology and evidence references.
- Reconcile declared and observed impact.
- Add semantic diff, discrepancy disposition, plan history, and checkpoint
  evidence to Review.
- Ensure ontology candidates and implementation approval remain separate.

### Phase 6: tldraw Visual Surface

- Select and pin one compatible tldraw version.
- Build the ontology-specific adapter and custom shapes.
- Add the bounded React island and vanilla bridge.
- Add typed command batches, visual candidate diff, and layout projection.
- Add SSE reconciliation and stable mounted-canvas behavior.
- Add accessible structured-editor parity.

## Verification Strategy

### Model And Validation

- valid and invalid entity, property, relation, behavior, rule, and mapping
  documents;
- duplicate and malformed IDs;
- unknown references;
- cardinality and value-type failures;
- cyclic or invalid semantic references where prohibited;
- normalization and stable digest behavior;
- rule effect and enforcement validation;
- legacy Governance rule normalization.

### Persistence And Concurrency

- atomic save and readback;
- monotonic revisions;
- stale candidate and stale command-batch conflict;
- two-daemon concurrent application;
- interrupted write recovery;
- state sync and migration behavior;
- accepted ontology unchanged on provider, parse, validation, or cancellation
  failure;
- layout failure isolated from ontology state.

### Generation

- concise prompt contract;
- generate and refresh modes;
- stable-ID preservation;
- malformed and prose-wrapped provider output;
- missing evidence warnings;
- provider authentication failure;
- no static fallback;
- operation cancellation, interruption, retry, and result retention.

### Slicing And Goal Context

- declared-impact validation;
- deterministic relation and rule closure;
- token or record budgeting;
- selection-reason retention;
- agent-proposed expansion;
- agent inability to remove applicable rules;
- exact revision and digest pinning;
- no-ontology no-op behavior;
- old Round context compatibility.

### Planning

- valid one-step and multi-step plans;
- unknown ontology references;
- cyclic dependencies;
- omitted applicable rules injected by Refine;
- missing declared-impact coverage;
- forbidden authority requests;
- planning worktree mutation detection;
- plan revision history;
- completed-step immutability;
- bounded replanning.

### Step Execution

- focused context assembly;
- one active step per Goal;
- distinct Goals executing concurrently;
- attach and detach from the active native session;
- valid completion and explicit needs-input;
- wrong plan, revision, or step signal;
- observed-impact validation;
- changed-path reconciliation;
- structural, repository, behavioral, judgment, and human rule modes;
- checkpoint creation and final candidate selection;
- no-change and non-software steps;
- provider failure and partial work retention.

### Cancellation And Recovery

- cancellation before launch, during planning, during a step, during
  verification, and between steps;
- late completion after cancellation cannot restart or succeed the plan;
- process-settlement failure;
- restart with running, exited, interrupted, and missing processes;
- checkpoint/worktree divergence;
- duplicate-runner exclusion;
- retained worktree, transcripts, attempts, and evidence.

### Governance And Review

- findings cite valid rule and ontology IDs;
- current ontology changes cannot alter pinned evaluation;
- discrepancy classifications and candidate linkage;
- semantic and code diff presentation;
- ontology candidate application independent from implementation approval;
- final full Governance and Quality still gate progression.

### Web And tldraw

- typed ontology commands rather than raw canvas persistence;
- stable ID mapping;
- entity, relation, behavior, rule, and diff rendering;
- atomic save and stale revision conflict;
- SSE reconciliation without polling;
- mounted canvas survives recurring Refine redraws;
- route exit unmounts exactly one canvas instance;
- agent-originated updates preserve compatible layout;
- keyboard-accessible structured fallback;
- real-browser validation for mounted shapes, bindings, selection, editing, and
  renderer-instance isolation.

### Surface Parity

- CLI, API, MCP, and web use the shared service;
- equivalent validation errors and revision conflicts;
- thin adapters contain no ontology business rules;
- Smoke AI can deterministically generate an ontology, a plan, step results,
  discrepancies, and Governance verdicts for integration coverage.

## Acceptance Criteria

- Governance has Product, Constitution, and Ontology as its authoritative model.
- Existing rules are preserved as typed ontology rules.
- Refine works normally with no ontology.
- Ontology generation and refresh produce candidates and never silently mutate
  accepted state.
- Candidates are validated, diffed, explicitly applied, revision-checked, and
  auditable.
- Humans and programs mutate ontology through one shared capability.
- New ontology-aware Rounds pin an exact revision, digest, slice, and selection
  reasons.
- Goal impact and applicable-rule selection cannot reference unknown IDs or be
  silently bypassed by an agent.
- The planner emits a constrained plan; Refine validates and owns its execution.
- The implementation sub-workflow remains subordinate to the existing
  `InProgress` state and cannot transition or approve the Goal.
- Each active step receives focused context and produces validated evidence.
- Successful mutating steps create auditable checkpoints.
- Replanning preserves completed history.
- Cancellation and restart recovery remain monotonic, durable, and
  process-aware.
- Final Governance, Quality, Ready Merge, build, and human Review boundaries
  remain intact.
- Governance findings and Review cite ontology concepts, rules, plan steps, and
  actual evidence.
- Ontology changes cannot retroactively change a Round's contract or outcome.
- tldraw is a replaceable projection over authoritative ontology state.
- Canvas changes become typed, atomic, revision-checked ontology commands.
- The visual surface uses SSE, survives redraws without duplicate renderers, and
  has non-visual accessible parity.
- Efficacy results demonstrate material implementation-quality improvement
  before full visual productization is considered complete.
