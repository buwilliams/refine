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

    /// Record a drafted plan on the current Round. The plan stays a draft
    /// until `approve_plan` binds its exact effective digest. Validation is
    /// deterministic: unique stable Goal keys, charter-covered criteria, and
    /// charter-covered output obligations.
    pub fn record_plan(
        &self,
        mission_id: &str,
        plan: MissionPlan,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let value = self.show_mission_value(&mission_id)?;
        self.check_revision(&mission_id, &value, observed_revision)?;
        let mut mission = parse_mission(&value)?;
        let round_number = mission.current_round.unwrap_or(0);
        let round = mission
            .rounds
            .iter_mut()
            .find(|round| round.number == round_number)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!(
                    "Mission {mission_id} has no Round {round_number}"
                ))
            })?;
        if plan_is_approved(round) {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} Round {round_number} plan is already approved; later changes are plan amendments"
            )));
        }
        let charter = round.request.clone();
        validate_plan_against_charter(&plan, &charter)?;
        let mut plan = plan;
        if plan.charter_digest.is_none() {
            plan.charter_digest = charter.charter_digest.clone();
        }
        plan.effective_digest = Some(super::reconciliation::compute_plan_digest(&plan));
        round.plan = Some(plan);
        round.updated = Self::now_timestamp();
        self.write_typed(&mission_id, &mission, value)
    }

    /// Approve the exact drafted plan — or a pending amendment — by its
    /// effective digest. Approval is one authorization: it binds the digest,
    /// applies pending Goal adoptions and safe removals, and permits the
    /// engine to materialize Goals and distribute work.
    pub fn approve_plan(
        &self,
        mission_id: &str,
        plan_digest: &str,
        actor: &str,
        rationale: &str,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let value = self.show_mission_value(&mission_id)?;
        self.check_revision(&mission_id, &value, observed_revision)?;
        let mut mission = parse_mission(&value)?;
        let round_number = mission.current_round.unwrap_or(0);
        let round = mission
            .rounds
            .iter_mut()
            .find(|round| round.number == round_number)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!(
                    "Mission {mission_id} has no Round {round_number}"
                ))
            })?;
        if round.snapshots.is_empty() {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} Round {round_number} has no snapshot to plan against"
            )));
        }
        let approved_digests: Vec<String> = round
            .phase_evidence
            .get("plan_approval")
            .and_then(|approval| approval.get("approved_digests"))
            .and_then(Value::as_array)
            .map(|digests| {
                digests
                    .iter()
                    .filter_map(|digest| digest.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let pending_amendment = round
            .plan_amendments
            .last()
            .filter(|amendment| {
                amendment.effective_digest.as_deref().is_some_and(|digest| {
                    !approved_digests.iter().any(|approved| approved == digest)
                })
            })
            .cloned();
        let (approved_digest, is_amendment) = match &pending_amendment {
            Some(amendment) => (amendment.effective_digest.clone().unwrap_or_default(), true),
            None => {
                let plan = round.plan.as_ref().ok_or_else(|| {
                    RefineError::NotFound(format!(
                        "Mission {mission_id} has no drafted plan to approve"
                    ))
                })?;
                (plan.effective_digest.clone().unwrap_or_default(), false)
            }
        };
        if plan_digest != approved_digest {
            return Err(RefineError::Conflict(format!(
                "plan digest {plan_digest} does not match the current effective plan digest {approved_digest}"
            )));
        }

        // Pending Goal adoptions and safe removals commit with the approval.
        let adoptions = round
            .phase_evidence
            .get("plan_adoptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.apply_plan_adoptions(&mission_id, &adoptions)?;

        if is_amendment {
            let amendment = pending_amendment.expect("checked above");
            if let Some(base) = round.plan.as_mut() {
                base.criticism = amendment.criticism.clone();
                base.resolutions = amendment.resolutions.clone();
                base.waves = amendment.waves.clone();
                base.criteria_coverage = amendment.criteria_coverage.clone();
                base.artifact_obligations = amendment.artifact_obligations.clone();
                base.effective_digest = amendment.effective_digest.clone();
            }
        }
        let mut approved_digests = approved_digests;
        approved_digests.push(plan_digest.to_string());
        let mut evidence = round
            .phase_evidence
            .get("plan_approval")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        evidence.insert("actor".to_string(), Value::String(actor.to_string()));
        evidence.insert(
            "rationale".to_string(),
            Value::String(rationale.to_string()),
        );
        evidence.insert(
            "plan_digest".to_string(),
            Value::String(plan_digest.to_string()),
        );
        evidence.insert("amendment".to_string(), Value::Bool(is_amendment));
        evidence.insert(
            "approved_at".to_string(),
            Value::String(Self::now_timestamp()),
        );
        evidence.insert(
            "approved_digests".to_string(),
            Value::Array(approved_digests.into_iter().map(Value::String).collect()),
        );
        round
            .phase_evidence
            .insert("plan_approval".to_string(), Value::Object(evidence));
        round.phase_evidence.remove("plan_adoptions");
        round.updated = Self::now_timestamp();
        self.write_typed(&mission_id, &mission, value)
    }

    /// Apply pending plan adoptions and safe removals. Adoptions bind Goals
    /// to the Mission by stable key; removals unbind Goals whose Rounds never
    /// pinned Mission context (a pinned membership is historical and stays).
    fn apply_plan_adoptions(&self, mission_id: &str, adoptions: &[Value]) -> RefineResult<()> {
        let work_items = crate::application::work_items::FileWorkItemService::new(&self.refine_dir);
        for adoption in adoptions {
            let action = adoption
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("adopt");
            let Some(goal_id) = adoption.get("goal_id").and_then(Value::as_str) else {
                continue;
            };
            match action {
                "remove" => {
                    work_items.remove_goal_mission_binding(goal_id)?;
                }
                _ => {
                    let key = adoption
                        .get("mission_goal_key")
                        .and_then(Value::as_str)
                        .unwrap_or(goal_id);
                    work_items.bind_goal_to_mission(goal_id, mission_id, key)?;
                }
            }
        }
        Ok(())
    }

    /// Serialize a typed mutation and durably write it, returning the
    /// authoritative read-back.
    pub(crate) fn write_typed(
        &self,
        mission_id: &str,
        mission: &Mission,
        mut value: Value,
    ) -> RefineResult<Mission> {
        let typed = serde_json::to_value(mission).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Mission: {error}"))
        })?;
        if let (Some(object), Some(typed_object)) = (value.as_object_mut(), typed.as_object()) {
            for (key, typed_value) in typed_object {
                object.insert(key.clone(), typed_value.clone());
            }
        } else {
            value = typed;
        }
        let written = write_mission_atomically(&self.refine_dir, mission_id, &value)?;
        parse_mission(&written)
    }

    pub(crate) fn check_revision(
        &self,
        mission_id: &str,
        value: &Value,
        observed_revision: Option<u64>,
    ) -> RefineResult<()> {
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        Ok(())
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

    /// Fail the current Round with a recorded reason. A Failed Round is
    /// immutable; continuing the Mission appends a new Round with an
    /// explicit recovery request.
    pub fn fail_round(&self, mission_id: &str, round: &str, reason: &str) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
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
        if round != current_round.to_string() {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} current Round is {current_round}, not {round}"
            )));
        }
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Mission {mission_id} has no rounds array"))
            })?;
        let round_value = rounds
            .iter_mut()
            .find(|round| round.get("number").and_then(Value::as_u64) == Some(current_round as u64))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Mission {mission_id} Round {current_round} was not found"
                ))
            })?;
        let round_object = round_value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("MissionRound is not a JSON object".to_string())
        })?;
        round_object.insert(
            "failure".to_string(),
            serde_json::json!({
                "reason": reason,
                "created": Self::now_timestamp(),
            }),
        );
        round_object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        object.insert("status".to_string(), Value::String("failed".to_string()));
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

