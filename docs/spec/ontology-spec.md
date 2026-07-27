# Ontology-Driven Implementation

## Status

Draft.

## Summary

Refine will add two things to Governance and one thing to implementation.

To Governance:

- **Architecture** — a concise, human-owned statement of the target app's
  intended shape: tech stack, surfaces, persistence, business logic and
  authority, and integrations.
- **Rules** — the existing plain-English Governance rules, unchanged in shape,
  but supplied wherever implementation decisions are made rather than only at
  final Governance.

To implementation: Refine will replace the current single semantic Goal Agent
completion with an optional, durable implementation sub-workflow driven by a
**two-stage planner**.

```text
Product        the intended outcomes and users
Constitution   the principles every realization must preserve
Architecture   the intended shape of the system
Rules          the constraints Governance enforces
Source code    one concrete realization, which may currently violate any of them
```

Stage one derives a **plan model**: the concepts, seams, and authority
boundaries relevant to one Goal, derived just-in-time from Governance, the
repository, and Goal history. Stage two decomposes the work into steps cut along
those seams. Refine validates and pins both stages, then executes the plan
deterministically, one focused step at a time.

The plan model is derived per implementation attempt and retained on its Round
as evidence. Refine does not maintain a durable ontology graph. What persists
across Goals is Architecture and Rules — the parts a repository cannot express —
while the structural model that a specific Goal needs is derived fresh each
time.

Retention is not authority. A Round's plan is durable history that later Rounds
of the same Goal read to learn what was tried and how it went. It is never a
model that a later Round starts from and edits.

This is deliberate. A durable graph must choose one granularity for all future
work, which is unsolvable in principle because the right granularity depends on
the task. It also freezes today's model's articulation into a maintained
artifact. Deriving per Goal fits granularity to the work, improves automatically
as providers improve, and eliminates the maintenance burden that ends most
ontology projects.

Refine continues to work normally when no Architecture is configured.

## Motivation

Goal Agents currently receive a broad pinned specification and autonomously
inspect, implement, and verify the entire Round before emitting one completion
signal. Refine deterministically controls the worktree, commit, Governance,
Quality, integration, and Review around that probabilistic implementation, but
the internal implementation process remains one semantic step.

This leaves several recurring failure modes:

- the agent infers intended structure from current structure, and cannot
  distinguish a deliberate seam from an accident of history;
- relevant project-wide constraints compete with unrelated prompt context;
- cross-capability and cross-surface effects are discovered late or missed;
- the agent decides its own implementation sequence without durable checkpoints;
- Governance detects violations only after the complete implementation attempt;
- reviewers see code and workflow evidence without a shared semantic account of
  what application concepts changed;
- a violated intention is corrected in one Round and forgotten, rather than
  becoming a durable constraint that holds for every later Goal.

Refine's Workflow already demonstrates the desired control pattern. It is an
executable ontology of delivery: Goals, Rounds, processes, candidates, evidence,
states, transitions, and review boundaries have explicit meaning, and agents
cannot bypass the deterministic engine that applies that meaning.

Two-stage planning extends the same pattern to implementation semantics:

```text
Workflow governs how implementation proceeds.
The plan model governs what a valid implementation means for this Goal.
Rules govern what must remain true across every Goal.
```

Decomposition quality is the mechanism. Implementation steps cut along declared
seams produce checkpoints that are individually coherent: a plan that fails at
step four of five leaves four complete, reviewable units and one unstarted
piece. Steps cut by file-layout intuition instead produce partial states no one
designed, which are harder to review than a single coherent failed attempt.
Stage one exists so that stage two has seams to cut along.

## Design Principles

### Stochastically Derived, Deterministically Enforced

This is the central axiom of the design.

Semantics are **derived and judged stochastically** — an LLM proposes concepts,
seams, granularity, and wording, and an LLM judges whether a change honors a
rule. Selecting the right concepts at the right level of detail, and deciding
whether prose intent was violated, are judgment problems that cannot be
programmed. No deterministic procedure extracts intent from a repository or
adjudicates it against a diff.

Process is **enforced deterministically** — the model and plan are pinned as
data, and everything structural about their execution is computed rather than
argued: step readiness and sequencing, evidence obligations, Git and worktree
observation, checkpoint creation, cancellation, recovery, immutability of
completed work, and workflow transitions. An agent cannot advance a step, mark
itself successful, skip a rule from its context, or alter what it was judged
against.

The distinction is precise and worth stating in the negative: **Refine is not
deterministic about verdicts. It is deterministic about process.** Whether a
change violates a rule is reasoned. Whether that reasoning was performed against
the pinned context, recorded with evidence, and permitted to advance state is
computed.

Both halves are required. Stochastic semantics without deterministic process is
a document nobody obeys. Deterministic process without stochastic semantics is a
schema nobody can fill in usefully. The value comes from the seam between them:
judgment good enough to be worth obeying, wrapped in machinery that cannot be
talked out of applying it.

### Durable Intent, Derived Structure

Anything a competent reader could conclude from the repository is derived, not
stored. Anything the repository cannot express is durable.

```text
Durable    Product, Constitution, Architecture, Rules
           choices, boundaries, prohibitions, rationale
           small, human-owned, slowly changing

Derived    plan model, step decomposition, Governance findings
           regenerated per implementation attempt
           pinned for the life of a plan, retained on its Round as evidence
```

A durable artifact that describes current structure competes with the source
code and loses, because the code is never stale. A durable artifact that
declares intended structure competes with nothing and cannot be derived, because
code cannot distinguish "this is how it is" from "this is how it must remain."

When Governance and the repository disagree, the default reading is that the
implementation is wrong. That inversion is why the durable half is worth
enforcing, and why its accuracy matters more than a descriptive model's would.

### Deterministic Outer Engine, Probabilistic Inner Actors

Agents propose plan models, implementation plans, step results, discrepancy
classifications, and rule proposals. Refine owns:

- schema validation;
- referential integrity;
- applicable-rule closure;
- plan and step readiness and state transitions;
- process ownership;
- Git observation and checkpoints;
- cancellation and recovery;
- durable evidence;
- Goal workflow transitions.

No agent response independently advances durable state.

### One-Way Handoff

Planning is where judgment lives. Once a plan is pinned, the engine reads it as
a program. Agents execute individual steps but never choose the sequence, decide
readiness, or declare success.

Replanning is the single re-entry point where control returns to the stochastic
side mid-execution, and it is bounded. Without a hard bound, deterministic
execution quietly degrades into continuous renegotiation and the property is
lost.

### One Authoritative Capability

Architecture, rule, planning, and execution behavior belongs to one shared
product capability. Web, CLI, API, MCP, Workflow, background operations, and
agents must use that capability rather than implementing their own parsers,
validators, mutation rules, or persistence.

### Rules Stay Prose And Are Judged, Not Validated

