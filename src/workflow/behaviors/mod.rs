use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::agent_sessions::{GoalAgentLaunch, run_goal_agent_with_settlement};
use crate::process::supervisor::config::{FileGovernanceService, FileGuidanceService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptEngine, PromptTemplate};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::host::quality::{
    POST_BUILD, QualityCheckResult, QualityOperationRunner, is_quality_harness_fault,
    quality_error_summary,
};
use crate::tools::host::target_apps::FileTargetAppService;
use crate::tools::product::merging::{FileMergerService, ReconciliationRequest};
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::context::WorkflowContext;
use crate::workflow::{
    GovernanceEvaluation, active_implementation_operation_id, agent_worktree_cwd,
    complete_implementation_planning, fail_implementation_phase, governed_implementation_prompt,
    implementation_branch_name, json_object, now_timestamp, parse_governance_provider_output,
    post_implementation_governance_prompt, record_current_process_settlement, round_agent_context,
    run_governed_implementation_planning, selected_agent_context, setting_string,
};

#[derive(Clone, Debug, Default)]
pub struct WorkflowBacklog;

#[derive(Clone, Debug, Default)]
pub struct WorkflowTodo;

#[derive(Clone, Debug, Default)]
pub struct WorkflowImplementation;

#[derive(Clone, Debug, Default)]
pub struct WorkflowQa;

#[derive(Clone, Debug, Default)]
pub struct WorkflowReadyMerge;

#[derive(Clone, Debug, Default)]
pub struct WorkflowBuild;

#[derive(Clone, Debug, Default)]
pub struct WorkflowReview;

#[derive(Clone, Debug, Default)]
pub struct WorkflowDone;

#[derive(Clone, Debug, Default)]
pub struct WorkflowFailed;

#[derive(Clone, Debug, Default)]
pub struct WorkflowCancelled;

impl WorkflowBehavior for WorkflowBacklog {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Backlog
    }

    fn advance(&self, _ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        Ok(WorkflowAdvanceOutcome::Blocked {
            reason: "backlog Goals wait until todo eligibility rules promote them".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowTodo {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Todo
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let app_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
        if let Some(outcome) = prepare_already_merged_reconciliation(ctx, &app_git)? {
            return Ok(outcome);
        }
        let branch = implementation_branch_name(
            setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}").as_str(),
            &ctx.goal_id,
            ctx.round_idx,
        );
        let target_branch = setting_string(&ctx.settings, "merge_target_branch", "main");
        let base_commit = match app_git.resolve_commit(&target_branch) {
            Ok(commit) => commit,
            Err(error) => return fail(ctx, "branch", error),
        };
        // Todo is a queue, not an execution workspace. The scheduler has already
        // acquired capacity for this Running claim before this transition, and
        // the durable Goal state must cross into in-progress before Git is
        // allowed to materialize a repository copy.
        ctx.request_transition(GoalStatus::Todo, GoalStatus::InProgress)?;
        let worktree_path = match materialize_in_progress_worktree(ctx, &app_git, &branch) {
            Ok(path) => path,
            Err(error) => return fail(ctx, "branch", error),
        };
        ctx.log(
            "git",
            &format!("Created implementation worktree for {branch}"),
            Some(json_object(json!({
                "branch": branch,
                "worktree": worktree_path
            }))),
        )?;
        if let Err(error) = ctx.work_items.update_goal_git_refs(
            &ctx.goal_id,
            &branch,
            &target_branch,
            &base_commit,
            None,
        ) {
            return fail(ctx, "branch", error);
        }
        ctx.branch = Some(branch);
        ctx.worktree_path = Some(worktree_path);
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Todo,
            to: GoalStatus::InProgress,
            reason: "Goal entered implementation".to_string(),
        })
    }
}

