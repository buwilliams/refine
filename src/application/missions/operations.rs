//! Mission operations: decision answers, stage retries, coordinator
//! transfer, plan Goal adoptions and removals, and the context projection.
//!
//! Every operation is a short, fenced mutation of durable Mission (and for
//! adoptions, Goal) state. None of them advance workflow status: decisions
//! and retries unblock the engine, transfers change only the coordinator,
//! and plan membership changes bind only when their exact effective plan
//! digest is approved. See `docs/mission-spec.md`.

use serde_json::{Value, json};

use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::application::missions::reconciliation::compute_plan_digest;
use crate::application::missions::service::FileMissionService;
use crate::application::missions::workflow::plan_is_approved;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::model::mission::{Mission, MissionGoalSpec, MissionPlan, MissionRound, MissionWave};

impl FileMissionService {
    /// Answer one reconciliation decision request with a recorded choice.
    /// The answer is durable evidence; reconciliation consumes it at the
    /// next boundary and the attention resolves.
    pub fn answer_decision(
        &self,
        mission_id: &str,
        decision_id: &str,
        choice: &str,
        rationale: &str,
        actor: &str,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let value = self.show_mission_value(&mission_id)?;
        self.check_revision(&mission_id, &value, observed_revision)?;
        let mut mission = self.parse_for_mutation(&value)?;
        let round_number = mission.current_round.unwrap_or(0);
        let round = Self::round_mut(&mut mission, round_number)?;

        let request = round
            .reconciliation_receipts
            .iter()
            .flat_map(|receipt| receipt.decision_requests.iter())
            .find(|request| request.id == decision_id)
            .cloned()
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Mission {mission_id} has no open decision request {decision_id}"
                ))
            })?;
        if !request.choices.is_empty()
            && !request.choices.iter().any(|candidate| candidate == choice)
        {
            return Err(RefineError::InvalidInput(format!(
                "decision {decision_id} accepts one of: {}",
                request.choices.join(", ")
            )));
        }

        let mut decisions = round
            .phase_evidence
            .get("decisions")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        decisions.insert(
            decision_id.to_string(),
            json!({
                "choice": choice,
                "rationale": rationale,
                "actor": actor,
                "answered_at": Self::now_timestamp(),
                "group": request.group,
                "load_bearing": request.load_bearing,
            }),
        );
        round
            .phase_evidence
            .insert("decisions".to_string(), Value::Object(decisions));
        round.updated = Self::now_timestamp();
        self.write_typed(&mission_id, &mission, value)
    }

    /// Authorize the retry of one retryable stage failure. Only a recorded
    /// stage failure in a nonterminal Round may be retried; a Failed Round
    /// requires a new Round instead.
    pub fn retry_stage(
        &self,
        mission_id: &str,
        stage: &str,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let value = self.show_mission_value(&mission_id)?;
        self.check_revision(&mission_id, &value, observed_revision)?;
        let mission = self.parse_for_mutation(&value)?;
        if mission.status.is_terminal() {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} is {}; a terminal Round requires a new Round, not a retry",
                mission.status.as_str()
            )));
        }
        crate::application::missions::phases::authorize_stage_retry(self, &mission_id, stage)
    }

    /// Transfer Mission coordination to another Node. Transfer is explicit,
    /// changes only the coordinator, and never moves child Goals.
    pub fn transfer_mission(
        &self,
        mission_id: &str,
        node_id: &str,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        FileNodeRegistryService::new(&self.refine_dir).ensure_transfer_target(node_id)?;
        let value = self.show_mission_value(&mission_id)?;
        self.check_revision(&mission_id, &value, observed_revision)?;
        let mut mission = self.parse_for_mutation(&value)?;
        mission.coordinator_node_id = Some(node_id.trim().to_string());
        let written = self.write_typed(&mission_id, &mission, value)?;
        Ok(written)
    }

    /// Adopt one existing Goal into the Mission plan. Before the first plan
    /// approval this edits the drafted plan; afterwards it drafts a material
    /// amendment. The Goal binds to the Mission only when the resulting
    /// effective plan digest is approved.
    #[allow(clippy::too_many_arguments)]
    pub fn add_plan_goal(
        &self,
        mission_id: &str,
        goal_id: &str,
        wave: usize,
        role: Option<&str>,
        required: bool,
        criterion_ids: &[String],
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let goal_id = goal_id.trim().to_uppercase();
        let work_items = FileWorkItemService::new(&self.refine_dir);
        let goal = work_items.show_goal_summary(&goal_id)?;
        if goal.goal.mission.is_some() {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is already bound to a Mission"
            )));
        }
        // The Goal's latest Round prompt is the authoritative work
        // description the adopted specification carries.
        let prompt = work_items
            .show_goal_detail(&goal_id)?
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.last())
            .and_then(|round| round.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or(goal.goal.name.as_str())
            .to_string();
        let spec = MissionGoalSpec {
            mission_goal_key: goal_id.clone(),
            name: goal.goal.name.clone(),
            prompt,
            role: role.map(str::to_string),
            required,
            criterion_ids: criterion_ids.to_vec(),
            input_artifact_keys: vec![],
            output_artifact_keys: vec![],
            expected_findings: vec![],
            feature_id: goal.goal.feature_id.clone(),
            feature_order: None,
            preferred_node: goal.goal.node_id.clone(),
        };
        self.mutate_plan_membership(
            mission_id,
            observed_revision,
            "adopt",
            &goal_id,
            &goal_id,
            wave,
            Some(spec),
        )
    }

    /// Exclude one Goal specification from the Mission plan. Before the
    /// first plan approval this edits the drafted plan; afterwards it drafts
    /// a material amendment. A bound Goal whose Rounds never pinned Mission
    /// context is safely unbound when the amendment is approved.
    pub fn remove_plan_goal(
        &self,
        mission_id: &str,
        goal_id: &str,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let goal_id = goal_id.trim().to_uppercase();
        let mission_id = mission_id.trim().to_uppercase();
        let _work_items = FileWorkItemService::new(&self.refine_dir);
        let bound = crate::application::missions::phases::execution::mission_bound_goals(
            &self.refine_dir,
            &mission_id,
        )?
        .into_iter()
        .find(|goal| goal.goal_id == goal_id);
        let key = bound
            .map(|goal| goal.mission_goal_key)
            .unwrap_or_else(|| goal_id.clone());
        self.mutate_plan_membership(
            &mission_id,
            observed_revision,
            "remove",
            &goal_id,
            &key,
            0,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn mutate_plan_membership(
        &self,
        mission_id: &str,
        observed_revision: Option<u64>,
        action: &str,
        goal_id: &str,
        mission_goal_key: &str,
        wave: usize,
        spec: Option<MissionGoalSpec>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let value = self.show_mission_value(&mission_id)?;
        self.check_revision(&mission_id, &value, observed_revision)?;
        let mut mission = self.parse_for_mutation(&value)?;
        let round_number = mission.current_round.unwrap_or(0);
        let approved = plan_is_approved(Self::round(&mission, round_number)?);
        let round = Self::round_mut(&mut mission, round_number)?;

        let mut plan = if approved {
            // A material amendment drafts on top of the current effective
            // plan (or the pending amendment, so drafts compose).
            round
                .plan_amendments
                .last()
                .cloned()
                .or_else(|| round.plan.clone())
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Mission {mission_id} Round {round_number} has no plan to amend"
                    ))
                })?
        } else {
            round.plan.clone().unwrap_or_else(empty_plan)
        };

        let key = mission_goal_key.trim().to_string();
        match action {
            "remove" => {
                let before: usize = plan.waves.iter().map(|wave| wave.goal_specs.len()).sum();
                for wave in plan.waves.iter_mut() {
                    wave.goal_specs.retain(|spec| spec.mission_goal_key != key);
                }
                let after: usize = plan.waves.iter().map(|wave| wave.goal_specs.len()).sum();
                if before == after {
                    return Err(RefineError::NotFound(format!(
                        "Mission {mission_id} plan has no Goal specification {key}"
                    )));
                }
                plan.waves.retain(|wave| !wave.goal_specs.is_empty());
            }
            _ => {
                let spec = spec.expect("adoption carries its specification");
                let target_wave = plan
                    .waves
                    .iter_mut()
                    .find(|candidate| candidate.number == wave);
                match target_wave {
                    Some(target_wave) => {
                        if target_wave
                            .goal_specs
                            .iter()
                            .any(|existing| existing.mission_goal_key == key)
                        {
                            return Err(RefineError::Conflict(format!(
                                "Mission {mission_id} wave {wave} already specifies Goal key {key}"
                            )));
                        }
                        target_wave.goal_specs.push(spec);
                    }
                    None => {
                        let max_wave = plan.waves.iter().map(|wave| wave.number).max();
                        if let Some(max_wave) = max_wave
                            && wave <= max_wave
                        {
                            return Err(RefineError::InvalidInput(format!(
                                "wave {wave} precedes the latest wave {max_wave}; waves are linear"
                            )));
                        }
                        plan.waves.push(MissionWave {
                            number: wave,
                            purpose: format!("adopted Goal {key}"),
                            goal_specs: vec![spec],
                            required_snapshot: None,
                            completion_condition: None,
                        });
                    }
                }
            }
        }
        plan.effective_digest = Some(compute_plan_digest(&plan));

        let round = Self::round_mut(&mut mission, round_number)?;
        if approved {
            round.plan_amendments.push(plan);
        } else {
            round.plan = Some(plan);
        }
        let mut adoptions = round
            .phase_evidence
            .get("plan_adoptions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        adoptions
            .retain(|adoption| adoption.get("goal_id").and_then(Value::as_str) != Some(goal_id));
        adoptions.push(json!({
            "action": action,
            "goal_id": goal_id,
            "mission_goal_key": key,
            "wave": wave,
        }));
        round
            .phase_evidence
            .insert("plan_adoptions".to_string(), Value::Array(adoptions));
        round.updated = Self::now_timestamp();
        self.write_typed(&mission_id, &mission, value)
    }

    /// The current effective plan digest awaiting action: a pending
    /// amendment's digest when one is drafted, otherwise the base plan's.
    pub fn current_effective_plan_digest(mission: &Mission) -> Option<String> {
        let round = mission
            .current_round
            .and_then(|number| mission.rounds.iter().find(|round| round.number == number))?;
        let approved_digests: Vec<&str> = round
            .phase_evidence
            .get("plan_approval")
            .and_then(|approval| approval.get("approved_digests"))
            .and_then(Value::as_array)
            .map(|digests| {
                digests
                    .iter()
                    .filter_map(|digest| digest.as_str())
                    .collect()
            })
            .unwrap_or_default();
        round
            .plan_amendments
            .last()
            .filter(|amendment| {
                amendment
                    .effective_digest
                    .as_deref()
                    .is_some_and(|digest| !approved_digests.contains(&digest))
            })
            .and_then(|amendment| amendment.effective_digest.clone())
            .or_else(|| {
                round
                    .plan
                    .as_ref()
                    .and_then(|plan| plan.effective_digest.clone())
            })
    }

    /// The derived Mission context projection: the current Round's snapshot
    /// chain, accepted knowledge with derived states, artifacts,
    /// contradictions, and open decisions. Bounded references, never bodies.
    /// A Mission that has not started a Round reports an empty context.
    pub fn mission_context_summary(&self, mission_id: &str) -> RefineResult<Value> {
        let mission = self.show_mission(mission_id)?;
        let round = mission
            .current_round
            .and_then(|number| mission.rounds.iter().find(|round| round.number == number));
        let Some(round) = round else {
            return Ok(json!({
                "mission_id": mission.id,
                "mission_round": Value::Null,
                "status": mission.status.as_str(),
                "latest_snapshot": 0,
                "snapshots": [],
                "assertions": [],
                "open_contradictions": [],
                "artifacts": [],
                "open_decisions": [],
                "answered_decisions": [],
            }));
        };
        let snapshots: Vec<Value> = round
            .snapshots
            .iter()
            .map(|snapshot| {
                json!({
                    "version": snapshot.version,
                    "parent_version": snapshot.parent_version,
                    "target_head": snapshot.target_head,
                    "digest": snapshot.digest,
                    "corrects_snapshot": snapshot.corrects_snapshot,
                    "created": snapshot.created,
                })
            })
            .collect();
        let latest = round
            .snapshots
            .last()
            .map(|snapshot| snapshot.version)
            .unwrap_or(0);
        let ledger = crate::application::missions::phases::ledger_summary(&mission)?;
        let assertions: Vec<Value> = serde_json::from_str(&ledger).unwrap_or_default();
        let open_contradictions: Vec<Value> = assertions
            .iter()
            .filter(|assertion| {
                assertion.get("kind").and_then(Value::as_str) == Some("contradiction")
            })
            .cloned()
            .collect();
        let artifacts: Vec<Value> = round
            .snapshots
            .last()
            .map(|snapshot| snapshot.artifact_refs.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|artifact| {
                json!({
                    "key": artifact.key,
                    "title": artifact.title,
                    "kind": artifact.kind,
                    "authority": artifact.authority.as_str(),
                    "path": artifact.path,
                    "sha256": artifact.sha256,
                    "provenance": artifact.provenance,
                })
            })
            .collect();
        let answered: Vec<&String> = round
            .phase_evidence
            .get("decisions")
            .and_then(Value::as_object)
            .map(|decisions| decisions.keys().collect())
            .unwrap_or_default();
        let decisions: Vec<Value> = round
            .reconciliation_receipts
            .iter()
            .flat_map(|receipt| receipt.decision_requests.iter())
            .filter(|request| !answered.contains(&&request.id))
            .map(|request| {
                json!({
                    "id": request.id,
                    "summary": request.summary,
                    "choices": request.choices,
                    "load_bearing": request.load_bearing,
                    "group": request.group,
                })
            })
            .collect();
        Ok(json!({
            "mission_id": mission.id,
            "mission_round": mission.current_round,
            "status": mission.status.as_str(),
            "latest_snapshot": latest,
            "snapshots": snapshots,
            "assertions": assertions,
            "open_contradictions": open_contradictions,
            "artifacts": artifacts,
            "open_decisions": decisions,
            "answered_decisions": answered,
        }))
    }

    fn round(mission: &Mission, round_number: usize) -> RefineResult<&MissionRound> {
        mission
            .rounds
            .iter()
            .find(|round| round.number == round_number)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!("Mission has no Round {round_number}"))
            })
    }

    fn round_mut(mission: &mut Mission, round_number: usize) -> RefineResult<&mut MissionRound> {
        mission
            .rounds
            .iter_mut()
            .find(|round| round.number == round_number)
            .ok_or_else(|| {
                RefineError::InvalidInput(format!("Mission has no Round {round_number}"))
            })
    }

    fn parse_for_mutation(&self, value: &Value) -> RefineResult<Mission> {
        crate::application::missions::persistence::parse_mission(value)
    }
}