Rules remain plain-English statements, as they are today. They are not typed,
scoped, or given machine-readable enforcement metadata, and they are never
evaluated deterministically.

Rule evaluation is Governance judgment against an actual change — a stochastic
reading of prose by an agent that can see the diff. Dressing rules in a schema
would add machinery without changing that, and would falsely imply that a
verdict was computed when it was reasoned. Product, Constitution, and
Architecture are prose for the same reason; rules being the sole typed exception
would be the odd choice.

Two consequences, both simplifying:

- **Every rule goes into every relevant context.** With no scope field there is
  no selection step, so there is nothing for an agent to omit. Sending all rules
  is strictly stronger than computed closure over a scoped set, and the rule
  count is small enough that this is affordable.
- **Mechanical verification lives in Quality, not in rules.** Refine already has
  a registered Quality system for re-runnable checks. A constraint worth
  checking mechanically becomes a Quality check; the rule stays prose. Rules and
  checks reinforce each other without either absorbing the other's job.

### Focused Context

Each step receives the context relevant to its own work: its slice of the plan
model, its prerequisites' outcomes, every applicable rule, and the repository
locations it is likely to need.

The full plan model, the full Goal history, and prior step transcripts are not
repeated by default. A step that needs more may request it; a step cannot
decline a rule Refine determines is applicable.

Refine records why every record was selected, so a bad implementation can be
traced to bad context rather than guessed at.

### Evidence Before Conclusions

Plan-model claims, step outcomes, rule findings, and rule proposals cite
repository, Git, test, Quality, runtime, or human evidence. Explanatory prose
does not substitute for an observed result.

### Mitigation And Accountability

The design should increase implementation quality, evidence, and review pressure
without becoming a broad permission wall. Governance keeps the latitude it has
today to weigh a finding's seriousness rather than treating every rule as an
absolute gate.

## Goals

- Give Refine a durable, human-owned statement of intended system shape.
- Keep rules as plain English, and supply them wherever implementation decisions
  are made rather than only at final Governance.
- Derive a Goal-scoped structural model just-in-time, at the granularity the
  Goal requires.
- Decompose implementation along the seams that model declares.
- Execute the resulting plan deterministically, with durable state, evidence,
  checkpoints, cancellation, recovery, and bounded replanning.
- Give each step only the context relevant to its work.
- Show declared impact, observed impact, plan progress, and semantic changes in
  Review.
- Convert recurring discrepancies into durable rules, so a lesson learned in one
  Goal applies to the next.
- Preserve existing behavior when no Architecture is configured.
- Be useful with a one-paragraph Architecture and three rules.
- Measure whether two-stage planning materially improves candidate quality
  before investing further.

## Non-Goals

- Maintain a durable ontology graph, entity registry, or realization mapping.
- Type, scope, or schematize Governance rules, or evaluate them
  deterministically. Rules are prose and are judged by Governance.
- Replace or absorb the existing Quality system. Mechanical checks stay there.
- Describe the application as it currently is. Architecture states intent, not
  structure; it is not a code map or an architecture snapshot.
- Require a complete or comprehensive Architecture before the system is useful.
- Guarantee that the implementation satisfies Architecture and Rules at adoption
  time.
- Replace Git, tests, Quality, Governance judgment, or human Review.
- Treat an LLM-derived plan model or implementation plan as trusted execution
  authority.
- Store arbitrary executable shell commands in generated rules.
- Make a graph database, RDF store, OWL reasoner, or external ontology service a
  runtime dependency.
- Surface plan models or plan steps as Rounds, Goal statuses, or any other
  public workflow state. A plan is recorded on a Round as evidence; it is not a
  Round and does not behave like one.
- Carry a plan model forward as the starting point for a later Round's
  derivation.
- Convert the rest of Refine's web surface to React.
- Execute implementation steps concurrently in the same worktree in the first
  version.
- Let a rule change retroactively alter the evidence or outcome of a pinned
  plan.
- Require small or non-software Goals to use a multi-step implementation plan.

## Governance Model

Governance becomes:

```text
Governance (durable, human-owned, all prose)
├── Product         intended outcomes and users            (Markdown)
├── Constitution    principles every realization preserves (Markdown)
├── Architecture    intended shape of the system           (Markdown, sectioned)
└── Rules           constraints Governance judges          (plain-English list)
```

All four are concise, human-editable prose. None is a typed model.

The division is one of use, not of form:

- **Architecture is context.** Read during plan-model derivation and step
  execution. It shapes what the agent understands the system to be.
- **Rules are the standard.** Read by Governance when judging an actual change.
  They shape the verdict.

The same concern may appear in both, doing different jobs. An architectural
statement becomes a rule when it is worth judging every change against. Nothing
requires that every architectural statement have a rule, and most should not.

Mechanical verification remains the existing Quality system's job. Where a
constraint admits a re-runnable check, that check belongs in Quality; the rule
stays prose.

Governance remains configured when Product and Constitution are both present.
Architecture and Rules do not affect the existing `configured` meaning.

## Terminology

### Architecture

A concise Markdown statement of the target app's intended shape, in fixed
sections. Durable, human-owned, optional.

### Rule

A plain-English constraint every change is judged against. Durable,
human-owned, unchanged in shape from today's Governance rules.

### Plan Model

The Goal-scoped structural model produced by stage one of planning: the
concepts, relations, seams, and authority boundaries relevant to this
implementation attempt. Derived, pinned to a plan revision, retained on its
Round. Never workflow state, and never authoritative for a later Round's
derivation.

### Seam

A boundary in the plan model across which authority, ownership, or
responsibility changes. Seams determine where step boundaries should fall.

### Implementation Plan

A versioned, validated set of dependent implementation steps owned by one Goal
Round, one plan model, and one Governance revision.

### Implementation Step

A focused unit of inspection, change, reconciliation, or verification with a
model scope, prerequisites, success conditions, required evidence, attempts, and
durable outcome.

### Declared Impact

The concepts, seams, and rules the plan model identifies as affected before
implementation.

### Observed Impact

The impact supported by actual changed paths, agent reports, tests, Quality
results, and runtime observations after work occurs.

### Semantic Discrepancy

Durable evidence that observed behavior and declared intent do not align. The
default reading is an implementation violation.

## Canonical Data Model

### Governance Document

`governance.json` remains the authoritative Governance document:

```json
{
  "product": "Product intent in Markdown.",
  "constitution": "Constitution in Markdown.",
  "architecture": {
    "schema_version": 1,
    "revision": 3,
    "updated_at": "2026-07-27T18:00:00Z",
    "digest": "sha256:...",
    "sections": {
      "tech_stack": "Markdown.",
      "surfaces": "Markdown.",
      "persistence": "Markdown.",
      "business_logic": "Markdown.",
      "integrations": "Markdown.",
      "concurrency": "Markdown."
    }
  },
  "rules": [],
  "configured": true
}
```

`configured` remains derived rather than caller-controlled.

