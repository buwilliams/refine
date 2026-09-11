use super::*;
use serde_json::json;

use super::workflow_attempts::{goal_status, require_current_step};

impl FileWorkItemService {
    /// Repin a Goal's base and candidate onto a refreshed replacement.
    ///
    /// `authorizing_status` is the workflow status the refreshing behavior runs
    /// under — `Quality` for the Implement→Quality boundary refresh,
    /// `Governance` for the pre-integration one. The repin is otherwise
    /// identical at both boundaries: the same gate evidence is cleared, so
    /// whichever gate runs next runs against the new base.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_candidate_refresh(
        &self,
        goal_id: &str,
        authority: WorkflowStepAuthority,
        authorizing_status: &GoalStatus,
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
        if goal_status(object) != *authorizing_status
            || object
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or("default")
                != node_id
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} no longer authorizes candidate refresh during {} on node {node_id}",
                authorizing_status.as_str()
            )));
        }
        require_current_step(goal_id, object, authority)?;
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
        // A Round can refresh more than once — the Implement→Quality boundary
        // and then Governance, or Governance across its bounded retries — so
        // the slot it publishes into is not free: whatever it replaces travels
        // inside the replacement rather than being overwritten away.
        let superseded = superseded_refresh_history(round.get("workflow_candidate_refresh"));
        let mut evidence = json!({
            "state": "refreshed",
            "round_idx": authority.round_idx,
            "workflow_revision": authority.workflow_revision,
            "node_id": node_id,
            "authorizing_status": authorizing_status.as_str(),
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
        if !superseded.is_empty() {
            evidence["superseded_refresh"] = json!(superseded);
        }
        round.insert("workflow_candidate_refresh".to_string(), evidence.clone());
        round.insert("updated".to_string(), json!(now));
        object.insert("base_commit".to_string(), json!(replacement_base));
        object.insert("candidate_commit".to_string(), json!(replacement_candidate));
        object.insert("updated".to_string(), json!(now));
        write_json_atomically(&goal_path, &value)?;
        Ok(evidence)
    }

    /// Queue (or exhaust) the fenced integration recovery Round.
    ///
    /// `authorizing_status` is the workflow status the caller runs under: the
    /// candidate refresh reaches this from `Quality` as well as `Governance`,
    /// and the queued Round is identical either way — an `integration` retry
    /// that replays from a fresh base.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn settle_integration_failure_summary(
        &self,
        goal_id: &str,
        authority: WorkflowStepAuthority,
        authorizing_status: &GoalStatus,
        node_id: &str,
        reason: &str,
        retained_evidence: Value,
    ) -> RefineResult<GoalSummaryProjection> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} is not a JSON object"))
        })?;
        if goal_status(object) != *authorizing_status
            || object
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or("default")
                != node_id
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} no longer authorizes integration recovery during {} on node {node_id}",
                authorizing_status.as_str()
            )));
        }
        require_current_step(goal_id, object, authority)?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no Rounds")))?;
        if rounds.len() != authority.round_idx + 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} Round changed before integration recovery"
            )));
        }
        // A refresh failure records its evidence in the originating attempt. Only
        // an explicit workflow decision may authorize another Round.
        rounds[authority.round_idx]["workflow_recovery"] = json!({
            "state": "failed", "kind": "integration", "reason": reason,
            "source_round": authority.round_idx + 1, "successor_round": null,
            "retained_evidence": retained_evidence, "recorded_at": now_timestamp(),
            "automatic_recovery": false
        });
        write_json_atomically(&goal_path, &value)?;
        drop(_goal_lock);
        self.settle_workflow_attempt_failure(
            goal_id,
            authority,
            "integration",
            reason,
            &now_timestamp(),
        )?;
        self.show_goal_summary(goal_id)
    }
}

/// How many superseded refreshes a Round keeps beside its current one.
const SUPERSEDED_REFRESH_HISTORY: usize = 3;

/// The refresh history a replacement carries: the refresh it supersedes,
/// compacted, appended to that refresh's own history and trimmed to the most
/// recent [`SUPERSEDED_REFRESH_HISTORY`].
///
/// The number of refreshes in one Round is not bounded — every resumed
/// Quality pass and every Governance retry may repin — and the Goal record is
/// synchronized plain-text state that is republished and three-way merged
/// whole on every sync. So the history is a flat, capped list of what each
/// refresh moved, never a chain of whole nested refreshes: the gate evidence
/// a superseded refresh cleared belongs to the refresh that cleared it, and
/// only the current one can still be acted on.
fn superseded_refresh_history(current: Option<&Value>) -> Vec<Value> {
    let Some(current) = current.filter(|value| !value.is_null()) else {
        return Vec::new();
    };
    let mut history = current
        .get("superseded_refresh")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut compact = current.clone();
    if let Some(object) = compact.as_object_mut() {
        object.remove("previous_gate_evidence");
        object.remove("superseded_refresh");
    }
    history.push(compact);
    if history.len() > SUPERSEDED_REFRESH_HISTORY {
        history.drain(..history.len() - SUPERSEDED_REFRESH_HISTORY);
    }
    history
}
