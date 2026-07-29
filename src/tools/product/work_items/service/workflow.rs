use super::*;
use serde_json::json;

impl FileWorkItemService {
    pub fn retry_goal_quality_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::RetryQa)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Qa)?;
        self.show_goal_summary(goal_id)
    }

    pub fn retry_goal_merge_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::RetryMerge)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::ReadyMerge)?;
        self.show_goal_summary(goal_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn queue_stale_candidate_recovery_summary(
        &self,
        goal_id: &str,
        round_idx: usize,
        candidate_commit: &str,
        recorded_base: &str,
        target_branch: &str,
        target_commit: &str,
        failure_message: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock()?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        validate_automated_goal_transition(&current.goal.status, &GoalStatus::Todo)?;
        if current.goal.status != GoalStatus::ReadyMerge {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from ready-merge to {} before stale-candidate recovery",
                current.goal.status.as_str()
            )));
        }

        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        for (field, expected) in [
            ("candidate_commit", candidate_commit),
            ("base_commit", recorded_base),
            ("target_branch", target_branch),
        ] {
            let recorded = object
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            if recorded != expected {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} {field} changed from {expected} to {recorded} before stale-candidate recovery"
                )));
            }
        }
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        if rounds.len() != round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} round changed from {} to {} before stale-candidate recovery",
                round_idx + 1,
                rounds.len()
            )));
        }

        let now = now_timestamp();
        let source_recovery = json!({
            "state": "superseded",
            "reason": "stale_candidate",
            "candidate_commit": candidate_commit,
            "recorded_base": recorded_base,
            "target_branch": target_branch,
            "target_commit": target_commit,
            "successor_round": round_idx + 2,
            "updated_at": now
        });
        let source_round = rounds
            .get_mut(round_idx)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!(
                    "round {} for Goal {goal_id} is not an object",
                    round_idx + 1
                ))
            })?;
        source_round.insert(
            "failure_category".to_string(),
            Value::String("stale_candidate".to_string()),
        );
        source_round.insert(
            "failure_message".to_string(),
            Value::String(failure_message.to_string()),
        );
        source_round.insert("failure_at".to_string(), Value::String(now.clone()));
        source_round.insert("workflow_recovery".to_string(), source_recovery);
        source_round.insert("updated".to_string(), Value::String(now.clone()));

        let prompt = format!(
            "Recover the completed implementation from stale candidate {candidate_commit} (round {}) onto the current {target_branch} target at {target_commit}. Preserve the prior round and candidate as audit evidence, reapply the intended changes on the current target, and rerun Governance and Quality before integration.",
            round_idx + 1
        );
        let mut successor = new_round_value("Refine", "Refine", &prompt);
        successor["workflow_recovery"] = json!({
            "state": "queued",
            "reason": "stale_candidate",
            "source_round": round_idx + 1,
            "candidate_commit": candidate_commit,
            "recorded_base": recorded_base,
            "target_branch": target_branch,
            "target_commit": target_commit,
            "queued_at": now
        });
        rounds.push(successor);
        object.insert(
            "status".to_string(),
            Value::String(GoalStatus::Todo.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn queue_missing_reconciled_candidate_recovery_summary(
        &self,
        goal_id: &str,
        round_idx: usize,
        recorded_reconciliation_state: &str,
        candidate_commit: &str,
        target_branch: &str,
        target_commit: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock()?;
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
        source_round.insert(
            "workflow_recovery".to_string(),
            json!({
                "state": "superseded",
                "reason": "reconciliation_candidate_absent",
                "recorded_reconciliation_state": recorded_reconciliation_state,
                "candidate_commit": candidate_commit,
                "target_branch": target_branch,
                "target_commit": target_commit,
                "successor_round": round_idx + 2,
                "updated_at": now
            }),
        );
        source_round.insert("updated".to_string(), Value::String(now.clone()));

        let prompt = format!(
            "Recover candidate {candidate_commit} from round {} because its recorded reconciliation state ({recorded_reconciliation_state}) no longer matches {target_branch} at {target_commit}. Preserve the prior round as audit evidence, reapply the intended changes on the current target, and rerun Governance and Quality before integration.",
            round_idx + 1
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
        rounds.push(successor);
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn submit_goal_for_merge_summary(
        &self,
        goal_id: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        if current.goal.status == GoalStatus::ReadyMerge {
            return Ok(current);
        }
        Err(RefineError::Conflict(
            "Ready Merge is workflow-owned; queue or retry the Goal through its current stage"
                .to_string(),
        ))
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

    pub(crate) fn fail_automated_goal_if_active(
        &self,
        goal_id: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock()?;
        let current = self.show_goal_summary(goal_id)?;
        if matches!(
            current.goal.status,
            GoalStatus::Cancelled | GoalStatus::Done
        ) {
            return Ok(current);
        }
        validate_automated_goal_transition(&current.goal.status, &GoalStatus::Failed)?;
        self.set_goal_status_unchecked_locked(goal_id, &GoalStatus::Failed)?;
        self.show_goal_summary(goal_id)
    }

    pub fn rollback_in_progress_goal_to_todo(
        &self,
        goal_id: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        if current.goal.status != GoalStatus::InProgress {
            return Err(RefineError::InvalidInput(format!(
                "Goal {goal_id} is not in-progress"
            )));
        }
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Todo)?;
        self.show_goal_summary(goal_id)
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
        let _goal_lock = self.acquire_goal_mutation_lock()?;
        let snapshot = self.projection_snapshot()?;
        let current = snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
        })?;
        self.transition_goal_status_from_projection(&current, target)?;

        let refreshed = self.projection_snapshot()?;
        refreshed.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} disappeared after transition"))
        })
    }

    /// Applies a status transition against a summary from a caller-owned
    /// coherent snapshot without loading or refreshing another projection.
    pub(crate) fn transition_goal_status_from_projection(
        &self,
        current: &GoalSummaryProjection,
        target: GoalStatus,
    ) -> RefineResult<()> {
        self.ensure_goal_owned(current)?;
        validate_manual_goal_transition(&current.goal.status, &target)?;

        let goal_path = self.refine_dir.join(&current.goal.json_path);
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
        if durable_status != current.goal.status
            || durable_updated != current.goal.updated
            || Some(durable_node_id.as_str()) != current.goal.node_id.as_deref()
        {
            return Err(RefineError::Conflict(format!(
                "Goal {} changed after the projection snapshot was read",
                current.goal.id
            )));
        }
        object.insert(
            "status".to_string(),
            Value::String(target.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));

        write_json_atomically(&goal_path, &value)?;
        Ok(())
    }

    pub fn cancel_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock()?;
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

    pub(crate) fn prepare_goal_cancellation_if_current(
        &self,
        goal_id: &str,
        expected: &GoalCancellationExpectation,
        target_status: GoalStatus,
    ) -> RefineResult<GoalCancellationTransaction> {
        let goal_lock = self.acquire_goal_mutation_lock()?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        if current.goal.status != expected.status
            || current.goal.round_count != expected.round_count
            || current.goal.updated != expected.updated
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} ownership fence changed before stop settlement (expected status {}, round {}, revision {}; observed status {}, round {}, revision {}); the Goal was not changed",
                expected.status.as_str(),
                expected.round_count,
                expected.updated,
                current.goal.status.as_str(),
                current.goal.round_count,
                current.goal.updated
            )));
        }
        if current.goal.status == GoalStatus::Done {
            return Err(RefineError::InvalidInput(format!(
                "done Goal {goal_id} cannot be settled to {} by process control",
                target_status.as_str()
            )));
        }
        let (goal_path, original) = self.read_goal_value_unchecked_locked(&current)?;
        let mut settled = original.clone();
        let object = settled.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "status".to_string(),
            Value::String(target_status.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        set_workflow_revision(&mut settled, workflow_revision(&original).saturating_add(1))?;
        Ok(GoalCancellationTransaction {
            service: self.clone(),
            _lock: goal_lock,
            goal_id: goal_id.to_string(),
            goal_path,
            original,
            settled,
            committed: false,
        })
    }

    pub(crate) fn prepare_goal_cancellation_replay(
        &self,
        goal_id: &str,
        original: &Value,
        settled: &Value,
        restored: Option<&Value>,
        superseded: Option<&Value>,
    ) -> RefineResult<GoalCancellationTransaction> {
        let goal_lock = self.acquire_goal_mutation_lock()?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, current_value) = self.read_goal_value_unchecked_locked(&current)?;
        let replay_original = if &current_value == original {
            original
        } else if restored.is_some_and(|restored| current_value == *restored) {
            restored.expect("restored Goal replay state was checked")
        } else if &current_value == settled {
            original
        } else if superseded.is_some_and(|superseded| current_value == *superseded) {
            superseded.expect("superseded Goal replay state was checked")
        } else {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed outside the interrupted cancellation settlement; replay did not overwrite the newer Goal state"
            )));
        };
        Ok(GoalCancellationTransaction {
            service: self.clone(),
            _lock: goal_lock,
            goal_id: goal_id.to_string(),
            goal_path,
            original: replay_original.clone(),
            settled: settled.clone(),
            committed: current_value == *settled,
        })
    }
}