The server owns `schema_version`, `revision`, `updated_at`, and `digest`.
Provider output cannot set authoritative metadata.

### Architecture Document

Architecture is Markdown in fixed, named sections so that step context assembly
can select the relevant ones without parsing prose:

- `tech_stack`;
- `surfaces`;
- `persistence`;
- `business_logic`;
- `integrations`;
- `concurrency` (optional).

Every section is optional. An empty section is better than a padded one — a
fixed set of headings invites completionism, and Architecture is judged by
whether it says something non-obvious, not by whether it is complete.

Each section states decisions, boundaries, and prohibitions rather than
describing current structure:

| Description (rots) | Architecture (holds) |
|---|---|
| The web surface is static HTML with SSE | Live browser updates use SSE only; polling is prohibited |
| Workflow state lives in `src/workflow` | Only the workflow capability may transition Goal state |
| We use tldraw for the canvas | Canvas state is never authoritative; one pinned version only |

Section-specific guidance:

- **Tech stack** — the valuable half is the prohibitions. Chosen dependencies
  are inferable from manifests; "no React outside one bounded island" is not.
  Record what is forbidden and what is being migrated away from.
- **Surfaces** — which surfaces exist and what they are permitted to do.
  Surfaces quietly growing product logic is the failure this design targets.
- **Persistence** — high-level only. Not a schema dump, which is derivable.
  Record what is authoritative versus derived, durable versus cache, and which
  invariants hold across stores.
- **Business logic** — where authority lives: which capability owns which
  decision. For most systems this is the load-bearing section, and the one a
  repository is least able to state.
- **Integrations** — trust boundaries and failure modes, not an API inventory.
- **Concurrency** — process model, exclusivity, isolation, and recovery
  expectations, where these carry risk.

Architecture is versioned and digested so that a plan can pin the exact revision
it was derived against. It is edited as text; there is no structured mutation
API, diff review workflow, or command batch. It changes on the order of monthly,
by hand.

### Rule

Rules keep their existing shape:

```json
{
  "id": "rule-9",
  "text": "Use SSE as the only mechanism for live browser updates.",
  "created": "2026-07-27T18:00:00Z",
  "updated": "2026-07-27T18:00:00Z",
  "source": "generated"
}
```

No schema change, no migration, no scope, no effect, no enforcement metadata.
Refine validates that `text` is non-empty and that IDs are unique, and nothing
further. The constraint lives entirely in the prose.

Rules are supplied whole to plan-model derivation, to every implementation step,
and to post-implementation Governance. Because there is no scope field there is
no selection, and therefore no way for an agent to be handed an incomplete set.

Writing guidance, enforced by nothing but review:

- state the constraint and, where it is not obvious, why it exists — the reason
  is what lets a future reader decide whether the rule still applies;
- prefer constraints a Governance agent can check against a diff over aspirations
  it cannot;
- keep the list short enough that every rule can be read on every Goal.

### Evidence Reference

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
`test`, `quality`, `runtime`, `review`, and `human`.

An unavailable evidence reference never deletes or weakens a durable claim.
Intent does not expire because the code that satisfied it moved. It becomes an
evidence gap for Review.

## Proposing Architecture And Rules

### Bootstrap And Proposal

Refine can propose Architecture sections and rules by inspecting the repository
through an installed agent provider. Proposals are the on-ramp, not a
maintenance mechanism — they run at adoption and occasionally afterward, never
on the hot path of a Goal.

Both outputs are prose. Proposal generation produces text a user edits and
accepts; there is no structured model to review, diff, or apply.

Most teams cannot articulate their own architecture on demand, but nearly all
can recognize a good articulation of it and correct a flawed one. Proposal
generation exists to convert an authoring problem into a recognition problem.

Because generation runs rarely, it should be slow, multi-pass, and expensive.
Minutes and many provider invocations are acceptable for an artifact that shapes
every subsequent Goal.

The pipeline:

```text
1. Survey        map the repository: surfaces, capabilities, data flow,
                 module boundaries, test topology, naming conventions
2. Hypothesize   propose candidate constraints, each with a claim about what
                 must remain true and why it might be intended
3. Test          check each hypothesis against the repository: where does it
                 hold, where does it fail, how many sites
4. Classify      universal / partial / aspirational / refuted
5. Rank          by non-derivability, blast radius, and violation cost
6. Bound         emit the top N proposals; discard the tail
7. Present       per-proposal review with evidence both ways
```

Stage 3 is what separates this from a description generator, and stage 4 is
where the signal is.

**Partial regularity is the highest-value output.** A pattern holding in eleven
of thirteen surfaces is almost certainly intended structure plus two defects. A
pattern holding everywhere may be intent or may be coincidence. A pattern
holding nowhere is usually not intent. Refine surfaces partial regularities
first, states the exception sites explicitly, and asks which reading is correct
— that single question is the densest judgment a user supplies, and it produces
both a rule and a list of sites to fix.

Proposals are judged by parsimony first. Twenty constraints a user will read
beat two hundred they will not; coverage that is never reviewed is worse than
absent, because it carries apparent authority.

### Proposal Record

A proposal is the prose plus the evidence that justifies it. The record is
transient scaffolding for the review decision, not durable state:

```json
{
  "proposal_id": "prop-7",
  "target": "rule",
  "text": "Only the workflow capability may transition Goal state.",
  "why": "Every transition is funneled through one module, and two surfaces call
    it rather than reimplementing it.",
  "classification": "partial",
  "confidence": "high",
  "non_derivable_because": "The repository shows the pattern but cannot show
    whether the two exceptions are defects or intentional.",
  "holds_at": ["src/surfaces/web", "src/surfaces/cli"],
  "violated_at": ["src/surfaces/mcp/approve.rs"],
  "question_for_user": "Is the MCP path a defect to fix, or a deliberate
    exception to carve out?"
}
```

Accepting a rule proposal appends its `text` to the rules list. Nothing else
from the record persists — the evidence informed the decision and its job is
done. Accepting an Architecture proposal inserts its text into the named
section, where the user edits it like any other prose.

`violated_at` is the adoption cost, visible at decision time rather than
discovered on the next Goal. It is a list of places to fix or exceptions to
reconsider, not stored state that Refine later reasons about.

Review is per-proposal. Users accept, edit, or reject each one individually;
there is no whole-document diff, because a whole-document diff is exactly the
artifact users rubber-stamp. Rejections are durable, with reasons retained so
later runs do not re-propose settled questions.

Where evidence underdetermines intent, generation asks rather than guesses.
Questions are bounded, skippable, and never block; a skipped question yields a
lower-confidence proposal, not a fabricated answer.

### On Aspirational Rules

A newly accepted rule may describe something the codebase does not yet satisfy.
This matters less than it would in a static-analysis system, because Governance
judges the *change*, not the repository: a pre-existing violation the Goal did
not touch generally does not surface at all.

