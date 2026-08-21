//! Mission workflow capabilities: Round creation, plan approval, context
//! compilation, reconciliation, and Outcome settlement.
//!
//! These are the fenced, deterministic transitions the Mission engine owns.
//! Long agent calls and publication work run as one-shot managed processes;
//! this module only advances short, fenced state transitions.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{RefineError, RefineResult};
use crate::model::mission::{
    ArtifactRef, Mission, MissionPlan, MissionRound, MissionRoundRequest as ModelRoundRequest,
    MissionSnapshot, OutcomeManifest, OutcomePublication, ReconciliationReceipt,
};

use super::persistence::*;
use super::service::FileMissionService;

/// A request to author a new Mission Round.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionRoundAuthoring {
    pub reporter: String,
    pub prompt: String,
}

/// A request to approve a Mission plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionPlanApproval {
    pub plan_digest: String,
    pub actor: String,
    pub rationale: String,
}

/// A request to author a Mission.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionAuthoringRequest {
    pub name: String,
    pub intent: String,
    #[serde(default)]
    pub reporter: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

impl FileMissionService {
    /// Append a new MissionRound, freezing the current charter into its request.
    pub fn append_round(
        &self,
        mission_id: &str,
        reporter: &str,
        prompt: &str,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        let intent = object
            .get("intent")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let criteria = object
            .get("success_criteria")
            .and_then(Value::as_array)
            .map(|criteria| {
                criteria
                    .iter()
                    .filter_map(|criterion| serde_json::from_value(criterion.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let artifact_obligations = object
            .get("artifact_contract")
            .and_then(Value::as_array)
            .map(|contract| {
                contract
                    .iter()
                    .filter_map(|obligation| serde_json::from_value(obligation.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Mission {mission_id} has no rounds array"))
            })?;
        let number = rounds.len() + 1;
        let now = Self::now_timestamp();
        let request = ModelRoundRequest {
            intent,
            constraints: Vec::new(),
            criteria,
            artifact_obligations,
            authorizing_request: prompt.to_string(),
            charter_digest: None,
        };
        let round = MissionRound {
            number,
            request,
            input_bindings: Vec::new(),
            plan: None,
            plan_amendments: Vec::new(),
            snapshots: Vec::new(),
            reconciliation_receipts: Vec::new(),
            phase_evidence: {
                let mut evidence = Map::new();
                evidence.insert("reporter".to_string(), Value::String(reporter.to_string()));
                evidence
            },
            review: None,
            outcome: None,
            outcome_publication: None,
            failure: None,
            created: now.clone(),
            updated: now.clone(),
        };
        rounds.push(serde_json::to_value(&round).map_err(|error| {
            RefineError::Serialization(format!("failed to encode MissionRound: {error}"))
        })?);
        object.insert("current_round".to_string(), Value::from(number));
        object.insert("updated".to_string(), Value::String(now));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Record an approved plan on the current Round, binding its effective digest.
    pub fn approve_plan(
        &self,
        mission_id: &str,
        plan: MissionPlan,
        actor: &str,
        rationale: &str,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        let current_round = object
            .get("current_round")
            .and_then(Value::as_u64)
            .map(|round| round as usize)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
            })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Mission {mission_id} has no rounds array"))
            })?;
        let round = rounds
            .iter_mut()
            .find(|round| round.get("number").and_then(Value::as_u64) == Some(current_round as u64))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Mission {mission_id} Round {current_round} was not found"
                ))
            })?;
        let round_object = round.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("MissionRound is not a JSON object".to_string())
        })?;
        round_object.insert(
            "plan".to_string(),
            serde_json::to_value(&plan).map_err(|error| {
                RefineError::Serialization(format!("failed to encode MissionPlan: {error}"))
            })?,
        );
        let mut evidence = round_object
            .get("phase_evidence")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        evidence.insert(
            "plan_approval".to_string(),
            serde_json::json!({
                "actor": actor,
                "rationale": rationale,
                "plan_digest": plan.effective_digest,
                "approved_at": Self::now_timestamp(),
            }),
        );
        round_object.insert("phase_evidence".to_string(), Value::Object(evidence));
        round_object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Publish the next immutable MissionSnapshot on the current Round.
    pub fn publish_snapshot(
        &self,
        mission_id: &str,
        snapshot: MissionSnapshot,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        let current_round = object
            .get("current_round")
            .and_then(Value::as_u64)
            .map(|round| round as usize)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
            })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Mission {mission_id} has no rounds array"))
            })?;
        let round = rounds
            .iter_mut()
            .find(|round| round.get("number").and_then(Value::as_u64) == Some(current_round as u64))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Mission {mission_id} Round {current_round} was not found"
                ))
            })?;
        let round_object = round.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("MissionRound is not a JSON object".to_string())
        })?;
        let snapshots = round_object
            .get_mut("snapshots")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization("MissionRound has no snapshots array".to_string())
            })?;
        snapshots.push(serde_json::to_value(&snapshot).map_err(|error| {
            RefineError::Serialization(format!("failed to encode MissionSnapshot: {error}"))
        })?);
        round_object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Record a reconciliation receipt on the current Round.
    pub fn record_reconciliation(
        &self,
        mission_id: &str,
        receipt: ReconciliationReceipt,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        let current_round = object
            .get("current_round")
            .and_then(Value::as_u64)
            .map(|round| round as usize)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
            })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Mission {mission_id} has no rounds array"))
            })?;
        let round = rounds
            .iter_mut()
            .find(|round| round.get("number").and_then(Value::as_u64) == Some(current_round as u64))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Mission {mission_id} Round {current_round} was not found"
                ))
            })?;
        let round_object = round.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("MissionRound is not a JSON object".to_string())
        })?;
        let receipts = round_object
            .get_mut("reconciliation_receipts")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(
                    "MissionRound has no reconciliation_receipts array".to_string(),
                )
            })?;
        receipts.push(serde_json::to_value(&receipt).map_err(|error| {
            RefineError::Serialization(format!("failed to encode ReconciliationReceipt: {error}"))
        })?);
        round_object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Publish one applied reconciliation: write the next immutable snapshot
    /// and its receipt under one fenced window.
    ///
    /// The attempt fence re-derives the expected attempt identity from
    /// current durable state, so a competing attempt that closed in between
    /// is rejected rather than interleaved. The two writes (snapshot, then
    /// receipt) are individually fenced by revision; a crash between them
    /// recovers by re-publishing the same idempotent content.
    pub fn publish_reconciliation(
        &self,
        mission_id: &str,
        applied: &super::reconciliation::AppliedReduction,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let current = self.show_mission(&mission_id)?;
        let expected_attempt = super::reconciliation::next_attempt_id(&current, applied.wave);
        if applied.attempt_id != expected_attempt {
            return Err(RefineError::Conflict(format!(
                "reconciliation attempt {} is stale; expected {}",
                applied.attempt_id, expected_attempt
            )));
        }
        let expected_parent = current
            .rounds
            .iter()
            .find(|round| round.number == current.current_round.unwrap_or(0))
            .and_then(|round| round.snapshots.last())
            .map(|snapshot| snapshot.version);
        if expected_parent != Some(applied.parent_snapshot) {
            return Err(RefineError::Conflict(format!(
                "parent snapshot changed while attempt {} was open",
                applied.attempt_id
            )));
        }
        let mut snapshot = applied.snapshot.clone();
        snapshot.created = Self::now_timestamp();
        let published = self.publish_snapshot(&mission_id, snapshot, observed_revision)?;
        let mut receipt = applied.receipt.clone();
        receipt.created = Self::now_timestamp();
        self.record_reconciliation(&mission_id, receipt, Some(published.revision))
    }

    /// Settle the Outcome manifest on the current Round.
    pub fn settle_outcome(
        &self,
        mission_id: &str,
        manifest: OutcomeManifest,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        let current_round = object
            .get("current_round")
            .and_then(Value::as_u64)
            .map(|round| round as usize)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
            })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Mission {mission_id} has no rounds array"))
            })?;
        let round = rounds
            .iter_mut()
            .find(|round| round.get("number").and_then(Value::as_u64) == Some(current_round as u64))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Mission {mission_id} Round {current_round} was not found"
                ))
            })?;
        let round_object = round.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("MissionRound is not a JSON object".to_string())
        })?;
        round_object.insert(
            "outcome".to_string(),
            serde_json::to_value(&manifest).map_err(|error| {
                RefineError::Serialization(format!("failed to encode OutcomeManifest: {error}"))
            })?,
        );
        round_object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Record the Outcome publication receipt on the current Round.
    pub fn record_publication(
        &self,
        mission_id: &str,
        publication: OutcomePublication,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        let current_round = object
            .get("current_round")
            .and_then(Value::as_u64)
            .map(|round| round as usize)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
            })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Mission {mission_id} has no rounds array"))
            })?;
        let round = rounds
            .iter_mut()
            .find(|round| round.get("number").and_then(Value::as_u64) == Some(current_round as u64))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Mission {mission_id} Round {current_round} was not found"
                ))
            })?;
        let round_object = round.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("MissionRound is not a JSON object".to_string())
        })?;
        round_object.insert(
            "outcome_publication".to_string(),
            serde_json::to_value(&publication).map_err(|error| {
                RefineError::Serialization(format!("failed to encode OutcomePublication: {error}"))
            })?,
        );
        round_object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Compile a scoped, deterministic context capsule for a Goal specification
    /// from the selected MissionSnapshot.
    ///
    /// The capsule embeds the exact manifest of included assertions and
    /// artifacts with reasons, plus the manifest digest recorded on the
    /// GoalRound Mission context, so invalidation can later determine
    /// precisely what this GoalRound observed.
    pub fn compile_context_capsule(
        &self,
        mission: &Mission,
        snapshot: &MissionSnapshot,
        goal_key: &str,
    ) -> RefineResult<Value> {
        let goal_spec = mission
            .rounds
            .iter()
            .flat_map(|round| round.plan.iter())
            .flat_map(|plan| plan.waves.iter())
            .flat_map(|wave| wave.goal_specs.iter())
            .find(|spec| spec.mission_goal_key == goal_key)
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Goal specification {goal_key} was not found in the Mission plan"
                ))
            })?;

        let included_artifacts: Vec<&ArtifactRef> = snapshot
            .artifact_refs
            .iter()
            .filter(|artifact| {
                goal_spec
                    .input_artifact_keys
                    .iter()
                    .any(|key| key == &artifact.key)
            })
            .collect();

        let manifest = super::reconciliation::compile_capsule_manifest(
            mission,
            snapshot,
            goal_spec,
            &std::collections::BTreeSet::new(),
        )?;
        // The manifest accumulates the whole snapshot chain, so the rendering
        // lookup must too: an assertion accepted by an earlier snapshot still
        // renders when a later snapshot is selected.
        let chain: std::collections::BTreeMap<String, _> =
            super::reconciliation::assertions_through(
                mission,
                snapshot.version,
                &std::collections::BTreeSet::new(),
            )
            .into_iter()
            .map(|(assertion, _)| (assertion.assertion_id.clone(), assertion))
            .collect();
        let included_assertions: Vec<Value> = manifest
            .assertions
            .iter()
            .filter_map(|inclusion| {
                let assertion = chain.get(&inclusion.id)?;
                Some(serde_json::json!({
                    "assertion_id": assertion.assertion_id,
                    "kind": assertion.kind.as_str(),
                    "authority": assertion.authority.as_str(),
                    "qualified": assertion.qualified,
                    "claim": assertion.scope,
                    "evidence_refs": assertion.evidence_refs,
                    "included_because": inclusion.reason,
                }))
            })
            .collect();

        Ok(serde_json::json!({
            "mission_id": mission.id,
            "mission_round": mission.current_round,
            "snapshot_version": snapshot.version,
            "snapshot_digest": snapshot.digest,
            "intent": mission.intent,
            "criteria": mission.success_criteria.iter()
                .filter(|criterion| goal_spec.criterion_ids.iter().any(|id| id == &criterion.id))
                .collect::<Vec<_>>(),
            "role": goal_spec.role,
            "scope": goal_spec.prompt,
            "artifacts": included_artifacts,
            "assertions": included_assertions,
            "capsule_manifest": manifest,
            "capsule_manifest_digest": manifest.digest,
            "expected_findings": goal_spec.expected_findings,
            "target_head": snapshot.target_head,
        }))
    }
}
