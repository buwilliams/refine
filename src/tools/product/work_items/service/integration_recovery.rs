use super::*;
use serde_json::json;

use super::workflow_attempts::{goal_status, require_current_attempt};
use crate::prompts::{PromptTemplate, render};

impl FileWorkItemService {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_candidate_refresh(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        node_id: &str,
        branch: &str,
        worktree: &str,
        original_base: &str,
        original_candidate: &str,
        replacement_base: &str,
        replacement_candidate: &str,
        conflict_resolution: Option<Value>,
    ) -> RefineResult<Value> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} is not a JSON object"))
        })?;
        if goal_status(object) != GoalStatus::Governance
            || object
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or("default")
                != node_id
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} no longer authorizes candidate refresh on node {node_id}"
            )));
        }
        require_current_attempt(goal_id, object, authority)?;
        for (field, expected) in [
            ("branch_name", branch),
            ("base_commit", original_base),
            ("candidate_commit", original_candidate),
        ] {
            if object.get(field).and_then(Value::as_str) != Some(expected) {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} {field} changed before candidate refresh persistence"
                )));
            }
        }
        let now = now_timestamp();
        let round = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .and_then(|rounds| rounds.get_mut(authority.round_idx))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} has no authoritative Round for candidate refresh"
                ))
            })?;
        let mut prior_gates = Map::new();
        for key in [
            "quality_state",
            "quality_message",
            "quality_details",
            "quality_checked_at",
            "quality_candidate_commit",
            "rule_state",
            "meta_rule_state",
            "product_state",
            "constitution_state",
            "governance_message",
            "governance_details",
            "governance_checked_at",
            "governance_candidate_commit",
        ] {
            prior_gates.insert(
                key.to_string(),
                round.get(key).cloned().unwrap_or(Value::Null),
            );
            round.insert(key.to_string(), Value::Null);
        }
        let mut evidence = json!({
            "state": "refreshed",
            "round_idx": authority.round_idx,
            "workflow_revision": authority.workflow_revision,
            "node_id": node_id,
            "branch": branch,
            "worktree": worktree,
            "original_base_commit": original_base,
            "original_candidate_commit": original_candidate,
            "replacement_base_commit": replacement_base,
            "replacement_candidate_commit": replacement_candidate,
            "previous_gate_evidence": prior_gates,
            "refreshed_at": now
        });
        // The resolution note travels inside the refresh evidence, the same
        // way the recovery path retains `rebase.conflicts`: workflow state is
        // the single source of truth for how the conflict was resolved.
        if let Some(resolution) = conflict_resolution {
            evidence["conflict_resolution"] = resolution;
        }
        round.insert("workflow_candidate_refresh".to_string(), evidence.clone());
        round.insert("updated".to_string(), json!(now));
        object.insert("base_commit".to_string(), json!(replacement_base));
        object.insert("candidate_commit".to_string(), json!(replacement_candidate));
        object.insert("updated".to_string(), json!(now));
        write_json_atomically(&goal_path, &value)?;
        Ok(evidence)
    }

    pub(crate) fn queue_integration_recovery_summary(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        node_id: &str,
        reason: &str,
        retained_evidence: Value,
        max_automatic_round_retries: u32,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} is not a JSON object"))
        })?;
        if goal_status(object) != GoalStatus::Governance
            || object
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or("default")
                != node_id
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} no longer authorizes integration recovery on node {node_id}"
            )));
        }
        require_current_attempt(goal_id, object, authority)?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no Rounds")))?;
        if rounds.len() != authority.round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} Round changed before integration recovery"
            )));
        }
        // `require_current_attempt` above proved the trailing Round carries
        // this caller's claim, so the claim alone must not disqualify reuse.
        let reuse_inert = last_round_is_unstarted_recovery(rounds, Some(authority));
        let current_attempt = rounds[authority.round_idx]
            .get("automatic_retry")
            .and_then(|retry| retry.get("attempt"))
            .and_then(Value::as_u64)
            .and_then(|attempt| u32::try_from(attempt).ok())
            .unwrap_or(0);
        let exhausted = current_attempt >= max_automatic_round_retries;
        let next_attempt = current_attempt.saturating_add(1);
        let now = now_timestamp();
        let recovery = json!({
            "state": if exhausted { "exhausted" } else { "queued" },
            "kind": "integration",
            "reason": reason,
            "source_round": authority.round_idx + 1,
            "successor_round": if exhausted {
                Value::Null
            } else {
                json!(successor_round_number(authority.round_idx, reuse_inert))
            },
            "attempt": if exhausted { current_attempt } else { next_attempt },
            "max_automatic_round_retries": max_automatic_round_retries,
            "workflow_revision": authority.workflow_revision,
            "node_id": node_id,
            "retained_evidence": retained_evidence,
            "recorded_at": now
        });
        let source = rounds[authority.round_idx].as_object_mut().ok_or_else(|| {
            RefineError::Serialization("source Round is not an object".to_string())
        })?;
        source.insert("workflow_recovery".to_string(), recovery.clone());
        source.insert("workflow_attempt_authority".to_string(), Value::Null);
        source.insert("updated".to_string(), json!(&now));

        if exhausted {
            source.insert(
                "failure_category".to_string(),
                json!("integration_retry_exhausted"),
            );
            source.insert(
                "failure_message".to_string(),
                json!(format!(
                    "Integration recovery remained necessary after {max_automatic_round_retries} automatic recovery Rounds: {reason}"
                )),
            );
            source.insert("failure_at".to_string(), json!(&now));
            object.insert("status".to_string(), json!(GoalStatus::Failed.as_str()));
            object.insert("updated".to_string(), json!(&now));
            write_json_atomically(&goal_path, &value)?;
            return self.show_goal_summary(goal_id);
        }

        let encoded_evidence = serde_json::to_string_pretty(&recovery).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode integration recovery evidence: {error}"
            ))
        })?;
        let prompt = render(
            PromptTemplate::GoalWorkflowRecoverIntegration,
            &[("reason", reason), ("evidence", &encoded_evidence)],
        );
        let mut successor = new_round_value("Refine", "Refine", &prompt);
        successor["workflow_recovery"] = recovery;
        successor["automatic_retry"] = json!({
            "kind": "integration",
            "source_round": authority.round_idx + 1,
            "attempt": next_attempt,
            "generated_at": now
        });
        append_or_reuse_recovery_round(rounds, successor, reuse_inert);
        object.insert("status".to_string(), json!(GoalStatus::Todo.as_str()));
        object.insert("updated".to_string(), json!(&now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }
}