Where it does surface, the Governance agent has the diff and the rule and can
say so in its finding, and Review can accept the explanation. No baseline
tracking, debt counting, or ratchet mechanism is required. If the same
pre-existing violation is repeatedly flagged and repeatedly dismissed, the
signal is that the rule needs rewording — which is a prose edit.

## Two-Stage Planning

### Placement In Workflow

Planning and step execution are a subordinate state machine inside the existing
`InProgress` Workflow behavior.

They introduce no new public Goal statuses and no new workflow state. The Goal
is `InProgress` throughout. Only the existing Workflow may transition the Goal
to QA, Ready Merge, Failed, Cancelled, or any later state.

The plan is recorded on its Round as durable evidence, alongside the Round's
existing `governance`, `quality`, and `logs` artifacts. A plan is a thing a
Round produced, not a kind of Round: steps are never surfaced as Rounds, never
carry Round semantics, and never appear in any Round count.

One Goal Round owns one active implementation plan. Distinct Goals may continue
to execute concurrently. Steps within one plan execute sequentially in the first
version.

### Stage One: Plan Model Derivation

A planning agent derives the structural model this Goal requires. It receives:

- Product;
- Constitution;
- Architecture;
- all accepted rules;
- Goal identity and history, including all previous Rounds;
- previous Rounds' plans with their outcomes: the model derived, the steps cut,
  which succeeded and checkpointed, which failed and why;
- the current Round request;
- target-app repository access;
- the plan-model output schema.

Derivation is read-only. Provider adapters should enforce read-only planning
where supported. Refine verifies that planning did not mutate the worktree; an
unexpected mutation fails planning and is retained as evidence rather than
silently discarded.

The agent inspects the repository. Architecture and Rules are the durable
minimum, not a substitute for repository evidence — Architecture states what
must be true, and the repository shows what currently is.

### Prior Plans Are History, Not A Seed

Every Round derives its model fresh. Prior plans are supplied as evidence of
what was tried and what happened; a later Round does not start from an earlier
model and edit it.

The distinction is load-bearing. If Round 2 refines Round 1's model and Round 3
refines Round 2's, the result is a durable, incrementally maintained ontology
reassembled through the back door, without any of the review discipline that
would make one trustworthy. Fresh derivation is the property that keeps
granularity fitted to each attempt and keeps a wrong model from outliving the
Round that produced it.

What carries forward is the *outcome*, which is why bare prior models are less
useful than annotated ones. "Steps one through three checkpointed clean; step
four failed twice on a dependency the model did not represent" tells the next
planner something. The model alone does not, and a model from a failed Round is
often wrong in exactly the way that caused the failure.

Refine therefore supplies prior plans with per-step outcomes, failure detail,
checkpoint SHAs, and any recorded discrepancies attached. Where Goal history is
long, older Rounds reduce to outcome summaries rather than full models, so
context stays bounded as attempts accumulate.

This gives within-Goal accumulation through Round history and leaves cross-Goal
accumulation to rules. The split is deliberate: a Goal's own failed attempts are
specific enough to replay in detail, while anything worth carrying to a
different Goal must be general enough to state as a rule.

### Plan Model

```json
{
  "schema_version": 1,
  "entities": [
    {
      "id": "capability.workflow",
      "kind": "Capability",
      "name": "Workflow",
      "description": "Coordinates durable Goal state advancement.",
      "locations": ["src/workflow"]
    }
  ],
  "relations": [
    {
      "id": "relation.web-delegates-workflow",
      "type": "delegates_to",
      "from": "surface.web",
      "to": "capability.workflow"
    }
  ],
  "seams": [
    {
      "id": "seam.workflow-authority",
      "name": "Goal state transition authority",
      "description": "Only the workflow capability may transition Goal state.",
      "boundary_between": ["surface.web", "capability.workflow"]
    }
  ],
  "declared_impact": {
    "entity_ids": ["capability.workflow"],
    "seam_ids": ["seam.workflow-authority"],
    "rationale": "The Goal changes workflow state advancement."
  }
}
```

Properties of the plan model:

- IDs are stable within one plan and carry no meaning outside it;
- it is scoped to this Goal, at whatever granularity the Goal requires;
- `locations` are hints for context assembly, not claims that a constraint
  holds;
- seams are the output stage two consumes; an entity that participates in no
  seam and no declared impact is probably noise.

Granularity is a stage-one judgment. A settings-field Goal produces a small,
shallow model; a state-ownership refactor produces a deeper one. Neither is
wrong, and no fixed project-wide granularity would suit both.

### Plan Model Validation

Before decomposition, Refine validates:

- schema conformance and unique IDs;
- referential integrity of relations, seams, and declared impact;
- Architecture revision and digest;
- base commit;
- declared impact is non-empty for mutating Goals.

Validation is structural only. Refine checks that the model is internally
coherent and pinned to the right inputs. Whether the model is a *good* reading of
the system is a judgment question, answered by whether the resulting
implementation survives Governance and Review — not by a validator.

There is no applicable-rule computation. All rules are attached to the pinned
plan in full, so there is no selection step that could omit one.

### Stage Two: Decomposition

The same planning session decomposes the work into steps, cut along the seams
stage one declared.

```json
{
  "schema_version": 1,
  "id": "plan-...",
  "revision": 1,
  "status": "active",
  "goal_id": "GOAL1",
  "round": 2,
  "base_commit": "...",
  "governance_revision": 3,
  "governance_digest": "sha256:...",
  "provider": "codex",
  "created_at": "2026-07-27T18:00:00Z",
  "model": {},
  "steps": []
}
```

The plan model is a field on the plan, versioned with the plan revision. During
execution, nothing outside the plan reads it.

The plan itself is recorded on its Round, following the existing `governance`
and `quality` pattern:

```json
{
  "reporter": "...",
  "prompt": "...",
  "governance": {},
  "quality": {},
  "plan": {
    "plan_id": "plan-...",
    "revisions": [],
    "model": {},
    "steps": [],
    "outcome": "failed",
    "failed_step_id": "step.persistence",
    "checkpoints": ["sha", "sha"],
    "discrepancies": []
  },
  "logs": []
}
```

`plan` is nullable, exactly as `governance` and `quality` are. A Round that ran
legacy one-shot implementation has none, and old Round records remain valid
without migration.

Later Rounds of the same Goal read this field as history. Nothing else does.

Server-owned plan status values: `planning`, `active`, `replanning`,
`completed`, `failed`, `cancelled`.

### Step Model

```json
{
  "id": "step.shared-capability",
  "kind": "implement",
  "objective": "Implement the behavior in the shared Workflow capability.",
  "depends_on": [],
  "model_scope": {
    "entity_ids": ["capability.workflow"],
    "seam_ids": ["seam.workflow-authority"]
  },
  "expected_areas": ["src/workflow"],
  "success_conditions": [
    "The shared capability owns the behavior.",
    "Existing surfaces use the shared behavior."
  ],
  "required_evidence": ["focused tests", "changed-path observation"],
  "status": "pending",
  "attempts": [],
  "checkpoint_commit": null
}
```