fn prepare_already_merged_reconciliation(
    ctx: &mut WorkflowContext<'_>,
    app_git: &FileGitWorktreeService,
) -> RefineResult<Option<WorkflowAdvanceOutcome>> {
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let Some(round) = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
    else {
        return Ok(None);
    };
    let Some(integration_value) = round
        .get("workflow_integration")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let integration =
        serde_json::from_value::<crate::model::goal::RoundIntegration>(integration_value.clone())
            .map_err(|error| {
            RefineError::Serialization(format!(
                "Goal {} has invalid Ready Merge evidence: {error}",
                ctx.goal_id
            ))
        })?;
    let recorded_reconciliation_state = round
        .get("workflow_reconciliation")
        .and_then(Value::as_object)
        .and_then(|evidence| evidence.get("state"))
        .and_then(Value::as_str)
        .filter(|state| matches!(*state, "reverted" | "completed"))
        .map(str::to_string);
    let candidate = detail
        .get("candidate_commit")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} has integration evidence but no recorded candidate",
                ctx.goal_id
            ))
        })?;
    if candidate != integration.candidate_commit {
        return Err(RefineError::Conflict(format!(
            "Goal {} candidate changed from integrated commit {} to {}; reconciliation was not started",
            ctx.goal_id, integration.candidate_commit, candidate
        )));
    }
    let (target_commit, published_commit, candidate_present) = with_repository_git_lock(
        ctx.target_root,
        || -> RefineResult<(String, Option<String>, bool)> {
            let target_commit = app_git.resolve_commit(&integration.target_branch)?;
            if !app_git.commit_is_ancestor(candidate, &target_commit)? {
                return Ok((target_commit, None, false));
            }
            let head = app_git.head_ref()?;
            if head.branch.as_deref() != Some(integration.target_branch.as_str())
                || head.commit.as_deref() != Some(target_commit.as_str())
            {
                return Err(RefineError::Conflict(format!(
                    "Goal {} already-merged reconciliation requires target worktree {} at {} on branch {}, found {} at {}; user work was preserved",
                    ctx.goal_id,
                    ctx.target_root.display(),
                    target_commit,
                    integration.target_branch,
                    head.branch.as_deref().unwrap_or("<detached>"),
                    head.commit.as_deref().unwrap_or("<unborn>")
                )));
            }
            let published = if integration.pushed {
                app_git.fetch_branch(&integration.remote, &integration.target_branch)?;
                let published = app_git.resolve_commit(&format!(
                    "{}/{}",
                    integration.remote, integration.target_branch
                ))?;
                if published != target_commit {
                    return Err(RefineError::Conflict(format!(
                        "Goal {} cannot reconcile while local {} ({target_commit}) differs from {}/{} ({published})",
                        ctx.goal_id,
                        integration.target_branch,
                        integration.remote,
                        integration.target_branch
                    )));
                }
                Some(published)
            } else {
                None
            };
            Ok((target_commit, published, true))
        },
    )?;
    if !candidate_present {
        let Some(recorded_state) = recorded_reconciliation_state.as_deref() else {
            return Ok(None);
        };
        ctx.work_items
            .queue_missing_reconciled_candidate_recovery_summary(
                &ctx.goal_id,
                ctx.round_idx,
                recorded_state,
                candidate,
                &integration.target_branch,
                &target_commit,
            )?;
        ctx.log(
            "reconcile",
            "Recorded reconciliation state disagreed with the target branch; queued a fresh recovery round",
            Some(json_object(json!({
                "recorded_reconciliation_state": recorded_state,
                "candidate_commit": candidate,
                "target_branch": integration.target_branch,
                "target_commit": target_commit,
                "successor_round": ctx.round_idx + 2
            }))),
        )?;
        ctx.branch = detail
            .get("branch_name")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        ctx.provider_output = Some(
            round
                .get("implementation_report")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| "Queued recovery for absent integrated candidate".to_string()),
        );
        ctx.commit = Some(candidate.to_string());
        ctx.implementation_changed = true;
        ctx.merge = Some(integration.merge.clone());
        ctx.final_status = Some(GoalStatus::Todo);
        return Ok(Some(WorkflowAdvanceOutcome::Completed {
            final_status: GoalStatus::Todo,
            reason:
                "Reconciliation evidence superseded; fresh recovery round queued from current target"
                    .to_string(),
        }));
    }
    if let Some(recorded_state) = recorded_reconciliation_state.as_deref() {
        ctx.log(
            "reconcile",
            "Recorded reconciliation state was stale; the candidate is currently present in the target branch",
            Some(json_object(json!({
                "recorded_reconciliation_state": recorded_state,
                "candidate_commit": candidate,
                "target_branch": integration.target_branch,
                "target_commit": target_commit
            }))),
        )?;
    }
    ctx.work_items.update_goal_round_evaluation_summary(
        &ctx.goal_id,
        ctx.round_idx,
        &json!({
            "workflow_reconciliation": {
                "state": "detected",
                "candidate_commit": candidate,
                "target_branch": integration.target_branch,
                "detected_target_commit": target_commit,
                "published_target_commit": published_commit,
                "recorded_reconciliation_state": recorded_reconciliation_state,
                "detected_at": now_timestamp()
            }
        }),
    )?;
    ctx.log(
        "reconcile",
        "Detected already-merged candidate; routing round to merged-target Quality",
        Some(json_object(json!({
            "candidate_commit": candidate,
            "target_branch": integration.target_branch,
            "target_commit": target_commit,
            "published_target_commit": published_commit
        }))),
    )?;
    ctx.branch = detail
        .get("branch_name")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    ctx.provider_output = Some(
        round
            .get("implementation_report")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| "Reconciled existing integrated candidate".to_string()),
    );
    ctx.commit = Some(candidate.to_string());
    ctx.implementation_changed = true;
    ctx.merge = Some(integration.merge.clone());
    ctx.reconciliation = Some(integration);
    ctx.reconciliation_state = Some("detected".to_string());
    ctx.start_status = GoalStatus::Qa;
    ctx.request_transition(GoalStatus::Todo, GoalStatus::Qa)?;
    Ok(Some(WorkflowAdvanceOutcome::Transition {
        from: GoalStatus::Todo,
        to: GoalStatus::Qa,
        reason: "Already-merged candidate requires reconciliation".to_string(),
    }))
}

