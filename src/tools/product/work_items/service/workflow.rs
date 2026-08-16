use super::*;
use serde_json::json;

use crate::prompts::{PromptTemplate, render};

impl FileWorkItemService {
    pub fn retry_goal_quality_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::RetryQuality)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Quality)?;
        self.show_goal_summary(goal_id)
    }

    pub fn retry_goal_governance_summary(
        &self,
        goal_id: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::RetryGovernance)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Governance)?;
        self.show_goal_summary(goal_id)
    }

    pub(crate) fn queue_governance_recovery_summary(
        &self,
        goal_id: &str,
        round_idx: usize,
        retiring_authority: Option<WorkflowAttemptAuthority>,
        attempt: u32,
        analysis: &str,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        self.queue_automatic_recovery_summary(
            goal_id,
            round_idx,
            retiring_authority,
            attempt,
            &GoalStatus::Governance,
            "governance",
            analysis,
            prompt,
        )
    }

    pub(crate) fn queue_quality_recovery_summary(
        &self,
        goal_id: &str,
        round_idx: usize,
        retiring_authority: Option<WorkflowAttemptAuthority>,
        attempt: u32,
        analysis: &str,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        self.queue_automatic_recovery_summary(
            goal_id,
            round_idx,
            retiring_authority,
            attempt,
            &GoalStatus::Quality,
            "quality",
            analysis,
            prompt,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_automatic_recovery_summary(
        &self,
        goal_id: &str,
        round_idx: usize,
        retiring_authority: Option<WorkflowAttemptAuthority>,
        attempt: u32,
        source_status: &GoalStatus,
        kind: &str,
        analysis: &str,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        if current.goal.status != *source_status {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from {} to {} before automatic recovery",
                source_status.as_str(),
                current.goal.status.as_str()
            )));
        }
        validate_automated_goal_transition(&current.goal.status, &GoalStatus::Todo)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        if rounds.len() != round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} round changed before automatic {kind} recovery"
            )));
        }
        let reuse_inert = last_round_is_unstarted_recovery(rounds, retiring_authority);
        let now = now_timestamp();
        let source = rounds
            .get_mut(round_idx)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Serialization("source Round is not an object".to_string())
            })?;
        source.insert(format!("{kind}_recovery_analysis"), json!(analysis));
        source.insert(format!("{kind}_recovery_attempt"), json!(attempt));
        source.insert("updated".to_string(), json!(now.clone()));

        let mut successor = new_round_value("Refine", "Refine", prompt);
        successor["automatic_retry"] = json!({
            "kind": kind,
            "source_round": round_idx + 1,
            "attempt": attempt,
            "generated_at": now
        });
        append_or_reuse_recovery_round(rounds, successor, reuse_inert);
        object.insert("status".to_string(), json!(GoalStatus::Todo.as_str()));
        object.insert("updated".to_string(), json!(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn queue_missing_reconciled_candidate_recovery_summary(
        &self,
        goal_id: &str,
        round_idx: usize,
        retiring_authority: Option<WorkflowAttemptAuthority>,
        recorded_reconciliation_state: &str,
        candidate_commit: &str,
        target_branch: &str,
        target_commit: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        if current.goal.status != GoalStatus::Todo {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from todo to {} before reconciliation recovery",
                current.goal.status.as_str()
            )));
        }

        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        for (field, expected) in [
            ("candidate_commit", candidate_commit),
            ("target_branch", target_branch),
        ] {
            let recorded = object
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            if recorded != expected {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} {field} changed from {expected} to {recorded} before reconciliation recovery"
                )));
            }
        }
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        if rounds.len() != round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} round changed from {} to {} before reconciliation recovery",
                round_idx + 1,
                rounds.len()
            )));
        }
        let reuse_inert = last_round_is_unstarted_recovery(rounds, retiring_authority);

        let now = now_timestamp();
        let source_round = rounds
            .get_mut(round_idx)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!(
                    "round {} for Goal {goal_id} is not an object",
                    round_idx + 1
                ))
            })?;
        let current_reconciliation_state = source_round
            .get("workflow_reconciliation")
            .and_then(Value::as_object)
            .and_then(|evidence| evidence.get("state"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if current_reconciliation_state != recorded_reconciliation_state {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} reconciliation changed from {recorded_reconciliation_state} to {current_reconciliation_state} before recovery"
            )));
        }
        let failure_message = format!(
            "Recorded reconciliation state {recorded_reconciliation_state} no longer matches {target_branch}: candidate {candidate_commit} is absent from target {target_commit}"
        );
        source_round.insert(
            "failure_category".to_string(),
            Value::String("reconciliation_candidate_absent".to_string()),
        );
        source_round.insert(
            "failure_message".to_string(),
            Value::String(failure_message),
        );
        source_round.insert("failure_at".to_string(), Value::String(now.clone()));
        source_round.insert("workflow_attempt_authority".to_string(), Value::Null);
        source_round.insert(
            "workflow_recovery".to_string(),
            json!({
                "state": "superseded",
                "reason": "reconciliation_candidate_absent",
                "recorded_reconciliation_state": recorded_reconciliation_state,
                "candidate_commit": candidate_commit,
                "target_branch": target_branch,
                "target_commit": target_commit,
                "successor_round": successor_round_number(round_idx, reuse_inert),
                "updated_at": now
            }),
        );
        source_round.insert("updated".to_string(), Value::String(now.clone()));

        let source_round = (round_idx + 1).to_string();
        let prompt = render(
            PromptTemplate::GoalWorkflowRecoverReconciliation,
            &[
                ("candidate_commit", candidate_commit),
                ("source_round", &source_round),
                ("reconciliation_state", recorded_reconciliation_state),
                ("target_branch", target_branch),
                ("target_commit", target_commit),
            ],
        );
        let mut successor = new_round_value("Refine", "Refine", &prompt);
        successor["workflow_recovery"] = json!({
            "state": "queued",
            "reason": "reconciliation_candidate_absent",
            "source_round": round_idx + 1,
            "recorded_reconciliation_state": recorded_reconciliation_state,
            "candidate_commit": candidate_commit,
            "target_branch": target_branch,
            "target_commit": target_commit,
            "queued_at": now
        });
        append_or_reuse_recovery_round(rounds, successor, reuse_inert);
        // The recovery Round is only runnable from todo; restate it so a Goal
        // whose status drifted repairs itself instead of stalling.
        object.insert("status".to_string(), json!(GoalStatus::Todo.as_str()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn undo_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        if current.goal.status == GoalStatus::Review {
            return Err(RefineError::InvalidInput(
                "submit a new round to decline review and preserve the integration history"
                    .to_string(),
            ));
        }
        validate_goal_operation(&current.goal.status, &GoalOperation::Undo)?;
        let target = match current.goal.status {
            GoalStatus::Done => GoalStatus::Review,
            GoalStatus::Cancelled => GoalStatus::Todo,
            _ => {
                return Err(RefineError::InvalidInput(
                    "Goal undo is only available from done or cancelled; submit a new round to decline review"
                        .to_string(),
                ));
            }
        };
        self.set_goal_status_unchecked(goal_id, &target)?;
        self.show_goal_summary(goal_id)
    }

    pub fn start_goal_workflow(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        match current.goal.status {
            GoalStatus::Backlog => self.transition_goal_status(goal_id, GoalStatus::Todo),
            GoalStatus::Todo => Ok(current),
            _ => Err(RefineError::InvalidInput(format!(
                "Goal {goal_id} can only be queued from backlog or todo"
            ))),
        }
    }

    pub fn advance_automated_goal_status(
        &self,
        goal_id: &str,
        target: GoalStatus,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_automated_goal_transition(&current.goal.status, &target)?;
        self.set_goal_status_unchecked(goal_id, &target)?;
        self.show_goal_summary(goal_id)
    }

    pub(crate) fn authored_goal_commitment(
        &self,
        goal_id: &str,
    ) -> RefineResult<(usize, u64, String)> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (_goal_lock, _goal_path, value) = self.read_goal_value(goal_id)?;
        let rounds = value
            .get("rounds")
            .and_then(Value::as_array)
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no Round array")))?;
        let round_idx = rounds.len().checked_sub(1).ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no authored Round and is not workflow-eligible"
            ))
        })?;
        let request = rounds[round_idx]
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|request| !request.is_empty())
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} Round {} has an empty request and is not workflow-eligible",
                    round_idx + 1
                ))
            })?
            .to_string();
        Ok((round_idx, workflow_revision(&value), request))
    }

    pub fn set_goal_branch_name(
        &self,
        goal_id: &str,
        branch_name: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let branch_name = branch_name.trim();
        if branch_name.is_empty() {
            return Err(RefineError::InvalidInput(
                "branch name is required".to_string(),
            ));
        }
        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "branch_name".to_string(),
            Value::String(branch_name.to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn workflow_enforcement_summary(&self) -> RefineResult<WorkflowEnforcementSummary> {
        let snapshot = self.projection_snapshot()?;
        let automated = snapshot
            .goals
            .values()
            .filter(|goal| is_automated_status(&goal.goal.status))
            .map(|goal| goal.goal.id.clone())
            .collect();
        Ok(WorkflowEnforcementSummary {
            ok: true,
            checked: snapshot.goals.len(),
            automated,
        })
    }

    pub fn transition_goal_status(
        &self,
        goal_id: &str,
        target: GoalStatus,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let snapshot = self.projection_snapshot()?;
        let current = snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
        })?;
        self.transition_goal_status_from_projection(&current.goal, target)?;

        let refreshed = self.projection_snapshot()?;
        refreshed.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} disappeared after transition"))
        })
    }

    /// Applies a status transition against a summary from a caller-owned
    /// coherent snapshot without loading or refreshing another projection.
    pub(crate) fn transition_goal_status_from_projection(
        &self,
        current: &GoalIndexProjection,
        target: GoalStatus,
    ) -> RefineResult<()> {
        self.ensure_goal_index_owned(current)?;
        validate_manual_goal_transition(&current.status, &target)?;

        let goal_path = self.refine_dir.join(&current.json_path);
        let bytes = fs::read(&goal_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let durable_status = object
            .get("status")
            .and_then(Value::as_str)
            .and_then(GoalStatus::parse_wire)
            .unwrap_or(GoalStatus::Backlog);
        let durable_updated = object
            .get("updated")
            .and_then(Value::as_str)
            .or_else(|| object.get("created").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string());
        let durable_node_id = object
            .get("node_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                object
                    .get("instance_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            })
            .map(str::to_string)
            .unwrap_or_else(|| "default".to_string());
        if durable_status != current.status
            || durable_updated != current.updated
            || Some(durable_node_id.as_str()) != current.node_id.as_deref()
        {
            return Err(RefineError::Conflict(format!(
                "Goal {} changed after the projection snapshot was read",
                current.id
            )));
        }
        object.insert(
            "status".to_string(),
            Value::String(target.as_str().to_string()),
        );
        if target != GoalStatus::Failed {
            clear_latest_round_failure(object);
        }
        if !is_automated_status(&target) {
            clear_latest_round_workflow_attempt(object);
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));

        write_json_atomically(&goal_path, &value)?;
        Ok(())
    }

    pub fn cancel_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        if current.goal.status == GoalStatus::Cancelled {
            return Ok(current);
        }
        if current.goal.status == GoalStatus::Done {
            return Err(RefineError::InvalidInput(
                "done Goals cannot be cancelled".to_string(),
            ));
        }
        self.set_goal_status_unchecked_locked(goal_id, &GoalStatus::Cancelled)?;
        self.show_goal_summary(goal_id)
    }

    pub(crate) fn fail_goal_after_process_stop_if_current(
        &self,
        goal_id: &str,
        expected: &GoalCancellationExpectation,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        if current.goal.status == GoalStatus::Cancelled {
            return Ok(current);
        }
        let node = current.goal.node_id.as_deref().unwrap_or("default");
        if current.goal.status != expected.status
            || current.goal.round_count != expected.round_count
            || current.goal.updated != expected.updated
            || node != expected.node_id
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed after process Stop preflight; its newer status, Round, or node ownership was preserved"
            )));
        }
        if current.goal.status == GoalStatus::Done {
            return Err(RefineError::InvalidInput(format!(
                "done Goal {goal_id} cannot be failed by process Stop"
            )));
        }
        self.set_goal_status_unchecked_locked(goal_id, &GoalStatus::Failed)?;
        self.show_goal_summary(goal_id)
    }
}
