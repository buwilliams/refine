//! The knowledge ledger: derived assertion state and the invalidation
//! closure.
//!
//! Assertion state is never stored. `active`, `superseded`, `contested`, and
//! `invalidated` are computed by walking the immutable snapshot chain, so the
//! dependency structure the Mission needs for invalidation is a disposable
//! projection exactly like every other projection in Refine. A cache rebuild
//! must reproduce it exactly.
//!
//! See `docs/mission-reconciliation.md` ("The knowledge ledger" and
//! "Invalidation").

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::mission::{
    ContradictionResolution, KnowledgeAssertion, Mission, MissionGoalSpec,
};

/// The derived state of one assertion. Precedence when several apply:
/// superseded, then invalidated, then contested, then active.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionState {
    Active,
    Superseded,
    Invalidated,
    Contested,
}

/// Why one assertion is invalidated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidatedBecause {
    /// The provenance source (a Goal contribution or Outcome input) was
    /// invalidated, for example because its Goal left Review or was declined.
    SourceInvalidated { source: String },
    /// A later surviving assertion reports this one as wrong.
    CorrectedBy { by: String },
    /// A premise this assertion was derived from was invalidated.
    DerivedFrom { premise: String },
}

/// One GoalRound's pinned capsule, reduced to the exact assertions it
/// included. This is what makes "affected" exact: a GoalRound is affected by
/// an invalidation only if its capsule actually included the invalidated
/// assertion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapsuleBinding {
    pub goal_id: String,
    pub goal_round: usize,
    pub mission_goal_key: String,
    pub assertion_ids: Vec<String>,
}

/// A GoalRound whose pinned premises no longer hold.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AffectedGoalRound {
    pub goal_id: String,
    pub goal_round: usize,
    pub mission_goal_key: String,
    pub invalidated_assertions: Vec<String>,
}

/// A Goal specification whose criteria, inputs, or obligations reference
/// invalidated knowledge.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AffectedGoalSpec {
    pub mission_goal_key: String,
    pub invalidated_assertions: Vec<String>,
}

/// The computed invalidation report for one Mission.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct InvalidationReport {
    pub invalidated: BTreeMap<String, InvalidatedBecause>,
    pub superseded: BTreeSet<String>,
    pub contested: BTreeSet<String>,
    pub affected_goal_rounds: Vec<AffectedGoalRound>,
    pub affected_specs: Vec<AffectedGoalSpec>,
}

/// Walk the Mission's immutable snapshot chain and derive the state of every
/// assertion in its knowledge ledger.
///
/// `invalidated_sources` is the set of provenance sources known to be
/// invalidated (for example, contribution references whose Goal left Review
/// or was declined). It is an input, not a derivation: the caller computes it
/// from Goal state.
pub fn derive_assertion_states(
    mission: &Mission,
    invalidated_sources: &BTreeSet<String>,
) -> BTreeMap<String, AssertionState> {
    let ledger = collect_ledger(mission);
    let mut states = BTreeMap::new();
    for id in &ledger.order {
        states.insert(id.clone(), derive(id, &ledger, invalidated_sources, 0).0);
    }
    states
}