fn materialize_in_progress_worktree(
    ctx: &WorkflowContext<'_>,
    app_git: &FileGitWorktreeService,
    branch: &str,
) -> RefineResult<String> {
    let status = ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status;
    if status != GoalStatus::InProgress {
        return Err(RefineError::Conflict(format!(
            "refusing to create an implementation worktree for Goal {} while it is {}; worktrees are materialized only after admission to in-progress",
            ctx.goal_id,
            status.as_str()
        )));
    }
    let worktree_target = app_git
        .git_path("refine-worktrees")?
        .join(branch.replace('/', "-"));
    with_repository_git_lock(ctx.target_root, || {
        app_git.ensure_worktree(branch, &worktree_target)
    })
}

impl WorkflowBehavior for WorkflowImplementation {
    fn observes(&self) -> GoalStatus {
        GoalStatus::InProgress
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
        let final_plan = match run_governed_implementation_planning(
            ctx,
            &goal,
            &agent_context,
            &agent_cwd,
            &branch,
        ) {
            Ok(plan) => plan,
            Err(error) => return fail(ctx, "implementation_planning", error),
        };
        let prompt = match governed_implementation_prompt(&ctx.goal_id, &agent_context, &final_plan)
        {
            Ok(prompt) => prompt,
            Err(error) => return fail(ctx, "implementation_planning", error),
        };
        let mut process_metadata =
            ctx.workflow_process_metadata("in-progress", "WorkflowImplementation");
        process_metadata.insert("implementation_phase".to_string(), json!("implement"));
        let implementation_operation_id = match active_implementation_operation_id(ctx) {
            Ok(operation_id) => operation_id,
            Err(error) => return fail(ctx, "implementation_planning", error),
        };
        process_metadata.insert(
            "operation_id".to_string(),
            json!(implementation_operation_id),
        );
        process_metadata.insert(
            "worktree".to_string(),
            json!({"path": worktree_path, "branch": branch}),
        );
        let implementation_started_at = now_timestamp();
        let agent_result = match run_goal_agent_with_settlement(
            GoalAgentLaunch {
                runtime_root: ctx.runtime_root.to_path_buf(),
                cwd: agent_cwd.clone(),
                provider: ctx.provider.clone(),
                prompt,
                metadata: process_metadata,
            },
            |attention| {
                let _ = ctx.log(
                    "agent",
                    "Goal Agent is waiting for user input",
                    Some(json_object(json!({
                        "provider": ctx.provider,
                        "message": attention.message,
                        "branch": branch,
                        "worktree": worktree_path
                    }))),
                );
            },
            |settlement| record_current_process_settlement(ctx, settlement),
        ) {
            Ok(result) => result,
            Err(error) => {
                let failure = fail_implementation_phase(ctx, "provider", &error);
                return fail(ctx, "agent", failure);
            }
        };
        let provider_output = agent_result.output;
        if let Err(error) = complete_implementation_planning(
            ctx,
            implementation_started_at,
            agent_result.process_id.clone(),
            agent_result.session_id.clone(),
            provider_output.clone(),
            agent_result.implementation_evidence.clone(),
        ) {
            return fail(ctx, "implementation_planning", error);
        }
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

        let worktree_git =
            FileGitWorktreeService::with_runtime_root(&worktree_path, ctx.runtime_root);
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
        let changed_paths = match worktree_git.changed_paths_since(&target_branch, &commit.commit) {
            Ok(paths) => paths,
            Err(error) => return fail(ctx, "guidance", error),
        };
        let code_changed = changed_paths.iter().any(|path| is_code_path(path));
        let guidance_decision = match guidance_decision(
            &agent_context,
            agent_result.guidance_applied.as_deref(),
            code_changed,
        ) {
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

        let governance =
            match evaluate_workflow_governance(ctx, &worktree_path, &agent_cwd, &agent_context) {
                Ok(evaluation) => evaluation,
                Err(error) => return fail(ctx, "governance", error),
            };
        record_governance(ctx, &governance)?;
        if governance.failed {
            let error = RefineError::Conflict(
                governance
                    .message
                    .clone()
                    .unwrap_or_else(|| "governance checks failed".to_string()),
            );
            return fail(ctx, "governance", error);
        }

        let remote = match ctx.git_remote() {
            Ok(remote) => remote,
            Err(error) => return fail(ctx, "git", error),
        };
        if worktree_git.remote_exists(&remote)? {
            if let Err(error) =
                with_repository_git_lock(ctx.target_root, || worktree_git.push(&remote, &branch))
            {
                return fail(ctx, "git", error);
            }
            ctx.log(
                "git",
                &format!("Published implementation candidate {branch}"),
                Some(json_object(json!({
                    "branch": branch,
                    "remote": remote,
                    "commit": commit.commit
                }))),
            )?;
        }

        ctx.agent_cwd = Some(agent_cwd);
        ctx.provider_output = Some(provider_output);
        ctx.implementation_changed = commit.has_changes_since_base;
        ctx.commit = Some(commit.commit);
        let next = match ctx.quality_timing(GoalStatus::InProgress) {
            Ok(timing) if timing == POST_BUILD => GoalStatus::ReadyMerge,
            Ok(_) => GoalStatus::Qa,
            Err(error) => return fail(ctx, "quality", error),
        };
        ctx.request_transition(GoalStatus::InProgress, next.clone())?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::InProgress,
            to: next,
            reason: "Implementation completed".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowQa {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Qa
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let quality = match run_workflow_quality(ctx) {
            Ok(result) => result,
            Err(error) if ctx.reconciliation.is_some() => {
                let _ = ctx.log(
                    "reconcile",
                    "Already-merged reconciliation Quality could not run; target and Goal status were preserved",
                    Some(json_object(json!({"error": error.to_string()}))),
                );
                return Err(error);
            }
            Err(error) => {
                let category = if is_quality_harness_fault(&error) {
                    "quality_harness"
                } else {
                    "quality"
                };
                return fail(
                    ctx,
                    category,
                    RefineError::Conflict(quality_error_summary(&error)),
                );
            }
        };
        if let Some(integration) = ctx.reconciliation.clone() {
            if quality.ok {
                ctx.work_items.update_goal_round_evaluation_summary(
                    &ctx.goal_id,
                    ctx.round_idx,
                    &json!({
                        "workflow_reconciliation": {
                            "state": "completed",
                            "candidate_commit": integration.candidate_commit,
                            "target_branch": integration.target_branch,
                            "quality_target_commit": quality.candidate_commit,
                            "completed_at": now_timestamp()
                        }
                    }),
                )?;
                ctx.log(
                    "reconcile",
                    "Already-merged candidate passed Quality; Goal reconciled as done",
                    Some(json_object(json!({
                        "candidate_commit": integration.candidate_commit,
                        "target_branch": integration.target_branch,
                        "quality_target_commit": quality.candidate_commit
                    }))),
                )?;
                ctx.reconciliation_state = Some("completed".to_string());
                ctx.request_transition(GoalStatus::Qa, GoalStatus::Done)?;
                return Ok(WorkflowAdvanceOutcome::Transition {
                    from: GoalStatus::Qa,
                    to: GoalStatus::Done,
                    reason: "Already-merged candidate passed reconciliation Quality".to_string(),
                });
            }

            let merger = FileMergerService::with_target_root(
                ctx.runtime_root,
                ctx.refine_dir(),
                ctx.target_root,
            );
            let reconciliation_failure = RefineError::Conflict(format!(
                "{} The already-merged candidate was reverted and complete evidence was preserved.",
                quality.summary
            ));
            let goal_id = ctx.goal_id.clone();
            let round_idx = ctx.round_idx;
            let claim_id = ctx.claim_id.clone();
            let execution_id = ctx.execution_id.clone();
            let node_id = ctx.node_id.clone();
            let reverted = match merger.revert_reconciled_candidate_and_settle(
                ReconciliationRequest {
                    goal_id: &goal_id,
                    round_idx,
                    claim_id: &claim_id,
                    execution_id: &execution_id,
                    node_id: &node_id,
                    integration: &integration,
                    expected_target_commit: &quality.candidate_commit,
                },
                |reverted| {
                    ctx.work_items.update_goal_round_evaluation_summary(
                        &ctx.goal_id,
                        ctx.round_idx,
                        &json!({
                            "workflow_reconciliation": {
                                "state": "reverted",
                                "candidate_commit": integration.candidate_commit,
                                "target_branch": integration.target_branch,
                                "quality_target_commit": quality.candidate_commit,
                                "merge_commit": reverted.merge_commit,
                                "revert_commit": reverted.revert_commit,
                                "revert": reverted.result,
                                "completed_at": now_timestamp()
                            }
                        }),
                    )?;
                    ctx.log(
                        "reconcile",
                        "Already-merged candidate failed Quality and was reverted from the target branch",
                        Some(json_object(json!({
                            "candidate_commit": integration.candidate_commit,
                            "target_branch": integration.target_branch,
                            "quality_target_commit": quality.candidate_commit,
                            "merge_commit": reverted.merge_commit,
                            "revert_commit": reverted.revert_commit,
                            "revert": reverted.result
                        }))),
                    )?;
                    ctx.reconciliation_state = Some("reverted".to_string());
                    ctx.fail("quality", &reconciliation_failure)
                },
            ) {
                Ok((reverted, ())) => reverted,
                Err(error) => {
                    let goal_status = ctx
                        .work_items
                        .show_goal_summary(&ctx.goal_id)
                        .ok()
                        .map(|summary| summary.goal.status);
                    if goal_status == Some(GoalStatus::Failed) {
                        return Err(reconciliation_failure);
                    }
                    let _ = ctx.work_items.update_goal_round_evaluation_summary(
                        &ctx.goal_id,
                        ctx.round_idx,
                        &json!({
                            "workflow_reconciliation": {
                                "state": "revert_blocked",
                                "candidate_commit": integration.candidate_commit,
                                "target_branch": integration.target_branch,
                                "quality_target_commit": quality.candidate_commit,
                                "error": error.to_string(),
                                "updated_at": now_timestamp()
                            }
                        }),
                    );
                    let _ = ctx.log(
                        "reconcile",
                        "Quality failed but the exact safe revert was blocked; target and Goal status were preserved",
                        Some(json_object(json!({
                            "candidate_commit": integration.candidate_commit,
                            "target_branch": integration.target_branch,
                            "quality_target_commit": quality.candidate_commit,
                            "error": error.to_string()
                        }))),
                    );
                    return Err(error);
                }
            };
            let _ = reverted;
            return Err(reconciliation_failure);
        }
        if !quality.ok {
            return fail(ctx, "quality", RefineError::Conflict(quality.summary));
        }
        let next = if ctx.quality_timing(GoalStatus::Qa)? == POST_BUILD {
            GoalStatus::Review
        } else {
            GoalStatus::ReadyMerge
        };
        ctx.request_transition(GoalStatus::Qa, next.clone())?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Qa,
            to: next,
            reason: "Quality checks passed".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowReadyMerge {
    fn observes(&self) -> GoalStatus {
        GoalStatus::ReadyMerge
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let branch = ctx.require_branch()?.to_string();
        let commit = ctx.require_commit()?.to_string();
        let remote = ctx.git_remote()?;
        let timing = ctx.quality_timing(GoalStatus::ReadyMerge)?;
        let next = GoalStatus::Build;
        let merger = FileMergerService::with_target_root(
            ctx.runtime_root,
            ctx.refine_dir(),
            ctx.target_root,
        );
        let goal_id = ctx.goal_id.clone();
        let claim_id = ctx.claim_id.clone();
        let execution_id = ctx.execution_id.clone();
        let node_id = ctx.node_id.clone();
        let round_idx = ctx.round_idx;
        let integration = match merger.integrate_workflow_candidate_and_settle(
            &goal_id,
            round_idx,
            &claim_id,
            &execution_id,
            &node_id,
            &branch,
            &commit,
            &remote,
            |integration| {
                ctx.log(
                    "merge",
                    &format!("Ready Merge integrated implementation candidate {branch}"),
                    Some(json_object(json!({
                        "branch": branch,
                        "quality_timing": timing,
                        "integration": integration
                    }))),
                )?;
                ctx.request_transition(GoalStatus::ReadyMerge, next.clone())
            },
        ) {
            Ok((integration, _)) => integration,
            Err(RefineError::StaleCandidate {
                candidate_commit,
                recorded_base,
                target_branch,
                target_commit,
            }) => {
                let stale_error = RefineError::StaleCandidate {
                    candidate_commit: candidate_commit.clone(),
                    recorded_base: recorded_base.clone(),
                    target_branch: target_branch.clone(),
                    target_commit: target_commit.clone(),
                };
                if let Err(recovery_error) = ctx.work_items.queue_stale_candidate_recovery_summary(
                    &ctx.goal_id,
                    ctx.round_idx,
                    &candidate_commit,
                    &recorded_base,
                    &target_branch,
                    &target_commit,
                    &stale_error.to_string(),
                ) {
                    return fail(
                        ctx,
                        "merge",
                        RefineError::Conflict(format!(
                            "{stale_error}; automatic fresh-round recovery failed: {recovery_error}"
                        )),
                    );
                }
                ctx.log(
                    "merge",
                    "Ready Merge rejected a stale candidate and queued a fresh recovery round",
                    Some(json_object(json!({
                        "candidate_commit": candidate_commit,
                        "recorded_base": recorded_base,
                        "target_branch": target_branch,
                        "target_commit": target_commit,
                        "successor_round": ctx.round_idx + 2,
                        "error": stale_error.to_string()
                    }))),
                )?;
                ctx.final_status = Some(GoalStatus::Todo);
                return Ok(WorkflowAdvanceOutcome::Completed {
                    final_status: GoalStatus::Todo,
                    reason:
                        "Stale candidate preserved; fresh recovery round queued from current target"
                            .to_string(),
                });
            }
            Err(error)
                if error.to_string().contains("Ready Merge execution")
                    && error.to_string().contains("was cancelled") =>
            {
                return Err(error);
            }
            Err(error) => return fail(ctx, "merge", error),
        };
        ctx.merge = Some(integration.merge);
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::ReadyMerge,
            to: next,
            reason: "Ready Merge integrated the implementation candidate".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowBuild {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Build
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        if let Err(error) = verify_integrated_target_checkout(ctx, "before build") {
            return fail(ctx, "build", error);
        }
        let target_app =
            FileTargetAppService::new(ctx.refine_dir(), ctx.runtime_root, ctx.target_root);
        let build = match target_app
            .build_with_metadata(ctx.workflow_process_metadata("build", "WorkflowBuild"))
        {
            Ok(snapshot) if snapshot.ok => snapshot,
            Ok(snapshot) => {
                let error = RefineError::Conflict(snapshot.message.clone());
                ctx.log(
                    "build",
                    "Target app build failed",
                    Some(json_object(json!({"target_app": &snapshot}))),
                )?;
                return fail(ctx, "build", error);
            }
            Err(error) => return fail(ctx, "build", error),
        };
        if let Err(error) = verify_integrated_target_checkout(ctx, "after build") {
            return fail(ctx, "build", error);
        }
        let skipped = build.last_operation.is_none();
        ctx.log(
            "build",
            if skipped {
                "Target app rebuild skipped"
            } else {
                "Target app rebuild passed"
            },
            Some(json_object(json!({
                "target_app": &build,
                "skipped": skipped,
                "checkout": ctx.target_root.display().to_string()
            }))),
        )?;
        let next = if ctx.quality_timing(GoalStatus::Build)? == POST_BUILD {
            GoalStatus::Qa
        } else {
            GoalStatus::Review
        };
        ctx.request_transition(GoalStatus::Build, next.clone())?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Build,
            to: next,
            reason: if skipped {
                "Target app rebuild was not configured".to_string()
            } else {
                "Target app rebuild passed".to_string()
            },
        })
    }
}

fn verify_integrated_target_checkout(ctx: &WorkflowContext<'_>, phase: &str) -> RefineResult<()> {
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let expected_commit = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .and_then(|round| round.get("workflow_integration"))
        .and_then(|integration| integration.get("target_commit"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|commit| !commit.is_empty())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} has no integrated target commit for {phase}",
                ctx.goal_id
            ))
        })?;
    let git = FileGitWorktreeService::new(ctx.target_root);
    let head = git.head_ref()?;
    if head.commit.as_deref() != Some(expected_commit) {
        return Err(RefineError::Conflict(format!(
            "integrated-target {phase} found HEAD {}, expected {expected_commit}; checkout was preserved",
            head.commit.as_deref().unwrap_or("<unborn>")
        )));
    }
    let status = git.inspect("")?;
    if status.dirty_user_changes {
        return Err(RefineError::Conflict(format!(
            "integrated-target {phase} found a dirty index or worktree at {}; residue remains attributed to Goal {}",
            ctx.target_root.display(),
            ctx.goal_id
        )));
    }
    Ok(())
}

