use super::*;
use std::collections::BTreeSet;

impl FileProcessControlService {
    /// Cancels one Goal through the authoritative process/workflow settlement capability.
    ///
    /// Active workflow executions use the same operation tombstone, managed-process termination,
    /// claim/capacity settlement, receipt, and replay path as execution cancellation. Claimed but
    /// not yet started Goals and inactive Goals use the same journaled settlement without
    /// inventing process evidence.
    pub fn cancel_goal(&self, goal_id: &str) -> RefineResult<Value> {
        let goal_id = goal_id.trim();
        if goal_id.is_empty() {
            return Err(RefineError::InvalidInput("Goal id is required".to_string()));
        }
        let refine_dir = self.resolve_refine_dir()?;
        let registration_lock = acquire_workflow_process_registration_lock(&self.runtime_root)?;
        if let Some(replayed) = self.replay_cancellation_settlement_for_goal(
            &refine_dir,
            goal_id,
            TerminationIntent::ExplicitCancellation,
        )? {
            return Ok(replayed);
        }

        let work_items = FileWorkItemService::new(&refine_dir);
        let current = work_items.show_goal_summary(goal_id)?;
        if current.goal.status == GoalStatus::Cancelled {
            let worktree_retention = WorkflowWorktreeRetention::from_targets(
                &self.retained_worktrees_for_goal(goal_id)?,
            );
            return Ok(json!({
                "cancelled": true,
                "goal_id": goal_id,
                "goal": current.goal,
                "already_cancelled": true,
                "termination_intent": TerminationIntent::ExplicitCancellation,
                "worktree_retention": worktree_retention
            }));
        }
        let expectation = preflight_goal_state(&refine_dir, goal_id)?;
        let state = WorkflowEngine::new(&self.runtime_root).load_state()?;
        let active_claims = state
            .claims
            .iter()
            .filter(|claim| {
                claim.goal_id == goal_id
                    && matches!(
                        claim.state,
                        WorkflowClaimState::Claimed | WorkflowClaimState::Running
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        if active_claims.len() > 1 {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} has multiple active workflow claims; cancellation requires operator inspection"
            )));
        }

