//! Planning and implementation Skills and candidate settlement.
use super::*;

impl WorkflowBehavior for WorkflowPlan {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Plan
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let branch = ctx.require_branch()?.to_string();
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
        if let Err(error) =
            run_governed_implementation_planning(ctx, &goal, &agent_context, &agent_cwd, &branch)
        {
            return fail(ctx, "plan", error);
        }
        ctx.agent_cwd = Some(agent_cwd);
        ctx.request_transition(GoalStatus::Plan, GoalStatus::Implement)?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Plan,
            to: GoalStatus::Implement,
            reason: "Plan Skills produced validated implementation plans".to_string(),
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
        let final_plan = match begin_implementation_phase(ctx) {
            Ok(plan) => plan,
            Err(error) => return fail(ctx, "implement", error),
        };
        let implementation_started_at = now_timestamp();
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
        let mut evidence = crate::model::goal::ImplementationExecutionEvidence {
            checklist: Vec::new(),
            verification: Vec::new(),
        };
        for result in results.iter().filter(|r| r.role == "implement") {
            let value = result
                .artifacts
                .get("implementation_evidence")
                .cloned()
                .ok_or_else(|| {
                    RefineError::InvalidInput("Implement Skill omitted checklist evidence".into())
                })?;
            let next: crate::model::goal::ImplementationExecutionEvidence =
                serde_json::from_value(value)
                    .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
            for item in next.checklist {
                if let Some(existing) = evidence.checklist.iter_mut().find(|e| e.id == item.id) {
                    existing
                        .evidence
                        .push_str(&format!("\n{}: {}", result.binding_id, item.evidence));
                    if !matches!(
                        item.outcome,
                        crate::model::goal::ImplementationChecklistOutcome::Completed
                            | crate::model::goal::ImplementationChecklistOutcome::NoChangeNeeded
                    ) {
                        existing.outcome = item.outcome;
                    }
                } else {
                    evidence.checklist.push(item);
                }
            }
            evidence.verification.extend(next.verification);
        }
        complete_implementation_planning(
            ctx,
            implementation_started_at,
            provider_output.clone(),
            Some(evidence),
        )?;
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
        let changed_paths = match worktree_git.changed_paths_since(&target_branch, &commit.commit) {
            Ok(paths) => paths,
            Err(error) => return fail(ctx, "guidance", error),
        };
        let code_changed = changed_paths.iter().any(|path| is_code_path(path));
        let guidance_decision = match guidance_decision(&agent_context, None, code_changed) {
            Ok(decision) => decision,
            Err(error) => return fail(ctx, "guidance", error),
        };
        if let Err(error) = ctx.work_items.update_latest_goal_round_evaluation_summary(
            &ctx.goal_id,
            &json!({"guidance_decision": guidance_decision}),
        ) {
            return fail(ctx, "guidance", error);
        }
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