Step kinds:

- `inspect`: establish repository or runtime facts without intended mutation;
- `implement`: add or change behavior;
- `reconcile`: correct an earlier step, integrate discoveries, or resolve a
  discrepancy;
- `verify`: gather focused behavioral evidence.

`expected_areas` guide investigation and review. They do not prohibit justified
changes elsewhere.

Server-owned step statuses: `pending`, `ready`, `running`, `needs_input`,
`succeeded`, `failed`, `cancelled`, `superseded`.

### Plan Validation

Before execution, Refine validates:

- Goal, Round, base commit, Governance revision, and digest;
- all referenced plan-model IDs;
- unique plan and step IDs;
- acyclic step dependencies;
- bounded step count and description sizes;
- declared-impact coverage across steps;
- success conditions and evidence obligations;
- no requested Goal transition, approval, merge, or other unavailable authority;
- at least one verification path for mutating plans.

### Adaptive Granularity

Step size is a planning output, not an engine parameter. There is no minimum
decomposition and no uniform granularity.

A trivial Goal gets one step, and that is a correct plan rather than a
degenerate one. A one-step plan degrades gracefully into one-shot execution with
a derived model and explicit success conditions — which is a real improvement
over today's behavior and does not force decomposition onto work that does not
need it.

Initial defaults should favor:

- one step for trivial or non-software work;
- two to four steps for ordinary feature work;
- additional explicit verification or reconciliation steps for high-risk,
  cross-capability, persistence, concurrency, process, Git, migration, or
  cross-surface changes.

Planning limits prevent both a single unbounded step for a high-impact Goal and
pathological micro-steps.

### Sizing Heuristics

Two constraints govern where step boundaries fall:

**Seam alignment.** Boundaries should land on seams the plan model declared. A
step that completes one side of an authority boundary leaves a checkpoint that
is coherent on its own: reviewable, testable, and a valid stopping point if the
plan later fails. A step cut across a seam leaves a half-migrated state no one
designed.

**Redo cost.** A checkpoint lands at each step boundary, so step length sets
recovery granularity — a failed step loses everything since the last checkpoint.
A step should be a unit of work you would be willing to lose and repeat.
Seam-coherent units tend to also be the ones cheap to redo in isolation, so the
two heuristics usually agree; where they conflict, prefer the smaller step.

Plan validation warns when a mutating step partially covers a seam it touches,
or spans entities with no declared relation. These are warnings rather than
failures, because legitimate work sometimes crosses seams. Crossing one silently
is the problem.

## Deterministic Implementation Engine

### Execution Cycle

```text
select next dependency-ready step
  -> assemble step context
  -> launch one managed native Goal Agent session
  -> receive step completion claim or needs-input signal
  -> observe Git, paths, tests, Quality, and process evidence
  -> validate observed impact against declared impact
  -> evaluate step obligations and applicable rules
  -> checkpoint success, replan, retry, or fail
```

The engine, not the agent, marks a step succeeded.

### Step Context

Each step receives only what its own work requires:

- Goal identity and the current Round request;
- the step objective, prerequisites, and success conditions;
- relevant summaries from completed prerequisite steps, not their transcripts;
- the step's slice of the plan model — its entities, the seams it touches, and
  the relations between them;
- the complete rule list;
- the Architecture sections relevant to the step's scope;
- relevant Guidance candidates;
- likely repository locations from the plan model, marked as hints;
- current branch, worktree, base commit, and latest checkpoint;
- the step completion protocol.

The full plan model, the full Goal history, full Architecture, and prior step
transcripts are not included by default.

Rules are the exception to focusing: the complete list goes to every step. They
are prose with no scope, the list is short, and any attempt to select a subset
would reintroduce the possibility of dropping the one that mattered.

Context assembly records the selection reason for every included record, so a
failed step can be diagnosed as a context problem rather than assumed to be a
capability problem. A step may request additional model records or Architecture
sections; Refine validates and expands.

### Agent Sessions

Each step uses a fresh managed native provider session by default. This provides
context isolation and prevents accumulated conversation from defeating focused
context assembly.

The user experience remains one logical Goal Agent:

- CLI and web "Open Agent" actions attach to the currently active step;
- process metadata includes Goal ID, Round, plan ID, plan revision, and step ID;
- step transcripts aggregate under the Round;
- exactly one step session may own a Goal at a time;
- closing a surface detaches without stopping work;
- explicit `needs_input` preserves the same active step session and workflow
  claim until answered.

The engine reuses installed provider CLIs and their native TUI behavior. It does
not rebuild the agent interaction experience.

### Step Completion Contract

```json
{
  "state": "completed",
  "plan_id": "plan-...",
  "plan_revision": 1,
  "step_id": "step.shared-capability",
  "message": "Implemented the shared behavior and ran focused tests.",
  "observed_impact": {
    "entity_ids": ["capability.workflow"],
    "seam_ids": ["seam.workflow-authority"]
  },
  "verification": [
    {"command": "cargo test ...", "outcome": "passed", "evidence": "..."}
  ],
  "guidance_applied": [],
  "discrepancies": [],
  "plan_amendment_requested": false
}
```

The engine rejects wrong plan or step identity, unknown plan-model IDs,
malformed evidence, invalid Guidance selection, or a stale plan revision.

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
- declared versus observed impact.

Everything in that list is a mechanical fact. The engine gathers facts; it does
not judge whether a rule was honored.

Rule judgment is a Governance-agent activity and may run at milestones when its
cost is proportional to step risk. Registered Quality checks run according to
existing Quality timing. Full configured Governance and Quality still run after
the entire plan, and that final pass remains the authoritative rule evaluation.

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

Replanning is the only point at which control returns to the stochastic side
during execution. A step may request it because of:

- an unexpected dependency;
- a seam the plan model missed or described incorrectly;
- an invalid assumption;
- failed required evidence;
- a need for reconciliation.

Replanning receives the original plan and plan model, immutable completed-step
outcomes, current worktree and checkpoint state, discrepancies, and remaining
Goal obligations.

Plan revisions are append-only. A revision may add, change, reorder, or
supersede pending steps, and may revise the plan model. It cannot rewrite
completed steps, their evidence, their checkpoint commits, or the model version
they were judged against. Each step records the plan revision that governed it.

Refine validates every revision, recomputes applicable-rule closure, and
re-pins. Replanning has bounded attempts; exceeding the configured bound fails
the implementation while preserving all evidence.

### Failure

A failed step records:

- attempt number;
- provider and process identity;
- error and phase;
- worktree observation;
- partial changed paths;
- verification outcomes;
- applicable rules and their findings;
- transcript and logs;
- whether retry or replan is available.

Refine never reports plan or Goal success because an agent produced a plausible
completion message.

