# Mission Reconciliation

Status: proposed design
Scope: the reconciliation reducer, knowledge lineage, verification tiers, wave-boundary semantics, contested knowledge, and reduction-error recovery
Relationship: extends `mission-spec.md`; every addition is listed in [Spec deltas](#spec-deltas). Where this doc is silent, mission-spec.md governs.
Implementation: `src/application/missions/reconciliation/` (`ledger`, `verify`, `settlement`, `engine`, `capsule`); the deterministic steps are implemented and tested, and the agent phases are typed contracts consumed by the engine.

## Summary

Reconciliation is the only behavior that turns parallel Goal evidence into canonical Mission context. Everything else in the Mission design is mechanics: digests, gates, immutable files, frozen capsules. Reconciliation is where the system judges what it has learned, and a wrong reduction poisons every later capsule while passing every structural gate.

This design makes three commitments that mission-spec.md implies but does not specify:

1. **Lineage is a derived closure, not a stored graph.** Every piece of accepted knowledge gets a stable identity inside the snapshot that accepts it. What depends on what is computed from immutable history on demand. No mutable dependency records exist, honoring the Mission non-goals, because the graph is a disposable projection exactly like every other projection in Refine.
2. **Determinism has an honest boundary.** Deterministic verifiers prove provenance and machine-checkable claims. They never prove that a claim about the system is true. The auto-promotion policy is written against that boundary, so nothing semantic hides behind the word "deterministic."
3. **The reducer is itself criticized.** Planning gets proposal, independent criticism, and revision. Reconciliation, whose errors compound, gets the same loop. Dissent is preserved as evidence, never averaged away.

The governing failure mode this design prevents: a Mission that procedurally succeeds — every digest matches, every gate passes — while its canonical context is epistemically wrong, with no defined path to notice or repair.

## Position and authority

Reconciliation is a fenced Mission stage operation, not an entity. It:

- runs only at defined wave boundaries and correction triggers;
- claims one exact contribution set and one parent snapshot;
- may publish the next MissionSnapshot, draft a plan amendment, or raise typed decision requests;
- may never edit an accepted snapshot, a completed GoalRound, or a closed receipt;
- is the only behavior that promotes artifact authority beyond `Evidence`;
- is itself adjudicated: its drafts pass through adversarial criticism before publication.

Everything the Mission "knows" after reconciliation is either a file in an immutable snapshot or a derivation from that history. There is no reconciliation working state outside the durable receipt.

## The knowledge ledger

### Assertions

Each snapshot manifest carries a `knowledge_index`. An entry is an **assertion**: the unit of accepted knowledge and the unit of invalidation.

```text
KnowledgeAssertion
  assertion_id          stable, unique per Mission, never reused
  kind                  fact | model | risk | assumption | contradiction | question
  authority             Evidence | Model | Decision | Directive
  provenance            source contribution ref, investigation ref,
                        plan digest, or input Outcome ref
  qualified             goal_review_pending? | unverified_extent?
  supersedes[]          assertion ids this entry replaces
  corrects[]            assertion ids this entry reports as wrong
  derived_from[]        assertion ids this entry was reasoned from
  scope                 applicability statement
  scope_refs[]          structural applicability links: criterion ids,
                        artifact keys, Mission Goal keys
  evidence_refs[]       commits, paths, tests, logs
  supersedable          false for Decisions and Directives without
                        an explicit human gate
  members[]             kind == contradiction only: conflicting assertion ids
  resolution            kind == contradiction only: evidence | scope_split |
                        superseded | open
  resolved_by?          kind == contradiction only
```

`scope_refs` is what keeps invalidation and capsule compilation exact: an
assertion is structurally applicable to a Goal specification, criterion, or
artifact when a scope ref names it, never by matching free text. A
contradiction is a first-class assertion kind; its record lives in the
knowledge index like any other assertion, with `members` naming the
conflicting assertion ids.

This resolves the central tension in mission-spec.md: invalidation needs a dependency graph, but the design forbids one. The answer is that the graph is a pure function of immutable history. It is rebuilt like any other projection, and a cache rebuild must reproduce it exactly. (Implementation: `src/application/missions/reconciliation/ledger.rs`.)

### Contradictions

A contradiction is a first-class assertion kind, not an error condition: an assertion of kind `contradiction` whose `members` name two or more conflicting assertion ids, with a `resolution` of `evidence`, `scope_split`, `superseded`, or `open` and a `resolved_by` reference.

Reconciliation may resolve a contradiction three ways without a human: by **evidence** (one member's source is proven stale or unreachable), by **scope-split** (both members are true under different applicability; each is rewritten with narrowed scope), or by **supersession** (newer evidence replaces both). If none applies, the contradiction stays `open`, remains visible in every later capsule, and raises a Decision attention. Reconciliation must never resolve a contradiction by silently choosing the louder member.

### Where the ledger lives

`knowledge_index` members are data inside existing snapshot files. There is no new aggregate, no new top-level record, and no mutable artifact header. Artifacts remain files selected by `ArtifactRef`; assertions give those files' individual claims addressable identity.

## Verification tiers

Validation of a contribution happens in three tiers with different authorities. Which tier applies is declared by the finding kind and the verifier registry, never inferred from text.

| Tier | Checks | Authority over truth |
| --- | --- | --- |
| 1. Envelope | schema, path, media type, size, digest, provenance shape, obligation binding | None. A schema-valid claim may be false. |
| 2. Claim | named deterministic verifiers run against pinned evidence | Provenance and machine-checkable claims only |
| 3. Judgment | reduction agent, then adversarial critic | Proposed acceptance; never auto-authoritative |

The verifier registry in v1 (`src/application/missions/reconciliation/verify.rs`). Evidence references use an exact positional wire syntax so the applicable verifier is declared by shape, never inferred from claim text: `commit:<id>`, `path:<path>@<commit>`, `quote:<text>@<commit>`, `test:<name>`, `digest:<sha256>`. References without a registered verifier route to tier 3.

| Verifier | Proves | Deterministic because |
| --- | --- | --- |
| `quote_at_commit` | Quoted text exists at cited commit | Snapshot pins target_head; commits are immutable |
| `path_exists` | Cited path exists at cited commit | Same |
| `test_passed` | Cited test exists and its recorded result is in Goal evidence | Evidence is digest-bound |
| `commit_reachable` | Cited commit is reachable from the wave's target_head | Git history |
| `digest_matches` | Candidate bytes match their recorded digest | Already established |

A verifier failure at tier 2 does not discard the finding. It demotes the finding to `unverified` and routes it to tier 3 judgment with the failure recorded. Verifier results are part of the durable receipt and are reproducible: the same claim set, parent snapshot, and target head must yield byte-identical verifier output.

What tier 2 can never do: verify a universal negative ("no code calls this interface") beyond the mechanical greps it can name, compare two Goals' incompatible observations, or assess whether a derived model describes the system faithfully. The auto-promotion policy is sized to exactly what the registry can prove.

## Auto-promotion policy

Routing of each verified finding:

| Finding shape | Route | Resulting authority |
| --- | --- | --- |
| Claim restating tier-2-verifiable evidence | Auto-promote | Evidence, `goal_review_pending` if source Goal is in Review |
| Derived description with full source coverage (every claim links evidence) | Agent judgment, critic confirmation | Model |
| Proposed design choice | Never auto | Decision (human gate, per authority table) |
| Proposed normative guidance | Never auto | Directive (human gate) |
| Universal negative without a named verifier | Agent judgment | Model, flagged `unverified_extent` |
| Contradiction with members | Reduction resolves or escalates per above | As resolved |

Rules:

- Auto-promotion may never raise authority above Model, may never resolve a contradiction by preference, and may never create a Directive.
- Source coverage is mandatory for Model authority: a model artifact whose claims do not each cite evidence refs is returned for bounded repair, then parked as `deferred` if repair fails.
- The boundary between auto-promotable and human-routed is configuration data (the registry plus this table), not prompt discretion.

## The reconciliation loop

One fenced attempt per wave boundary. Attempt identity is monotonic: `mission:<id>:round:<n>:reconcile:<wave>:<attempt>`.

```text
1. claim
   fence the attempt; freeze the exact contribution set,
   parent snapshot, target head, and plan digest
2. envelope verification (deterministic, tier 1)
   reject malformed contributions with durable diagnostics
3. claim verification (deterministic, tier 2)
   run the verifier registry; record per-finding results
4. reduction draft (agent, bounded)
   group compatible findings; separate facts, proposals,
   contradictions, unproven claims; draft assertion and
   artifact changes; draft affected-spec amendments
5. adversarial criticism (fresh agent, bounded)
   attempt to refute each proposed acceptance and rejection
   using only the pinned claim set and parent snapshot
6. revision (agent, bounded)
   apply surviving criticisms; preserve dissent verbatim;
   open contradictions stay open
7. publication (deterministic)
   publish one snapshot, or draft the amendment, or raise
   typed decision requests; write the receipt
```

Steps 4-6 mirror the plan proposal/criticism/revision rhythm deliberately. The critic sees the draft, the claim set, and the parent snapshot; it does not see the drafter's reasoning transcript, only its output, so criticism targets the artifact rather than the author.

### Criticism contract

The critic must, for each drafted acceptance: re-check cited evidence, attempt to construct a counter-case from the claim set, and mark the acceptance `confirmed`, `contested`, or `insufficient_evidence`. For each drafted rejection: check whether rejection discards evidence rather than reasoning. Criticism output is preserved verbatim in the receipt as first-class evidence. Disagreement between draft and criticism that revision cannot settle becomes a contested assertion plus, when load-bearing, a Decision attention.

### Budgets

Each attempt carries independent bounded budgets:

| Budget | Governs | Exhaustion |
| --- | --- | --- |
| `repair` | Structured-output repair of agent phases | Fail attempt, retryable attention |
| `agent` | Reduction, criticism, revision calls | Publish partial reconciliation with `deferred` findings and a Risk attention |
| `decision` | Human interrupts raised by this attempt | See [Decision traffic](#decision-traffic) |
| `publication` | Snapshot write attempts | Fail attempt, retryable |

Exhaustion never loops and never silently drops evidence: whatever was not consumed is recorded in the receipt as `deferred` and enters the next boundary's claim set.

## Wave-boundary semantics

### When reconciliation fires

A wave is **settled** when:

1. every required Goal of the wave is terminal or in Review with valid integration, Quality, and Governance evidence; and
2. every optional Goal is terminal, in Review with valid evidence, or has exceeded the configured optional-wait budget.

Settlement is evaluated by the workflow runner; reconciliation runs as a one-shot supervised stage operation once settled. Expected capacity waits elsewhere in the fleet are neutral and do not delay settlement.

### Stragglers and late contributions

A closed reconciliation is closed. A contribution settling after the claim is `late`:

- late contributions are durable, inspectable evidence owned by their GoalRound;
- they are never consumed by the attempt that already closed;
- they enter the claim set of the **next** reconciliation unconditionally;
- if the wave was the last execution wave, a mandatory **pre-Synthesis sweep** reconciliation claims all remaining contributions before Synthesis begins, so no evidence is lost to ordering;
- snapshot cadence is therefore bounded: one investigation snapshot, one per wave boundary (including the sweep), plus correction snapshots. A single straggler never mints a snapshot by itself.

Late contributions are claims about the world as of their GoalRound's pinned snapshot. At consumption time their evidence is re-verified against the current target head; unreachable evidence marks the finding stale rather than wrong, and staleness is a Risk attention, not a contradiction.

## Invalidation

Invalidation triggers:

1. a Goal whose contribution was consumed leaves Review, gains another Round, or is declined;
2. a correction assertion reports an earlier assertion as wrong;
3. a later Goal's `challenged_assumption` survives criticism in a subsequent reconciliation.

The closure rule, computed over the ledger:

```text
invalidated(a) :=
    provenance(a) is an invalidated source
 or exists b: corrects(b) contains a and b is active
 or exists p: derived_from(a) contains p and invalidated(p)

affected GoalRounds := those whose pinned capsule manifest
    includes an invalidated assertion
affected specs      := MissionGoalSpecs whose criteria, inputs, or
    obligations reference an invalidated assertion through its
    scope_refs
```

Invalidation propagates from premise to derivation: when a fact falls, the
models reasoned from it fall with it, transitively. A correction only holds
while its corrector survives; a retracted correction lifts. Superseded
assertions keep their history but are never reported invalidated: their
consumers already moved to the superseding assertion.

Effects:

- affected future specs cannot be admitted; the engine raises a Blocker attention naming the exact assertions and the amendment path;
- affected active GoalRounds are labeled `premises_invalidated`; the Mission never kills them, but their evidence cannot support criteria until re-based on a snapshot without the invalidated assertion;
- completed GoalRounds and their contributions are never rewritten; they remain history with their observations intact.

The closure is a projection: deterministic, disposable, and reproducible from snapshot history alone. `capsule_manifest` (see [Spec deltas](#spec-deltas)) is what makes "affected" exact — a GoalRound is affected only if its capsule actually included the assertion, which the context compiler already records.

### Correction snapshots

Because snapshots are immutable, reduction errors are repaired by appending, never editing. A **correction snapshot** is an ordinary reconciliation attempt with a small or empty contribution set and a correction mandate:

- it supersedes or corrects the named assertions and records why;
- its provenance is `reduction_error`, `source_invalidated`, or `challenge_accepted`;
- consumers are computed by the closure and surfaced with a one-line reason each;
- a correction triggered by Goal decline (mechanical invalidation) must complete before the next wave admits Goals.

This gives the system epistemic recovery: a wrong-but-valid reduction is repairable through the same append-only channel that built it.

## Decision traffic

Decision volume is the product risk that turns an autonomous fleet into a human queue. Policy:

- A typical wave boundary should produce **zero or one** human interrupts. More than two is a plan-quality signal: the reconciliation receipt records a `plan_quality` note when decision volume exceeds the threshold, because contradictions at scale usually mean the plan's wave decomposition was wrong, not that the world is contradictory.
- Reconciliation must **batch and rank**: related decision requests are grouped into one attention with per-item choices, not one attention per finding.
- The `decision` budget shapes presentation, never truth. When ranked requests exceed the budget, lower-ranked items become `deferred` findings with a single summarizing Risk attention. Load-bearing items (contradiction affecting a criterion, material amendment, authority promotion) cannot be deferred and do not count against being surfaced first.
- Auto-resolving or averaging a genuine decision to stay under budget is prohibited and testable.

## Capsule rendering of contested and dissenting knowledge

The context compiler renders the ledger honestly rather than comfortably:

- A `contested` assertion renders as the full set of members with their provenances and the open question, labeled `contested`. The compiler never selects a winner.
- Surviving criticism attached to an accepted assertion renders as a bounded `dissent` note plus a pointer into the receipt.
- An `invalidated` assertion never enters a new capsule. Superseded assertions do not either; the superseding assertion enters instead, with `supersedes` context when the Goal's role needs the change history.
- A contested or dissenting assertion that is load-bearing for a Goal spec (criterion-relevant or adjacent to a Directive) blocks that spec's admission pending the decision. Background contested context passes through labeled; Goal agents are entitled to work under uncertainty, and their completion contract already includes `challenged_assumptions` as the feedback path.
- Capsule budget accounting counts contested pairs at full member cost; the compiler may not drop a member to fit, it must defer the whole pair and record why.

A Goal that receives contested context may challenge either member; surviving challenges are the primary correction trigger, closing the loop between context consumers and context quality.

## Domain invariants

1. Reconciliation is the only reducer of Mission context; no other behavior writes snapshots or promotes authority.
2. Every accepted assertion has a stable id and explicit provenance; ids are never reused within a Mission.
3. Assertion state is derived from snapshot history and is never stored mutably.
4. A closed reconciliation attempt and its receipt are immutable; late evidence waits for the next boundary.
5. Tier-2 verification proves provenance and machine-checkable claims only; it may never promote authority above Model nor resolve a contradiction by preference.
6. Dissent and contested members are preserved verbatim; the compiler and reducer never average them.
7. Invalidation is a computed closure over immutable history; it never edits a snapshot, receipt, or GoalRound.
8. A GoalRound is affected by an invalidation only if its pinned capsule manifest included the invalidated assertion.
9. The decision budget shapes batching and presentation; it may never suppress a load-bearing decision or auto-resolve one.
10. Every attempt carries independent bounded budgets with explicit exhaustion semantics; exhaustion defers evidence, it never discards it.
11. A correction snapshot records exactly what it corrects and why; consumers are computed by the closure, each with a reason.
12. Verification and capsule compilation are deterministic functions of (claim set, parent snapshot, target head, plan digest).

## Spec deltas

This design adds members to existing embedded records. No new aggregate, entity, or top-level surface is introduced.

`MissionSnapshot` gains:

```text
  knowledge_index[]
  corrects_snapshot?     set when this snapshot is a correction
```

`ReconciliationReceipt` gains:

```text
  attempt               mission:<id>:round:<n>:reconcile:<wave|sweep>:<k>
  claim_set[]            exact contribution digests claimed
  verifier_results[]     per-finding tier-2 outcomes
  criticism_ref          verbatim criticism evidence reference
  dissent[]              preserved criticism that revision overruled
  deferred[]             unconsumed findings, carried to next boundary
  decision_requests[]    ranked, batched
  budgets                per-budget usage and limits
  plan_quality?          note when decision volume exceeds threshold
  correction?            provenance and reason when this attempt
                        is a correction snapshot
```

`GoalRound.mission_context` gains:

```text
  capsule_manifest_digest  digest of the included-assertion and
                           included-artifact manifest with reasons
```

Mission prompt templates (mission-spec.md, "Prompt and structured-output contracts") split `reconciliation` into `reduction`, `adversarial criticism`, and `revision`, and add `correction reconciliation` as a variant input of reduction. The browser reconciliation view (mission-spec.md, "Context experience") renders verifier badges, contested pairs, the decision batch, the deferred queue, and the correction timeline. Attention classes are reused; no new class is added.

## Verification plan

Determinism and closure:

- The same claim set, parent snapshot, target head, and plan digest produce byte-identical verifier results, capsule manifests, and assertion ids across repeated runs.
- Given a synthetic ledger with supersession, correction, and derivation chains, the computed invalidation closure equals a hand-computed expected set, including transitive derivation and the "not superseded" exemption.
- A GoalRound whose capsule excluded an assertion is unaffected when that assertion is invalidated; a GoalRound whose capsule included it is labeled.
- Assertion state is never stored: corrupting or deleting all projections and rebuilding reproduces identical closure and rendering.

Reducer quality control:

- Agent output containing a proposed Decision or Directive never results in that authority without a recorded human action.
- Criticism evidence survives verbatim in the receipt; an overruled criticism appears in `dissent`.
- A schema-valid but evidence-free model artifact is not promoted; it is repaired within budget or deferred.
- An irreconcilable draft/criticism disagreement produces a contested assertion and, when criterion-relevant, a blocking Decision attention.

Boundaries and stragglers:

- A contribution settling after the claim cannot enter the closed receipt; the same bytes are claimed by the next boundary; after the final wave, the pre-Synthesis sweep claims them.
- Optional-wait expiry settles the wave with the optional Goal's later contribution classified `late`.
- Stale evidence (commit unreachable from current target head) produces a Risk attention, not a contradiction record.
- Budget exhaustion produces a receipt with `deferred` entries and no evidence loss; no attempt loops.

Decision traffic:

- A wave boundary generating more than the configured decision threshold produces a `plan_quality` note.
- Ranking never reorders a load-bearing decision below the budget line; deferral never applies to it.
- Batched decisions present one attention with per-item choices.

Corrections:

- A Goal decline after its `goal_review_pending` evidence was consumed triggers a correction attempt before the next admission; the correction snapshot lists corrected assertions with reasons.
- A surviving `challenged_assumption` from a later Goal Round appears in the next claim set with elevated priority.
- A correction invalidating an assertion used by an active GoalRound labels that Round and blocks its evidence from criteria coverage until re-based.

## Deliberately deferred

- Pluggable third-party or custom verifiers beyond the named registry.
- Cross-Mission assertion-level lineage (Outcome bindings remain the unit of cross-Mission reuse).
- Automatic scope-split suggestions beyond the reduction agent's normal draft.
- Confidence scores or numeric epistemics; qualified provenance labels carry the v1 signal.
- Pruning or compacting the ledger for very long-lived Missions; scale limits come first.

## Open questions

- Default `decision` budget and the plan-quality threshold need calibration from real wave volumes; the mechanism ships configurable.
- Whether the criticism agent should see prior attempts' criticism within the same Round, or only the current draft. Current design: only the current draft, to keep criticism independent.
- Whether correction snapshots should be visually distinguished in the browser timeline beyond their `corrects_snapshot` marker.
- The minimal ledger coverage needed for the first vertical: investigation snapshot plus one wave boundary, before any long-Mission scale work.
