# Mission Specification

Status: implemented through consolidation (agent phases, Goal materialization, wave admission, contribution settlement, reconciliation, synthesis, Quality, Governance, and the two-commit Outcome read-back); fleet distribution compilation and Mission prompt-template browser surfaces remain
Scope: Mission model, workflow, Goal composition, durable artifacts, and Refine surface changes
Target: one attached Target App and its existing Git-backed Refine state

## Summary

A Mission is a larger goal semantically and a container for Goal workflows behaviorally. It is not a larger `Goal` record and it does not replace Goal workflow.

The Mission owns a holistic outcome. It investigates the target system, establishes shared context, plans waves of Goals, gives each Goal a pinned and relevant view of that context, receives findings and evidence back from completed Goal work, reconciles what the Mission has learned, and judges the combined result. Its final product is an immutable Outcome manifest containing exact target-app changes, durable meta-code artifacts, criteria results, and provenance.

The central loop is:

```text
Mission snapshot N
        |
        | compile a scoped, immutable capsule
        v
Goal Round -> Plan -> Implement -> Quality -> Governance -> Review
        |
        | code evidence plus structured findings
        v
Mission reconciliation -> Mission snapshot N+1
```

Goals remain useful outside Missions. A standalone Goal follows today's workflow without Mission fields, Mission context, or Mission contribution requirements.

Subtraction is a governing design rule. The first version adds one new aggregate root, `Mission`. Plans, waves, inputs, findings, artifacts, snapshots, reviews, and outcomes are records owned by Mission or existing Goal Rounds; they are not independent mutable products.

## Product goals

Mission should:

- preserve system-level intent while many Goals execute locally;
- let Goal workflows compose without letting sibling Goals mutate shared state;
- make research, design, decisions, risks, verification, and documentation durable inputs to later work;
- adapt later waves when earlier Goals reveal new facts;
- evaluate cross-Goal coherence rather than equating child completion with outcome success;
- use the existing fleet, Goal workflow, agent invocation, Quality, Governance, operations, activity, and persistence-sync capabilities;
- publish an exact, reusable Outcome in Git-backed Refine state;
- let a later Mission consume one exact published Outcome from an earlier Mission;
- present the same semantics through browser, CLI, HTTP API, MCP, and installed-agent surfaces.

## Non-goals

The first version does not add:

- a generic persisted `WorkItem` superclass;
- storage inheritance between Mission and Goal;
- nested child Missions;
- a Mission-to-Feature entity;
- a Mission-to-Goal join table;
- a mutable dependency graph;
- a mutable shared blackboard;
- a second code-integration path;
- direct agent writes to live Refine state;
- floating dependencies on another Mission's latest result;
- a separate hosted control plane;
- arbitrary cross-Target-App execution;
- user-configurable automation policy for every Mission gate;
- a separate top-level Artifacts surface;
- a surface-specific Mission workflow implementation.

## Relationship to existing concepts

Refine should expose three recognizable work concepts:

| Concept | Meaning | Workflow |
| --- | --- | --- |
| Mission | A governed system-level outcome | Own composite lifecycle |
| Feature | A named grouping and linear ordering of Goals | Derived rollup only |
| Goal | The smallest useful independently governed unit of work | Own leaf lifecycle |

A Goal may belong to one Mission and one Feature at the same time.

Feature answers: "How are these Goals grouped and ordered?"

Mission answers: "Why does this work exist, what shared model guides it, what did the work teach us, and did the combined result achieve the outcome?"

The existing model describes Goal as the execution unit and Feature as its grouping and ordering layer in [the model intent](intent/02-model/02-models.md). Mission adds a lifecycle above that relationship without changing Feature into a workflow entity.

## Composition contracts

The synergy comes from narrow, durable boundaries rather than shared mutable state:

| Producer | Consumer | Contract | Result |
| --- | --- | --- | --- |
| Mission | GoalRound | Approved Goal specification plus a pinned, scoped context capsule | A myopic Goal acts with system intent without becoming Mission-aware throughout its workflow |
| GoalRound | Mission | Ordinary code evidence plus typed findings and artifact candidates | Local implementation can improve later system-level understanding and meta-code |
| Mission reconciliation | Later GoalRound | A new immutable MissionSnapshot | Later work learns from earlier work without changing what active or completed Goals observed |
| Feature | Mission scheduler | Existing Goal order and placement unit | Mission waves reuse dependency and fleet semantics instead of creating another graph |
| Node fleet | Mission | Existing Goal ownership, capacity, health, and completion evidence | Mission coordinates distributed work without owning a second execution substrate |
| Published Outcome | Later MissionRound | Exact manifest and selected artifact digests | Documentation, modernization, and later operations can build on proven prior knowledge |

The Mission knows the whole; Goals know the bounded part needed for their work. The handoff into Goal workflow is frozen context. The handoff back is evidence. Reconciliation is the only reducer that turns parallel findings into new canonical Mission context.

## Terminology

### User-facing terms

- **Mission**: the larger outcome.
- **Round**: one preserved attempt to achieve or revise a Mission.
- **Wave**: Goals admitted together against one shared Mission snapshot. Goals in a wave are independent unless existing Feature order says otherwise.
- **Mission context**: the accepted knowledge guiding contained Goals.
- **Artifact**: a durable Mission document or structured file, such as an architecture map, interface contract, or risk register.
- **Finding**: evidence or a proposed correction returned by a Goal.
- **Outcome**: the immutable accepted result of one Mission Round.
- **Attention**: an approval, decision, blocker, or risk requiring notice. Attention is not a workflow status.

### Internal terms

- `MissionRound`
- `MissionSnapshot`
- `MissionGoalBinding`
- `GoalRoundMissionContext`
- `GoalContribution`
- `MissionPlan`
- `MissionGoalSpec`
- `MissionWave`
- `ArtifactRef`
- `OutcomeBinding`
- `OutcomeManifest`
- `OutcomePublication`
- `ReconciliationReceipt`
- `mission_goal_key`

Users should not normally need to reason about manifests, receipts, bindings, claim attempts, or digests. Those belong in evidence and provenance inspectors.

## Reduced domain model

### Relationship overview

```text
Target App
|
+-- Mission
|   +-- MissionRound 1
|   |   +-- MissionPlan and amendments
|   |   +-- MissionSnapshot 1..N
|   |   +-- reconciliation receipts
|   |   `-- OutcomeManifest?
|   `-- MissionRound 2...
|
+-- Goal -- optional MissionGoalBinding --> Mission
|   `-- GoalRound
|       +-- pinned GoalRoundMissionContext --> one MissionSnapshot
|       `-- optional GoalContribution -----> Mission reconciliation
|
+-- Feature -- groups and orders Goals
`-- Node ---- coordinates a Mission or executes child Goals
```

### Mission

`Mission` is the only new aggregate root and the only new top-level mutable record.

Conceptual shape:

```text
Mission
  id
  name
  intent
  status
  reporter
  assignee?
  coordinator_node_id
  success_criteria[]
  artifact_contract[]
  current_round
  revision
  rounds[]
  created
  updated
```

Success criteria carry stable ids; artifact-contract entries carry stable keys. The top-level intent, criteria, and artifact contract are the editable current frame used to author the next Round. Active workflow never relies on those mutable projections directly.

Responsibilities:

- preserve the Mission charter and success criteria;
- own the current Mission status and Round;
- identify the sole coordinator Node for active Mission transitions;
- own plans, snapshots, reconciliation receipts, reviews, and outcomes;
- derive contained Goals by their Goal-owned Mission binding;
- authorize holistic settlement.

`Mission.status` is explicit workflow state. It is never mechanically derived from Goal counts or Feature rollups.

`Mission.revision` fences stale UI, CLI, agent, worker, and fleet mutations. Every consequential mutation of an existing Mission compares the observed revision and returns a structured conflict rather than overwriting newer state.

### MissionRound

`MissionRound` is an append-only history record within Mission, analogous to `GoalRound` within Goal.

```text
MissionRound
  number
  request
  input_bindings[]
  plan?
  plan_amendments[]
  snapshots[]
  reconciliation_receipts[]
  phase_evidence
  review?
  outcome?
  outcome_publication?
  failure?
  created
  updated
```

`MissionRound.request` freezes the complete charter for that attempt: intent, constraints, criteria with stable ids, artifact obligations with stable keys, and the authorizing user request. Plans, Goal specifications, capsules, gates, and the Outcome bind that charter digest rather than later top-level edits.

One Round is one user-authorized attempt to achieve or revise the Mission outcome. Only the latest Round may execute. Prior Rounds, their charter, snapshots, and their settled Outcomes are immutable. Changing active intent, criteria, or artifact obligations requires a new Round; renaming or annotating the Mission does not.

An automatic retry of one bounded phase remains within the same Round and gets a new attempt identity. A user-requested revision after Mission Review, or reopening a Done or Failed Mission with materially new intent, appends a MissionRound.

### MissionSnapshot

`MissionSnapshot` is an immutable, digest-addressed manifest of what the Mission accepts at one wave boundary.

```text
MissionSnapshot
  version
  parent_version?
  target_head
  plan_digest
  artifact_refs[]
  input_refs[]
  consumed_contribution_refs[]
  digest
  created
