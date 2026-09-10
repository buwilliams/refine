use super::*;

impl WorkflowEngine {
    pub(super) fn run_goal_attempt(&self, goal_id: &str) -> RefineResult<WorkflowStepResult> {
        // Preparation may panic after claiming. Keep authority outside the unwind boundary.
        let mut authority = None;
        let prepared = contain(goal_id, || {
            self.prepare_goal(goal_id, &mut authority)
                .map_err(|failure| failure.error)
        });
        match prepared {
            Ok(PreparedGoal::Execute(ctx)) => self.execute_prepared_goal(*ctx),
            Ok(PreparedGoal::Completed(result)) => Ok(*result),
            Err(error) => {
                self.settle_attempt_outcome(goal_id, authority, "preparation", Err(error))
            }
        }
    }

    fn settle_attempt_outcome(
        &self,
        goal_id: &str,
        authority: Option<WorkflowAttemptAuthority>,
        stage: &str,
        outcome: RefineResult<WorkflowStepResult>,
    ) -> RefineResult<WorkflowStepResult> {
        if let (Some(authority), Err(error)) = (authority, &outcome)
            && !matches!(error, RefineError::Degraded(message) if message.starts_with("workspace is in use"))
        {
            // Settlement owns panic containment and the common originating-evidence path.
            // Its outcome cannot replace the original attempt error delivered to the controller.
            self.settle_goal_failure(goal_id, authority, stage, error);
        }
        outcome
    }

