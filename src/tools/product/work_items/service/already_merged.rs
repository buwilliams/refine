use super::*;
use crate::model::goal::RoundIntegration;
use serde_json::json;

use super::workflow_attempts::{goal_status, require_current_attempt};

#[derive(Clone, Debug)]
pub(crate) struct AlreadyMergedResolutionSnapshot {
    pub(crate) authority: WorkflowAttemptAuthority,
    pub(crate) candidate_commit: String,
    pub(crate) integration: Option<RoundIntegration>,
    pub(crate) gate_evidence: Value,
    pub(crate) gate_failure: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum AlreadyMergedInspection {
    Eligible(AlreadyMergedResolutionSnapshot),
    AlreadyResolved(GoalSummaryProjection, Value),
    AlreadyFailed(GoalSummaryProjection, Value),
}

#[derive(Clone, Debug)]
pub(crate) enum AlreadyMergedSettlementDecision {
    Review(Value),
    Failed {
        category: String,
        message: String,
        evidence: Value,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum AlreadyMergedSettlement {
    Resolved(GoalSummaryProjection, Value),
    Failed(GoalSummaryProjection, Value),
    AlreadyResolved(GoalSummaryProjection, Value),
    AlreadyFailed(GoalSummaryProjection, Value),
}

impl FileWorkItemService {
    pub(crate) fn inspect_already_merged_resolution(
        &self,
        goal_id: &str,
    ) -> RefineResult<AlreadyMergedInspection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let summary = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&summary)?;
        let (_, detail) = self.read_goal_value_unchecked_locked(&summary)?;
        let round_idx =
            summary.goal.round_count.checked_sub(1).ok_or_else(|| {
                RefineError::Conflict(format!("Goal {goal_id} has no current Round"))
            })?;
        let round = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(round_idx))
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no current Round {}",
                    round_idx + 1
                ))
            })?;
        if let Some(evidence) = terminal_reconciliation(round, "resolved")
            && summary.goal.status == GoalStatus::Review
        {
            return Ok(AlreadyMergedInspection::AlreadyResolved(
                summary,
                evidence.clone(),
            ));
        }
        if let Some(evidence) = terminal_reconciliation(round, "failed")
            && summary.goal.status == GoalStatus::Failed
        {
            return Ok(AlreadyMergedInspection::AlreadyFailed(
                summary,
                evidence.clone(),
            ));
        }
        if summary.goal.status != GoalStatus::Quality {
            return Err(RefineError::InvalidInput(format!(
                "Goal {goal_id} can only resolve an already-merged candidate from quality"
            )));
        }
        if round.get("workflow_integration").is_none_or(Value::is_null) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} has no authoritative current-Round integration identity; candidate ancestry alone is not resolvable"
            )));
        }

        let authority = workflow_attempt_authority(goal_id, round, round_idx)?;
        let candidate_commit = detail
            .get("candidate_commit")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        let integration = round
            .get("workflow_integration")
            .cloned()
            .map(serde_json::from_value::<RoundIntegration>)
            .transpose()
            .ok()
            .flatten();
        let gate_evidence = resolution_gate_evidence(&detail, round);
        let gate_failure =
            validate_resolution_gates(goal_id, &candidate_commit, integration.as_ref(), round);
        Ok(AlreadyMergedInspection::Eligible(
            AlreadyMergedResolutionSnapshot {
                authority,
                candidate_commit,
                integration,
                gate_evidence,
                gate_failure,
            },
        ))
    }

    pub(crate) fn settle_already_merged_resolution(
        &self,
        goal_id: &str,
        snapshot: &AlreadyMergedResolutionSnapshot,
        decision: AlreadyMergedSettlementDecision,
    ) -> RefineResult<AlreadyMergedSettlement> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        let round = object
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(snapshot.authority.round_idx))
            .and_then(Value::as_object)
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no authoritative Round {} for already-merged resolution",
                    snapshot.authority.round_idx + 1
                ))
            })?;

        if observed_status == GoalStatus::Review
            && let Some(evidence) = terminal_reconciliation_object(round, "resolved")
        {
            return Ok(AlreadyMergedSettlement::AlreadyResolved(
                self.show_goal_summary(goal_id)?,
                evidence.clone(),
            ));
        }
        if observed_status == GoalStatus::Failed
            && let Some(evidence) = terminal_reconciliation_object(round, "failed")
        {
            return Ok(AlreadyMergedSettlement::AlreadyFailed(
                self.show_goal_summary(goal_id)?,
                evidence.clone(),
            ));
        }
        if observed_status != GoalStatus::Quality {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from quality to {} before already-merged resolution",
                observed_status.as_str()
            )));
        }
        require_current_attempt(goal_id, object, snapshot.authority)?;
        let current_round = object
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(snapshot.authority.round_idx))
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} Round changed")))?;
        let current_gate_evidence =
            resolution_gate_evidence(&Value::Object(object.clone()), current_round);
        if current_gate_evidence != snapshot.gate_evidence {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} gate or integration evidence changed before already-merged resolution"
            )));
        }

        let round = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .and_then(|rounds| rounds.get_mut(snapshot.authority.round_idx))
            .and_then(Value::as_object_mut)
            .expect("already-merged Round was validated above");

        let now = now_timestamp();
        let (status, evidence) = match decision {
            AlreadyMergedSettlementDecision::Review(mut evidence) => {
                if snapshot.gate_failure.is_some() {
                    return Err(RefineError::Conflict(format!(
                        "Goal {goal_id} cannot resolve to review without exact passed gates"
                    )));
                }
                evidence["state"] = json!("resolved");
                evidence["resolved_at"] = json!(now);
                (GoalStatus::Review, evidence)
            }
            AlreadyMergedSettlementDecision::Failed {
                category,
                message,
                mut evidence,
            } => {
                evidence["state"] = json!("failed");
                evidence["failed_at"] = json!(now);
                round.insert("failure_category".to_string(), json!(category));
                round.insert("failure_message".to_string(), json!(message));
                round.insert("failure_at".to_string(), json!(now));
                (GoalStatus::Failed, evidence)
            }
        };
        round.insert("workflow_reconciliation".to_string(), evidence.clone());
        round.insert("workflow_attempt_authority".to_string(), Value::Null);
        round.insert("updated".to_string(), json!(now));
        object.insert("status".to_string(), json!(status.as_str()));
        object.insert("updated".to_string(), json!(now));
        write_json_atomically(&goal_path, &value)?;
        let summary = self.show_goal_summary(goal_id)?;
        Ok(match status {
            GoalStatus::Review => AlreadyMergedSettlement::Resolved(summary, evidence),
            GoalStatus::Failed => AlreadyMergedSettlement::Failed(summary, evidence),
            _ => unreachable!("already-merged settlement is terminal"),
        })
    }
}