/// Compute the full invalidation report: invalidated assertions, the
/// GoalRounds whose pinned capsules included them, and the Goal
/// specifications that reference them.
///
/// The closure rule (`docs/mission-reconciliation.md`, "Invalidation"): an
/// assertion is invalidated when its provenance source is invalidated, when a
/// surviving later assertion corrects it, or when any premise it was derived
/// from is invalidated. Superseded assertions keep their history but their
/// consumers already moved to the superseding assertion, so supersession
/// outranks invalidation.
pub fn compute_invalidation(
    mission: &Mission,
    invalidated_sources: &BTreeSet<String>,
    capsules: &[CapsuleBinding],
) -> InvalidationReport {
    let ledger = collect_ledger(mission);
    let mut report = InvalidationReport::default();

    let mut superseded: BTreeSet<String> = BTreeSet::new();
    for assertion in ledger.by_id.values() {
        for id in &assertion.supersedes {
            superseded.insert(id.clone());
        }
    }
    report.superseded = superseded.clone();

    let mut invalidated: BTreeMap<String, InvalidatedBecause> = BTreeMap::new();
    let mut contested: BTreeSet<String> = BTreeSet::new();
    for id in &ledger.order {
        let (state, reason) = derive(id, &ledger, invalidated_sources, 0);
        match state {
            AssertionState::Invalidated => {
                if let Some(reason) = reason {
                    invalidated.insert(id.clone(), reason);
                }
            }
            AssertionState::Contested => {
                contested.insert(id.clone());
            }
            _ => {}
        }
    }
    report.contested = contested;
    report.invalidated = invalidated.clone();

    // Affected GoalRounds: only those whose pinned capsule manifest actually
    // included an invalidated assertion.
    let mut affected_rounds: BTreeMap<(String, usize), AffectedGoalRound> = BTreeMap::new();
    for capsule in capsules {
        let hits: Vec<String> = capsule
            .assertion_ids
            .iter()
            .filter(|id| invalidated.contains_key(*id))
            .cloned()
            .collect();
        if !hits.is_empty() {
            affected_rounds
                .entry((capsule.goal_id.clone(), capsule.goal_round))
                .or_insert_with(|| AffectedGoalRound {
                    goal_id: capsule.goal_id.clone(),
                    goal_round: capsule.goal_round,
                    mission_goal_key: capsule.mission_goal_key.clone(),
                    invalidated_assertions: Vec::new(),
                })
                .invalidated_assertions
                .extend(hits);
        }
    }
    report.affected_goal_rounds = affected_rounds.into_values().collect();

    // Affected specs: an invalidated assertion whose structural scope links
    // (criterion ids, artifact keys, Mission Goal keys) name the spec.
    let specs = mission_goal_specs(mission);
    let mut affected_specs: BTreeMap<String, AffectedGoalSpec> = BTreeMap::new();
    for id in invalidated.keys() {
        let Some(assertion) = ledger.by_id.get(id) else {
            continue;
        };
        for spec in &specs {
            if !assertion
                .scope_refs
                .iter()
                .any(|link| spec_references(spec, link))
            {
                continue;
            }
            affected_specs
                .entry(spec.mission_goal_key.clone())
                .or_insert_with(|| AffectedGoalSpec {
                    mission_goal_key: spec.mission_goal_key.clone(),
                    invalidated_assertions: Vec::new(),
                })
                .invalidated_assertions
                .push(id.clone());
            break;
        }
    }
    for spec in affected_specs.values_mut() {
        spec.invalidated_assertions.sort();
        spec.invalidated_assertions.dedup();
    }
    report.affected_specs = affected_specs.into_values().collect();

    report
}

/// The assertions of the current Round's snapshot chain through one snapshot
/// version, each with its derived state. Capsule compilation renders the
/// accumulated ledger as accepted at that boundary, while later corrections
/// and supersessions still respect the full chain.
pub fn assertions_through(
    mission: &Mission,
    snapshot_version: usize,
    invalidated_sources: &BTreeSet<String>,
) -> Vec<(KnowledgeAssertion, AssertionState)> {
    let ledger = collect_ledger(mission);
    let round_number = mission.current_round.unwrap_or(0);
    let mut collected: Vec<(KnowledgeAssertion, AssertionState)> = Vec::new();
    if let Some(round) = mission
        .rounds
        .iter()
        .find(|round| round.number == round_number)
    {
        for snapshot in round
            .snapshots
            .iter()
            .filter(|s| s.version <= snapshot_version)
        {
            for assertion in &snapshot.knowledge_index {
                let state = derive(&assertion.assertion_id, &ledger, invalidated_sources, 0).0;
                collected.push((assertion.clone(), state));
            }
        }
    }
    collected
}