    fn prepare_goal<'a>(
        &'a self,
        goal_id: &str,
        claimed_authority: &mut Option<WorkflowAttemptAuthority>,
    ) -> Result<PreparedGoal<'a>, PreparedGoalError> {
        let unclaimed = |error| PreparedGoalError { error };
        let target_root = self
            .target_root
            .as_ref()
            .ok_or_else(|| {
                RefineError::InvalidInput(
                    "target root is required to execute workflow work".to_string(),
                )
            })
            .map_err(unclaimed)?;
        let refine_dir = prepare_refine_dir(target_root).map_err(unclaimed)?;
        let work_items = FileWorkItemService::with_projection_cache(
            &refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let policy = self.policy().map_err(unclaimed)?;
        let summary = work_items.show_goal_summary(goal_id).map_err(unclaimed)?;
        if work_items
            .show_goal_detail(goal_id)
            .map_err(unclaimed)?
            .get("pending_event_transition")
            .is_some_and(|p| p["state"] == "pending")
        {
            return Err(unclaimed(RefineError::Conflict(format!(
                "{} {goal_id}",
                crate::application::events::transitions::PENDING
            ))));
        }
        let node_id = summary
            .goal
            .node_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        if !crate::application::fleet::nodes::node_ids_match(&node_id, &policy.active_node_id) {
            return Err(unclaimed(RefineError::Conflict(format!(
                "Goal {goal_id} is owned by node {node_id}, not active node {}",
                policy.active_node_id
            ))));
        }
        if !matches!(
            summary.goal.status,
            GoalStatus::Todo
                | GoalStatus::Plan
                | GoalStatus::Implement
                | GoalStatus::Governance
                | GoalStatus::Quality
        ) {
            return Err(unclaimed(RefineError::Conflict(format!(
                "Goal {goal_id} is no longer eligible from {}",
                summary.goal.status.as_str()
            ))));
        }
        let (round_idx, authored_revision, authored_request) =
            authored_workflow_commitment(&work_items, goal_id).map_err(unclaimed)?;
        #[cfg(test)]
        test_hooks::run(
            self,
            goal_id,
            "before_claim",
            WorkflowAttemptAuthority {
                round_idx,
                workflow_revision: authored_revision,
            },
        )
        .map_err(unclaimed)?;
        let authority = work_items
            .claim_workflow_attempt(
                goal_id,
                summary.goal.status.clone(),
                round_idx,
                authored_revision,
                &authored_request,
            )
            .map_err(unclaimed)?;
        *claimed_authority = Some(authority);
        let claimed = |error| PreparedGoalError { error };
        #[cfg(test)]
        test_hooks::run(self, goal_id, "claimed", authority).map_err(claimed)?;
        let settings = FileSettingsService::with_active_root(&refine_dir, &self.runtime_root)
            .load()
            .map_err(claimed)?;
        let mut ctx = WorkflowContext::new(
            &self.runtime_root,
            target_root,
            goal_id.to_string(),
            node_id,
            policy.provider,
            round_idx,
            authority,
            settings,
            work_items,
        );
        match summary.goal.status {
            GoalStatus::Todo => match WorkflowTodo.advance(&mut ctx).map_err(claimed)? {
                WorkflowAdvanceOutcome::Transition {
                    to: GoalStatus::Plan,
                    ..
                }
                | WorkflowAdvanceOutcome::Transition {
                    to: GoalStatus::Quality,
                    ..
                } => Ok(PreparedGoal::Execute(Box::new(ctx))),
                WorkflowAdvanceOutcome::Completed { final_status, .. } => {
                    ctx.final_status = Some(final_status);
                    Ok(PreparedGoal::Completed(Box::new(
                        Self::workflow_step_result(ctx).map_err(claimed)?,
                    )))
                }
                outcome => Err(claimed(RefineError::Conflict(outcome_reason(outcome)))),
            },
            GoalStatus::Plan | GoalStatus::Implement => {
                let start_status = summary.goal.status.clone();
                let pattern =
                    setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}");
                let target = setting_string(&ctx.settings, "merge_target_branch", "main");
                hydrate_plan_or_implement_context(&mut ctx, &pattern, &target).map_err(claimed)?;
                ctx.start_status = start_status;
                Ok(PreparedGoal::Execute(Box::new(ctx)))
            }
            current => {
                hydrate_retry_context(&mut ctx, current).map_err(claimed)?;
                Ok(PreparedGoal::Execute(Box::new(ctx)))
            }
        }
    }

    pub(crate) fn execute_prepared_goal(
        &self,
        mut ctx: WorkflowContext<'_>,
    ) -> RefineResult<WorkflowStepResult> {
        let goal_id = ctx.goal_id.clone();
        let authority = ctx.attempt_authority;
        let outcome = contain(&goal_id, || {
            #[cfg(test)]
            test_hooks::run(self, &goal_id, "executing", authority)?;
            let start_status = ctx.start_status.clone();
            self.advance_behaviors(&mut ctx, start_status)
        });
        match outcome {
            Ok(()) => self.complete_goal_result(ctx),
            Err(error) => {
                self.settle_attempt_outcome(&goal_id, Some(authority), "workflow", Err(error))
            }
        }
    }

    fn complete_goal_result(&self, ctx: WorkflowContext<'_>) -> RefineResult<WorkflowStepResult> {
        let goal_id = ctx.goal_id.clone();
        let authority = ctx.attempt_authority;
        let result = contain(&goal_id, || {
            #[cfg(test)]
            test_hooks::run(self, &goal_id, "result", authority)?;
            Self::workflow_step_result(ctx)
        });
        self.settle_attempt_outcome(&goal_id, Some(authority), "result", result)
    }

    fn workflow_step_result(ctx: WorkflowContext<'_>) -> RefineResult<WorkflowStepResult> {
        let branch = ctx
            .branch
            .clone()
            .ok_or_else(|| missing_workflow_artifact("branch", &ctx.goal_id))?;
        let commit = ctx
            .commit
            .clone()
            .ok_or_else(|| missing_workflow_artifact("commit", &ctx.goal_id))?;
        let provider_output = ctx
            .provider_output
            .clone()
            .ok_or_else(|| missing_workflow_artifact("provider output", &ctx.goal_id))?;
        let final_status = ctx
            .final_status
            .clone()
            .unwrap_or(GoalStatus::Review)
            .as_str()
            .to_string();
        Ok(WorkflowStepResult {
            goal_id: ctx.goal_id,
            provider: ctx.provider,
            branch,
            commit,
            merge: ctx.merge,
            final_status,
            provider_output,
        })
    }

    pub(crate) fn advance_behaviors(
        &self,
        ctx: &mut WorkflowContext<'_>,
        mut current: GoalStatus,
    ) -> RefineResult<()> {
        let plan = WorkflowPlan;
        let implementation = WorkflowImplementation;
        let quality = WorkflowQuality;
        let governance = WorkflowGovernance;
        let review = WorkflowReview;
        let done = WorkflowDone;
        let behaviors: [&dyn WorkflowBehavior; 6] = [
            &plan,
            &implementation,
            &quality,
            &governance,
            &review,
            &done,
        ];
        let mut integrated_target_lane = None;
        loop {
            if integrated_target_lane.is_none()
                && workflow_status_uses_integrated_target(ctx, &current)?
            {
                integrated_target_lane = Some(IntegratedTargetWorkflowLease::acquire(
                    ctx.target_root,
                    &ctx.goal_id,
                    ctx.round_idx,
                )?);
            }
            let Some(behavior) = behaviors
                .iter()
                .copied()
                .find(|behavior| behavior.observes() == current)
            else {
                return Err(RefineError::Conflict(format!(
                    "No workflow behavior registered for {}",
                    current.as_str()
                )));
            };
            let _target_workspace = if integrated_target_lane.is_some() {
                Some(
                    crate::infrastructure::storage::workspace::WorkspaceLease::acquire(
                        ctx.target_root,
                    )?,
                )
            } else {
                None
            };
            let workspace = ctx
                .worktree_path
                .as_deref()
                .map(std::path::Path::new)
                .unwrap_or(ctx.target_root);
            let _workspace =
                crate::infrastructure::storage::workspace::WorkspaceLease::acquire(workspace)?;
            match behavior.advance(ctx) {
                Ok(WorkflowAdvanceOutcome::Transition { to, .. }) => current = to,
                Ok(WorkflowAdvanceOutcome::Completed { .. }) => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish()?;
                    }
                    return Ok(());
                }
                Ok(outcome) => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish_if_clean();
                    }
                    return Err(RefineError::Conflict(outcome_reason(outcome)));
                }
                Err(error) => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish_if_clean();
                    }
                    return Err(error);
                }
            }
        }
    }
}

fn outcome_reason(outcome: WorkflowAdvanceOutcome) -> String {
    match outcome {
        WorkflowAdvanceOutcome::Transition { reason, .. }
        | WorkflowAdvanceOutcome::Completed { reason, .. }
        | WorkflowAdvanceOutcome::Noop { reason }
        | WorkflowAdvanceOutcome::Blocked { reason }
        | WorkflowAdvanceOutcome::Failed { reason } => reason,
    }
}

fn workflow_status_uses_integrated_target(
    ctx: &mut WorkflowContext<'_>,
    status: &GoalStatus,
) -> RefineResult<bool> {
    match status {
        GoalStatus::Governance => Ok(true),
        GoalStatus::Quality if ctx.reconciliation.is_some() => Ok(true),
        _ => Ok(false),
    }
}

pub(super) fn contain<T>(
    goal_id: &str,
    operation: impl FnOnce() -> RefineResult<T>,
) -> RefineResult<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).unwrap_or_else(|_| {
        Err(RefineError::Conflict(format!(
            "workflow attempt panicked for Goal {goal_id}"
        )))
    })
}

#[cfg(test)]
mod tests;