```

MissionSnapshot replaces the broader ideas of a mutable blackboard, a context entity, and an artifact collection entity. It is the canonical source used to compile Goal context capsules. A selected item may still carry an explicit provisional provenance such as `goal_review_pending`; canonical selection means the Mission chose to expose that exact qualified evidence, not that every source outcome is finally accepted.

Publishing the next snapshot is the only way canonical Mission context advances. Existing snapshots never change.

### Artifacts are files plus references

There is no mutable `MissionArtifact` entity.

Meta-code is stored as immutable files and selected by `ArtifactRef` records:

```text
ArtifactRef
  key
  title
  kind
  authority
  path
  media_type
  size
  sha256
  provenance
  applicability
```

The same logical artifact may have several immutable files over the life of a Mission. A newer snapshot selects the newer file and records which prior file it supersedes. Git history and snapshot manifests provide versioning; no mutable artifact header is needed.

Common artifact kinds include:

- system or component model;
- architecture map;
- interface or data contract;
- decision record;
- domain terminology;
- dependency or migration inventory;
- risk and assumption register;
- criteria and verification matrix;
- operational runbook;
- synthesized outcome report.

Artifact authority is explicit:

| Authority | Meaning | Promotion rule |
| --- | --- | --- |
| Evidence | Quoted observation with provenance | May be accepted automatically when deterministic validation passes |
| Model | A derived description of the system | Requires reconciliation and source coverage |
| Decision | An accepted design choice | Requires the configured Mission decision gate; human by default in v1 |
| Directive | Content allowed to guide Goal agents normatively | Requires explicit approval and cannot weaken higher Governance |

An artifact never becomes an instruction merely because an agent wrote it.

Small Markdown, JSON, and text artifacts may live directly in Refine state. Application code stays in target-app Git history. Large or binary artifacts stay in an immutable external or target-repository location and are referenced by exact commit, path, blob, size, media type, and digest.

### Mission plan, waves, and Goal specifications

Mission plan is an immutable base plan plus append-only amendments, not a mutable graph.

```text
MissionPlan
  charter_digest
  summary
  assumptions[]
  risks[]
  criteria_coverage[]
  waves[]
  artifact_obligations[]
  criticism
  resolutions[]
  effective_digest
```

```text
MissionWave
  number
  purpose
  goal_specs[]
  required_snapshot
  completion_condition
```

```text
MissionGoalSpec
  mission_goal_key
  name
  prompt
  role
  required
  criterion_ids[]
  input_artifact_keys[]
  output_artifact_keys[]
  expected_findings[]
  feature_id?
  feature_order?
  preferred_node?
```

Each plan-level artifact obligation names a stable artifact key, kind, purpose, required flag, validation policy, and intended consumers. A Goal specification references the obligations it must satisfy rather than copying them. A code-producing Goal may therefore return meta-code as a first-class secondary deliverable without changing the Goal's primary code lifecycle.

The base plan and each append-only amendment produce one effective digest. A material amendment changes that digest. Waves are linear. Goals within one wave may run in parallel when ordinary Goal and Feature scheduling permits. Existing Feature order remains the executable dependency mechanism; Mission does not add dependency-edge records or a DAG.

Each approval is an append-only value record binding charter digest, effective plan digest, snapshot digest, Mission revision, actor, rationale, and time. It is not a separate Plan entity or workflow.

### Goal extension

Goal receives one optional binding:

```text
Goal.mission?
  mission_id
  mission_goal_key
```

`(mission_id, mission_goal_key)` is unique. It lets Goal materialization retry after a crash without creating duplicates.

Mission membership is authoritative on Goal. Mission never stores a competing mutable `goal_ids` list. Mission detail derives membership through projection.

A replacement Goal gets a new `mission_goal_key`. A later MissionRound may reuse an existing Goal by appending a new GoalRound with new Mission context when ordinary Goal transition rules allow it.

### GoalRound extension

A Mission-bound GoalRound receives a typed context binding:

```text
GoalRound.mission_context?
  mission_id
  mission_round
  snapshot_version
  snapshot_digest
  capsule_digest
```

The complete scoped capsule remains within the existing frozen `agent_context`. The typed binding makes validation and indexing possible without parsing arbitrary prompt text.

A Mission-bound GoalRound may also settle one contribution:

```text
GoalRound.mission_contribution?
  bound_context_digest
  criteria_evidence[]
  findings[]
  challenged_assumptions[]
  artifact_candidates[]
  suggested_followups[]
  downstream_invalidations[]
  digest
```

Each artifact candidate binds an output-obligation key to a kind, media type, size, digest, immutable handoff reference, evidence references, provenance, and proposed authority. The Goal Application handoff validates and copies candidate bytes from operation-owned staging to an immutable pending-contribution path. The Goal agent never writes live Mission state, and the candidate does not become canonical merely because it was stored.

Contribution is GoalRound evidence, not an independently mutable entity. The Goal cannot directly edit canonical Mission artifacts or snapshots. Reconciliation is the only behavior that can select a candidate into a MissionSnapshot.

### OutcomeManifest

Outcome is a prominent product concept but an embedded immutable settlement record, not a new mutable aggregate.

Exactly one Outcome may be published for a completed MissionRound:

```text
OutcomeManifest
  mission_id
  mission_round
  charter_digest
  final_snapshot
  criteria_results[]
  artifact_refs[]
  goal_evidence_refs[]
  target_commit_refs[]
  input_bindings[]
  manifest_digest
  approved_at
  approved_by
```

A MissionRound records publication separately, after Git assigns the commit containing the manifest:

```text
OutcomePublication
  manifest_digest
  outcome_state_commit
  verified_path_digests[]
  published_by
  verified_at
```

`OutcomePublication` is embedded settlement evidence, not an aggregate or revision lifecycle. The later state commit containing this receipt and the Done transition is returned by consolidation and proven by read-back; it is not stored inside itself.

A correction or materially revised result creates a new MissionRound and therefore a new Outcome. There is no separate Outcome revision lifecycle.

### OutcomeBinding

A MissionRound may consume exact published Outcomes:

```text
OutcomeBinding
  source_mission_id
  source_mission_round
  source_manifest_digest
  source_state_commit
  selected_artifact_refs[]
  purpose
  required
```

Only a published Outcome may be bound. `latest` is forbidden. A new upstream Outcome produces an informational notice but never silently changes a consumer.

Because only completed Outcomes can be inputs, cross-Mission support is immutable data lineage rather than an active Mission dependency graph.

### Derived projections

The following are projections, not sources of truth:

- `MissionIndexProjection`: identity, status, Round, coordinator, current wave, attention, criteria summary, and Outcome availability;
- `MissionDetail`: a bounded current-Round summary with current plan and snapshot metadata, rollups, attention, operation receipts, section links and cursors, and available capabilities;
- `MissionRollup`: child status counts, required failures, current wave, criteria coverage, and contradictions;
- `MissionFleetProjection`: planned and observed Goal placement plus node health and capability;
- `MissionLineageProjection`: input Outcomes and downstream consumers;
- Goal projection additions: Mission id, name, Goal key, context snapshot, and contribution state.

Mission status never derives from these projections. A cache rebuild must reproduce them from durable Mission, Goal, node, and activity state.

## Explicitly omitted entities

The design intentionally does not create:

- `WorkItem` as a persisted superclass;
- `MissionGoal` as a join entity;
- `MissionFeature`;
- `MissionNode`;
- `MissionContribution` separate from GoalRound;
- `ContextCapsule` separate from GoalRound context;
- `Artifact` or `ArtifactRevision` aggregate;
- `MissionOutcome` aggregate;
- dependency-edge records;
- a mutable Mission memory document.

Mission and Goal may later share workflow interfaces or evidence vocabulary. They should not share persistence inheritance merely because their behavior has the same abstract rhythm.

## Domain invariants

1. A Goal belongs to zero or one Mission. No binding means today's standalone behavior.
2. A Goal may simultaneously belong to one Mission and one Feature.
3. Mission membership is Goal-owned. Mission stores no second mutable member list.
4. `(mission_id, mission_goal_key)` uniquely identifies materialized planned work.
5. Each MissionRound freezes its charter digest; only the latest Round may execute against it.
6. One coordinator Node owns active Mission transitions; child Goals keep independent Node ownership.
7. Mission transfer never transfers child Goals.
8. A Mission-bound GoalRound pins exactly one MissionRound, MissionSnapshot, and capsule before Plan. That binding never changes.
9. Parallel Goals cannot edit shared Mission context. They append contributions.
10. Contributions are advisory evidence. Only fenced Mission reconciliation may publish a new snapshot or promote authority.
11. Review-ready sibling evidence may guide later waves only with exact integrated provenance and an explicit `goal_review_pending` label. A Goal is an accepted child outcome only at Done; failed or rejected output is excluded.
12. Mission cannot mutate the target application directly. All application changes pass through ordinary Goal workflow.
13. Target-app Governance outranks every Goal request. Applicable Mission directives and hard constraints are compiled into the GoalRound request before admission; a conflict is rejected rather than resolved by hidden prompt precedence.
14. Goal completion is evidence, not Mission completion.
15. Required failed or cancelled Goals block acceptance unless an explicit waiver with rationale is part of exact Mission review evidence.
16. Mission completion requires holistic criteria judgment and an exact Outcome manifest.
17. A Mission reaches Done only after Git read-back proves the Outcome manifest and stored artifact bytes.
18. Cross-Mission inputs pin exact source Round, state commit, manifest digest, and selected artifact digests.
19. Replanning and reconciliation are append-only. They never rewrite what an earlier GoalRound observed.
20. Cancellation preserves Mission, Goal, contribution, snapshot, and Outcome history.

## Mission workflow

### Status model

Mission uses a distinct enum rather than `GoalStatus`:

```text
Draft
  -> Investigate
  -> Plan
  -> Execute
  -> Synthesize
  -> Quality
  -> Governance
  -> Review
  -> Consolidate
  -> Done