fn workflow_attempt_authority(
    goal_id: &str,
    round: &Value,
    round_idx: usize,
) -> RefineResult<WorkflowAttemptAuthority> {
    let attempt = round
        .get("workflow_attempt_authority")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no current workflow attempt authority for already-merged resolution"
            ))
        })?;
    let recorded_round = attempt
        .get("round_idx")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let workflow_revision = attempt.get("workflow_revision").and_then(Value::as_u64);
    if recorded_round != Some(round_idx) || workflow_revision.is_none() {
        return Err(RefineError::Conflict(format!(
            "Goal {goal_id} has mismatched workflow attempt authority for Round {}",
            round_idx + 1
        )));
    }
    Ok(WorkflowAttemptAuthority {
        round_idx,
        workflow_revision: workflow_revision.unwrap_or_default(),
    })
}

fn resolution_gate_evidence(detail: &Value, round: &Value) -> Value {
    let mut evidence = Map::new();
    evidence.insert(
        "candidate_commit".to_string(),
        detail
            .get("candidate_commit")
            .cloned()
            .unwrap_or(Value::Null),
    );
    for key in [
        "workflow_integration",
        "quality_state",
        "quality_candidate_commit",
        "quality_details",
        "quality_checked_at",
        "rule_state",
        "meta_rule_state",
        "product_state",
        "constitution_state",
        "governance_candidate_commit",
        "governance_checked_at",
    ] {
        evidence.insert(
            key.to_string(),
            round.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    Value::Object(evidence)
}

fn validate_resolution_gates(
    goal_id: &str,
    candidate: &str,
    integration: Option<&RoundIntegration>,
    round: &Value,
) -> Option<String> {
    if candidate.is_empty() {
        return Some(format!("Goal {goal_id} has no exact candidate commit"));
    }
    let Some(integration) = integration else {
        return Some(format!(
            "Goal {goal_id} has invalid current-Round integration evidence"
        ));
    };
    if integration.candidate_commit != candidate {
        return Some(format!(
            "Goal {goal_id} candidate {candidate} does not match integrated candidate {}",
            integration.candidate_commit
        ));
    }
    if !integration.merge.ok
        || !integration.merge.conflicts.is_empty()
        || integration.target_branch.trim().is_empty()
        || integration.target_commit.trim().is_empty()
        || integration.integrated_at.trim().is_empty()
        || integration.pushed && integration.remote.trim().is_empty()
    {
        return Some(format!(
            "Goal {goal_id} has incomplete or unsuccessful current-Round integration evidence"
        ));
    }
    if round.get("quality_state").and_then(Value::as_str) != Some("passed")
        || round
            .get("quality_candidate_commit")
            .and_then(Value::as_str)
            != Some(candidate)
        || round
            .get("quality_details")
            .and_then(|details| details.get("candidate_commit"))
            .and_then(Value::as_str)
            != Some(candidate)
        || round
            .get("quality_details")
            .and_then(|details| details.get("source_candidate_commit"))
            .and_then(Value::as_str)
            != Some(candidate)
        || round
            .get("quality_details")
            .and_then(|details| details.get("evaluation_scope"))
            .and_then(Value::as_str)
            != Some("isolated_candidate")
        || !nonempty_round_string(round, "quality_checked_at")
    {
        return Some(format!(
            "Goal {goal_id} lacks durable passed isolated Quality evidence for exact candidate {candidate}"
        ));
    }
    if [
        "rule_state",
        "meta_rule_state",
        "product_state",
        "constitution_state",
    ]
    .into_iter()
    .any(|key| round.get(key).and_then(Value::as_str) != Some("passed"))
        || round
            .get("governance_candidate_commit")
            .and_then(Value::as_str)
            != Some(candidate)
        || !nonempty_round_string(round, "governance_checked_at")
    {
        return Some(format!(
            "Goal {goal_id} lacks durable passed Governance evidence for exact candidate {candidate}"
        ));
    }
    None
}

fn nonempty_round_string(round: &Value, key: &str) -> bool {
    round
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn terminal_reconciliation<'a>(round: &'a Value, state: &str) -> Option<&'a Value> {
    round
        .get("workflow_reconciliation")
        .filter(|evidence| evidence.get("state").and_then(Value::as_str) == Some(state))
}

fn terminal_reconciliation_object<'a>(
    round: &'a Map<String, Value>,
    state: &str,
) -> Option<&'a Value> {
    round
        .get("workflow_reconciliation")
        .filter(|evidence| evidence.get("state").and_then(Value::as_str) == Some(state))
}