/// A fresh empty Draft plan for authoring before any agent proposal exists.
fn empty_plan() -> MissionPlan {
    MissionPlan {
        charter_digest: None,
        summary: "Draft plan authored through Goal adoption".to_string(),
        assumptions: vec![],
        risks: vec![],
        criteria_coverage: vec![],
        waves: vec![],
        artifact_obligations: vec![],
        criticism: None,
        resolutions: vec![],
        effective_digest: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mission::MissionStatus;
    use serde_json::json;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-mission-ops-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn mission_with_approved_plan(
        service: &FileMissionService,
        spec: Option<MissionGoalSpec>,
    ) -> crate::model::mission::Mission {
        let mission = service
            .create_mission("M", "intent", Some("Buddy"), None, None)
            .unwrap();
        let criteria = json!([{"id": "crit:tokens", "description": "d"}]);
        service
            .edit_mission_frame(&mission.id, None, None, Some(&criteria), None, None)
            .unwrap();
        service
            .append_round(&mission.id, "Buddy", "go", None)
            .unwrap();
        let snapshot = crate::model::mission::MissionSnapshot {
            version: 1,
            parent_version: None,
            target_head: Some("head0".to_string()),
            plan_digest: None,
            artifact_refs: vec![],
            input_refs: vec![],
            consumed_contribution_refs: vec![],
            knowledge_index: vec![],
            corrects_snapshot: None,
            digest: None,
            created: String::new(),
        };
        service
            .publish_snapshot(&mission.id, snapshot, None)
            .unwrap();
        let spec = spec.unwrap_or(MissionGoalSpec {
            mission_goal_key: "k1".to_string(),
            name: "Document tokens".to_string(),
            prompt: "document tokens".to_string(),
            role: None,
            required: true,
            criterion_ids: vec!["crit:tokens".to_string()],
            input_artifact_keys: vec![],
            output_artifact_keys: vec![],
            expected_findings: vec![],
            feature_id: None,
            feature_order: None,
            preferred_node: None,
        });
        let plan = MissionPlan {
            charter_digest: None,
            summary: "one wave".to_string(),
            assumptions: vec![],
            risks: vec![],
            criteria_coverage: vec!["crit:tokens".to_string()],
            waves: vec![MissionWave {
                number: 1,
                purpose: "p".to_string(),
                goal_specs: vec![spec],
                required_snapshot: None,
                completion_condition: None,
            }],
            artifact_obligations: vec![],
            criticism: None,
            resolutions: vec![],
            effective_digest: None,
        };
        let mission = service.record_plan(&mission.id, plan, None).unwrap();
        let digest = mission.rounds[0]
            .plan
            .as_ref()
            .and_then(|plan| plan.effective_digest.clone())
            .unwrap();
        service
            .approve_plan(&mission.id, &digest, "Buddy", "ok", None)
            .unwrap()
    }

    #[test]
    fn approve_plan_rejects_a_stale_digest() {
        let dir = temp_dir("stale-digest");
        let service = FileMissionService::new(&dir);
        let mission = mission_with_approved_plan(&service, None);
        let err = service
            .approve_plan(&mission.id, "sha256:wrong", "", "", None)
            .unwrap_err();
        assert!(err.to_string().contains("does not match"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transfer_changes_only_the_coordinator() {
        let dir = temp_dir("transfer");
        let service = FileMissionService::new(&dir);
        let nodes = FileNodeRegistryService::new(&dir);
        nodes.create("worker-2").unwrap();
        let mission = mission_with_approved_plan(&service, None);
        let transferred = service
            .transfer_mission(&mission.id, "worker-2", None)
            .unwrap();
        assert_eq!(transferred.coordinator_node_id.as_deref(), Some("worker-2"));
        // Child Goals are untouched: transfer never moves them.
        assert!(transferred.rounds.len() == mission.rounds.len());
        let err = service
            .transfer_mission(&mission.id, "missing-node", None)
            .unwrap_err();
        assert!(err.to_string().contains("was not found"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_goal_after_approval_drafts_a_material_amendment() {
        let dir = temp_dir("amendment");
        let service = FileMissionService::new(&dir);
        let work_items = FileWorkItemService::new(&dir);
        let mission = mission_with_approved_plan(&service, None);

        let adopted = work_items
            .author_goal(crate::application::work_items::GoalAuthoringRequest {
                id: None,
                goal_id: None,
                name: Some("Adopted".to_string()),
                prompt: "do the adopted work".to_string(),
                reporter: "Buddy".to_string(),
                assignee: None,
                priority: "medium".to_string(),
                feature_id: None,
                placement: crate::application::work_items::FeatureGoalPlacement::Unordered,
                duplicate_decision: String::new(),
            })
            .unwrap();
        let goal_id = adopted.goal.as_ref().unwrap().id.clone();
        let mission = service
            .add_plan_goal(
                &mission.id,
                &goal_id,
                2,
                Some("implementer"),
                true,
                &[],
                None,
            )
            .unwrap();
        let round = &mission.rounds[0];
        assert_eq!(
            round.plan_amendments.len(),
            1,
            "adoption drafts an amendment"
        );
        let amendment = &round.plan_amendments[0];
        assert_eq!(amendment.waves.len(), 2);
        assert_eq!(amendment.waves[1].goal_specs[0].mission_goal_key, goal_id);

        // The Goal is not bound until the amendment digest is approved.
        assert!(
            work_items
                .show_goal_summary(&goal_id)
                .unwrap()
                .goal
                .mission
                .is_none(),
            "adoption binds only at approval"
        );
        let digest = amendment.effective_digest.clone().unwrap();
        let mission = service
            .approve_plan(&mission.id, &digest, "Buddy", "add the Goal", None)
            .unwrap();
        let round = &mission.rounds[0];
        assert_eq!(round.plan.as_ref().unwrap().waves.len(), 2);
        assert!(
            work_items
                .show_goal_summary(&goal_id)
                .unwrap()
                .goal
                .mission
                .is_some(),
            "approval commits the adoption binding"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_goal_unbinds_only_unpinned_membership() {
        let dir = temp_dir("removal");
        let service = FileMissionService::new(&dir);
        let work_items = FileWorkItemService::new(&dir);
        let spec = MissionGoalSpec {
            mission_goal_key: "k1".to_string(),
            name: "Document tokens".to_string(),
            prompt: "document tokens".to_string(),
            role: None,
            required: true,
            criterion_ids: vec!["crit:tokens".to_string()],
            input_artifact_keys: vec![],
            output_artifact_keys: vec![],
            expected_findings: vec![],
            feature_id: None,
            feature_order: None,
            preferred_node: None,
        };
        let mission = mission_with_approved_plan(&service, Some(spec));
        let goal_id = "GOAL1".to_string();
        work_items
            .author_goal(crate::application::work_items::GoalAuthoringRequest {
                id: Some(goal_id.clone()),
                goal_id: None,
                name: Some("Document tokens".to_string()),
                prompt: "document tokens".to_string(),
                reporter: "Buddy".to_string(),
                assignee: None,
                priority: "medium".to_string(),
                feature_id: None,
                placement: crate::application::work_items::FeatureGoalPlacement::Unordered,
                duplicate_decision: String::new(),
            })
            .unwrap();
        work_items
            .bind_goal_to_mission(&goal_id, &mission.id, "k1")
            .unwrap();
        let mission = service
            .remove_plan_goal(&mission.id, &goal_id, None)
            .unwrap();
        let digest = mission.rounds[0]
            .plan_amendments
            .last()
            .and_then(|amendment| amendment.effective_digest.clone())
            .unwrap();
        let mission = service
            .approve_plan(&mission.id, &digest, "Buddy", "remove it", None)
            .unwrap();
        let round = &mission.rounds[0];
        assert!(
            round.plan.as_ref().unwrap().waves.is_empty(),
            "the spec is gone from the effective plan"
        );
        assert!(
            work_items
                .show_goal_summary(&goal_id)
                .unwrap()
                .goal
                .mission
                .is_none(),
            "an unpinned membership is safely unbound at approval"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn answer_decision_validates_choices_and_records_evidence() {
        let dir = temp_dir("decision");
        let service = FileMissionService::new(&dir);
        let mission = mission_with_approved_plan(&service, None);
        let mut mission = service.show_mission(&mission.id).unwrap();
        let round = &mut mission.rounds[0];
        round
            .reconciliation_receipts
            .push(crate::model::mission::ReconciliationReceipt {
                attempt: "mission:M:round:1:reconcile:1:1".to_string(),
                parent_snapshot: 1,
                next_snapshot: 2,
                wave: Some(1),
                claim_set: vec![],
                verifier_results: vec![],
                accepted: vec![],
                rejected: vec![],
                deferred: vec![],
                contested: vec![],
                dissent: vec![],
                criticism_ref: None,
                decision_requests: vec![crate::model::mission::DecisionRequest {
                    id: "dec-1".to_string(),
                    group: None,
                    summary: "which contract applies".to_string(),
                    choices: vec!["keep-old".to_string(), "adopt-new".to_string()],
                    load_bearing: true,
                    rank: 0,
                    deferred: false,
                }],
                budgets: Default::default(),
                plan_quality: None,
                correction: None,
                created: String::new(),
            });
        crate::application::missions::MissionService::update_mission(&service, mission.clone())
            .unwrap();

        let err = service
            .answer_decision(&mission.id, "dec-1", "neither", "", "Buddy", None)
            .unwrap_err();
        assert!(err.to_string().contains("accepts one of"), "{err}");
        let answered = service
            .answer_decision(
                &mission.id,
                "dec-1",
                "adopt-new",
                "new is proven",
                "Buddy",
                None,
            )
            .unwrap();
        let evidence = &answered.rounds[0].phase_evidence["decisions"]["dec-1"];
        assert_eq!(evidence["choice"], json!("adopt-new"));
        let err = service
            .answer_decision(&mission.id, "missing", "x", "", "Buddy", None)
            .unwrap_err();
        assert!(
            err.to_string().contains("no open decision request"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retry_requires_a_recorded_stage_failure() {
        let dir = temp_dir("retry");
        let service = FileMissionService::new(&dir);
        let mission = mission_with_approved_plan(&service, None);
        let err = service
            .retry_stage(&mission.id, "quality", None)
            .unwrap_err();
        assert!(
            err.to_string().contains("no retryable stage failure"),
            "{err}"
        );
        crate::application::missions::phases::mark_stage_failed(
            &service,
            &mission.id,
            "quality",
            "judgment failed",
        )
        .unwrap();
        let retried = service.retry_stage(&mission.id, "quality", None).unwrap();
        assert_eq!(
            retried.rounds[0].phase_evidence["quality"]["retry_authorized"],
            json!(true)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_summary_reports_ledger_artifacts_and_decisions() {
        let dir = temp_dir("context");
        let service = FileMissionService::new(&dir);
        let mission = mission_with_approved_plan(&service, None);
        let context = service.mission_context_summary(&mission.id).unwrap();
        assert_eq!(context["mission_id"], json!(mission.id));
        assert_eq!(context["latest_snapshot"], json!(1));
        assert_eq!(context["status"], json!("draft"));
        assert!(context["snapshots"].as_array().unwrap().len() == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