```

The distinct names still realize the same outcome-seeking rhythm:

| Shared rhythm | Goal realization | Mission realization |
| --- | --- | --- |
| Frame | Backlog and Todo | Draft |
| Plan | Plan | Investigate and Plan |
| Produce | Implement | Execute and Synthesize |
| Verify | Quality | Quality |
| Govern | Governance | Governance |
| Judge | Review | Review |
| Settle | Done | Consolidate and Done |

Mission is therefore behaviorally recursive without forcing storage inheritance or reuse of the Rust Goal status enum.

`Failed` and `Cancelled` are terminal for the current Round. A new Round may resume the Mission when explicitly authorized.

A failed stage attempt is not `Mission.status = Failed`. It creates retryable `stage_failed` attention while the Mission remains in its current nonterminal phase. The Round becomes Failed only when recovery is unsafe, explicitly chosen, or exhausted; after that, only a new Round may execute.

`NeedsDecision` is not a status. It is derived attention attached to the current status so the UI can distinguish a planning question from an execution contradiction or publication blocker.

### Draft

Draft captures:

- name and outcome intent;
- reporter and optional assignee;
- initial success criteria and artifact expectations when known;
- source material;
- exact prior Outcome bindings;
- coordinator Node.

Draft is editable and consumes no agent or fleet capacity. Starting the Mission appends the first MissionRound if one does not exist and moves to Investigate.

### Investigate

A fresh installed agent investigates the target app and bound inputs. It produces typed:

- facts with sources;
- inferred system models;
- unknowns and questions;
- contradictions;
- risks and assumptions;
- proposed criteria and artifact contract;
- initial meta-code artifacts.

Investigation is observational. The agent may read the target app and use ordinary read-only tools, but it may not change the application worktree or live Refine state.

Agent output first lands in runtime staging. Application validates schemas, paths, sizes, media types, digests, provenance, and Git cleanliness before atomically promoting immutable files and recording the initial MissionSnapshot.

Rejected output and diagnostics remain durable phase evidence under a bounded structured-output repair budget.

### Plan

Mission planning uses proposal, independent criticism, and revision against:

- Mission intent and success criteria;
- the investigation snapshot;
- exact Outcome inputs;
- target-app Governance and Guidance;
- existing Goals and Features;
- fleet capacity and node capabilities;
- repository state.

The accepted plan defines linear waves, stable Goal keys, Goal prompts, required versus optional work, criteria coverage, expected findings, artifact inputs, Feature placement, and preferred Nodes.

The browser, CLI, API, and agent surfaces receive the same plan projection and distribution preview. Human plan approval is mandatory in v1 and binds the frozen Round charter, exact effective plan digest, current snapshot, and observed Mission revision.

Plan approval is one authorization. It permits the Mission engine to materialize Goals idempotently and distribute eligible work according to the approved plan. The product does not require separate user actions named Create Goals, Apply Plan, and Publish Wave.

Any later material amendment—changing a Goal prompt or role, required work, wave, Feature placement, criteria coverage, artifact input, or output obligation—creates a new effective plan digest. Reconciliation or a user may draft the amendment, but affected Goals cannot be materialized, rebound, or admitted until `Approve plan` authorizes that digest. Capacity-driven Node placement within already approved constraints is not a material amendment.

### Execute

Execute is the container phase for ordinary Goal workflows:

```text
for each approved wave:
  compile a scoped capsule for every Goal
  materialize or adopt Goals idempotently
  revalidate and distribute eligible Goal ownership
  let ordinary Goal workflow run
  wait for required Goal evidence
  reconcile exact findings and candidate evidence
  publish the next MissionSnapshot
  amend later work when accepted knowledge requires it
