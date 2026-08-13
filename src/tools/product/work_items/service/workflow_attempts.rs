use super::*;
use serde_json::json;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowAttemptAuthority {
    pub(crate) round_idx: usize,
    pub(crate) workflow_revision: u64,
}

impl FileWorkItemService {
    pub(crate) fn claim_workflow_attempt(
        &self,
        goal_id: &str,
        expected_status: GoalStatus,
        expected_round_idx: usize,
        expected_revision: u64,
        expected_request: &str,
    ) -> RefineResult<WorkflowAttemptAuthority> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let observed_revision = workflow_revision(&value);
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no Round array")))?;
        let observed_request = rounds
            .get(expected_round_idx)
            .and_then(|round| round.get("prompt"))
            .and_then(Value::as_str);
        if observed_status != expected_status
            || !matches!(
                observed_status,
                GoalStatus::Todo
                    | GoalStatus::Plan
                    | GoalStatus::Implement
                    | GoalStatus::Quality
                    | GoalStatus::Governance
            )
            || rounds.len() != expected_round_idx + 1
            || observed_revision != expected_revision
            || observed_request != Some(expected_request)
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed before workflow attempt claim (expected {} at Round {} revision {}; observed {} at {} Rounds revision {})",
                expected_status.as_str(),
                expected_round_idx + 1,
                expected_revision,
                observed_status.as_str(),
                rounds.len(),
                observed_revision
            )));
        }

        let authority = WorkflowAttemptAuthority {
            round_idx: expected_round_idx,
            workflow_revision: expected_revision,
        };
        let now = now_timestamp();
        let round = rounds
            .get_mut(expected_round_idx)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!(
                    "round {} for Goal {goal_id} is not a JSON object",
                    expected_round_idx + 1
                ))
            })?;
        round.insert(
            "workflow_attempt_authority".to_string(),
            json!({
                "round_idx": authority.round_idx,
                "workflow_revision": authority.workflow_revision,
                "claimed_at": now
            }),
        );
        round.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        Ok(authority)
    }

    pub(crate) fn advance_claimed_goal_status(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        from: GoalStatus,
        to: GoalStatus,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        if observed_status != from && observed_status != to {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from expected {} to {} before workflow transition to {}",
                from.as_str(),
                observed_status.as_str(),
                to.as_str()
            )));
        }
        require_current_attempt(goal_id, object, authority)?;
        if observed_status == to {
            return self.show_goal_summary(goal_id);
        }
        validate_automated_goal_transition(&observed_status, &to)?;
        object.insert("status".to_string(), Value::String(to.as_str().to_string()));
        if !is_automated_status(&to) {
            clear_latest_round_workflow_attempt(object);
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub(crate) fn settle_workflow_attempt_failure(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        failure_category: &str,
        failure_message: &str,
        failure_at: &str,
    ) -> RefineResult<bool> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        if !matches!(
            observed_status,
            GoalStatus::Todo
                | GoalStatus::Plan
                | GoalStatus::Implement
                | GoalStatus::Quality
                | GoalStatus::Governance
        ) || require_current_attempt(goal_id, object, authority).is_err()
        {
            return Ok(false);
        }

        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no Round array")))?;
        let round = rounds
            .get_mut(authority.round_idx)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!(
                    "round {} for Goal {goal_id} is not a JSON object",
                    authority.round_idx + 1
                ))
            })?;
        round.insert(
            "failure_category".to_string(),
            Value::String(failure_category.to_string()),
        );
        round.insert(
            "failure_message".to_string(),
            Value::String(failure_message.to_string()),
        );
        round.insert(
            "failure_at".to_string(),
            Value::String(failure_at.to_string()),
        );
        round.insert("updated".to_string(), Value::String(failure_at.to_string()));
        object.insert(
            "status".to_string(),
            Value::String(GoalStatus::Failed.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(failure_at.to_string()));
        write_json_atomically(&goal_path, &value)?;
        Ok(true)
    }
}

fn goal_status(object: &Map<String, Value>) -> GoalStatus {
    object
        .get("status")
        .and_then(Value::as_str)
        .and_then(GoalStatus::parse_wire)
        .unwrap_or(GoalStatus::Backlog)
}

fn require_current_attempt(
    goal_id: &str,
    object: &Map<String, Value>,
    authority: WorkflowAttemptAuthority,
) -> RefineResult<()> {
    let rounds = object
        .get("rounds")
        .and_then(Value::as_array)
        .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no Round array")))?;
    let current = rounds
        .get(authority.round_idx)
        .and_then(|round| round.get("workflow_attempt_authority"));
    let current_round_idx = current
        .and_then(|attempt| attempt.get("round_idx"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let current_revision = current
        .and_then(|attempt| attempt.get("workflow_revision"))
        .and_then(Value::as_u64);
    if rounds.len() != authority.round_idx + 1
        || current_round_idx != Some(authority.round_idx)
        || current_revision != Some(authority.workflow_revision)
    {
        return Err(RefineError::Conflict(format!(
            "Goal {goal_id} workflow attempt for Round {} revision {} was superseded",
            authority.round_idx + 1,
            authority.workflow_revision
        )));
    }
    Ok(())
}
