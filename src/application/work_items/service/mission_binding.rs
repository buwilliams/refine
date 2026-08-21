//! Mission membership on Goals: the Goal-owned binding, the pinned Mission
//! context of a GoalRound, and the settled Mission contribution.
//!
//! Mission membership is authoritative on Goal; Mission never stores a
//! competing mutable member list. The capsule is pinned onto the GoalRound
//! before Goal execution begins and never changes afterwards; the
//! contribution settles only when the GoalRound holds valid evidence.
//! Reconciliation — not the Goal — decides what any of this means for
//! canonical Mission context.

use serde_json::{Value, json};

use crate::application::projects::projection::GoalSummaryProjection;
use crate::error::{RefineError, RefineResult};
use crate::model::mission::{GoalContribution, GoalRoundMissionContext, MissionGoalBinding};
use crate::model::workflow::GoalStatus;

use super::FileWorkItemService;

impl FileWorkItemService {
    /// Bind one Goal to a Mission by stable key. `(mission_id,
    /// mission_goal_key)` is unique across Goals; a competing binding fails
    /// closed rather than being overwritten.
    pub fn bind_goal_to_mission(
        &self,
        goal_id: &str,
        mission_id: &str,
        mission_goal_key: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let binding = MissionGoalBinding {
            mission_id: mission_id.trim().to_uppercase(),
            mission_goal_key: mission_goal_key.trim().to_string(),
        };
        if binding.mission_id.len() < 3 || binding.mission_goal_key.is_empty() {
            return Err(RefineError::InvalidInput(
                "a Mission binding requires a Mission id and a mission goal key".to_string(),
            ));
        }
        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let existing = object.get("mission");
        if let Some(existing) = existing.filter(|existing| !existing.is_null()) {
            let parsed: Option<MissionGoalBinding> = serde_json::from_value(existing.clone()).ok();
            if parsed.as_ref() != Some(&binding) {
                return Err(RefineError::Conflict(format!(
                    "Goal {} is already bound to a different Mission membership",
                    goal_id
                )));
            }
            return self.show_goal_summary(goal_id);
        }
        object.insert(
            "mission".to_string(),
            serde_json::to_value(&binding).map_err(|error| {
                RefineError::Serialization(format!("failed to encode Mission binding: {error}"))
            })?,
        );
        object.insert(
            "updated".to_string(),
            Value::String(super::goal_filters::now_timestamp()),
        );
        super::record_persistence::write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    /// Pin the typed Mission context binding and the compiled capsule onto
    /// the Goal's latest Round. The capsule becomes the `mission` member of
    /// the pinned agent context; once the workflow pins that context the
    /// capsule is frozen for the Round.
    pub fn pin_goal_mission_context(
        &self,
        goal_id: &str,
        context: &GoalRoundMissionContext,
        capsule: &Value,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        if !matches!(current.goal.status, GoalStatus::Backlog | GoalStatus::Todo) {
            return Err(RefineError::Conflict(format!(
                "Goal {} is {}; Mission context pins only while the Goal is Backlog or Todo",
                goal_id,
                current.goal.status.as_str()
            )));
        }
        let round_idx = current
            .goal
            .round_count
            .checked_sub(1)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        self.update_goal_round_evaluation_summary(
            goal_id,
            round_idx,
            &json!({
                "mission_context": serde_json::to_value(context).map_err(|error| RefineError::Serialization(format!("failed to encode Mission context: {error}")))?,
                "mission_capsule": capsule.clone(),
            }),
        )
    }

    /// Settle one Mission contribution onto the Goal's latest Round. The
    /// GoalRound must have reached Review with valid Quality and Governance
    /// evidence; a contribution never settles onto executing or failed work.
    /// The contribution is advisory evidence: only reconciliation may select
    /// it into a MissionSnapshot.
    pub fn settle_goal_mission_contribution(
        &self,
        goal_id: &str,
        contribution: GoalContribution,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let round_idx = current
            .goal
            .round_count
            .checked_sub(1)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let (_goal_lock, _goal_path, value) = self.read_goal_value_unchecked(&current)?;
        let rounds = value
            .get("rounds")
            .and_then(Value::as_array)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let round = rounds.get(round_idx).ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} has no round {}", round_idx + 1))
        })?;
        if current.goal.status != GoalStatus::Review {
            return Err(RefineError::Conflict(format!(
                "Goal {} is {}; contributions settle only at Review",
                goal_id,
                current.goal.status.as_str()
            )));
        }
        if round.get("quality_state").and_then(Value::as_str) != Some("passed") {
            return Err(RefineError::Conflict(format!(
                "Goal {} Round {} has no passed Quality evidence",
                goal_id,
                round_idx + 1
            )));
        }
        if round.get("rule_state").and_then(Value::as_str) != Some("passed") {
            return Err(RefineError::Conflict(format!(
                "Goal {} Round {} has no passed Governance evidence",
                goal_id,
                round_idx + 1
            )));
        }
        if round
            .get("mission_context")
            .map(|context| context.is_null())
            != Some(false)
        {
            return Err(RefineError::Conflict(format!(
                "Goal {} Round {} has no pinned Mission context",
                goal_id,
                round_idx + 1
            )));
        }
        let mut contribution = contribution;
        let encoded = serde_json::to_vec(&contribution).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Mission contribution: {error}"))
        })?;
        let digest =
            crate::application::missions::reconciliation::engine::compute_snapshot_digest_bytes(
                &encoded,
            );
        contribution.digest = Some(digest.clone());
        self.update_goal_round_evaluation_summary(
            goal_id,
            round_idx,
            &json!({
                "mission_contribution": serde_json::to_value(&contribution).map_err(|error| RefineError::Serialization(format!("failed to encode Mission contribution: {error}")))?,
            }),
        )
    }

    /// Read the settled Mission contribution of a Goal's latest Round, when
    /// one exists.
    pub fn goal_mission_contribution(
        &self,
        goal_id: &str,
    ) -> RefineResult<Option<(GoalContribution, usize)>> {
        let current = self.show_goal_summary(goal_id)?;
        let round_idx = current
            .goal
            .round_count
            .checked_sub(1)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let (_goal_lock, _goal_path, value) = self.read_goal_value_unchecked(&current)?;
        let rounds = value
            .get("rounds")
            .and_then(Value::as_array)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let round = rounds.get(round_idx).ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} has no round {}", round_idx + 1))
        })?;
        let contribution = round
            .get("mission_contribution")
            .filter(|contribution| !contribution.is_null())
            .cloned();
        Ok(contribution
            .map(|value| {
                serde_json::from_value(value).map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to parse Mission contribution of Goal {goal_id}: {error}"
                    ))
                })
            })
            .transpose()?
            .map(|contribution| (contribution, round_idx)))
    }
}