Partial work remains in the retained implementation worktree. Existing recovery
Round behavior remains available after Goal failure.

### Cancellation

Explicit Goal cancellation is terminal and has precedence over plan progression.

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
2. It reconciles declared and observed impact.
3. It records the complete implementation report from step outcomes rather than
   relying on one final agent narrative.
4. It runs full post-implementation Governance against the pinned Governance
   revision.
5. It advances to configured Quality timing only when Governance passes.
6. Existing Ready Merge, build, post-build Quality, and human Review behavior
   continues unchanged.

The implementation engine cannot integrate, approve, merge, mark Review
accepted, or move the Goal directly to Done.

## Governance Findings

```json
{
  "rule_id": "rule-9",
  "message": "The web surface introduced product semantics outside the shared capability.",
  "plan_id": "plan-...",
  "step_id": "step.web-adapter",
  "model_ids": ["surface.web", "seam.workflow-authority"],
  "evidence": [
    {
      "kind": "path",
      "reference": "src/surfaces/web/...",
      "claim": "Contains the new eligibility decision."
    }
  ]
}
```

Governance judges actual changes against the rules pinned to the plan. It must
not report preferences, hypothetical risks, or violations of rules adopted after
pinning.

A finding is a reasoned claim, not a computed result. What the engine guarantees
is that the judgment happened against the pinned rules and the observed diff,
that it cites evidence, and that its verdict is recorded — not that the verdict
is correct. Review remains the check on the judgment itself.

Where a finding concerns a pre-existing condition the Goal did not introduce,
the Governance agent should say so in the finding. Refine does not track this as
structured state.

The engine records rules supplied, provider used, evidence cited, and result.

`model_ids` are plan-scoped and meaningful only alongside the retained plan
model. Findings that need to outlive the plan cite rule IDs, paths, and commits.

## Review

Review adds semantic evidence alongside existing code and workflow evidence:

- Governance revision and digest;
- the pinned plan model and its declared impact;
- plan revisions and the model version governing each step;
- step statuses and checkpoint commits;
- observed impact;
- intended versus observed diff;
- the pinned rules and the findings against them;
- discrepancies and their dispositions;
- proposed rules arising from this Goal;
- code diff, Quality, build, Git, and existing Governance evidence.

Review may accept the integrated implementation through the existing approval
path, request a new implementation Round, accept or reject proposed rules,
record a follow-up Goal, or fail or cancel through existing actions.

Accepting an implementation does not implicitly accept a proposed rule.
Accepting a rule does not implicitly accept an implementation.

## Discrepancies And Rule Evolution

This is the only durable feedback path. Without it, each Goal re-derives its
model from the same sources and can make the same mistake indefinitely.

```json
{
  "id": "discrepancy-...",
  "goal_id": "GOAL1",
  "round": 2,
  "plan_id": "plan-...",
  "step_id": "step.web-adapter",
  "classification": "implementation_violation",
  "expected": "Workflow transitions are evidence-backed.",
  "observed": "A transition occurred before evidence persistence.",
  "evidence": [],
  "proposed_response": "rule_proposal"
}
```

Classifications:

- `implementation_violation`: correct the implementation. **This is the
  default.**
- `rule_gap`: a constraint exists in intent but was never written down; propose
  a rule;
- `rule_error`: an accepted rule is wrong; propose a correction;
- `architecture_gap`: Architecture failed to state something a correct
  derivation needed;
- `model_error`: stage one derived the structure incorrectly; a planning-quality
  signal, not a Governance change;
- `evidence_gap`: gather evidence without changing intent.

The default matters. `rule_error` requires explicit justification rather than
being the path of least resistance — otherwise every inconvenient constraint is
reclassified into nonexistence.

`model_error` changes no durable Governance, but it is not lost. It is recorded
on the Round's plan, so the next Round of the same Goal sees both the model that
was wrong and the discrepancy explaining how — which is precisely the history
that keeps a second attempt from repeating the first one's structural mistake.

A pattern of `model_error` across different Goals is a different signal: it
indicates Architecture is underspecified, which is an `architecture_gap` and
does have a durable target.

A discrepancy that becomes a rule produces one sentence appended to the rules
list, reviewed like any other proposal. That is the entire durable feedback
mechanism, and its cheapness is the point: a lesson costs a line of prose, so
recording one is never the expensive option.

## Surfaces

### Web

Governance settings retain Product and Constitution and add:

- an Architecture section with the fixed headings, edited as Markdown;
- the existing Rules list, unchanged in shape and editing behavior;
- a proposal review queue with per-proposal accept, edit, and reject;
- empty-state explanation framing Architecture as intent, not documentation.

Architecture is six textareas and Rules is the list that exists today. Neither
needs a structured editor, a scope picker, or a validation panel, and adding one
would misrepresent prose as data.

Goal detail may summarize active plan progress. The plan model and step evidence
appear in plan/step evidence views and logs — debuggable when you go looking,
invisible when you are not. Neither appears as Round or Goal state.

The surface must be genuinely useful with one Architecture paragraph and two
rules. Nothing should imply that Governance is invalid, unfinished, or unready
because it is small.

### API, CLI, And MCP

All surfaces expose the same shared operations.

```text
refine governance architecture show
refine governance architecture edit
refine governance rules list
refine governance propose [--scope <path>]
refine governance proposals
refine governance accept <proposal-id>
refine governance reject <proposal-id> --reason <text>
refine goal plan <goal-id>
refine goal plan <goal-id> --model
refine goal plan <goal-id> --steps
```

CLI output supports structured JSON for agents and programs. MCP tools delegate
to the same service and return the same typed contracts.

Surfaces never receive a private bypass that writes `governance.json` directly.

### Visual Projection

Deferred. If built, a plan model renders as a read-only diagram attached to plan
evidence: entities, relations, seams, and step boundaries against them.

Because the plan model is derived rather than authored, there is no editing
surface, no typed command protocol, no layout persistence, and no stale-revision
conflict handling. It renders what a Round did; it is not a model anyone edits. This is a rendering, not an application, and it must not become the
justification for introducing React or an external canvas dependency into the
web surface.

## Prompt Templates

- `governance-survey.md`: map repository structure for proposal generation;
- `governance-hypothesize.md`: propose candidate Architecture and rules;
- `governance-test.md`: check each hypothesis against the repository;
- `governance-propose.md`: rank and emit bounded proposals;
- `plan-model.md`: stage one — derive the Goal-scoped structural model;
- `plan-steps.md`: stage two — decompose along seams;
- `plan-step-execute.md`: execute one focused implementation step;
- existing post-implementation Governance template extended with the pinned plan
  model and observed impact.

A representative contract for stage one:

