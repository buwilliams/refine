use super::*;
use serde_json::json;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowStepAuthority {
    pub(crate) round_idx: usize,
    pub(crate) workflow_revision: u64,
    pub(crate) generation: u64,
}

impl FileWorkItemService {
    pub(crate) fn bind_workflow_occurrence(
        &mut self,
        goal_id: &str,
        authority: WorkflowStepAuthority,
    ) {
        self.execution_occurrence = Some((goal_id.to_string(), authority));
    }
    pub(crate) fn verify_workflow_attempt(
        &self,
        goal_id: &str,
        authority: WorkflowStepAuthority,
        expected_status: GoalStatus,
        expected_node_id: &str,
    ) -> RefineResult<()> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (_, value) = self
            .read_goal_value_unchecked_locked(&current)
            .map_err(|error| match error {
                RefineError::Conflict(message) => RefineError::Conflict(format!(
                    "Goal {goal_id} no longer authorizes {} work: {message}",
                    expected_status.as_str()
                )),
                other => other,
            })?;
        let object = value.as_object().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} is not a JSON object"))
        })?;
        let status = goal_status(object);
        let node = object
            .get("node_id")
            .and_then(Value::as_str)
            .unwrap_or("default");
        if status != expected_status || node != expected_node_id {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} no longer authorizes {} work on node {expected_node_id}",
                expected_status.as_str()
            )));
        }
        require_current_step(goal_id, object, authority)
    }

    pub(crate) fn claim_workflow_attempt(
        &self,
        goal_id: &str,
        expected_status: GoalStatus,
        expected_round_idx: usize,
        expected_revision: u64,
        expected_request: &str,
    ) -> RefineResult<WorkflowStepAuthority> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let observed_revision = workflow_revision(&value);
        let generation = value["event_generation"].as_u64().unwrap_or(0);
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        for key in [
            "pending_workflow_outcome",
            "pending_event_transition",
            "workflow_integration_control",
        ] {
            if object
                .get(key)
                .is_some_and(|pending| pending["state"] == "pending")
            {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} has pending workflow work: {key}"
                )));
            }
        }
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

        let authority = WorkflowStepAuthority {
            round_idx: expected_round_idx,
            workflow_revision: expected_revision,
            generation,
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
        if let Some(previous) = round
            .get("workflow_attempt_authority")
            .filter(|v| !v.is_null())
            .cloned()
        {
            round
                .entry("workflow_claim_history")
                .or_insert(json!([]))
                .as_array_mut()
                .ok_or_else(|| RefineError::Serialization("Invalid claim history".into()))?
                .push(previous);
        }
        round.insert(
            "workflow_attempt_authority".to_string(),
            json!({
                "round_idx": authority.round_idx,
                "workflow_revision": authority.workflow_revision,
                "generation": authority.generation,
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
        authority: WorkflowStepAuthority,
        from: GoalStatus,
        to: GoalStatus,
    ) -> RefineResult<WorkflowStepAuthority> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        if observed_status != from {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from expected {} to {} before workflow transition to {}",
                from.as_str(),
                observed_status.as_str(),
                to.as_str()
            )));
        }
        require_current_step(goal_id, object, authority)?;
        validate_automated_goal_transition(&observed_status, &to)?;
        object.insert("status".to_string(), Value::String(to.as_str().to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        Ok(WorkflowStepAuthority {
            generation: authority.generation.saturating_add(1),
            ..authority
        })
    }

    pub(crate) fn settle_workflow_attempt_failure(
        &self,
        goal_id: &str,
        authority: WorkflowStepAuthority,
        failure_category: &str,
        failure_message: &str,
        failure_at: &str,
    ) -> RefineResult<crate::application::work_items::FailureSettlement> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        use crate::application::work_items::FailureSettlement;
        match self.ensure_goal_owned(&current) {
            Ok(()) => {}
            Err(RefineError::Conflict(message)) if message.contains("is owned by node") => {
                return Ok(FailureSettlement::SupersededAttempt);
            }
            Err(error) => return Err(error),
        }
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let observed_status = goal_status(object);
        if object
            .get("pending_event_transition")
            .is_some_and(|pending| pending["state"] == "pending")
        {
            return Ok(FailureSettlement::SupersededAttempt);
        }
        let recovery = object
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.get(authority.round_idx))
            .and_then(|round| round.get("workflow_recovery"));
        if recovery.is_some_and(|recovery| {
            recovery.get("workflow_revision").and_then(Value::as_u64)
                == Some(authority.workflow_revision)
                && recovery.get("source_round").and_then(Value::as_u64)
                    == Some(authority.round_idx as u64 + 1)
                && matches!(
                    recovery.get("state").and_then(Value::as_str),
                    Some("queued" | "exhausted")
                )
        }) {
            return Ok(FailureSettlement::ExistingVerifiedOutcome);
        }

        if object
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|r| r.last())
            .is_some_and(|r| {
                r["workflow_failure_occurrence"]["generation"].as_u64()
                    == Some(authority.generation)
                    && r["workflow_failure_occurrence"]["round_idx"].as_u64()
                        == Some(authority.round_idx as u64)
            })
            && observed_status == GoalStatus::Failed
            && object
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|rounds| rounds.get(authority.round_idx))
                .is_some_and(|round| {
                    round
                        .get("failure_message")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                })
        {
            return Ok(FailureSettlement::ExistingVerifiedOutcome);
        }
        if !matches!(
            observed_status,
            GoalStatus::Todo
                | GoalStatus::Plan
                | GoalStatus::Implement
                | GoalStatus::Quality
                | GoalStatus::Governance
        ) || require_current_step(goal_id, object, authority).is_err()
        {
            return Ok(FailureSettlement::SupersededAttempt);
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
            "workflow_failure_occurrence".into(),
            json!({"generation": authority.generation, "round_idx": authority.round_idx}),
        );
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
        object.insert("updated".to_string(), Value::String(failure_at.to_string()));
        if !crate::application::events::outcomes::prepare_error(
            &self.refine_dir,
            &mut value,
            failure_category,
            failure_message,
        )? {
            value["status"] = json!("failed");
        }
        write_json_atomically(&goal_path, &value)?;
        Ok(FailureSettlement::AuthoritativeFailure)
    }
}

