use super::*;
use crate::model::goal::{QUALITY_PROOF_SCHEMA_VERSION, QualityProof};
use crate::process::supervisor::operations::{OperationHandle, OperationState};
use crate::tools::host::quality::QualityCheckResult;
use serde_json::json;

use super::workflow_attempts::{goal_status, require_current_attempt};

pub(crate) const ALREADY_MERGED_QUALITY_FAILED: &str = "already_merged_quality_failed";

#[derive(Clone, Debug)]
struct FailedQualityEvidence {
    message: String,
    details: Value,
    proof: QualityProof,
}

impl FileWorkItemService {
    /// Settles the first exact failed already-merged Quality evaluation without consulting the
    /// passed-proof resolver. A terminal operation is accepted only as restart recovery when the
    /// same failed proof is not already present on the Round.
    pub(crate) fn settle_already_merged_quality_failure(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        candidate: &str,
        terminal_operation: Option<&OperationHandle>,
    ) -> RefineResult<Option<AlreadyMergedSettlement>> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        let existing_terminal = object
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(authority.round_idx))
            .and_then(|round| round.get("workflow_reconciliation"))
            .filter(|evidence| {
                evidence.get("state").and_then(Value::as_str) == Some("failed")
                    && evidence.get("category").and_then(Value::as_str)
                        == Some(ALREADY_MERGED_QUALITY_FAILED)
                    && evidence.get("round_idx").and_then(Value::as_u64)
                        == u64::try_from(authority.round_idx).ok()
                    && evidence.get("workflow_revision").and_then(Value::as_u64)
                        == Some(authority.workflow_revision)
                    && evidence.get("candidate_commit").and_then(Value::as_str) == Some(candidate)
            })
            .cloned();
        if observed_status == GoalStatus::Failed
            && let Some(evidence) = existing_terminal
        {
            return Ok(Some(AlreadyMergedSettlement::AlreadyFailed(
                current, evidence,
            )));
        }
        if observed_status != GoalStatus::Quality {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from quality to {} before failed already-merged Quality settlement",
                observed_status.as_str()
            )));
        }
        require_current_attempt(goal_id, object, authority)?;
        if object.get("candidate_commit").and_then(Value::as_str) != Some(candidate) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed exact candidate before failed already-merged Quality settlement"
            )));
        }
        let round = object
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(authority.round_idx))
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} Round changed")))?;
        if round
            .get("workflow_integration")
            .and_then(|integration| integration.get("candidate_commit"))
            .and_then(Value::as_str)
            != Some(candidate)
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} lacks exact current-Round integration identity for failed Quality settlement"
            )));
        }

        let failure = failed_quality_from_round(goal_id, authority.round_idx, candidate, round)
            .or_else(|| {
                terminal_operation.and_then(|operation| {
                    failed_quality_from_operation(goal_id, authority, candidate, operation)
                })
            });
        let Some(failure) = failure else {
            return Ok(None);
        };

        let now = now_timestamp();
        let round = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .and_then(|rounds| rounds.get_mut(authority.round_idx))
            .and_then(Value::as_object_mut)
            .expect("failed already-merged Quality Round was validated above");
        round.insert("quality_state".to_string(), json!("failed"));
        round.insert(
            "quality_message".to_string(),
            json!(failure.message.clone()),
        );
        round.insert("quality_details".to_string(), failure.details.clone());
        round.insert(
            "quality_checked_at".to_string(),
            json!(failure.proof.checked_at.clone()),
        );
        round.insert("quality_candidate_commit".to_string(), json!(candidate));
        let mut reconciliation = round
            .get("workflow_reconciliation")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        reconciliation.insert("state".to_string(), json!("failed"));
        reconciliation.insert("category".to_string(), json!(ALREADY_MERGED_QUALITY_FAILED));
        reconciliation.insert("round_idx".to_string(), json!(authority.round_idx));
        reconciliation.insert(
            "workflow_revision".to_string(),
            json!(authority.workflow_revision),
        );
        reconciliation.insert("candidate_commit".to_string(), json!(candidate));
        reconciliation.insert("quality_proof_mode".to_string(), json!("regenerated"));
        reconciliation.insert(
            "quality_operation_id".to_string(),
            json!(failure.proof.operation_id.clone()),
        );
        reconciliation.insert("quality_proof".to_string(), json!(failure.proof));
        reconciliation.insert("quality_failure".to_string(), failure.details);
        reconciliation.insert(
            "quality_message".to_string(),
            json!(failure.message.clone()),
        );
        reconciliation.insert("failed_at".to_string(), json!(now.clone()));
        round.insert(
            "workflow_reconciliation".to_string(),
            Value::Object(reconciliation.clone()),
        );
        round.insert(
            "failure_category".to_string(),
            json!(ALREADY_MERGED_QUALITY_FAILED),
        );
        round.insert("failure_message".to_string(), json!(failure.message));
        round.insert("failure_at".to_string(), json!(now.clone()));
        round.insert("workflow_attempt_authority".to_string(), Value::Null);
        round.insert("updated".to_string(), json!(now.clone()));
        object.insert("status".to_string(), json!(GoalStatus::Failed.as_str()));
        object.insert("updated".to_string(), json!(now));
        write_json_atomically(&goal_path, &value)?;
        Ok(Some(AlreadyMergedSettlement::Failed(
            self.show_goal_summary(goal_id)?,
            Value::Object(reconciliation),
        )))
    }
}