        if let Some(claim) = active_claims.first() {
            if claim.state == WorkflowClaimState::Running {
                let execution_id = claim.execution_id.as_deref().ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "running claim {} for Goal {goal_id} has no execution id; the Goal, claim, and capacity remain active",
                        claim.claim_id
                    ))
                })?;
                drop(registration_lock);
                let result = self.cancel_workflow_execution_managed(execution_id)?;
                return self.require_durable_goal_cancellation(goal_id, result);
            }
            if claim.execution_id.is_some() {
                return Err(RefineError::Conflict(format!(
                    "claimed workflow claim {} for Goal {goal_id} unexpectedly has an execution id; cancellation did not guess at ownership",
                    claim.claim_id
                )));
            }
            if !self.managed_processes_for_goal(goal_id)?.is_empty() {
                return Err(RefineError::Conflict(format!(
                    "claimed Goal {goal_id} has managed process evidence before execution ownership was recorded; cancellation did not terminate an ambiguous process"
                )));
            }
            let ownership = [WorkflowGoalOwnership {
                process_id: format!("unstarted workflow claim {}", claim.claim_id),
                claim_id: claim.claim_id.clone(),
                execution_id: None,
                round_idx: claim.round_idx,
            }];
            let worktrees = self.retained_worktrees_for_goal(goal_id)?;
            let goal = self.settle_goal_cancellation(
                &refine_dir,
                goal_id,
                &expectation,
                &ownership,
                TerminationIntent::ExplicitCancellation,
                &worktrees,
            )?;
            return self.require_durable_goal_cancellation(
                goal_id,
                json!({
                    "goal_id": goal_id,
                    "claim_id": claim.claim_id,
                    "processes": [],
                    "goal": goal.goal,
                    "worktree_retention": goal.worktree_retention
                }),
            );
        }

        let managed_process_ids = self
            .managed_processes_for_goal(goal_id)?
            .into_iter()
            .map(|(_, process)| process.id)
            .collect::<Vec<_>>();
        if !managed_process_ids.is_empty() {
            drop(registration_lock);
            let mut processes = Vec::new();
            for process_id in managed_process_ids {
                processes.push(self.terminate_process(
                    &process_id,
                    "terminate",
                    TerminationIntent::ExplicitCancellation,
                )?);
            }
            let goal = work_items.show_goal_summary(goal_id)?;
            let worktree_retention = WorkflowWorktreeRetention::from_targets(
                &self.retained_worktrees_for_goal(goal_id)?,
            );
            return self.require_durable_goal_cancellation(
                goal_id,
                json!({
                    "goal_id": goal_id,
                    "processes": processes,
                    "goal": goal.goal,
                    "worktree_retention": worktree_retention
                }),
            );
        }

        let worktrees = self.retained_worktrees_for_goal(goal_id)?;
        let goal = self.settle_goal_cancellation(
            &refine_dir,
            goal_id,
            &expectation,
            &[],
            TerminationIntent::ExplicitCancellation,
            &worktrees,
        )?;
        self.require_durable_goal_cancellation(
            goal_id,
            json!({
                "goal_id": goal_id,
                "processes": [],
                "goal": goal.goal,
                "worktree_retention": goal.worktree_retention
            }),
        )
    }

    /// Applies cancellation independently so one unsafe or failed Goal cannot be reported as
    /// successful and cannot prevent safe peers from settling.
    pub fn bulk_cancel_goals(
        &self,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkUpdateResult> {
        let refine_dir = self.resolve_refine_dir()?;
        let work_items = FileWorkItemService::new(&refine_dir);
        let ids = if let Some(selected_ids) = &selection.selected_ids {
            let excluded = selection
                .exclude_ids
                .iter()
                .map(|id| id.trim().to_uppercase())
                .collect::<BTreeSet<_>>();
            selected_ids
                .iter()
                .map(|id| id.trim().to_uppercase())
                .filter(|id| !id.is_empty() && !excluded.contains(id))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        } else {
            work_items.select_bulk_goal_ids(&selection)?
        };
        #[cfg(test)]
        if let Some(hook) = &self.after_bulk_goal_selection_hook {
            hook();
        }

        let active_node = FileNodeRegistryService::new(&refine_dir).active_node_id()?;
        let mut updated_ids = Vec::new();
        let mut skipped_details = Vec::new();
        let mut failures = Vec::new();
        for goal_id in ids {
            let selected = match work_items.show_goal_summary(&goal_id) {
                Ok(goal) => goal,
                Err(error) => {
                    failures.push(bulk_goal_failure(&goal_id, &error));
                    continue;
                }
            };
            if selected.goal.status == GoalStatus::Done {
                skipped_details.push(BulkSkippedDetail {
                    id: goal_id,
                    reason: "status:done".to_string(),
                });
                continue;
            }
            let owner = selected.goal.node_id.as_deref().unwrap_or("default");
            if owner != active_node {
                skipped_details.push(BulkSkippedDetail {
                    id: goal_id,
                    reason: format!("node:{owner}"),
                });
                continue;
            }
            match self.cancel_goal(&goal_id) {
                Ok(result) if result.get("cancelled").and_then(Value::as_bool) == Some(true) => {
                    updated_ids.push(goal_id)
                }
                Ok(result) => failures.push(json!({
                    "id": goal_id,
                    "error": {
                        "code": "partial",
                        "message": "Goal cancellation did not reach verified durable cancelled state",
                        "evidence": result
                    }
                })),
                Err(error) => failures.push(bulk_goal_failure(&goal_id, &error)),
            }
        }
        Ok(BulkUpdateResult {
            updated: updated_ids.len(),
            ids: updated_ids,
            field: "status".to_string(),
            value: GoalStatus::Cancelled.as_str().to_string(),
            skipped: skipped_details.len(),
            skipped_details,
            failed: failures.len(),
            failures,
        })
    }

    fn require_durable_goal_cancellation(
        &self,
        goal_id: &str,
        mut result: Value,
    ) -> RefineResult<Value> {
        let refine_dir = self.resolve_refine_dir()?;
        let durable = FileWorkItemService::new(&refine_dir).show_goal_summary(goal_id)?;
        if durable.goal.status != GoalStatus::Cancelled {
            return Err(RefineError::Degraded(format!(
                "Goal {goal_id} cancellation reached a partial outcome: durable status is {}, not cancelled; cancelled:true was not reported",
                durable.goal.status.as_str()
            )));
        }
        let object = result.as_object_mut().ok_or_else(|| {
            RefineError::Serialization("Goal cancellation result must be an object".to_string())
        })?;
        object.insert("cancelled".to_string(), json!(true));
        object.insert(
            "termination_intent".to_string(),
            json!(TerminationIntent::ExplicitCancellation),
        );
        object.insert("goal".to_string(), json!(durable.goal));
        Ok(result)
    }
}

fn bulk_goal_failure(goal_id: &str, error: &RefineError) -> Value {
    let code = match error {
        RefineError::InvalidInput(_) => "invalid_input",
        RefineError::NotFound(_) => "not_found",
        RefineError::Unauthorized(_) => "unauthorized",
        RefineError::Conflict(_) | RefineError::StaleCandidate { .. } => "conflict",
        RefineError::Degraded(_) => "degraded",
        RefineError::Io(_) | RefineError::Serialization(_) => "storage_error",
        RefineError::NotImplemented(_) => "not_implemented",
    };
    json!({
        "id": goal_id,
        "error": {
            "code": code,
            "message": error.to_string()
        }
    })
}
