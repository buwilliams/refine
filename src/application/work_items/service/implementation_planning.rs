use sha2::{Digest, Sha256};

use crate::model::goal::ImplementationPlan;

use super::*;

impl FileWorkItemService {
    /// Atomically replaces planning evidence only while its complete workflow/Git/context
    /// binding is still current. Planning orchestration owns the phase machine; this method
    /// owns the durable Goal-record compare-and-swap boundary.
    pub(crate) fn replace_goal_round_implementation_plan(
        &self,
        goal_id: &str,
        round_idx: usize,
        expected: Option<&ImplementationPlan>,
        plan: &ImplementationPlan,
    ) -> RefineResult<GoalSummaryProjection> {
        if plan.binding.goal_id != goal_id || plan.binding.round_idx != round_idx {
            return Err(RefineError::InvalidInput(format!(
                "implementation plan binding does not identify Goal {goal_id} round {}",
                round_idx + 1
            )));
        }
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        self.replace_goal_round_implementation_plan_in_value(
            goal_id, round_idx, expected, plan, &mut value,
        )?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let now = now_timestamp();
        let round = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .and_then(|rounds| rounds.get_mut(round_idx))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!(
                    "round {} for Goal {goal_id} is not a JSON object",
                    round_idx + 1
                ))
            })?;
        round.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub(super) fn replace_goal_round_implementation_plan_in_value(
        &self,
        goal_id: &str,
        round_idx: usize,
        expected: Option<&ImplementationPlan>,
        plan: &ImplementationPlan,
        value: &mut Value,
    ) -> RefineResult<()> {
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} is not a JSON object"))
        })?;
        if !matches!(
            object.get("status").and_then(Value::as_str),
            Some("plan" | "implement" | "in-progress")
        ) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} left plan/implement before planning evidence was written"
            )));
        }
        for (key, expected_value) in [
            ("branch_name", plan.binding.implementation_branch.as_str()),
            ("target_branch", plan.binding.target_branch.as_str()),
            ("base_commit", plan.binding.base_commit.as_str()),
        ] {
            let observed = object.get(key).and_then(Value::as_str).unwrap_or("");
            if observed != expected_value {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} {key} changed from {expected_value:?} to {observed:?} during implementation planning"
                )));
            }
        }
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        if rounds.len() != round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} changed from round {} to round {} during implementation planning",
                round_idx + 1,
                rounds.len()
            )));
        }
        let round = rounds
            .get_mut(round_idx)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                RefineError::Serialization(format!(
                    "round {} for Goal {goal_id} is not a JSON object",
                    round_idx + 1
                ))
            })?;
        let context = round
            .get("agent_context")
            .filter(|value| !value.is_null())
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {goal_id} round {} lost its pinned agent context",
                    round_idx + 1
                ))
            })?;
        let context_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(context).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to encode pinned agent context: {error}"
                ))
            })?)
        );
        if context_digest != plan.binding.context_digest {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} pinned agent context changed during implementation planning"
            )));
        }
        let observed = round
            .get("implementation_plan")
            .filter(|value| !value.is_null())
            .map(|value| {
                crate::application::agent_io::structured_output::decode_persisted::<
                    ImplementationPlan,
                >(
                    value.clone(),
                    &format!(
                        "Goal {goal_id} round {} implementation planning evidence",
                        round_idx + 1
                    ),
                )
            })
            .transpose()?;
        match (expected, observed.as_ref()) {
            (None, None) => {}
            (Some(expected), Some(observed)) if observed == expected => {}
            (None, Some(_)) => {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} round {} already has implementation planning evidence",
                    round_idx + 1
                )));
            }
            (Some(_), None) => {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} round {} implementation planning evidence disappeared",
                    round_idx + 1
                )));
            }
            (Some(_), Some(_)) => {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} round {} implementation planning authority changed",
                    round_idx + 1
                )));
            }
        }
        round.insert(
            "implementation_plan".to_string(),
            serde_json::to_value(plan).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to encode Goal implementation planning evidence: {error}"
                ))
            })?,
        );
        Ok(())
    }
}