impl WorkflowBehavior for WorkflowReview {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Review
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        ctx.final_status = Some(GoalStatus::Review);
        Ok(WorkflowAdvanceOutcome::Completed {
            final_status: GoalStatus::Review,
            reason: "Workflow reached review".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowDone {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Done
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        ctx.final_status = Some(GoalStatus::Done);
        Ok(WorkflowAdvanceOutcome::Completed {
            final_status: GoalStatus::Done,
            reason: "Workflow already done".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowFailed {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Failed
    }

    fn advance(&self, _ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        Ok(WorkflowAdvanceOutcome::Failed {
            reason: "Workflow is failed".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowCancelled {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Cancelled
    }

    fn advance(&self, _ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        Ok(WorkflowAdvanceOutcome::Blocked {
            reason: "Workflow is cancelled".to_string(),
        })
    }
}

fn run_workflow_quality(ctx: &WorkflowContext<'_>) -> RefineResult<QualityCheckResult> {
    QualityOperationRunner::new(ctx.refine_dir(), ctx.runtime_root, ctx.target_root)
        .run_goal_checks(
            &ctx.goal_id,
            &ctx.provider,
            ctx.workflow_process_metadata("qa", "WorkflowQa"),
        )
        .map(|operation| operation.result)
}

fn ensure_goal_agent_context(ctx: &WorkflowContext<'_>, goal: &Value) -> RefineResult<Value> {
    let round = goal
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .ok_or_else(|| {
            RefineError::NotFound(format!(
                "Goal {} has no round {}",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    if let Some(context) = round.get("agent_context").filter(|context| {
        context.get("version").and_then(Value::as_u64) == Some(1)
            && context.get("goal").is_some()
            && context.get("previous_rounds").is_some()
            && context.get("current_round").is_some()
    }) {
        return Ok(context.clone());
    }

    let governance = FileGovernanceService::new(ctx.refine_dir()).load()?;
    let guidance = FileGuidanceService::new(ctx.refine_dir()).list()?;
    let context = goal_agent_context(&governance, &guidance, goal, ctx.round_idx)?;
    ctx.work_items.update_latest_goal_round_evaluation_summary(
        &ctx.goal_id,
        &json!({"agent_context": context}),
    )?;
    Ok(context)
}

fn goal_agent_context(
    governance: &Value,
    guidance: &Value,
    goal: &Value,
    round_idx: usize,
) -> RefineResult<Value> {
    let rounds = goal
        .get("rounds")
        .and_then(Value::as_array)
        .ok_or_else(|| RefineError::Serialization("Goal rounds must be an array".to_string()))?;
    let current_round = rounds
        .get(round_idx)
        .filter(|round| round.is_object())
        .ok_or_else(|| RefineError::NotFound(format!("Goal has no round {}", round_idx + 1)))?;
    let goal_context = selected_agent_context(
        goal,
        &[
            "id",
            "name",
            "priority",
            "reporter",
            "assignee",
            "feature_id",
            "feature_order",
            "node_id",
            "notes",
        ],
    );
    let previous_rounds = rounds[..round_idx]
        .iter()
        .enumerate()
        .filter(|(_, round)| round.is_object())
        .map(|(index, round)| round_agent_context(round, index))
        .collect::<Vec<_>>();
    let guidance_candidates = guidance
        .get("guidance")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("enabled").and_then(Value::as_bool) != Some(false))
        .cloned()
        .collect::<Vec<_>>();
    Ok(json!({
        "version": 1,
        "assembled_at": now_timestamp(),
        "governance": {
            "product": governance.get("product").cloned().unwrap_or(Value::String(String::new())),
            "constitution": governance.get("constitution").cloned().unwrap_or(Value::String(String::new())),
            "rules": governance.get("rules").cloned().unwrap_or_else(|| json!([])),
            "configured": governance.get("configured").cloned().unwrap_or(Value::Bool(false)),
        },
        "workflow_summary": PromptEngine::load(PromptTemplate::GoalAgentWorkflowSummary),
        "guidance_candidates": guidance_candidates,
        "goal": goal_context,
        "previous_rounds": previous_rounds,
        "current_round": round_agent_context(current_round, round_idx),
    }))
}

const CODE_FILE_GUIDANCE_RULE: &str =
    "Apply whenever an agent adds or changes code files in any language.";

fn guidance_decision(
    agent_context: &Value,
    applied: Option<&[usize]>,
    code_changed: bool,
) -> RefineResult<Value> {
    let candidates = agent_context
        .get("guidance_candidates")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if candidates.is_empty() {
        if applied.is_some_and(|indexes| !indexes.is_empty()) {
            return Err(RefineError::InvalidInput(
                "Goal Agent selected guidance when no candidates were provided".to_string(),
            ));
        }
        return Ok(json!({
            "context_version": 1,
            "applied": [],
            "skipped": [],
            "recorded_at": now_timestamp(),
        }));
    }
    let applied = applied.ok_or_else(|| {
        RefineError::InvalidInput(
            "Goal Agent completion must include guidance_applied when guidance candidates exist"
                .to_string(),
        )
    })?;
    let unique = applied
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != applied.len() || unique.iter().any(|index| *index >= candidates.len()) {
        return Err(RefineError::InvalidInput(
            "Goal Agent guidance_applied must contain unique in-range candidate indexes"
                .to_string(),
        ));
    }
    let required_code_guidance = candidates
        .iter()
        .enumerate()
        .find_map(|(index, candidate)| {
            (candidate.get("rule").and_then(Value::as_str) == Some(CODE_FILE_GUIDANCE_RULE))
                .then_some(index)
        });
    if code_changed && required_code_guidance.is_some_and(|index| !unique.contains(&index)) {
        return Err(RefineError::InvalidInput(
            "Goal Agent must apply enabled Guidance whose Rule requires it for changed code files"
                .to_string(),
        ));
    }
    let applied_candidates = candidates
        .iter()
        .enumerate()
        .filter(|(index, _)| unique.contains(index))
        .map(|(_, candidate)| candidate.clone())
        .collect::<Vec<_>>();
    let skipped_candidates = candidates
        .iter()
        .enumerate()
        .filter(|(index, _)| !unique.contains(index))
        .map(|(_, candidate)| candidate.clone())
        .collect::<Vec<_>>();
    Ok(json!({
        "context_version": 1,
        "applied": applied_candidates,
        "skipped": skipped_candidates,
        "code_files_changed": code_changed,
        "recorded_at": now_timestamp(),
    }))
}

fn is_code_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase());
    !matches!(
        extension.as_deref(),
        Some(
            "adoc"
                | "avif"
                | "bmp"
                | "gif"
                | "ico"
                | "jpeg"
                | "jpg"
                | "md"
                | "mdx"
                | "pdf"
                | "png"
                | "rst"
                | "svg"
                | "txt"
                | "webp"
        )
    )
}

fn evaluate_workflow_governance(
    ctx: &WorkflowContext<'_>,
    worktree_path: &str,
    provider_cwd: &std::path::Path,
    agent_context: &Value,
) -> RefineResult<GovernanceEvaluation> {
    let governance = agent_context.get("governance").cloned().ok_or_else(|| {
        RefineError::Serialization(format!(
            "Goal {} round {} has no pinned governance context",
            ctx.goal_id,
            ctx.round_idx + 1
        ))
    })?;
    let rules = governance
        .get("rules")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if rules.is_empty() {
        return Ok(GovernanceEvaluation {
            failed: false,
            message: None,
            details: json_object(json!({
                "phase": "post_implementation",
                "configured": false,
                "governance_configured": governance.get("configured").and_then(Value::as_bool).unwrap_or(false),
                "rules_checked": 0,
                "failed_actions": []
            })),
        });
    }
    let prompt = post_implementation_governance_prompt(
        &governance,
        &rules,
        worktree_path,
        provider_cwd,
        &ctx.goal_id,
        ctx.round_idx,
    );
    let provider = HostAgentProviderService::with_runtime_root(ctx.runtime_root.join("agents"));
    let output = provider.invoke(ProviderInvocation {
        provider: ctx.provider.clone(),
        prompt,
        session_id: None,
        cwd: Some(provider_cwd.display().to_string()),
        process_metadata: ctx
            .workflow_process_metadata("in-progress", "WorkflowImplementationGovernance"),
    })?;
    let mut evaluation = parse_governance_provider_output(&output, rules.len());
    evaluation
        .details
        .insert("provider".to_string(), Value::String(ctx.provider.clone()));
    evaluation.details.insert(
        "worktree".to_string(),
        Value::String(worktree_path.to_string()),
    );
    evaluation.details.insert(
        "cwd".to_string(),
        Value::String(provider_cwd.display().to_string()),
    );
    evaluation.details.insert(
        "governance_configured".to_string(),
        governance
            .get("configured")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .into(),
    );
    Ok(GovernanceEvaluation {
        details: evaluation.details,
        ..evaluation
    })
}

fn record_governance(
    ctx: &WorkflowContext<'_>,
    evaluation: &GovernanceEvaluation,
) -> RefineResult<()> {
    let message = evaluation.message.clone().unwrap_or_else(|| {
        if evaluation.details["configured"].as_bool() == Some(true) {
            "Governance checks passed.".to_string()
        } else {
            "No governance rules configured.".to_string()
        }
    });
    ctx.work_items.update_latest_goal_round_evaluation_summary(
        &ctx.goal_id,
        &json!({
            "rule_state": if evaluation.failed { "failed" } else { "passed" },
            "meta_rule_state": "passed",
            "product_state": "passed",
            "constitution_state": "passed",
            "governance_message": message,
            "governance_details": evaluation.details,
            "governance_checked_at": now_timestamp(),
            "governance_rule_actions": evaluation.details
                .get("failed_actions")
                .cloned()
                .unwrap_or_else(|| json!([]))
        }),
    )?;
    ctx.log(
        "governance",
        if evaluation.failed {
            "Governance checks failed"
        } else {
            "Governance checks passed"
        },
        Some(evaluation.details.clone()),
    )
}

fn fail<T>(ctx: &WorkflowContext<'_>, category: &str, error: RefineError) -> RefineResult<T> {
    let _ = ctx.fail(category, &error);
    Err(error)
}

#[cfg(test)]
mod tests;