```markdown
Derive the structural model needed to implement this Goal. Return only JSON
conforming to schema version {{schema_version}}.

Model only what this Goal requires, at the granularity this Goal requires.
Identify seams: boundaries across which authority, ownership, or responsibility
changes. Seams determine where implementation steps will be cut.

Architecture states what must be true. The repository shows what currently is.
Where they disagree, the implementation is wrong unless you have evidence
otherwise; record the disagreement rather than modelling around it.

Derive this model fresh. Previous plans tell you what was attempted and how it
turned out — reuse what the outcomes taught you, not the structure they assumed.
A model from a failed attempt is often wrong in the way that caused the failure.

Do not edit files.

Product: {{product}}
Constitution: {{constitution}}
Architecture: {{architecture}}
Rules: {{rules}}
Goal: {{goal}}
Previous rounds: {{rounds}}
Previous plans and outcomes: {{prior_plans}}
Current request: {{request}}
Repository: {{target_root}}
```

The Rust model and validator define the schema. Prompts do not become a second
schema authority.

Prompt rendering tests enforce:

- all required variables are used;
- no undeclared variables are accepted;
- output schema identity is explicit;
- Product, Constitution, Architecture, Goal, Round, plan, and step identities
  are correctly pinned;
- current mutable Governance is never substituted for plan-pinned context.

## Migration And Compatibility

### Existing Governance Rules

Rules do not migrate. Their shape, IDs, text, timestamps, source field,
normalization, and editing behavior are unchanged, and existing rules continue
to work exactly as they do today.

The only change is where they are consumed: rules are now also supplied to
plan-model derivation and to every implementation step, in addition to
post-implementation Governance.

The one addition worth making is to `generate_rules`, which currently returns
two hardcoded placeholder rules. Proposal generation replaces it with rules
actually derived from the repository. That is a behavior improvement inside the
existing contract, not a schema change.

### Governance Document

Adding `architecture` to `governance.json` is additive. The loader accepts
documents without it, and `configured` continues to depend only on Product and
Constitution.

### Existing Goals And Rounds

- Existing Goal records remain valid.
- Existing pinned context version 1 remains readable and immutable.
- New planned Rounds use a new context version.
- A Round without plan context continues through legacy one-shot implementation.
- The first rollout allows a setting-controlled choice between one-shot and
  planned execution for comparison and rollback.

### Existing Surfaces

Product, Constitution, and `rules` keep their wire meaning and their read and
write behavior. Surfaces gain an `architecture` field and the proposal
operations; nothing existing changes shape.

## Security And Trust Boundaries

- Provider output is untrusted input.
- Derivation and planning do not gain merge, approval, Goal transition, or
  arbitrary state-write authority.
- Generated Governance content is prose and is treated as prose. It is never
  parsed into commands, checks, or executable content, which removes an entire
  class of injection surface that typed enforcement metadata would have
  introduced.
- Proposals cannot write Governance. Acceptance is an explicit user action.
- Planning is read-only and unexpected worktree mutation is an error.
- Step agents retain normal implementation authority only inside the isolated
  Goal worktree.
- Raw provider output is retained as potentially sensitive operation evidence
  according to existing redaction and export rules.
- API and remote-browser mutation origin rules apply to Governance mutations.

## Observability

Plan and step activity emits structured logs with:

- operation, Goal, Round, plan, plan revision, step, and process identity;
- provider;
- Governance revision and digest;
- plan model digest and the model version governing each step;
- state transition;
- selected model records and Architecture sections, with reasons;
- rule findings and their evidence;
- checkpoint commits;
- cancellation, interruption, retry, and replan details;
- validation or conflict errors.

Derivation inputs are recorded so a plan model is reproducible: Governance
digest, base commit, and Goal history version.

Dashboard and Goal detail may summarize active plan progress, but durable Round,
operation, process, Git, and Governance state remain authoritative.

A completed plan is readable from its Round alongside that Round's Governance
and Quality records. This is what makes a failed attempt debuggable after the
fact and what later Rounds read as history. It does not make plans or steps
workflow state: nothing transitions on them, and no surface presents them as
Rounds.

Provider or authentication failure must be visible as such and must not be
reported as a validation failure, plan failure, or workflow-capacity problem.

## Efficacy Gate

The first stage tests the product hypothesis before further investment.

Hand-author a compact Refine Architecture and a small rule set, then run
representative historical Goals through:

1. current one-shot implementation;
2. one-shot with a derived plan model as context;
3. two-stage planned step execution.

Use the same starting commit, model, provider configuration, authority, and
budget. Repeat conditions enough to distinguish a consistent effect from one
provider sample. Reviewers evaluate candidates without knowing the condition.

Hand-authoring Architecture is the point, not a shortcut. It isolates the
execution hypothesis from proposal-generation quality, which is measured
separately. If planned execution does not help with Architecture authored by the
person who knows the system best, better generation will not rescue it.

Primary measures:

- material semantic or architectural defects;
- first-candidate acceptability;
- missed affected capabilities or surfaces;
- corrective Rounds required;
- behavioral and failure-path coverage;
- unnecessary changes;
- reviewability of partial state when a plan fails mid-execution, compared with
  a coherent one-shot failure.

Secondary measures:

- elapsed time;
- provider invocations;
- token or budget usage where available;
- plan-model and decomposition quality as judged at Review;
- reviewer effort.

Longer implementation reports and richer Review output are not success measures
by themselves.

### Thresholds

Fixed before the first run, not after seeing results:

- **Quality:** planned execution must materially reduce material defects or
  corrective Rounds versus one-shot, consistently across repeated runs.
- **Cost:** record the maximum acceptable multiple of provider invocations and
  token spend per Goal before running. Cost is a gating measure — fresh sessions
  per step pay repository-orientation cost N times, and users pay for it
  directly. An unbounded "acceptable increase" is not a gate.
- **Failure shape:** mid-plan failure must not produce partial state that
  reviewers judge worse than a one-shot failure. Seam alignment is the
  mechanism; this measures whether it works.

Arm 2 matters independently. If a derived plan model as context captures most of
the benefit without decomposition, that is a far cheaper product and the correct
thing to ship.

Record thresholds, experiment design, and exact results before changing the
gate.

## Implementation Order

### Phase 0: Efficacy Prototype

- Hand-author a compact Refine Architecture and a few rules.
- Add typed plan-model and plan schemas behind a feature gate.
- Add a sequential step executor using existing provider and session
  capabilities.
- Fix thresholds, then run and record the efficacy comparison.
- Stop or revise if implementation outcomes do not improve.

### Phase 1: Governance Foundation

- Add the Architecture document, sections, revision, and digest.
- Leave rules untouched; extend only where they are consumed.
- Preserve no-Architecture behavior and old Round compatibility.
- Add CLI and API read/validate coverage.

This phase is deliberately small. Governance gains one prose document and no new
data model.

### Phase 2: Two-Stage Planning

- Add `plan-model.md` and `plan-steps.md`.
- Add plan-model derivation, validation, and applicable-rule closure.
- Add plan validation, pinning, and durable plan revisions.
- Keep one-step compatibility for small Goals.
- Expose plan state in Goal detail and process metadata.

