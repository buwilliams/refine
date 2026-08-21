//! Capsule manifest compilation: which assertions and artifacts a Goal's
//! pinned capsule includes, with reasons, and the manifest digest that makes
//! invalidation exact.
//!
//! The compiler renders contested knowledge honestly: contested pairs enter
//! at full member cost and are never dropped to fit a budget; load-bearing
//! contested context blocks the spec's admission instead of being silently
//! resolved.
//!
//! See `docs/mission-reconciliation.md` ("Capsule rendering of contested and
//! dissenting knowledge").

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{RefineError, RefineResult};
use crate::model::mission::{AssertionKind, Mission, MissionGoalSpec, MissionSnapshot};

use super::ledger::{AssertionState, assertions_through};

/// How many assertions one capsule may include before exclusions begin.
pub const DEFAULT_CAPSULE_ASSERTION_BUDGET: usize = 64;

/// One included or excluded ledger entry with the deterministic reason.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapsuleInclusion {
    pub id: String,
    pub reason: String,
}

/// The exact manifest of what one GoalRound capsule included. The manifest
/// is embedded in the capsule, pinned by `capsule_manifest_digest` on the
/// GoalRound Mission context, and is the input that makes invalidation
/// exact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapsuleManifest {
    pub mission_id: String,
    pub mission_round: usize,
    pub snapshot_version: usize,
    #[serde(default)]
    pub snapshot_digest: Option<String>,
    #[serde(default)]
    pub assertions: Vec<CapsuleInclusion>,
    #[serde(default)]
    pub artifacts: Vec<CapsuleInclusion>,
    /// Contested load-bearing assertion ids; the spec cannot be admitted
    /// until the underlying decision resolves.
    #[serde(default)]
    pub blocking: Vec<String>,
    /// Entries excluded by the budget, with reasons. Contested pairs are
    /// excluded whole, never one member at a time.
    #[serde(default)]
    pub excluded: Vec<CapsuleInclusion>,
    pub digest: String,
}

/// Compile the deterministic capsule manifest for one Goal specification
/// against one selected MissionSnapshot.
///
/// Inclusion rules: active and contested assertions whose structural scope
/// links (Mission Goal key, criterion ids, artifact keys) name the spec;
/// open contradictions join whenever one of their members is included.
/// `invalidated_sources` carries the provenance sources currently known to
/// be invalidated.
pub fn compile_capsule_manifest(
    mission: &Mission,
    snapshot: &MissionSnapshot,
    goal_spec: &MissionGoalSpec,
    invalidated_sources: &BTreeSet<String>,
) -> RefineResult<CapsuleManifest> {
    compile_capsule_manifest_with_budget(
        mission,
        snapshot,
        goal_spec,
        invalidated_sources,
        DEFAULT_CAPSULE_ASSERTION_BUDGET,
    )
}