```

The Mission coordinator does not implement target-app changes. Goal agents continue through Plan, Implement, Quality, Governance, and Review under existing authority.

#### Goal materialization

For each `MissionGoalSpec`, the Mission service scans for `(mission_id, mission_goal_key)`:

- one matching Goal is reused;
- no match creates one Goal and its actionable first GoalRound;
- more than one match is a conflict and blocks the Mission;
- a crash after creation but before receipt recovers by scanning the stable key;
- adopting an existing Goal writes the Mission binding only when Goal state and plan rules permit;
- admission rejects a Goal request that conflicts with an applicable Mission directive, criterion, or scope constraint.

Mission-created Goals start in Backlog until their wave is admitted. Admission moves only eligible Goals to Todo through existing work-item behavior.

#### Feature interaction

A Mission Goal may also belong to a Feature. Feature order continues to gate Goal scheduling.

Whole-Feature placement is automatic only when every Goal in the Feature is within the Mission operation's scope. A mixed Mission/non-Mission Feature remains pinned or requires an explicit compatible operator decision; Mission never silently moves or cancels part of an ordered Feature.

#### Fleet distribution

Mission distribution compiles an approved wave into existing Goal and Feature placement operations. It must:

- use only enabled, healthy, compatible Nodes;
- preserve active Goal ownership;
- honor Goal priority and Feature order;
- treat a scoped ordered Feature as one placement unit;
- show exclusions and reasons in preview;
- revalidate immediately before applying;
- make node assignment and Todo admission one fenced operation per Goal or Feature unit;
- record a durable distribution receipt.

Expected capacity waits are neutral. Missing eligible capacity, stale sync evidence, mixed Feature scope, or incompatible Nodes becomes attention with a concrete recovery path.

#### Mission context compiler

Before Goal Plan, Application deterministically compiles a bounded capsule from the selected MissionSnapshot and `MissionGoalSpec`.

The capsule includes only the relevant slice of:

- Mission intent and applicable criteria;
- the Goal's role, scope, and non-goals;
- approved architecture, terminology, contracts, and directives;
- accepted predecessor results;
- current risks, assumptions, contradictions, and questions;
- exact artifact and input identities;
- expected findings and downstream consumers;
- snapshot target head and digests.

The compiler records which artifacts were included and why. It enforces a configured context budget and prefers exact references plus bounded summaries over truncating content invisibly.

The capsule is added as an optional `mission` member of the existing pinned Goal agent context. Hard Mission constraints are also rendered into the authored GoalRound request, which remains authoritative for Goal execution. The capsule supplies evidence and rationale; it never silently overrides that request. The whole agent context remains digest-bound to implementation planning. A Mission update never changes an active GoalRound.

#### Goal contributions

A Mission-bound Goal completion contract may include findings in addition to ordinary implementation evidence. Findings must link claims to exact files, commits, tests, logs, or other source evidence where applicable.

Contribution is optional at the transport level for rolling compatibility. A Mission plan may mark specific findings or artifacts as required; absence then prevents criterion coverage rather than fabricating a result.

Contributions are append-only. A late or stale contribution remains inspectable but cannot be consumed unless its GoalRound, snapshot digest, candidate identity, and Mission attempt are still authoritative.

A contribution becomes eligible for reconciliation only when its Goal reaches Review with exact integration, Quality, and Governance evidence. Review is not Goal acceptance. Deterministic facts and low-authority models may enter the next snapshot with `goal_review_pending` provenance because the integrated target state is already real and existing Feature semantics permit later ordered work at Review. They cannot become Decisions or Directives on that basis. If the Goal leaves Review, gains another Round, or is declined, Mission reconciliation invalidates affected future context, blocks new dependent admission, and requires new GoalRounds for active work whose premises no longer hold.

#### Reconciliation

At a wave boundary, one fenced reconciliation attempt claims an exact contribution set and parent snapshot. It:

1. verifies Goal, Round, candidate, integration, Quality, and Governance evidence;
2. separates facts, proposals, contradictions, and unproven claims;
3. compares findings with current artifacts and inputs;
4. proposes immutable artifact files and authority changes;
5. identifies affected future Goal specifications;
6. records accepted, rejected, deferred, and contested findings;
7. publishes one next MissionSnapshot, drafts a plan amendment, or raises a typed decision request.

Only evidence and low-authority model updates satisfying deterministic policy may promote automatically in v1, and review-ready sources retain their `goal_review_pending` provenance until Goal approval. Decisions and directives require explicit approval. A drafted material plan amendment creates Approval attention in the current phase; affected work cannot enter a later wave until the new effective digest is approved. The system preserves dissent and uncertainty rather than synthesizing it away.

Reconciliation never edits an accepted snapshot or completed GoalRound.

### Synthesize

Once required execution waves have reconciled, a fresh synthesis agent receives only pinned Mission inputs:

- Mission charter and criteria;
- accepted plan and amendments;
- final execution snapshot;
- accepted Goal evidence;
- exact target head;
- artifact contract.

It produces a candidate Outcome summary and any final meta-code artifacts in isolated runtime staging. Application validates and promotes them into a candidate final snapshot.

Synthesis cannot repair target-app code directly. A discovered code gap returns the Mission to a bounded Execute recovery wave with new or revised Goals.

### Quality

Mission Quality evaluates the combined outcome, not each child in isolation. It verifies:

- all required Goal commits are reachable from the pinned target head;
- target movement has not invalidated accepted evidence;
- configured system-level tests and checks pass against the combined state;
- interfaces agree across Goal boundaries;
- accepted code matches applicable architecture and contracts;
- artifact schemas, paths, media, sizes, digests, and provenance are valid;
- every criterion is met, partial, unmet, contradicted, or explicitly waived;
- the candidate Outcome manifest contains exact evidence references.

Mission Quality uses deterministic checks where possible and a fresh read-only agent for holistic judgment where needed. A candidate change invalidates prior Mission Quality evidence.

Bounded correctable findings create an Execute recovery wave. Exhaustion or an unresolvable contradiction moves the current Round to Failed or raises a decision request according to policy.

### Governance

Mission Governance judges the exact tuple:

```text
(target_head, final_snapshot_digest, candidate_outcome_manifest_digest)
```

It evaluates system-level effects that individual Goal Governance could not see, while preserving target-app constitution and rules as the higher authority.

A changed target head, snapshot, or manifest invalidates the verdict. Governance may request bounded gap work, require a user decision, pass to Review, or fail the Round.

### Review

Mission Review is criterion-first. It presents:

- each success criterion and its result;
- exact supporting Goals, Rounds, commits, tests, and artifacts;
- contradictions, waivers, and residual risks;
- cross-Goal coherence findings;
- the candidate Outcome manifest;
- the exact Mission Governance verdict.

Approval authorizes that exact candidate only.

Contained Goals that remain in Review may be approved collectively as part of Mission approval, but only through the existing Goal approval capability after exact revalidation. Each settlement records the Mission reviewer actor, rationale, and reviewed evidence. The Mission cannot forge Goal evidence or bypass Goal Review. A child changed since the Mission review snapshot blocks collective approval and therefore consolidation.

Rejecting or materially revising the result preserves the reviewed candidate and appends a new MissionRound. Automated pre-review findings may still create bounded recovery waves within the current Round.

### Consolidate and Done

Final Mission approval starts deterministic consolidation; there is no separate ordinary Publish action.

Consolidation:

1. validates the exact approved tuple and child Goal approvals;
2. writes the immutable Outcome manifest and final artifact files to live Refine state;
3. invokes shared persistence synchronization;
4. receives state commit `C`;
5. reads every Outcome path back with `git show C:<path>`;
6. verifies exact bytes and digests;
7. records the Outcome publication receipt for `C` and the intended Done state in `mission.json`;
8. synchronizes that terminal record in a later state commit `D`;
9. reads `mission.json` back from `D` and only then exposes the Mission as Done.

This two-commit receipt avoids making a manifest claim the identity of the commit containing itself. `C` is the Outcome publication commit; `D` proves that Refine durably recorded the publication and terminal transition. A crash between steps is idempotently recoverable.

If the fleet uses a configured remote, Done additionally requires terminal commit `D` to be pushed or otherwise proven converged according to persistence-sync policy; `D` carries publication commit `C` as history.

### Zero-Goal Missions

A Mission may have no child Goals when its approved plan proves that the Outcome is entirely observational or artifact-based, such as documenting a system or producing a verified inventory.

It still passes Synthesis, Mission Quality, Governance, Review, and consolidation. This supports meta-code outcomes without inventing artifact-only Goal workflow in v1.

### Failure, retry, and new Rounds

- Phase retries retain the same MissionRound and use monotonically increasing attempt identities.
- Structured-output repair, reconciliation, recovery waves, and publication each have independent bounded budgets.
- Budget exhaustion creates a precise decision request or fails the Round; it never loops indefinitely.
- `Retry stage` is available only for retryable `stage_failed` attention in a nonterminal Round.
- A Failed Round is immutable; continuing it appends a new Round with an explicit recovery request.
- A Done Mission may append a new Round; its prior Outcome remains immutable and usable.
- A user request after Review appends a new Round rather than rewriting the reviewed candidate.

### Cancellation

Cancelling a Mission stops new Mission agent phases and new Goal admission.

Default cancellation does not cancel active or completed child Goals. An explicit cascade may request cancellation of queued or all cancellable child Goals through existing Goal behavior and must report per-Goal outcomes. No Mission cancellation deletes Goals, snapshots, contributions, or Outcomes.

### Transfer and ownership

Mission has one semantic coordinator Node. Transfer:

- is explicit;
- requires the active Mission agent attempt to be quiescent or fenced;
- changes only `coordinator_node_id`;
- never moves child Goals;
- preserves attempt and recovery evidence;
- rejects competing transfers as ambiguous.

Mission mutation authority is checked with:

```text
mission_id
mission_round
mission_revision
status
stage_attempt
coordinator_node_id
```

Workers never hold a Mission record lock or repository lock during an agent call.

### Pause and capacity

The existing workflow pause applies to Mission and Goal automation. A paused Mission remains inspectable and may still accept safe read operations; it admits no new phase or child work.

Mission agents and Goal agents share provider, Node, target-app, and global capacity limits. Mission coordination must not reserve fleet capacity while waiting on a human decision.

## Persistence and synchronization

### State layout

Durable paths below are relative to the live Refine state root at `<git-common-dir>/refine-live-state/`. Persistence sync mirrors them under `.refine/` only in the isolated `refine/state` worktree; live mutation remains outside the primary target-app worktree as described in [Target App intent](intent/02-model/03-target-app.md).

```text
missions/<shard>/<mission-id>/mission.json
missions/<shard>/<mission-id>/snapshots/<round>/<version>.json
missions/<shard>/<mission-id>/artifacts/<artifact-key>/<sha256>.<ext>
missions/<shard>/<mission-id>/contributions/<goal-id>/<goal-round>/<sha256>.<ext>
missions/<shard>/<mission-id>/outcomes/<round>/manifest.json
```

`mission.json` owns mutable Mission state and Round metadata. Snapshot, artifact, and Outcome files are immutable after publication.

Pending contribution files are immutable evidence owned by the referenced GoalRound; snapshots decide whether they become canonical Mission artifacts. Artifact keys and extensions are validated internal identifiers, never unchecked user paths.

Checkout-owned runtime is separate:

```text
<serving-checkout>/run/missions/<mission-id>/...
```

Agent staging, provider transcripts, process metadata, locks, and temporary files stay there and never enter synchronized state.

### Atomic writes

Every durable JSON or artifact write uses the established pattern:

1. write a sibling temporary file;
2. parse or validate the complete content;
3. calculate and compare expected digest;
4. atomically rename into place;
5. read the final path back;
6. prove the temporary path is absent.

An existing immutable path with the same bytes is idempotent success. The same path with different bytes is corruption and fails closed.

### State synchronization

Persistence sync already captures non-excluded files recursively, so Mission state should use the existing snapshot, merge, hydrate, commit, push, and retained-ref pipeline rather than adding Mission-specific Git commands.

Mission-specific merge policy must preserve:

- one-sided coordinator transfers;
- the coupling of coordinator, active Round, status, revision, and attempt authority;
- append-only Round, contribution, snapshot, reconciliation, and Outcome evidence;
- immutable artifact union when paths and digests agree;
- all ambiguous competing edits for explicit resolution;
- prior state through merge parents or retained refs.

Different bytes at one immutable contribution, artifact, snapshot, or Outcome path are never chosen by timestamp.

### Projection cache

The project projection fingerprint currently follows Goals, Features, logs, activity, and Git state. Mission delivery bumps the projection snapshot version and adds `mission.json` records to incremental fingerprints and delta rebuilds. Every snapshot, artifact-manifest, or Outcome publication must finish by revising `mission.json` with its exact reference and digest. The projection therefore does not scan immutable artifact trees or hash large artifact contents on every request.

Incremental projection adds changed and removed Missions, indexes Goals by optional Mission, and recomputes a Mission rollup whenever a member Goal changes even if `mission.json` did not. Full context, contribution bodies, and artifact bodies stay out of the projection. The persistence-sync commit summary also gains Mission-aware descriptions; this is presentation over the same state transaction, not another index.

Projection snapshots remain derived and disposable.

### Rolling fleet upgrades

Mission files are new paths and can continue to synchronize through older nodes that recursively preserve durable state. Goal mutation paths must continue preserving unknown JSON members.

Mission delivery must nevertheless treat execution capability explicitly:

- bump the daemon API contract when Mission mutations become public;
- advertise Mission workflow and contribution contract capability per Node;
- dispatch Mission-bound Goals only to compatible Nodes in v1;
- let older nodes continue unrelated standalone Goal work and state synchronization;
- report an incompatible destination as `pending upgrade`, not generic failure;
- keep the Mission capsule inside the existing Goal `agent_context` envelope so context remains readable and digest-bound;
- never let an older node's lack of Mission projection imply Mission deletion.

If a mixed-version node completes a Goal without the optional contribution contract, its ordinary code and workflow evidence remain valid. A required Mission finding remains unsatisfied and blocks Mission criterion coverage rather than being inferred.

### Security and retention

- Secrets, credentials, raw authentication material, and sensitive provider transcripts are forbidden in durable Mission artifacts.
- Artifact staging validates path traversal, symlinks, media types, size, encoding, and digest.
- Repository text and imported Outcome content are quoted evidence until explicitly promoted; they cannot inject directives by position alone.
- Context compilation preserves authority labels and separates directives from evidence.
- Large artifacts use immutable references rather than bloating `refine/state`.
- Outcome deletion is not supported in v1. Archival and retention may hide old Missions without breaking input lineage.
- State-growth metrics should report Mission count, snapshot count, pending contribution bytes, canonical artifact bytes, Outcome bytes, and retained synchronization refs.

## Application architecture

### Model

Add:

```text
src/model/mission/
```

It owns Mission types, Mission status and transition invariants, Round records, snapshots, plans, artifact references, outcomes, and projections.

Extend Goal and GoalRound with optional Mission binding, context binding, and contribution fields. Legacy Goal JSON without those fields remains valid.

Do not generalize the existing Goal workflow trait or status enum before the Mission engine demonstrates a genuinely shared abstraction.

### Application

Add:

```text
src/application/missions/
  service
  workflow
  context
  reconciliation
  outcome
  recovery