fn derive(
    id: &str,
    ledger: &Ledger,
    invalidated_sources: &BTreeSet<String>,
    depth: usize,
) -> (AssertionState, Option<InvalidatedBecause>) {
    // Cycles and pathological chains fail closed rather than recursing
    // forever; the ledger is append-only so legitimate chains stay shallow.
    let Some(assertion) = ledger.by_id.get(id) else {
        return (
            AssertionState::Invalidated,
            Some(InvalidatedBecause::SourceInvalidated {
                source: format!("unknown assertion {id}"),
            }),
        );
    };
    if depth > ledger.by_id.len() + 1 {
        return (
            AssertionState::Invalidated,
            Some(InvalidatedBecause::DerivedFrom {
                premise: id.to_string(),
            }),
        );
    }
    let superseded = ledger
        .by_id
        .values()
        .any(|other| other.supersedes.iter().any(|superseded| superseded == id));
    if superseded {
        return (AssertionState::Superseded, None);
    }
    if let Some(source) = assertion.provenance.as_ref()
        && invalidated_sources.contains(source)
    {
        return (
            AssertionState::Invalidated,
            Some(InvalidatedBecause::SourceInvalidated {
                source: source.clone(),
            }),
        );
    }
    if let Some(corrector) = surviving_corrector_of(assertion, ledger, invalidated_sources, depth) {
        return (
            AssertionState::Invalidated,
            Some(InvalidatedBecause::CorrectedBy {
                by: corrector.to_string(),
            }),
        );
    }
    for premise in &assertion.derived_from {
        let (state, _) = derive(premise, ledger, invalidated_sources, depth + 1);
        if state == AssertionState::Invalidated {
            return (
                AssertionState::Invalidated,
                Some(InvalidatedBecause::DerivedFrom {
                    premise: premise.clone(),
                }),
            );
        }
    }
    let contested = ledger.by_id.values().any(|other| {
        other.members.iter().any(|member| member == id)
            && other
                .resolution
                .is_none_or(|resolution| resolution == ContradictionResolution::Open)
    });
    if contested {
        return (AssertionState::Contested, None);
    }
    (AssertionState::Active, None)
}

/// The surviving corrector of one assertion: a later assertion listing it in
/// `corrects` that is itself active. The closure rule requires `b is active`,
/// so a contested or invalidated corrector does not invalidate its target.
fn surviving_corrector_of(
    assertion: &KnowledgeAssertion,
    ledger: &Ledger,
    invalidated_sources: &BTreeSet<String>,
    depth: usize,
) -> Option<String> {
    for other in ledger.by_id.values() {
        if !other.corrects.contains(&assertion.assertion_id) {
            continue;
        }
        let state = derive(&other.assertion_id, ledger, invalidated_sources, depth + 1).0;
        if state == AssertionState::Active {
            return Some(other.assertion_id.clone());
        }
    }
    None
}

struct Ledger {
    order: Vec<String>,
    by_id: BTreeMap<String, KnowledgeAssertion>,
}

fn collect_ledger(mission: &Mission) -> Ledger {
    let mut order = Vec::new();
    let mut by_id = BTreeMap::new();
    for round in &mission.rounds {
        for snapshot in &round.snapshots {
            for assertion in &snapshot.knowledge_index {
                order.push(assertion.assertion_id.clone());
                by_id.insert(assertion.assertion_id.clone(), assertion.clone());
            }
        }
    }
    Ledger { order, by_id }
}

fn mission_goal_specs(mission: &Mission) -> Vec<&MissionGoalSpec> {
    mission
        .rounds
        .iter()
        .flat_map(|round| round.plan.iter())
        .flat_map(|plan| plan.waves.iter())
        .flat_map(|wave| wave.goal_specs.iter())
        .collect()
}