pub fn compile_capsule_manifest_with_budget(
    mission: &Mission,
    snapshot: &MissionSnapshot,
    goal_spec: &MissionGoalSpec,
    invalidated_sources: &BTreeSet<String>,
    assertion_budget: usize,
) -> RefineResult<CapsuleManifest> {
    let chain = assertions_through(mission, snapshot.version, invalidated_sources);
    let spec_links: BTreeSet<&String> = goal_spec
        .criterion_ids
        .iter()
        .chain(goal_spec.input_artifact_keys.iter())
        .chain(goal_spec.output_artifact_keys.iter())
        .chain(std::iter::once(&goal_spec.mission_goal_key))
        .collect();

    // Open contradictions by member, so a pair renders whole.
    let mut open_contradictions: BTreeMap<&String, Vec<&str>> = BTreeMap::new();
    for (assertion, _) in &chain {
        if assertion.kind == AssertionKind::Contradiction
            && assertion.resolution.is_none_or(|resolution| {
                resolution == crate::model::mission::ContradictionResolution::Open
            })
        {
            for member in &assertion.members {
                open_contradictions
                    .entry(member)
                    .or_default()
                    .push(assertion.assertion_id.as_str());
            }
        }
    }

    let mut included: Vec<(String, String, AssertionState)> = Vec::new();
    let mut excluded: Vec<CapsuleInclusion> = Vec::new();
    for (assertion, state) in &chain {
        if assertion.kind == AssertionKind::Contradiction {
            continue;
        }
        if state == &AssertionState::Superseded || state == &AssertionState::Invalidated {
            // Superseded and invalidated assertions never enter a new
            // capsule; the superseding assertion enters instead.
            continue;
        }
        let link = assertion
            .scope_refs
            .iter()
            .find(|link| spec_links.contains(*link));
        let Some(link) = link else { continue };
        let label = match state {
            AssertionState::Contested => "contested",
            _ => "active",
        };
        // Contested pairs count at full member cost: when the pair would not
        // fit, both members defer, never just one.
        let pair_members = open_contradictions
            .get(&assertion.assertion_id)
            .map(|_| 2usize)
            .unwrap_or(1);
        if included.len() + pair_members > assertion_budget {
            let reason = if pair_members == 2 {
                "capsule-budget:contested-pair".to_string()
            } else {
                "capsule-budget".to_string()
            };
            excluded.push(CapsuleInclusion {
                id: assertion.assertion_id.clone(),
                reason,
            });
            continue;
        }
        included.push((
            assertion.assertion_id.clone(),
            format!("{label}:{link}"),
            *state,
        ));
        // The open contradiction record itself joins so the pair renders.
        if let Some(contradiction_ids) = open_contradictions.get(&assertion.assertion_id) {
            for contradiction_id in contradiction_ids {
                if !included.iter().any(|(id, _, _)| id == *contradiction_id)
                    && included.len() < assertion_budget
                {
                    included.push((
                        (*contradiction_id).to_string(),
                        "contested:pair".to_string(),
                        AssertionState::Active,
                    ));
                }
            }
        }
    }

    let mut blocking: Vec<String> = included
        .iter()
        .filter(|(_, _, state)| *state == AssertionState::Contested)
        .map(|(id, _, _)| id.clone())
        .collect();
    blocking.sort();
    blocking.dedup();

    let mut assertions: Vec<CapsuleInclusion> = included
        .into_iter()
        .map(|(id, reason, _)| CapsuleInclusion { id, reason })
        .collect();
    let mut artifacts: Vec<CapsuleInclusion> = snapshot
        .artifact_refs
        .iter()
        .filter(|artifact| goal_spec.input_artifact_keys.contains(&artifact.key))
        .map(|artifact| CapsuleInclusion {
            id: artifact.key.clone(),
            reason: format!("input-artifact:{}", artifact.key),
        })
        .collect();
    // Deterministic order regardless of snapshot construction order.
    assertions.sort_by(|a, b| a.id.cmp(&b.id));
    artifacts.sort_by(|a, b| a.id.cmp(&b.id));
    excluded.sort_by(|a, b| a.id.cmp(&b.id));

    let mut manifest = CapsuleManifest {
        mission_id: mission.id.clone(),
        mission_round: mission.current_round.unwrap_or(0),
        snapshot_version: snapshot.version,
        snapshot_digest: snapshot.digest.clone(),
        assertions,
        artifacts,
        blocking,
        excluded,
        digest: String::new(),
    };
    manifest.digest = manifest_digest(&manifest);
    if manifest.digest.is_empty() {
        return Err(RefineError::Serialization(
            "failed to compute capsule manifest digest".to_string(),
        ));
    }
    Ok(manifest)
}