```

Application owns:

- Mission CRUD and optimistic revisions;
- Round creation;
- plan and decision approvals;
- Mission attempt claims;
- Goal specification materialization and adoption;
- snapshot and artifact promotion;
- context compilation;
- contribution settlement and reconciliation;
- fleet placement compilation;
- Mission Quality and Governance;
- collective Goal review;
- Outcome consolidation and publication proof;
- cancellation, transfer, retry, and recovery;
- list, detail, Dashboard, lineage, and capability projections.

It reuses:

- installed agent invocation and provider selection;
- structured output and bounded repair;
- Goal and GoalRound services;
- Feature ordering and eligibility;
- fleet registry, transfer, capacity, and sync health;
- Quality and Governance capabilities;
- operations, process supervision, activity, and SSE;
- persistence synchronization and Git read-back.

### Workflow runner

Mission gets a separate `MissionWorkflowEngine`. The existing supervised workflow worker evaluates Mission readiness alongside Goal readiness so workflow pause and worker health remain authoritative, then queues a one-shot supervised Mission-stage operation. Mission does not add a second permanent scheduler loop.

The Mission engine advances only short, fenced state transitions synchronously. Long agent calls and publication work run as one-shot managed processes or durable operations with stable attempt ownership such as `mission:<id>:round:<n>:<stage>`. A terminal background write explicitly refreshes the project projection; the refresh performed when the initial `202` response is returned is too early. Goal workflow remains the leaf executor and is not called recursively on the same stack.

### Prompt and structured-output contracts

Add Mission prompt templates under the shared prompt engine for:

- investigation;
- plan proposal;
- independent plan criticism;
- plan revision;
- reconciliation;
- synthesis;
- holistic Quality judgment;
- Mission Governance;
- structured-output repair.

Mission prompts carry typed, pinned inputs. Agent text cannot define workflow transitions or publication authority. Every phase output has a bounded schema, raw-attempt preservation, diagnostic repair limit, and exact phase binding.

## Surface plan

All surfaces are adapters over the same Mission Application capabilities, following [surface principles](intent/05-surfaces/01-surface-principles.md). A rejected transition must be rejected everywhere. No browser JavaScript, CLI dispatcher, or MCP tool owns Mission semantics.

### Shared response contract

Mission detail responses should include:

- authoritative Mission and a bounded current-Round summary;
- Goal, Feature, and fleet rollups plus links or cursors for members;
- current plan and snapshot metadata plus cursors for history;
- artifact, contribution, and Outcome references rather than unbounded bodies;
- current criteria results and review summary;
- typed attention items;
- current durable operation receipts;
- capability flags for every allowed user action.

Surfaces render capability flags rather than recreating transition tables. Goal lists reuse the existing Mission-filtered Goal query; Activity reuses its Mission filter. Snapshot, finding, artifact, and history collections load by cursor or exact reference. One Mission detail request never expands the entire Mission corpus.

Attention classes are:

| Class | Examples | Expected response |
| --- | --- | --- |
| Approval | Plan or material amendment ready, final Outcome ready | Review exact candidate and approve |
| Decision | Contradictory findings, directive promotion, waiver | Choose with rationale |
| Blocker | Required Goal failed, no eligible Node, stale evidence, sync conflict | Inspect and recover |
| Risk | Optional work failed, partial criterion, budget nearly exhausted | Monitor or amend |
| Information | New upstream Outcome, expected wave wait, superseded artifact | No action required |

Paused and expected capacity waiting are neutral operating states.

## Browser surface

The browser remains the richest human oversight surface and stays a static vanilla JavaScript app over the local daemon, consistent with [browser intent](intent/05-surfaces/03-browser/00-overview.md).

### Navigation

Add `Missions` to the primary navigation:

```text
Dashboard | Missions | Features | Goals | Changes | Logs
```

Keep `+ New Goal` as the bright primary create action. Add `New Mission` to the adjacent create menu. This preserves fast standalone Goal capture while making Mission creation discoverable.

Add command-palette entries for:

- open Missions;
- create Mission;
- open a Mission by id or name;
- jump to Missions awaiting plan or Outcome approval;
- jump to Missions needing a decision.

### Routes

Add route-backed state:

```text
#/missions
#/missions/new
#/missions/<id>
#/missions/<id>?section=plan|work|context|review|outcome|activity
```

Mission detail is a full-page workbench, not a modal. Its evidence, snapshots, waves, and Outcome need the full viewport. Goal and Feature details remain modal and preserve their underlying context.

List filters, detail section, selected Round, snapshot, and artifact are URL-backed where they affect shareable inspection.

### Missions list

Reuse the shared dense table, sorting, filtering, pagination, loading, empty, and error patterns.

Columns:

- Mission;
- status and Round;
- current wave;
- criteria summary;
- active or blocked Goals;
- coordinator and active Nodes;
- highest attention;
- current snapshot;
- Outcome availability;
- last activity.

Filters:

- status;
- attention class;
- reporter and assignee;
- coordinator Node;
- current/all Node scope where meaningful;
- input Mission;
- published/unpublished Outcome;
- text search;
- sort, direction, and page.

Do not offer bulk Mission approval, bulk decision, or bulk Outcome settlement. Safe bulk pause or archive can be considered later.

### New Mission

The initial form asks only:

- name;
- desired outcome;
- why it matters, optional;
- known done conditions, optional;
- expected deliverables, optional;
- source material, optional;
- exact prior Outcomes, optional;
- Reporter.

Create produces Draft. It does not silently launch an agent. The Draft page lets the user edit the frame and explicitly `Begin investigation`.

An Outcome picker selects one exact source MissionRound, publication commit, and manifest. It may select specific artifacts. A later upstream Outcome produces `New result available`; it does not alter the Draft.

### Mission workbench

The permanent header shows:

- Mission name and short intent;
- status and Round;
- coordinator;
- workflow pause;
- current wave and active Nodes;
- highest attention;
- current snapshot;
- one capability-derived primary action.

Possible primary actions:

- Begin investigation;
- Review and approve plan;
- Answer decision;
- Retry failed stage;
- Review Outcome;
- Start new Round.

There is no ordinary `Publish` button: final approval authorizes consolidation. Publication failure changes the primary recovery action to retry consolidation.

Sections:

1. **Overview**: intent, criteria, investigation summary, inputs, scope, risks, and next action.
2. **Plan**: waves, Goal specifications, criteria coverage, artifact obligations, criticism, amendments, and distribution preview.
3. **Work**: Goal workflows grouped by wave and Feature, fleet placement, waits, failures, and findings state.
4. **Context**: accepted artifacts, snapshots, semantic diffs, pending findings, contradictions, decisions, and provenance.
5. **Review**: criterion-by-criterion evidence, holistic Quality, Governance, waivers, and exact approval candidate.
6. **Outcome**: final manifest, artifacts, target commits, publication receipt, inputs, and downstream consumers.
7. **Activity**: durable Mission events and operation progress.

### Plan experience

The Plan section shows a linear wave view and a coverage matrix. Users may edit, split, reorder, add, or remove Goal specifications before approval.

Each Goal specification shows:

- purpose and prompt;
- required or optional;
- criteria advanced;
- artifacts consumed;
- meta-code artifacts owed;
- findings expected;
- Feature placement;
- proposed Node.

The distribution preview shows capacity, Feature placement units, pinned Goals, incompatible Nodes, and exclusions. `Approve plan` binds the plan digest and authorizes materialization and dispatch after server-side revalidation.

Adding, removing, or adopting a Goal after approval drafts a visible plan amendment. The UI previews affected criteria, waves, future Goal context, and the new effective digest. `Approve plan` is reused for that exact amendment; no affected Goal is attached or admitted beforehand.

### Work and fleet experience

Collapsed waves show purpose, snapshot, criteria, state, and counts. Expanded waves show ordinary Goal workflow bars.

Expected waits are neutral. A mixed Feature, failed required Goal, unavailable Node, invalid context, or stale sync result is shown with the exact reason and supported recovery action.

The browser never advances Goal statuses itself. It reflects existing Goal workflow and calls Mission capabilities for admission, decisions, retry, or cancellation.

### Context experience

Context organizes artifacts by function rather than filesystem path:

- system model;
- architecture and decisions;
- interfaces and contracts;
- risks and assumptions;
- verification model;
- findings and contradictions;
- operational guidance.

Every artifact shows authority, snapshot, digest, provenance, applicability, and consuming Goal Rounds. The default view uses readable content and semantic diffs. Raw manifests, exact paths, hashes, and source evidence are available in an evidence inspector.

The reconciliation view groups compatible findings, artifact candidates, contradictions, challenged assumptions, proposed follow-up work, and affected future Goals. Users approve decisions, directives, and material plan amendments, not every low-risk evidence receipt.

### Review experience

Mission Review centers on a criteria matrix:

| Criterion | Result | Goals | Artifacts | Verification |
| --- | --- | --- | --- | --- |
| Preserve API compatibility | Met | G-12, G-14 | Interface contract | Contract suite |
| Document rollback | Partial | G-18 | Runbook draft | Exercise missing |

Results are `Met`, `Partial`, `Unmet`, `Contradicted`, or `Waived`.

Selecting a criterion reveals exact Goal Rounds, commits, tests, artifacts, findings, and reviewer evidence. Review separately calls out cross-Goal inconsistency, stale target state, missing integration checks, required failures, and documentation drift.

Actions are:

- Approve Outcome;
- Request changes, which appends a MissionRound;
- Answer a required decision;
- Waive with rationale where policy permits;
- Fail current Round.

### Outcome experience

Before approval, Outcome shows the exact candidate manifest and readiness checks. After Done, it shows:

- MissionRound;
- manifest digest;
- Refine-state publication commit;
- target-app commits;
- criteria results;
- accepted artifacts;
- publication actor and time;
- input Outcomes;
- downstream consumers;
- export and `Use in new Mission` actions.

Reloading during consolidation reconnects to its durable operation. Failure retains staged evidence and presents one precise retry or recovery action.

### Mission-bound Goal changes

Standalone Goal UI remains unchanged.

The Goals list gains an optional Mission filter and a compact Mission badge or column when Mission-bound results are present. Views containing only standalone Goals do not render empty Mission placeholders.

A Mission-bound Goal gains:

- Mission breadcrumb with Round and wave;
- Mission role and criteria;
- exact pinned snapshot and capsule digest;
- applicable artifact list;
- expected findings;
- contribution state;
- link back to Mission Work or Context.

The Goal modal labels newer context without silently applying it:

- `Running on snapshot 3; snapshot 4 is available` when still valid;
- `Snapshot 3 was invalidated; a new Goal Round is required` when reconciliation says its premises changed.

Goal workflow, actions, and evidence remain the existing authoritative surface described in [Goal surface intent](intent/05-surfaces/03-browser/08-goal.md).

### Feature changes

Feature remains a grouping and ordering surface. Its detail modal adds a Mission badge to each Goal and a Mission filter or link where useful. It does not gain a Mission lifecycle, artifact panel, or Outcome controls.

Feature movement explains when Mission scope prevents an otherwise valid bulk action. Existing order and membership behavior remains authoritative as described in [Feature surface intent](intent/05-surfaces/03-browser/07-feature.md).

### Dashboard changes

Add a compact Mission summary answering:

- which Missions are active;
- which await plan or Outcome approval;
- which need a decision;
- which wave or required Goal is blocked;
- which Outcome is consolidating or ready for inspection.

Dashboard remains orientation and routing, not a Mission editor. Goal workflow visualization remains leaf-work truth; Mission summary is not combined into Goal status counts.

### Changes, Logs, Processes, and toolbar

- Changes rows derived from Mission Goals show the Mission badge and link.
- Logs and Activity accept optional `mission_id` and Mission filters.
- Processes identify workflow-owned Mission agents by Mission, Round, phase, and attempt.
- After Mission attachment ships, the workbench offers `Open Mission Agent` only while an attachable Mission phase process exists; before then no placeholder action appears.
- Toolbar attachment uses an explicit Mission launch surface and never infers eligibility from a generic agent profile.
- Stopping a Mission agent follows existing process-exit confirmation and Mission recovery rules; it does not cancel the Mission implicitly.

### Browser implementation shape

Add route modules following existing static surface organization, for example:

```text
src/surfaces/web/static/js/features/missions-list.js
src/surfaces/web/static/js/features/missions-new.js
src/surfaces/web/static/js/features/missions-detail.js
src/surfaces/web/static/js/features/missions-context.js
src/surfaces/web/static/css/missions.css
```

Register Missions in `index.html`, `router.js`, command registry, static asset serving, and static-surface tests. Reuse shared tables, modal utilities where appropriate, operations, errors, DOM morphing, and SSE reconciliation. Do not introduce a frontend build system for Mission.

Register `/api/missions` with the existing screen cache and list prefetch. Track the active Mission route, include it in debounced refresh, and reconcile list/detail state after SSE reconnect. The browser treats events as invalidation and rereads authoritative Mission state.

## CLI surface

Add top-level `refine mission`. Commands map to real Application capabilities and route through the active checkout-owned daemon.

Core commands:

```text
refine mission create <name> --intent <text>|--file <path> --reporter <name>
refine mission list [filters]
refine mission show <id>
refine mission edit <id> [...]
refine mission round <id> --reporter <name> --prompt <text>|--file <path>
refine mission start <id>
refine mission approve-plan <id> --plan-digest <digest>
refine mission decide <id> <decision-id> --choice <choice> --rationale <text>
refine mission add-goal <id> <goal-id> [...role and wave options]
refine mission remove-goal <id> <goal-id>
refine mission approve-outcome <id> --outcome-digest <digest>
refine mission retry <id> [--stage <stage>]
refine mission cancel <id> [--cascade queued|cancellable]
refine mission transfer <id> <node-id>
refine mission outcome <id> [--round <number>] [--output <path>]
```

The command set is intentionally missing:

- `set-status`;
- `publish`;
- `reconcile`;
- direct artifact mutation;
- direct snapshot creation;
- arbitrary dependency editing.

Those are workflow or Application responsibilities.

Dedicated context and artifact subcommands are omitted from v1: `show` exposes their references, while raw reads remain available through the shared request path. Add ergonomic commands later only when repeated use justifies them.

`add-goal` and `remove-goal` edit the Draft plan or draft an amendment; they do not bypass `approve-plan`. `retry` is available only for retryable stage-failure attention, never for a terminal Failed Round.

CLI reads print stable pretty JSON unless Outcome export explicitly requests raw content. Mutations carry API contract version and idempotency key. Long actions print the durable operation receipt immediately; `--wait` optionally follows that bounded operation to terminal success, failure, cancellation, interruption, or timeout. It does not wait for the whole Mission.

`refine commands` and `refine next` include Mission capabilities. `next` prioritizes explicit Mission approvals, decisions, blocked waves, and publication recovery without hiding higher-severity node or state-sync failures.

Existing Goal commands gain only optional composition data: `refine goal list --mission <id>` filters membership, and Goal detail includes Mission binding, pinned snapshot, and contribution state when present. Their output and behavior are unchanged for standalone Goals.

## HTTP API surface

Mission routes live under the existing `/work` Application group:

```text
GET    /work/missions
POST   /work/missions
GET    /work/missions/{mission_id}
PATCH  /work/missions/{mission_id}