/// Whether a Round's plan carries recorded approval evidence.
pub fn plan_is_approved(round: &crate::model::mission::MissionRound) -> bool {
    round
        .phase_evidence
        .get("plan_approval")
        .map(|approval| !approval.is_null())
        .unwrap_or(false)
}

/// Validate a drafted plan against its frozen charter: unique stable Goal
/// keys, unique wave numbers, criteria ids covered by the charter, and output
/// obligations covered by the charter's artifact contract.
fn validate_plan_against_charter(
    plan: &MissionPlan,
    charter: &crate::model::mission::MissionRoundRequest,
) -> RefineResult<()> {
    let charter_criteria: std::collections::BTreeSet<&str> = charter
        .criteria
        .iter()
        .map(|criterion| criterion.id.as_str())
        .collect();
    let charter_keys: std::collections::BTreeSet<&str> = charter
        .artifact_obligations
        .iter()
        .map(|obligation| obligation.key.as_str())
        .collect();
    let mut keys = std::collections::BTreeSet::new();
    let mut waves = std::collections::BTreeSet::new();
    for wave in &plan.waves {
        if !waves.insert(wave.number) {
            return Err(RefineError::InvalidInput(format!(
                "plan wave numbers must be unique; wave {} repeats",
                wave.number
            )));
        }
        for spec in &wave.goal_specs {
            let key = spec.mission_goal_key.trim();
            if key.is_empty() || !keys.insert(key.to_string()) {
                return Err(RefineError::InvalidInput(format!(
                    "every Goal specification requires a unique mission_goal_key; {} repeats or is empty",
                    spec.mission_goal_key
                )));
            }
            if spec.prompt.trim().is_empty() {
                return Err(RefineError::InvalidInput(format!(
                    "Goal specification {key} requires a prompt"
                )));
            }
            for criterion_id in &spec.criterion_ids {
                if !charter_criteria.contains(criterion_id.as_str()) {
                    return Err(RefineError::InvalidInput(format!(
                        "Goal specification {key} references unknown criterion {criterion_id}"
                    )));
                }
            }
            for obligation_key in &spec.output_artifact_keys {
                if !charter_keys.contains(obligation_key.as_str()) {
                    return Err(RefineError::InvalidInput(format!(
                        "Goal specification {key} owes unknown artifact obligation {obligation_key}"
                    )));
                }
            }
        }
    }
    for criterion_id in &plan.criteria_coverage {
        if !charter_criteria.contains(criterion_id.as_str()) {
            return Err(RefineError::InvalidInput(format!(
                "plan coverage references unknown criterion {criterion_id}"
            )));
        }
    }
    for obligation in &plan.artifact_obligations {
        if !charter_keys.contains(obligation.key.as_str()) {
            return Err(RefineError::InvalidInput(format!(
                "plan adds artifact obligation {} outside the charter",
                obligation.key
            )));
        }
    }
    Ok(())
}
