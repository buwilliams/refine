//! Planning and implementation Skills and candidate settlement.
use super::*;

impl WorkflowBehavior for WorkflowPlan {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Plan
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let worktree_path = ctx.require_worktree_path()?.to_string();
        let goal = match ctx.work_items.show_goal_detail(&ctx.goal_id) {
            Ok(goal) => goal,
            Err(error) => return fail(ctx, "plan", error),
        };
        let agent_context = match ensure_goal_agent_context(ctx, &goal) {
            Ok(context) => context,
            Err(error) => return fail(ctx, "agent_context", error),
        };
        let agent_cwd = match agent_worktree_cwd(
            &worktree_path,
            setting_string(&ctx.settings, "agent_subpath", "").as_str(),
        ) {
            Ok(cwd) => cwd,
            Err(error) => return fail(ctx, "plan", error),
        };
        if let Err(error) = run_planning_skills(ctx, &agent_context, &agent_cwd) {
            return fail(ctx, "plan", error);
        }
        ctx.agent_cwd = Some(agent_cwd);
        ctx.request_transition(GoalStatus::Plan, GoalStatus::Implement)?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Plan,
            to: GoalStatus::Implement,
            reason: "Plan Skills reported success".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowImplementation {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Implement
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let branch = ctx.require_branch()?.to_string();
        let worktree_path = ctx.require_worktree_path()?.to_string();
        let goal = match ctx.work_items.show_goal_detail(&ctx.goal_id) {
            Ok(goal) => goal,
            Err(error) => return fail(ctx, "agent", error),
        };
        let agent_context = match ensure_goal_agent_context(ctx, &goal) {
            Ok(context) => context,
            Err(error) => return fail(ctx, "agent_context", error),
        };
        let agent_cwd = match agent_worktree_cwd(
            &worktree_path,
            setting_string(&ctx.settings, "agent_subpath", "").as_str(),
        ) {
            Ok(cwd) => cwd,
            Err(error) => return fail(ctx, "agent", error),
        };
        let final_plan = match planning_context(ctx) {
            Ok(plan) => plan,
            Err(error) => return fail(ctx, "implement", error),
        };
        let results = crate::application::events::workflow::run(
            ctx,
            GoalStatus::Implement,
            "enter",
            &agent_cwd,
            json!({"agent_context": agent_context, "plans": final_plan}),
            "implement",
        )?;
        crate::application::events::workflow::require_success(&results)?;
        let provider_output = results
            .iter()
            .map(|r| format!("{}: {}", r.binding_id, r.summary))
            .collect::<Vec<_>>()
            .join("\n");
        if let Err(error) = ctx
            .work_items
            .update_latest_goal_round_implementation_report(&ctx.goal_id, &provider_output)
        {
            return fail(ctx, "agent", error);
        }
        ctx.log(
            "agent",
            "Goal agent completed",
            Some(json_object(json!({
                "provider": ctx.provider,
                "output": provider_output,
                "branch": branch,
                "worktree": worktree_path
            }))),
        )?;

        let worktree_git = ctx.candidate_git()?;
        let target_branch = setting_string(&ctx.settings, "merge_target_branch", "main");
        let commit = match with_repository_git_lock(ctx.target_root, || {
            worktree_git.commit_or_clean_noop_since(
                &format!("Implement {} round {}", ctx.goal_id, ctx.round_idx + 1),
                &[],
                &target_branch,
            )
        }) {
            Ok(outcome) => outcome,
            Err(error) => return fail(ctx, "commit", error),
        };
        if let Err(error) = ctx
            .work_items
            .update_goal_candidate_commit(&ctx.goal_id, &commit.commit)
        {
            return fail(ctx, "commit", error);
        }
        let handoff_id = ctx
            .candidate_handoff_operation_id
            .clone()
            .or_else(|| {
                find_candidate_handoff(
                    ctx.runtime_root,
                    ctx.target_root,
                    &ctx.goal_id,
                    ctx.round_idx,
                )
                .ok()
                .flatten()
                .map(|operation| operation.id)
            })
            .ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {} Round {} has no active candidate handoff after commit",
                    ctx.goal_id,
                    ctx.round_idx + 1
                ))
            })?;
        if let Err(error) =
            record_candidate_handoff_commit(ctx.runtime_root, &handoff_id, &commit.commit)
        {
            return fail(ctx, "candidate_handoff", error);
        }
        ctx.candidate_handoff_operation_id = Some(handoff_id.clone());
        if commit.has_changes_since_base {
            ctx.log(
                "git",
                &format!("Committed implementation branch {branch}"),
                Some(json_object(json!({
                    "branch": branch,
                    "commit": commit.commit,
                    "worktree": worktree_path
                }))),
            )?;
        } else {
            ctx.log(
                "git",
                "No implementation changes to commit",
                Some(json_object(json!({
                    "branch": branch,
                    "commit": commit.commit,
                    "worktree": worktree_path,
                    "target_branch": target_branch
                }))),
            )?;
        }

        ctx.agent_cwd = Some(agent_cwd);
        ctx.provider_output = Some(provider_output);
        ctx.implementation_changed = commit.has_changes_since_base;
        ctx.commit = Some(commit.commit.clone());
        if let Err(error) = ctx.request_transition(GoalStatus::Implement, GoalStatus::Quality) {
            retain_candidate_handoff_after_failure(
                ctx.runtime_root,
                &handoff_id,
                "candidate_handoff_transition_failed",
                ctx.commit.as_deref(),
                &error,
            );
            return fail(ctx, "candidate_handoff", error);
        }
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Implement,
            to: GoalStatus::Quality,
            reason: "Implementation completed".to_string(),
        })
    }
}