### Phase 3: Deterministic Step Execution

- Add `plan-step-execute.md` and structured step completion.
- Add focused step context assembly with recorded selection reasons.
- Launch fresh managed native sessions per step.
- Add deterministic observation, checkpoints, replanning, retry bounds,
  cancellation, and restart recovery.
- Aggregate step evidence into the implementation report.
- Preserve final Governance, Quality, integration, and Review boundaries.

### Phase 4: Proposal Generation

- Add the staged proposal pipeline and its prompt templates.
- Add hypothesis testing and universal/partial/aspirational classification.
- Add proposal records with evidence both ways and violation sites.
- Add per-proposal accept, edit, reject, and durable rejection memory.
- Add interview mode for underdetermined intent.
- Replace the hardcoded `generate_rules` placeholders.
- Add proposal quality metrics.
- Evaluate against repositories other than Refine before proceeding.

### Phase 5: Governance-Aware Review

- Extend findings with rule, plan, step, and evidence references.
- Reconcile declared and observed impact.
- Add discrepancy disposition, plan history, model versions, and checkpoint
  evidence to Review.
- Add the discrepancy-to-rule-proposal loop.
- Ensure rule acceptance and implementation approval remain separate.

## Verification Strategy

### Governance Model And Validation

- Architecture sections present, absent, empty, and unknown;
- Architecture revision, digest stability, and normalization;
- documents with no `architecture` key load unchanged;
- `configured` unaffected by Architecture presence or absence;
- existing rule normalization, IDs, and round-trip behavior unchanged;
- existing Governance read and write contracts unchanged.

### Proposal Generation

- concise prompt contract per pipeline stage;
- proposal bounding and ranking;
- accepting a rule proposal appends prose and persists nothing else;
- accepting an Architecture proposal writes into the named section;
- per-proposal accept, edit, and reject;
- rejection memory suppresses re-proposal across runs;
- interview questions are skippable and never fabricate an answer;
- malformed and prose-wrapped provider output;
- provider authentication failure;
- no static fallback;
- operation cancellation, interruption, retry, and result retention.

### Plan-Model Derivation

- valid shallow and deep models for differently scoped Goals;
- referential integrity of relations, seams, and declared impact;
- the complete rule list is attached to every pinned plan;
- prior plans reach the planner with per-step outcomes and failure detail;
- a later Round's model is derived fresh rather than seeded from an earlier one;
- long Goal histories reduce older plans to outcome summaries within budget;
- read-only enforcement and worktree mutation detection;
- exact Governance revision, digest, and base commit pinning;
- no-Architecture behavior;
- derivation reproducibility from recorded inputs.

### Planning

- valid one-step and multi-step plans;
- unknown plan-model references;
- cyclic dependencies;
- seam-coherence warnings for steps partially covering a boundary;
- declared-impact coverage across steps;
- forbidden authority requests;
- plan revision history and model versioning;
- completed-step immutability across model revision;
- bounded replanning.

### Step Execution

- focused context assembly and recorded selection reasons;
- every step receives the complete rule list;
- step-requested context expansion;
- one active step per Goal;
- distinct Goals executing concurrently;
- attach and detach from the active native session;
- valid completion and explicit needs-input;
- wrong plan, revision, or step signal;
- observed-impact validation;
- changed-path reconciliation;
- checkpoint creation and final candidate selection;
- no-change and non-software steps;
- provider failure and partial work retention.

### Cancellation And Recovery

- cancellation before launch, during derivation, during decomposition, during a
  step, during verification, and between steps;
- late completion after cancellation cannot restart or succeed the plan;
- process-settlement failure;
- restart with running, exited, interrupted, and missing processes;
- checkpoint/worktree divergence;
- duplicate-runner exclusion;
- retained worktree, transcripts, attempts, and evidence.

### Governance And Review

- findings cite valid rule IDs and actual evidence;
- Governance changes after pinning cannot alter a pinned plan's evaluation;
- `implementation_violation` is the default discrepancy classification;
- `rule_error` requires explicit justification;
- `model_error` produces no Governance change;
- discrepancy-to-rule-proposal linkage;
- rule acceptance independent from implementation approval;
- final full Governance and Quality still gate progression.

### Workflow Boundaries

- no new Goal status or public workflow state is introduced;
- the Round `plan` field is nullable and old Round records load unmigrated;
- a Round that ran legacy one-shot implementation has no plan;
- plan steps never appear as Rounds or in any Round count;
- the engine cannot integrate, approve, merge, or advance the Goal.

### Surface Parity

- CLI, API, MCP, and web use the shared service;
- equivalent validation errors and revision conflicts;
- thin adapters contain no Governance business rules;
- Smoke AI can deterministically generate a plan model, a plan, step results,
  discrepancies, and Governance verdicts for integration coverage.

## Acceptance Criteria

- Governance has Product, Constitution, Architecture, and Rules as its
  authoritative, durable model.
- Architecture states intent in fixed Markdown sections and is useful at one
  paragraph.
- Semantics are derived and judged stochastically; process is enforced
  deterministically. Refine is deterministic about state, evidence, and
  transitions, never about verdicts.
- Rules remain plain English, unchanged in shape, and are never validated
  deterministically.
- No durable ontology graph, entity registry, or realization mapping exists.
- Plan models are derived per implementation attempt and pinned to a plan
  revision.
- Each Round records its plan, model, step outcomes, and checkpoints as durable
  evidence, in the same way it records Governance and Quality.
- Later Rounds of a Goal read prior plans as outcome-annotated history and
  derive their own model fresh, never seeded from an earlier one.
- Plans and steps introduce no Goal status or other public workflow state, and
  are never surfaced as Rounds.
- Existing rules, and every existing Governance read and write contract,
  continue to work without migration.
- Every step and the planner receive the complete rule list, so no selection
  step exists that could drop one.
- Refine works normally with no Architecture configured.
- Rule and Architecture proposals are reviewed individually, never as a
  whole-document diff, and rejections are not re-proposed.
- Planning emits a validated model and a plan cut along its seams; once pinned,
  the engine executes the plan as a program.
- Step size varies with complexity and redo cost; a one-step plan is valid.
- Replanning is the only mid-execution return to stochastic control, and it is
  bounded.
- Each step receives only the context relevant to its work, with recorded
  selection reasons.
- Successful mutating steps create auditable checkpoints.
- Replanning preserves completed history and the model version that governed it.
- Cancellation and restart recovery remain monotonic, durable, and
  process-aware.
- Final Governance, Quality, Ready Merge, build, and human Review boundaries
  remain intact.
- Findings and Review cite rules, plan steps, and actual evidence.
- Governance changes cannot retroactively alter a pinned plan's contract or
  outcome.
- Discrepancies convert into durable rule proposals, so a lesson from one Goal
  applies to the next.
- Efficacy results meet pre-registered quality, cost, and failure-shape
  thresholds before further investment.
