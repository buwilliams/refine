use super::*;
use serde_json::json;

impl FileWorkItemService {
    pub(crate) fn settle_event_transition(
        &self,
        goal_id: &str,
        transition_id: &str,
        failed: bool,
    ) -> RefineResult<()> {
        let current = self.show_goal_summary(goal_id)?;
        let (_lock, path, mut value) = self.read_goal_value_unchecked(&current)?;
        let pending = value
            .get("pending_event_transition")
            .cloned()
            .ok_or_else(|| RefineError::Conflict("Event transition was superseded".into()))?;
        if pending["id"].as_str() != Some(transition_id) || pending["state"] != "pending" {
            return Err(RefineError::Conflict(
                "Event transition was superseded".into(),
            ));
        }
        let stale = pending["revision"] != value["workflow_revision"];
        if failed || stale {
            value["pending_event_transition"]["state"] =
                json!(if stale { "superseded" } else { "failed" });
            value
                .as_object_mut()
                .unwrap()
                .entry("event_transition_history")
                .or_insert(json!([]))
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "id":pending["id"], "from":pending["from"], "to":pending["to"],
                    "state":if stale {"superseded"}else{"failed"}, "at":now_timestamp(),
                }));
            if failed
                && !stale
                && !crate::application::events::outcomes::prepare_error(
                    &self.refine_dir,
                    &mut value,
                    "event_transition",
                    "A required workflow event failed",
                )?
                && !matches!(
                    current.goal.status,
                    GoalStatus::Done | GoalStatus::Cancelled
                )
            {
                value["status"] = json!("failed");
            }
            write_json_atomically(&path, &value)?;
            return Ok(());
        }
        let to = pending["to"]
            .as_str()
            .ok_or_else(|| RefineError::InvalidInput("missing Event destination".into()))?;
        crate::application::events::transitions::approve_exit(&self.refine_dir, &value, to)?;
        let mut requested = pending["requested"].clone();
        requested["workflow_revision"] = value["workflow_revision"].clone();
        write_json_atomically(&path, &requested)
    }
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