/// The manifest digest: sha256 over the canonical JSON of the manifest with
/// the digest excluded, so the same inputs yield identical digests.
pub fn manifest_digest(manifest: &CapsuleManifest) -> String {
    let mut value = serde_json::to_value(manifest).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "digest".to_string(),
            serde_json::Value::String(String::new()),
        );
    }
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{digest}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mission::{
        ArtifactAuthority, ArtifactRef, ContradictionResolution, KnowledgeAssertion, MissionPlan,
        MissionRound, MissionRoundRequest, MissionSnapshot, MissionWave,
    };

    fn assertion(id: &str, scope_refs: &[&str]) -> KnowledgeAssertion {
        KnowledgeAssertion {
            assertion_id: id.to_string(),
            kind: AssertionKind::Fact,
            authority: ArtifactAuthority::Evidence,
            provenance: None,
            qualified: None,
            supersedes: vec![],
            corrects: vec![],
            derived_from: vec![],
            scope: None,
            scope_refs: scope_refs.iter().map(|r| r.to_string()).collect(),
            evidence_refs: vec![],
            supersedable: true,
            members: vec![],
            resolution: None,
            resolved_by: None,
        }
    }

    fn contradiction(id: &str, members: &[&str]) -> KnowledgeAssertion {
        KnowledgeAssertion {
            assertion_id: id.to_string(),
            kind: AssertionKind::Contradiction,
            authority: ArtifactAuthority::Evidence,
            provenance: None,
            qualified: None,
            supersedes: vec![],
            corrects: vec![],
            derived_from: vec![],
            scope: None,
            scope_refs: vec![],
            evidence_refs: vec![],
            supersedable: true,
            members: members.iter().map(|m| m.to_string()).collect(),
            resolution: Some(ContradictionResolution::Open),
            resolved_by: None,
        }
    }

    fn goal_spec() -> MissionGoalSpec {
        MissionGoalSpec {
            mission_goal_key: "k1".to_string(),
            name: "Goal".to_string(),
            prompt: "prompt".to_string(),
            role: None,
            required: true,
            criterion_ids: vec!["crit:tokens".to_string()],
            input_artifact_keys: vec!["risk-register".to_string()],
            output_artifact_keys: vec![],
            expected_findings: vec![],
            feature_id: None,
            feature_order: None,
            preferred_node: None,
        }
    }

    fn mission_with(snapshot: MissionSnapshot, spec: MissionGoalSpec) -> Mission {
        let mut mission = Mission {
            id: "MTEST".to_string(),
            name: "Test".to_string(),
            intent: "intent".to_string(),
            status: crate::model::mission::MissionStatus::Execute,
            reporter: None,
            assignee: None,
            coordinator_node_id: None,
            success_criteria: vec![],
            artifact_contract: vec![],
            current_round: Some(1),
            revision: 0,
            rounds: vec![MissionRound {
                number: 1,
                request: MissionRoundRequest {
                    intent: "intent".to_string(),
                    constraints: vec![],
                    criteria: vec![],
                    artifact_obligations: vec![],
                    authorizing_request: "request".to_string(),
                    charter_digest: None,
                },
                input_bindings: vec![],
                plan: Some(MissionPlan {
                    charter_digest: None,
                    summary: "summary".to_string(),
                    assumptions: vec![],
                    risks: vec![],
                    criteria_coverage: vec![],
                    waves: vec![MissionWave {
                        number: 1,
                        purpose: "wave".to_string(),
                        goal_specs: vec![spec],
                        required_snapshot: None,
                        completion_condition: None,
                    }],
                    artifact_obligations: vec![],
                    criticism: None,
                    resolutions: vec![],
                    effective_digest: None,
                }),
                plan_amendments: vec![],
                snapshots: vec![snapshot],
                reconciliation_receipts: vec![],
                phase_evidence: Default::default(),
                review: None,
                outcome: None,
                outcome_publication: None,
                failure: None,
                created: String::new(),
                updated: String::new(),
            }],
            created: String::new(),
            updated: String::new(),
        };
        mission.rounds[0].snapshots[0].version = 1;
        mission
    }

    fn snapshot_with(knowledge: Vec<KnowledgeAssertion>) -> MissionSnapshot {
        MissionSnapshot {
            version: 1,
            parent_version: None,
            target_head: None,
            plan_digest: None,
            artifact_refs: vec![ArtifactRef {
                key: "risk-register".to_string(),
                title: "Risk register".to_string(),
                kind: "register".to_string(),
                authority: ArtifactAuthority::Evidence,
                path: "missions/MT/MTEST/artifacts/risk-register/abc.md".to_string(),
                media_type: None,
                size: 0,
                sha256: None,
                provenance: None,
                applicability: None,
            }],
            input_refs: vec![],
            consumed_contribution_refs: vec![],
            knowledge_index: knowledge,
            corrects_snapshot: None,
            digest: Some("sha256:snap".to_string()),
            created: String::new(),
        }
    }

    #[test]
    fn includes_only_spec_linked_active_assertions() {
        let snapshot = snapshot_with(vec![
            assertion("a1", &["crit:tokens"]),
            assertion("a2", &["crit:other"]),
            assertion("a3", &[]),
        ]);
        let mission = mission_with(snapshot, goal_spec());
        let manifest = compile_capsule_manifest(
            &mission,
            &mission.rounds[0].snapshots[0].clone(),
            &goal_spec(),
            &BTreeSet::new(),
        )
        .unwrap();
        let ids: Vec<&str> = manifest
            .assertions
            .iter()
            .map(|inclusion| inclusion.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a1"]);
        assert!(
            manifest.assertions[0]
                .reason
                .starts_with("active:crit:tokens")
        );
        assert_eq!(manifest.artifacts.len(), 1);
        assert_eq!(manifest.artifacts[0].id, "risk-register");
        assert!(manifest.blocking.is_empty());
        assert!(!manifest.digest.is_empty());
    }

    #[test]
    fn contested_pairs_render_whole_and_block() {
        let snapshot = snapshot_with(vec![
            assertion("a1", &["crit:tokens"]),
            assertion("a2", &["crit:tokens"]),
            contradiction("c1", &["a1", "a2"]),
        ]);
        let mission = mission_with(snapshot, goal_spec());
        let manifest = compile_capsule_manifest(
            &mission,
            &mission.rounds[0].snapshots[0].clone(),
            &goal_spec(),
            &BTreeSet::new(),
        )
        .unwrap();
        let ids: Vec<&str> = manifest
            .assertions
            .iter()
            .map(|inclusion| inclusion.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a1", "a2", "c1"]);
        assert_eq!(manifest.blocking, vec!["a1", "a2"]);
    }

    #[test]
    fn budget_excludes_contested_pairs_whole() {
        let snapshot = snapshot_with(vec![
            assertion("a1", &["crit:tokens"]),
            assertion("a2", &["crit:tokens"]),
            contradiction("c1", &["a1", "a2"]),
        ]);
        let mission = mission_with(snapshot, goal_spec());
        let manifest = compile_capsule_manifest_with_budget(
            &mission,
            &mission.rounds[0].snapshots[0].clone(),
            &goal_spec(),
            &BTreeSet::new(),
            1,
        )
        .unwrap();
        // The pair did not fit: both members defer, never just one.
        assert!(manifest.assertions.is_empty());
        assert_eq!(manifest.excluded.len(), 2);
        assert!(
            manifest
                .excluded
                .iter()
                .all(|entry| entry.reason == "capsule-budget:contested-pair")
        );
    }

    #[test]
    fn invalidated_assertions_never_enter_a_new_capsule() {
        let snapshot = snapshot_with(vec![assertion("a1", &["crit:tokens"])]);
        let mission = mission_with(snapshot, goal_spec());
        let sources = BTreeSet::from(["contribution:g1/1".to_string()]);
        let mut mission = mission;
        mission.rounds[0].snapshots[0].knowledge_index[0].provenance =
            Some("contribution:g1/1".to_string());
        let manifest = compile_capsule_manifest(
            &mission,
            &mission.rounds[0].snapshots[0].clone(),
            &goal_spec(),
            &sources,
        )
        .unwrap();
        assert!(manifest.assertions.is_empty());
    }

    #[test]
    fn manifest_digest_is_deterministic() {
        let snapshot = snapshot_with(vec![assertion("a1", &["crit:tokens"])]);
        let mission = mission_with(snapshot, goal_spec());
        let first = compile_capsule_manifest(
            &mission,
            &mission.rounds[0].snapshots[0].clone(),
            &goal_spec(),
            &BTreeSet::new(),
        )
        .unwrap();
        let second = compile_capsule_manifest(
            &mission,
            &mission.rounds[0].snapshots[0].clone(),
            &goal_spec(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(first.digest, second.digest);
    }
}