POST   /work/missions/{mission_id}/rounds
POST   /work/missions/{mission_id}/start
POST   /work/missions/{mission_id}/approve-plan
POST   /work/missions/{mission_id}/decisions/{decision_id}
POST   /work/missions/{mission_id}/approve-outcome
POST   /work/missions/{mission_id}/retry
POST   /work/missions/{mission_id}/cancel
POST   /work/missions/{mission_id}/transfer

POST   /work/missions/{mission_id}/goals
DELETE /work/missions/{mission_id}/goals/{goal_id}

GET    /work/missions/{mission_id}/context
GET    /work/missions/{mission_id}/artifacts/{artifact_key}
GET    /work/missions/{mission_id}/outcome
```

List query supports status, attention, reporter, assignee, coordinator, Node scope, input Mission, Outcome availability, text, sort, direction, page, and limit.

Every mutation:

- uses shared Application services;
- requires a Mission idempotency key, validated by the new handler and replayed by the existing transport support;
- carries `observed_revision` when mutating an existing Mission and the exact plan, decision, or Outcome digest when applicable;
- returns authoritative read-back or a durable operation receipt;
- emits structured conflict reasons for stale state, changed evidence, invalid capability, incompatible Node, or ambiguous ownership;
- never accepts arbitrary status replacement.

Browser, CLI, and MCP request clients generate one idempotency key per user action and retain it for safe retry. Mission creation has no prior revision; every later mutation does.

`approve-plan` revalidates charter digest, effective plan digest, current snapshot, Goal key uniqueness, Feature scope, fleet eligibility, and Mission revision before authorization.

`approve-outcome` revalidates the exact Quality and Governance candidate and returns the consolidation operation. There is no separate ordinary `/publish` route.

`retry` claims only a retryable stage-failure attempt in a nonterminal Round. A terminal Failed Mission returns the `new_round_required` capability instead.

`POST .../goals` accepts a batch of Goal adoption specifications. Before first approval it edits the Draft plan; afterward it drafts a material amendment. It does not attach or admit the Goals until the resulting effective plan digest is approved. Applying an approved adoption or safe removal commits the plan/amendment and Goal bindings under one repository-coordinated transaction; partial membership success is forbidden and Mission still stores no duplicate Goal-id list.

Goal adoption or removal is allowed only while the Goal and Mission plan are safely editable. Once a GoalRound has pinned Mission context, membership is historical; later exclusion is a plan amendment rather than deletion of the binding.

Existing Goal list reads accept an optional Mission filter, and Goal detail includes optional Mission binding, context, and contribution fields. Existing Feature reads may include Mission badges on projected members. No separate page-shaped endpoints are added for these views.

API group discovery updates `/work` capability text to include Missions. Normalize `/api/missions` to the same handler beside the existing Goal and Feature aliases; it is an alias, not a second API. Additive browser routes ship with the daemon, while CLI and cross-node mutations obey the API contract version. The first public Mission mutation release should deliberately bump that contract.

## MCP surface

MCP remains a thin adapter over the daemon API. The first end-to-end vertical exposes Missions through `refine_request`; it needs no Mission-specific write or draft tool. Once the read schema and Outcome loop are stable, add only three high-value read conveniences:

```text
refine_list_missions
refine_show_mission
refine_show_mission_outcome
```

`refine_request` reaches the shared read and write routes with the same idempotency, revision, evidence, and recovery semantics. Mission framing requires structured nested values that do not fit the current fixed-field binding cleanly. Promote named write or draft tools only after a general structured binding exists and repeated use demonstrates their value.

MCP tool descriptions distinguish Mission outcome orchestration from Feature grouping and Goal execution.

## Installed-agent surface

CLI remains the primary explicit agent surface. Agents may list, inspect, draft, and operate Missions through `refine mission` commands or MCP.

After one-shot Mission-stage processes and their attachment identity are proven, add an attachable Mission workflow profile:

```text
refine agent open --profile mission <mission-id>
```

It attaches only to the exact live investigation, planning, reconciliation, synthesis, Quality, or Governance phase process. It never launches a diagnostic substitute when workflow is between phases and never creates a duplicate session merely for inspection.

Mission agents receive pinned Mission phase context and structured completion contracts. They do not own state transitions, Goal settlement, artifact promotion, or Outcome publication.

Goal agents receive Mission context automatically through GoalRound context. They do not need Mission CLI calls to discover mutable sibling state during execution.

The first vertical does not require this profile. Until attachment is delivered, Mission processes remain inspectable and cancellable through existing Process and operation surfaces without launching a substitute agent session.

## Activity, operations, processes, and SSE

### Activity

Extend durable activity with optional `mission_id` and `mission_round` fields. High-signal events include:

- Mission created or Round appended;
- investigation or plan completed;
- plan approved;
- wave admitted or settled;
- snapshot published;
- decision requested or answered;
- required Goal failed;
- synthesis, Quality, or Governance settled;
- Outcome approved;
- consolidation completed or failed;
- Mission transferred, cancelled, failed, or done.

Activity records summaries and exact evidence references, not full provider output or artifact bodies.

### Operations and processes

Goal materialization/distribution, agent phases, collective review, and consolidation expose existing durable operation and managed-process patterns. Operation ownership includes Mission id, Round, phase, and attempt. Owner-aware cancellation fences the exact operation before returning a terminal result.

Reload and daemon restart recover operation state. A running receipt is never presented as completion.

### SSE

Do not add a Mission-specific SSE event in v1. Extend `project_updated` with Mission counts and an attention digest, and reuse `activity_added`, `operation_progress`, process, runtime, and state-sync-health events. All are invalidation or progress signals, not authoritative Mission payloads. Initial load and reconnect perform one authoritative HTTP reconciliation; subsequent signals trigger a debounced reread without polling.

## Surface parity matrix

| Capability | Browser | CLI | API | MCP |
| --- | --- | --- | --- | --- |
| List/show Mission | First-class | First-class | First-class | First-class |
| Create/edit Draft | First-class | First-class | First-class | Via request |
| Start and approve plan | First-class | First-class | First-class | Via request |
| Inspect plan/waves | First-class visual | JSON | JSON | Show tool |
| Inspect Mission context | Semantic diff | Show references/request | JSON/content | Show/request |
| Add/remove safe Goal membership | Plan UI | First-class | First-class | Via request |
| Answer decision | Decision card | First-class | First-class | Via request |
| Inspect child Goal workflow | Nested links | Goal commands | Goal routes | Goal tools/request |
| Review/approve Outcome | Criteria matrix | First-class | First-class | Via request |
| Retry/cancel/transfer | Capability actions | First-class | First-class | Via request |
| Read published Outcome | First-class | First-class | First-class | First-class |
| Follow long operation | Live + reconnect | Receipt; optional wait | Operation receipt | Request operation |

This is target parity after phased delivery. In the first vertical, generic `refine_request` satisfies MCP access before named Mission reads arrive. Different ergonomics are allowed. Semantics, capability checks, evidence, and errors are shared.

## Representative user journey

1. The user creates `Modernize authentication` and binds the exact published Outcome from `Document current platform`.
2. Refine creates a Draft. The user starts investigation.
3. Investigation produces cited system facts, an architecture model, a risk register, and proposed criteria in snapshot 1.
4. Mission planning proposes three waves and a criteria coverage matrix. The user edits one Goal specification and approves the exact plan.
5. The Mission materializes stable Goal keys and distributes wave 1 to compatible Nodes.
6. Every GoalRound receives a scoped capsule from snapshot 1. One Goal discovers an undocumented token invariant and returns exact evidence plus an interface-contract artifact candidate.
7. Reconciliation selects that candidate into snapshot 2 with `goal_review_pending` provenance and drafts the required change to one future Goal specification.
8. The user answers the required decision and approves the amended effective plan digest. Later Goals receive snapshot 2; active and completed wave-1 GoalRounds remain bound to snapshot 1.
9. Wave 2 runs across the fleet. Ordinary Goal Quality and Governance validate each code candidate.
10. Mission synthesis creates the candidate modernization report and final context.
11. Mission Quality finds code and tests coherent but rollback verification absent. It drafts one bounded recovery-wave amendment; approval authorizes one new Goal.
12. After recovery, Mission Governance passes the exact target head, snapshot, and Outcome manifest.
13. Mission Review shows every criterion and its evidence. The user approves the Outcome, collectively approving unchanged contained Goals still in Review.
14. Consolidation commits the Outcome to `refine/state`, reads the exact bytes back, commits and verifies the terminal receipt, and exposes the Mission as Done.
15. A later Mission selects that exact Outcome. It begins with the accumulated model and evidence rather than rediscovering the system.

## Implementation plan

Each phase includes its Application behavior and thin surfaces; browser and CLI should not be deferred until the engine is complete.

### Phase 1: Model, storage, and read surfaces

- Add Mission types, status transitions, paths, JSON validation, and optimistic revision.
- Add optional Goal and GoalRound Mission fields with legacy deserialization.
- Add `mission.json` source fingerprints, Mission/Goal indexes, and incremental rollup support.
- Add Mission list/detail projections and API reads.
- Add `refine mission list/show`; expose API reads to MCP through `refine_request`.
- Add browser Missions navigation, list, and read-only workbench.
- Add Mission-aware Goal and Feature projection badges.

Exit proof: a hand-authored durable Mission and legacy Goals project identically across API, CLI, MCP, and browser; cache rebuild is exact.

### Phase 2: Authoring, investigation, and plan approval

- Add Mission create/edit/start/Round services.
- Add investigation staging, artifact validation, and snapshot publication.
- Add Mission plan proposal, criticism, revision, and exact approval.
- Add create/edit/start/approve-plan surfaces.
- Add one-shot durable Mission phase processes, activity, operations, and shared SSE invalidation.

Exit proof: a user creates a Mission, approves an investigated plan, and sees the same exact effective plan digest and capabilities on every surface; no Goal is created before approval.

### Phase 3: Goal context and findings loop

- Add stable Mission Goal keys and idempotent materialization/adoption.
- Add the deterministic Mission context compiler.
- Pin Mission capsules into GoalRound agent context.
- Extend Goal completion with optional structured findings and artifact candidates tied to approved output obligations.
- Persist validated candidate bytes as immutable GoalRound contribution evidence; only reconciliation selects them into snapshots.
- Add fenced reconciliation and next-snapshot publication.
- Add Goal modal Mission panels and Context semantic diff.

Exit proof: two parallel Goals consume one snapshot; one returns review-ready findings and an artifact candidate; reconciliation publishes the next snapshot with exact provisional provenance, and a later Goal consumes it while the earlier GoalRound remains unchanged.

### Phase 4: Waves and fleet orchestration

- Add wave admission and wait/reconcile loop.
- Compile placement into existing Goal and Feature fleet operations.
- Add compatibility, capacity, Feature scope, and state-sync freshness gates.
- Add distribution preview and Work/fleet UI.
- Add crash recovery around every child creation and move.

Exit proof: an interrupted multi-node wave resumes without duplicate Goals, duplicate moves, context drift, or lost Goal evidence.

### Phase 5: Synthesis, holistic gates, and Outcome

- Add synthesis staging and candidate final snapshot.
- Add Mission Quality and Mission Governance exact-candidate binding.
- Add criterion-first Mission Review and collective Goal approval.
- Add deterministic consolidation, persistence sync, Git read-back, and terminal receipt.
- Add Outcome browser, CLI, and API reads.
- Add the three named MCP Mission read tools after their schemas are stable.

Exit proof: Mission cannot expose Done until exact Outcome files and the later terminal record are proven in state commits; changing target head, snapshot, or manifest invalidates prior gates.

### Phase 6: Cross-Mission reuse and hardening

- Add exact Outcome picker and input bindings.
- Add input/consumer lineage projection.
- Add rolling-upgrade and incompatible-node behavior.
- Add scale limits, retention metrics, support-bundle evidence, and complete recovery UI.
- Add Dashboard, command-palette, Changes, Logs, and Process integrations after the core workbench loop is proven.
- Add the attachable Mission agent profile after one-shot process identity is proven.
- Exercise browser accessibility, reconnect, pagination, and large-corpus behavior.

Exit proof: a later Mission consumes an exact prior Outcome, rejects missing or changed digests, and never follows a newer result automatically.

## Verification plan

### Model and storage

- Every allowed and denied Mission transition has a table-driven test.
- Legacy Goal and GoalRound JSON deserialize with no Mission fields.
- Unknown Mission members survive unrelated Goal mutation.
- Active Round behavior remains bound to its frozen charter when top-level Mission framing changes.
- Plans, Goal specifications, and Outcomes reject unknown or duplicate criterion ids and artifact keys.
- Duplicate `(mission_id, mission_goal_key)` fails closed.
- Membership is derived from Goals; no second list can drift.
- Immutable path same bytes is idempotent; different bytes is corruption.
- Artifact path traversal, symlink, size, media, encoding, digest, and secret-policy failures are rejected.

### Context and findings

- Capsule compilation is deterministic for the same MissionSnapshot, Goal spec, Governance, and target head.
- Context selection honors applicability and budget and records inclusion reasons.
- A GoalRound cannot change snapshots after Plan admission.
- A stale Goal worker cannot settle findings against a newer Round, snapshot, candidate, or Mission attempt.
- Goal findings cannot directly create directives or overwrite artifacts.
- An artifact candidate with no approved obligation, wrong digest, changed handoff bytes, or stale GoalRound is rejected.
- A valid candidate remains noncanonical until one fenced reconciliation selects its exact reference into a snapshot.
- Parallel contributions append without overwriting one another.
- Contradictory findings produce preserved evidence and a typed decision.

### Workflow and recovery

- Crash after every Mission phase claim, agent completion, artifact promotion, snapshot publication, Goal creation, Goal move, reconciliation, review, and state commit resumes idempotently.
- Repeated Goal materialization creates one Goal per stable key.
- A material plan amendment changes the effective digest and blocks affected Goal attachment or admission until exact reapproval.
- Retryable stage failure remains nonterminal; a Failed Round rejects retry and requires a new Round.
- Required failed or cancelled Goals block acceptance without an exact waiver.
- Child terminal counts alone never complete Mission.
- Quality or Governance recovery is bounded and preserves the rejected candidate.
- Cancellation stops new admission and preserves active child Goal history.
- Transfer fences the coordinator without moving child Goals.
- Global workflow pause suppresses Mission and Goal automation consistently.

### Fleet and synchronization

- Only healthy, enabled, compatible Nodes receive Mission work.
- Active Goals never move.
- Ordered Feature placement is atomic within scope.
- Mixed Feature scope fails closed with a readable reason.
- Distribution preview and apply agree after revalidation or return a stale-state conflict.
- Disjoint Mission and Goal state edits converge.
- Competing coordinator transfers and active-Round edits fail closed without losing either side.
- Immutable contribution, snapshot, artifact, or Outcome disagreement is never resolved by timestamp.
- Old nodes continue syncing unknown Mission paths and unrelated work.
- Mission Goal dispatch excludes incompatible nodes and reports pending upgrade.

### Holistic gates and publication

- Mission Quality binds exact target head and snapshot.
- Mission Governance binds exact target head, snapshot, and manifest.
- Any component change invalidates the previous verdict.
- Collective Goal approval reuses Goal approval and rejects changed children.
- Review-ready evidence carries `goal_review_pending`; it cannot become a Decision or Directive, and later Goal decline invalidates dependent context.
- Done is impossible before Git read-back verifies every Outcome byte and the later terminal Mission record.
- Crash before or after publication commit recovers without a second Outcome.
- Cross-Mission input rejects unpublished, missing, floating, commit-mismatched, or digest-mismatched sources.

### Surfaces

- Browser, CLI, API, and MCP expose the same Mission identity, status, Round, plan digest, snapshot, and capabilities.
- Every mutation routes through the daemon and rejects stale revision.
- Every Mission mutation requires a replayable idempotency key; only existing-Mission mutations require an observed revision.
- Mission detail remains bounded and its Goal, history, finding, and artifact cursors cannot duplicate or skip records.
- Long operations survive reload and daemon restart.
- SSE reconnect refetches authoritative state once and does not poll.
- Mission list filters and detail section are URL-backed.
- Standalone Goal browser, CLI, API, and workflow behavior remains unchanged.
- Browser keyboard, focus, semantic headings, dialog, table, and narrow-screen behavior meet existing accessibility patterns.
- Browser tests that cannot run for lack of Chromium are reported as skipped, not passed.

### Scale

- Mission list and detail remain bounded with thousands of Missions and tens of thousands of Goals.
- Goals, findings, snapshots, and artifacts paginate or load incrementally.
- Projection updates reread moved Mission manifests rather than the entire corpus.
- Large artifacts do not enter prompt context or synchronized state by accident.
- Git object growth, Mission artifact bytes, snapshot counts, and retained refs are observable.

## Deliberately deferred

- Nested Mission execution.
- One Mission spanning several Target Apps.
- Artifact-only Goal workflow and polymorphic Goal delivery modes.
- Mutable or live inter-Goal communication.
- Arbitrary dependency graphs.
- Configurable per-Mission autonomous approval policies.
- Bulk Mission approval or publication.
- Destructive Mission or Outcome deletion.
- A global Artifacts navigation surface.
- A hosted Mission service.
- Premature extraction of a generic Goal/Mission workflow engine.

These may be added only when a real use case cannot be expressed through Mission Rounds, linear waves, immutable snapshots, ordinary Goals, and published Outcome bindings.

## Definition of a successful first release

The first release is successful when a user can:

1. create and investigate one Mission;
2. review and approve a plan of Goal waves;
3. watch ordinary Goals execute across the fleet with pinned shared context;
4. see a Goal finding and meta-code artifact candidate become reviewed Mission context for a later wave;
5. judge the combined result against Mission criteria;
6. approve one exact Outcome and prove it in Git-backed Refine state;
7. use that exact Outcome as context for another Mission;
8. perform all essential reads and decisions through browser, CLI, API, and agent-facing surfaces;
9. recover from interruption without duplicated Goals, moves, artifacts, snapshots, or Outcomes;
10. continue using standalone Goals exactly as before.

That is the smallest implementation that demonstrates the intended emergence: isolated Goal workflows becoming a learning, governed collective without surrendering their existing authority or recovery boundaries.