fn spec_references(spec: &MissionGoalSpec, link: &str) -> bool {
    link == spec.mission_goal_key
        || spec.criterion_ids.iter().any(|id| id == link)
        || spec.input_artifact_keys.iter().any(|key| key == link)
        || spec.output_artifact_keys.iter().any(|key| key == link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mission::{
        ArtifactAuthority, AssertionKind, MissionGoalSpec, MissionPlan, MissionRound,
        MissionRoundRequest, MissionSnapshot, MissionWave,
    };

    fn assertion(id: &str) -> KnowledgeAssertion {
        KnowledgeAssertion {
            assertion_id: id.to_string(),
            kind: AssertionKind::Fact,
            authority: ArtifactAuthority::Evidence,
            provenance: None,
            qualified: None,
            supersedes: Vec::new(),
            corrects: Vec::new(),
            derived_from: Vec::new(),
            scope: None,
            scope_refs: Vec::new(),
            evidence_refs: Vec::new(),
            supersedable: true,
            members: Vec::new(),
            resolution: None,
            resolved_by: None,
        }
    }

    fn mission_with_snapshots(snapshots: Vec<Vec<KnowledgeAssertion>>) -> Mission {
        let mut mission = Mission {
            id: "MTEST".to_string(),
            name: "Test".to_string(),
            intent: "intent".to_string(),
            status: crate::model::mission::MissionStatus::Execute,
            reporter: None,
            assignee: None,
            coordinator_node_id: None,
            success_criteria: Vec::new(),
            artifact_contract: Vec::new(),
            current_round: Some(1),
            revision: 0,
            rounds: Vec::new(),
            created: "2026-01-01T00:00:00Z".to_string(),
            updated: "2026-01-01T00:00:00Z".to_string(),
        };
        let mut round = MissionRound {
            number: 1,
            request: MissionRoundRequest {
                intent: "intent".to_string(),
                constraints: Vec::new(),
                criteria: Vec::new(),
                artifact_obligations: Vec::new(),
                authorizing_request: "request".to_string(),
                charter_digest: None,
            },
            input_bindings: Vec::new(),
            plan: None,
            plan_amendments: Vec::new(),
            snapshots: Vec::new(),
            reconciliation_receipts: Vec::new(),
            phase_evidence: Default::default(),
            review: None,
            outcome: None,
            outcome_publication: None,
            failure: None,
            created: "2026-01-01T00:00:00Z".to_string(),
            updated: "2026-01-01T00:00:00Z".to_string(),
        };
        for (idx, knowledge) in snapshots.into_iter().enumerate() {
            round.snapshots.push(MissionSnapshot {
                version: idx + 1,
                parent_version: if idx == 0 { None } else { Some(idx) },
                target_head: None,
                plan_digest: None,
                artifact_refs: Vec::new(),
                input_refs: Vec::new(),
                consumed_contribution_refs: Vec::new(),
                knowledge_index: knowledge,
                corrects_snapshot: None,
                digest: None,
                created: "2026-01-01T00:00:00Z".to_string(),
            });
        }
        mission.rounds.push(round);
        mission
    }

    fn spec(key: &str, criteria: &[&str]) -> MissionGoalSpec {
        MissionGoalSpec {
            mission_goal_key: key.to_string(),
            name: format!("Goal {key}"),
            prompt: "prompt".to_string(),
            role: None,
            required: true,
            criterion_ids: criteria.iter().map(|c| c.to_string()).collect(),
            input_artifact_keys: Vec::new(),
            output_artifact_keys: Vec::new(),
            expected_findings: Vec::new(),
            feature_id: None,
            feature_order: None,
            preferred_node: None,
        }
    }

    fn plan_with(specs: Vec<MissionGoalSpec>) -> MissionPlan {
        MissionPlan {
            charter_digest: None,
            summary: "summary".to_string(),
            assumptions: Vec::new(),
            risks: Vec::new(),
            criteria_coverage: Vec::new(),
            waves: vec![MissionWave {
                number: 1,
                purpose: "wave 1".to_string(),
                goal_specs: specs,
                required_snapshot: None,
                completion_condition: None,
            }],
            artifact_obligations: Vec::new(),
            criticism: None,
            resolutions: Vec::new(),
            effective_digest: None,
        }
    }

    #[test]
    fn simple_assertions_stay_active() {
        let mission = mission_with_snapshots(vec![vec![assertion("a1"), assertion("a2")]]);
        let states = derive_assertion_states(&mission, &BTreeSet::new());
        assert_eq!(states.get("a1"), Some(&AssertionState::Active));
        assert_eq!(states.get("a2"), Some(&AssertionState::Active));
    }

    #[test]
    fn supersession_retires_the_old_assertion() {
        let mut newer = assertion("a2");
        newer.supersedes = vec!["a1".to_string()];
        let mission = mission_with_snapshots(vec![vec![assertion("a1")], vec![newer]]);
        let states = derive_assertion_states(&mission, &BTreeSet::new());
        assert_eq!(states.get("a1"), Some(&AssertionState::Superseded));
        assert_eq!(states.get("a2"), Some(&AssertionState::Active));
    }

    #[test]
    fn correction_by_active_assertion_invalidates() {
        let mut correction = assertion("a2");
        correction.corrects = vec!["a1".to_string()];
        let mission = mission_with_snapshots(vec![vec![assertion("a1")], vec![correction]]);
        let report = compute_invalidation(&mission, &BTreeSet::new(), &[]);
        assert!(report.invalidated.contains_key("a1"));
        assert!(!report.invalidated.contains_key("a2"));
        assert!(matches!(
            report.invalidated.get("a1"),
            Some(InvalidatedBecause::CorrectedBy { by }) if by == "a2"
        ));
    }

    #[test]
    fn correction_by_invalidated_corrector_does_not_hold() {
        // a2 corrects a1, but a3 corrects a2: the retracted correction lifts.
        let mut a2 = assertion("a2");
        a2.corrects = vec!["a1".to_string()];
        let mut a3 = assertion("a3");
        a3.corrects = vec!["a2".to_string()];
        let mission = mission_with_snapshots(vec![vec![assertion("a1"), a2, a3]]);
        let report = compute_invalidation(&mission, &BTreeSet::new(), &[]);
        assert!(report.invalidated.contains_key("a2"));
        assert!(!report.invalidated.contains_key("a1"));
    }

    #[test]
    fn correction_by_contested_corrector_does_not_hold() {
        // The closure rule requires the corrector to be active. A contested
        // corrector (an open contradiction names it) does not invalidate its
        // target; the contested pair is surfaced instead, and only a human
        // decision or a surviving correction resolves it.
        let mut a2 = assertion("a2");
        a2.corrects = vec!["a1".to_string()];
        let mut contradiction = assertion("c1");
        contradiction.kind = AssertionKind::Contradiction;
        contradiction.members = vec!["a2".to_string(), "a3".to_string()];
        contradiction.resolution = None;
        let mission = mission_with_snapshots(vec![vec![
            assertion("a1"),
            a2,
            assertion("a3"),
            contradiction,
        ]]);
        let states = derive_assertion_states(&mission, &BTreeSet::new());
        assert_eq!(states.get("a2"), Some(&AssertionState::Contested));
        // a1 stays active: the corrector is contested, not surviving.
        assert_eq!(states.get("a1"), Some(&AssertionState::Active));
    }

    #[test]
    fn invalidation_propagates_through_derivation_chains() {
        // a1 is a fact; a2 derives from a1; a3 derives from a2.
        let mut a2 = assertion("a2");
        a2.derived_from = vec!["a1".to_string()];
        let mut a3 = assertion("a3");
        a3.derived_from = vec!["a2".to_string()];
        let mut correction = assertion("a4");
        correction.corrects = vec!["a1".to_string()];
        let mission = mission_with_snapshots(vec![vec![assertion("a1"), a2, a3, correction]]);
        let report = compute_invalidation(&mission, &BTreeSet::new(), &[]);
        assert!(report.invalidated.contains_key("a1"));
        assert!(report.invalidated.contains_key("a2"));
        assert!(report.invalidated.contains_key("a3"));
    }

    #[test]
    fn superseded_assertion_is_not_reported_invalidated() {
        // a1 is superseded by a2; a3 corrects a1. Supersession outranks
        // invalidation for reporting: consumers of a1 already moved to a2.
        let mut a2 = assertion("a2");
        a2.supersedes = vec!["a1".to_string()];
        let mut a3 = assertion("a3");
        a3.corrects = vec!["a1".to_string()];
        let mission = mission_with_snapshots(vec![vec![assertion("a1"), a2, a3]]);
        let report = compute_invalidation(&mission, &BTreeSet::new(), &[]);
        assert!(report.superseded.contains("a1"));
        assert!(!report.invalidated.contains_key("a1"));
    }

    #[test]
    fn provenance_source_invalidation_is_transitive() {
        let mut a1 = assertion("a1");
        a1.provenance = Some("contribution:g1/1".to_string());
        let mut a2 = assertion("a2");
        a2.derived_from = vec!["a1".to_string()];
        let mission = mission_with_snapshots(vec![vec![a1, a2]]);
        let sources = BTreeSet::from(["contribution:g1/1".to_string()]);
        let report = compute_invalidation(&mission, &sources, &[]);
        assert!(report.invalidated.contains_key("a1"));
        assert!(report.invalidated.contains_key("a2"));
        assert!(matches!(
            report.invalidated.get("a1"),
            Some(InvalidatedBecause::SourceInvalidated { source }) if source == "contribution:g1/1"
        ));
    }

    #[test]
    fn open_contradiction_contests_its_members() {
        let mut contradiction = assertion("c1");
        contradiction.kind = AssertionKind::Contradiction;
        contradiction.members = vec!["a1".to_string(), "a2".to_string()];
        contradiction.resolution = None;
        let mission =
            mission_with_snapshots(vec![vec![assertion("a1"), assertion("a2"), contradiction]]);
        let states = derive_assertion_states(&mission, &BTreeSet::new());
        assert_eq!(states.get("a1"), Some(&AssertionState::Contested));
        assert_eq!(states.get("a2"), Some(&AssertionState::Contested));
    }

    #[test]
    fn resolved_contradiction_releases_its_members() {
        let mut contradiction = assertion("c1");
        contradiction.kind = AssertionKind::Contradiction;
        contradiction.members = vec!["a1".to_string(), "a2".to_string()];
        contradiction.resolution = Some(ContradictionResolution::ScopeSplit);
        contradiction.resolved_by = Some("a3".to_string());
        let mission =
            mission_with_snapshots(vec![vec![assertion("a1"), assertion("a2"), contradiction]]);
        let states = derive_assertion_states(&mission, &BTreeSet::new());
        assert_eq!(states.get("a1"), Some(&AssertionState::Active));
        assert_eq!(states.get("a2"), Some(&AssertionState::Active));
    }

    #[test]
    fn affected_rounds_require_capsule_inclusion() {
        let mut correction = assertion("a2");
        correction.corrects = vec!["a1".to_string()];
        let mission = mission_with_snapshots(vec![vec![assertion("a1"), correction]]);
        let capsules = vec![
            CapsuleBinding {
                goal_id: "g1".to_string(),
                goal_round: 1,
                mission_goal_key: "k1".to_string(),
                assertion_ids: vec!["a1".to_string()],
            },
            CapsuleBinding {
                goal_id: "g2".to_string(),
                goal_round: 3,
                mission_goal_key: "k2".to_string(),
                assertion_ids: vec!["a2".to_string()],
            },
        ];
        let report = compute_invalidation(&mission, &BTreeSet::new(), &capsules);
        assert_eq!(report.affected_goal_rounds.len(), 1);
        assert_eq!(report.affected_goal_rounds[0].goal_id, "g1");
        assert_eq!(
            report.affected_goal_rounds[0].invalidated_assertions,
            vec!["a1"]
        );
    }

    #[test]
    fn affected_specs_match_structural_scope_refs() {
        let mut correction = assertion("a3");
        correction.corrects = vec!["a1".to_string()];
        let mut a1 = assertion("a1");
        a1.scope_refs = vec!["crit:rollback".to_string()];
        let mut mission = mission_with_snapshots(vec![vec![a1, correction]]);
        let mut round = mission.rounds[0].clone();
        round.plan = Some(plan_with(vec![spec("k1", &["crit:rollback"])]));
        mission.rounds[0] = round;
        let report = compute_invalidation(&mission, &BTreeSet::new(), &[]);
        assert_eq!(report.affected_specs.len(), 1);
        assert_eq!(report.affected_specs[0].mission_goal_key, "k1");
    }

    #[test]
    fn assertions_through_accumulates_the_chain_up_to_a_version() {
        let mut mission = mission_with_snapshots(vec![
            vec![assertion("a1")],
            vec![assertion("a2")],
            vec![assertion("a3")],
        ]);
        let through = assertions_through(&mission, 2, &BTreeSet::new());
        let ids: Vec<&str> = through
            .iter()
            .map(|(assertion, _)| assertion.assertion_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a1", "a2"]);
        // A later supersession still retires the earlier assertion when the
        // full chain is considered.
        let mut a3 = assertion("a3");
        a3.supersedes = vec!["a1".to_string()];
        mission.rounds[0].snapshots[2].knowledge_index = vec![a3];
        let through = assertions_through(&mission, 2, &BTreeSet::new());
        let states: Vec<AssertionState> = through.iter().map(|(_, state)| *state).collect();
        assert_eq!(
            states,
            vec![AssertionState::Superseded, AssertionState::Active]
        );
    }
}