fn failed_quality_from_round(
    goal_id: &str,
    round_idx: usize,
    candidate: &str,
    round: &Value,
) -> Option<FailedQualityEvidence> {
    if round.get("quality_state").and_then(Value::as_str) != Some("failed")
        || round
            .get("quality_candidate_commit")
            .and_then(Value::as_str)
            != Some(candidate)
    {
        return None;
    }
    let details = round.get("quality_details")?.clone();
    let proof =
        serde_json::from_value::<QualityProof>(details.get("quality_proof")?.clone()).ok()?;
    if !valid_failed_quality_proof(&proof, goal_id, round_idx, candidate, round, &details) {
        return None;
    }
    Some(FailedQualityEvidence {
        message: round
            .get("quality_message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())?
            .to_string(),
        details,
        proof,
    })
}

fn failed_quality_from_operation(
    goal_id: &str,
    authority: WorkflowAttemptAuthority,
    candidate: &str,
    operation: &OperationHandle,
) -> Option<FailedQualityEvidence> {
    if operation.state != OperationState::Failed
        || operation.owner != format!("quality:{goal_id}:{candidate}")
        || operation.request.get("goal_id").and_then(Value::as_str) != Some(goal_id)
        || operation.request.get("round_idx").and_then(Value::as_u64)
            != u64::try_from(authority.round_idx).ok()
        || operation
            .request
            .get("workflow_revision")
            .and_then(Value::as_u64)
            != Some(authority.workflow_revision)
        || operation
            .request
            .get("candidate_commit")
            .and_then(Value::as_str)
            != Some(candidate)
        || operation
            .request
            .get("source_candidate_commit")
            .and_then(Value::as_str)
            != Some(candidate)
        || operation
            .request
            .get("evaluation_scope")
            .and_then(Value::as_str)
            != Some("isolated_candidate")
        || operation
            .request
            .get("quality_proof_mode")
            .and_then(Value::as_str)
            != Some("regenerated")
    {
        return None;
    }
    let result = serde_json::from_value::<QualityCheckResult>(operation.result.clone()).ok()?;
    let checked_at = result.checked_at.clone()?;
    if result.ok
        || result.owner_id != goal_id
        || result.candidate_commit != candidate
        || result.summary.trim().is_empty()
        || !result
            .results
            .iter()
            .any(|item| item.status.as_str() == "failed")
    {
        return None;
    }
    let results = result
        .results
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let proof = QualityProof {
        schema_version: QUALITY_PROOF_SCHEMA_VERSION,
        goal_id: goal_id.to_string(),
        round_idx: authority.round_idx,
        evaluation_scope: "isolated_candidate".to_string(),
        operation_id: operation.id.clone(),
        checked_candidate_commit: candidate.to_string(),
        source_candidate_commit: candidate.to_string(),
        state: "failed".to_string(),
        checked_at,
        results,
    };
    let details = json!({
        "operation_id": operation.id,
        "candidate_commit": candidate,
        "source_candidate_commit": candidate,
        "evaluation_scope": "isolated_candidate",
        "cwd": operation.request.get("cwd").cloned().unwrap_or(Value::Null),
        "results": result.results,
        "diagnostics": result.diagnostics,
        "provider_attempts": result.provider_attempts,
        "quality_proof": proof,
        "quality_proof_mode": "regenerated"
    });
    Some(FailedQualityEvidence {
        message: result.summary,
        details,
        proof,
    })
}

fn valid_failed_quality_proof(
    proof: &QualityProof,
    goal_id: &str,
    round_idx: usize,
    candidate: &str,
    round: &Value,
    details: &Value,
) -> bool {
    proof.schema_version == QUALITY_PROOF_SCHEMA_VERSION
        && proof.goal_id == goal_id
        && proof.round_idx == round_idx
        && proof.evaluation_scope == "isolated_candidate"
        && !proof.operation_id.trim().is_empty()
        && proof.checked_candidate_commit == candidate
        && proof.source_candidate_commit == candidate
        && proof.state == "failed"
        && !proof.checked_at.trim().is_empty()
        && proof
            .results
            .iter()
            .any(|result| result.get("status").and_then(Value::as_str) == Some("failed"))
        && details.get("operation_id").and_then(Value::as_str) == Some(proof.operation_id.as_str())
        && details.get("candidate_commit").and_then(Value::as_str) == Some(candidate)
        && details
            .get("source_candidate_commit")
            .and_then(Value::as_str)
            == Some(candidate)
        && details.get("evaluation_scope").and_then(Value::as_str) == Some("isolated_candidate")
        && details.get("quality_proof_mode").and_then(Value::as_str) == Some("regenerated")
        && details.get("results").and_then(Value::as_array) == Some(&proof.results)
        && details
            .get("diagnostics")
            .and_then(Value::as_array)
            .is_some()
        && details
            .get("provider_attempts")
            .and_then(Value::as_array)
            .is_some()
        && round.get("quality_checked_at").and_then(Value::as_str)
            == Some(proof.checked_at.as_str())
}