pub(super) fn goal_status(object: &Map<String, Value>) -> GoalStatus {
    object
        .get("status")
        .and_then(Value::as_str)
        .and_then(GoalStatus::parse_wire)
        .unwrap_or(GoalStatus::Backlog)
}

pub(super) fn require_current_step(
    goal_id: &str,
    object: &Map<String, Value>,
    authority: WorkflowStepAuthority,
) -> RefineResult<()> {
    let rounds = object
        .get("rounds")
        .and_then(Value::as_array)
        .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no Round array")))?;
    let generation = object
        .get("event_generation")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if rounds.len() != authority.round_idx + 1
        || generation != authority.generation
        || object
            .get("pending_workflow_outcome")
            .is_some_and(|p| p["state"] == "pending")
        || object
            .get("pending_event_transition")
            .is_some_and(|p| p["state"] == "pending")
    {
        return Err(RefineError::Conflict(format!(
            "Goal {goal_id} workflow attempt for Round {} revision {} was superseded",
            authority.round_idx + 1,
            authority.workflow_revision
        )));
    }
    Ok(())
}

/// One workflow-owned occurrence clock. Record revisions also change for evidence and
/// metadata; claims do not create an occurrence. This runs at the Goal write boundary.
pub(super) fn prepare_occurrence(current: Option<&Value>, next: &mut Value) {
    let Some(current) = current else {
        if next.get("event_generation").is_none() {
            next["event_generation"] = json!(1);
        }
        return;
    };
    let round = |v: &Value| v["rounds"].as_array().map_or(0, Vec::len);
    let changed = current["status"] != next["status"]
        || current["node_id"] != next["node_id"]
        || round(current) != round(next)
        || current["rounds"]
            .as_array()
            .and_then(|r| r.last())
            .map(|r| &r["prompt"])
            != next["rounds"]
                .as_array()
                .and_then(|r| r.last())
                .map(|r| &r["prompt"])
        || current["workflow_controls"].as_array().map_or(0, Vec::len)
            != next["workflow_controls"].as_array().map_or(0, Vec::len);
    let generation = current["event_generation"].as_u64().unwrap_or(0);
    if !changed {
        next["event_generation"] = json!(generation);
    }
    // prepare_write may already have emitted this transition's occurrence.
    if changed && next["event_generation"].as_u64().unwrap_or(0) <= generation {
        next["event_generation"] = json!(generation.saturating_add(1));
    }
    if changed {
        let selected = next["event_generation"].clone();
        if next["pending_workflow_outcome"]["state"] == "pending"
            && next["pending_workflow_outcome"]["occurrence"]["generation"] != selected
        {
            next["pending_workflow_outcome"]["state"] = json!("superseded");
        }
        if next["pending_event_transition"]["state"] == "pending"
            && next["pending_event_transition"]["id"] == current["pending_event_transition"]["id"]
        {
            next["pending_event_transition"]["state"] = json!("superseded");
        }

        if !next["workflow_events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|event| event["generation"] == selected)
        {
            let occurrence = json!({"generation":selected, "from":current["status"], "to":next["status"], "node_id":next["node_id"], "round_idx":round(next).checked_sub(1), "at":chrono::Utc::now().to_rfc3339()});
            next.as_object_mut()
                .unwrap()
                .entry("workflow_events")
                .or_insert(json!([]))
                .as_array_mut()
                .unwrap()
                .push(occurrence);
        }
        // Retain the existing record revision on occurrence receipts as well as
        // the occurrence generation. It lets recovery order legacy failure
        // evidence that has no generation without relying on wall-clock time.
        if let Some(events) = next["workflow_events"].as_array_mut() {
            for event in events.iter_mut().filter(|e| e["generation"] == selected) {
                event["workflow_revision"] = json!(workflow_revision(current).saturating_add(1));
            }
        }
    }
}

impl WorkflowStepAuthority {
    pub(crate) fn from_goal(goal: &Value) -> RefineResult<Self> {
        let round_idx = goal["rounds"]
            .as_array()
            .and_then(|r| r.len().checked_sub(1))
            .ok_or_else(|| RefineError::Conflict("No current workflow Round".into()))?;
        Ok(Self {
            round_idx,
            workflow_revision: workflow_revision(goal),
            generation: goal["event_generation"].as_u64().unwrap_or(0),
        })
    }
}
