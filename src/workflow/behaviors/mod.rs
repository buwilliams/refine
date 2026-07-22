use serde_json::{Value, json};
use std::path::Path;

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::FileGovernanceService;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::host::quality::{
    FileQualityService, POST_BUILD, QualityCheckResult, QualityOperationRunner,
};
use crate::tools::host::target_apps::FileTargetAppService;
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::context::WorkflowContext;
use crate::workflow::{
    GovernanceEvaluation, agent_worktree_cwd, goal_agent_prompt, implementation_branch_name,
    json_object, now_timestamp, parse_governance_provider_output,
    post_implementation_governance_prompt, setting_string,
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
        let branch = implementation_branch_name(
            setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}").as_str(),
            &ctx.goal_id,
            ctx.round_idx,
        );
        let app_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
        let target_branch = setting_string(&ctx.settings, "merge_target_branch", "main");
        let base_commit = match app_git.resolve_commit(&target_branch) {
            Ok(commit) => commit,
            Err(error) => return fail(ctx, "branch", error),
        };
        ctx.request_transition(GoalStatus::Todo, GoalStatus::InProgress)?;
        let worktree_target = match app_git.git_path("refine-worktrees") {
            Ok(root) => root.join(branch.replace('/', "-")),
            Err(error) => return fail(ctx, "branch", error),
        };
        let worktree_path = match with_repository_git_lock(ctx.target_root, || {
            app_git.ensure_worktree(&branch, &worktree_target)
        }) {
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
        let prompt = match goal_agent_prompt(&ctx.goal_id, &goal, ctx.round_idx) {
            Ok(prompt) => prompt,
            Err(error) => return fail(ctx, "agent", error),
        };
        let agent_cwd = match agent_worktree_cwd(
            &worktree_path,
            setting_string(&ctx.settings, "agent_subpath", "").as_str(),
        ) {
            Ok(cwd) => cwd,
            Err(error) => return fail(ctx, "agent", error),
        };
        let provider = HostAgentProviderService::with_runtime_root(ctx.runtime_root.join("agents"));
        let provider_output = match provider.invoke(ProviderInvocation {
            provider: ctx.provider.clone(),
            prompt,
            session_id: None,
            cwd: Some(agent_cwd.display().to_string()),
            process_metadata: ctx
                .workflow_process_metadata("in-progress", "WorkflowImplementation"),
        }) {
            Ok(output) => output,
            Err(error) => return fail(ctx, "agent", error),
        };
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

        let governance = match evaluate_workflow_governance(ctx, &worktree_path, &agent_cwd) {
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

        let remote = setting_string(&ctx.settings, "git_remote", "origin");
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
        ctx.request_transition(GoalStatus::InProgress, GoalStatus::ReadyMerge)?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::InProgress,
            to: GoalStatus::ReadyMerge,
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
            Err(error) => return fail(ctx, "quality", error),
        };
        if !quality.ok {
            return fail(
                ctx,
                "quality",
                RefineError::Conflict(
                    "quality checks failed; the isolated candidate was preserved for recovery"
                        .to_string(),
                ),
            );
        }
        let next = if quality_timing(ctx)? == POST_BUILD {
            GoalStatus::Review
        } else {
            GoalStatus::Build
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
        ctx.log(
            "review",
            &format!("Prepared implementation candidate {branch} for validation"),
            Some(json_object(json!({"branch": branch}))),
        )?;
        let next = if quality_timing(ctx)? == POST_BUILD {
            GoalStatus::Build
        } else {
            GoalStatus::Qa
        };
        ctx.request_transition(GoalStatus::ReadyMerge, next.clone())?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::ReadyMerge,
            to: next,
            reason: "Implementation candidate is ready for validation".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowBuild {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Build
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let candidate_root = Path::new(ctx.require_worktree_path()?);
        let target_app =
            FileTargetAppService::new(ctx.refine_dir(), ctx.runtime_root, candidate_root);
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
        ctx.log(
            "build",
            "Target app build passed",
            Some(json_object(json!({"target_app": &build}))),
        )?;
        let next = if quality_timing(ctx)? == POST_BUILD {
            GoalStatus::Qa
        } else {
            GoalStatus::Review
        };
        ctx.request_transition(GoalStatus::Build, next.clone())?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Build,
            to: next,
            reason: "Target app build passed".to_string(),
        })
    }
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

fn quality_timing(ctx: &WorkflowContext<'_>) -> RefineResult<String> {
    FileQualityService::new(ctx.refine_dir())
        .load_settings()
        .map(|settings| settings.timing)
}

fn evaluate_workflow_governance(
    ctx: &WorkflowContext<'_>,
    worktree_path: &str,
    provider_cwd: &std::path::Path,
) -> RefineResult<GovernanceEvaluation> {
    let governance = FileGovernanceService::new(ctx.refine_dir()).load()?;
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
