use serde_json::{Value, json};

use crate::model::workflow::GapStatus;
use crate::process::supervisor::config::FileGovernanceService;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::host::quality::{
    FileQualityService, QualityCheckRequest, QualityCheckResult, QualityService,
};
use crate::tools::host::target_apps::FileTargetAppService;
use crate::tools::product::merging::FileMergerService;
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::context::WorkflowContext;
use crate::workflow::{
    GovernanceEvaluation, agent_worktree_cwd, gap_agent_prompt, implementation_branch_name,
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
    fn observes(&self) -> GapStatus {
        GapStatus::Backlog
    }

    fn advance(&self, _ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        Ok(WorkflowAdvanceOutcome::Blocked {
            reason: "backlog Gaps wait until todo eligibility rules promote them".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowTodo {
    fn observes(&self) -> GapStatus {
        GapStatus::Todo
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let branch = implementation_branch_name(
            setting_string(&ctx.settings, "branch_name_pattern", "refine/{gap_id}").as_str(),
            &ctx.gap_id,
            ctx.round_idx,
        );
        let app_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
        ctx.request_transition(GapStatus::Todo, GapStatus::InProgress)?;
        let worktree_path = match app_git.worktree(&branch) {
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
        if let Err(error) = ctx
            .work_items
            .update_gap_branch_name(&ctx.gap_id, Some(&branch))
        {
            return fail(ctx, "branch", error);
        }
        ctx.branch = Some(branch);
        ctx.worktree_path = Some(worktree_path);
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GapStatus::Todo,
            to: GapStatus::InProgress,
            reason: "Gap entered implementation".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowImplementation {
    fn observes(&self) -> GapStatus {
        GapStatus::InProgress
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let branch = ctx.require_branch()?.to_string();
        let worktree_path = ctx.require_worktree_path()?.to_string();
        let prompt = gap_agent_prompt(&ctx.gap_id);
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
        ctx.log(
            "agent",
            "Gap agent completed",
            Some(json_object(json!({
                "provider": ctx.provider,
                "output": provider_output,
                "branch": branch,
                "worktree": worktree_path
            }))),
        )?;

        let worktree_git =
            FileGitWorktreeService::with_runtime_root(&worktree_path, ctx.runtime_root);
        let commit = match worktree_git.commit(
            &format!("Implement {} round {}", ctx.gap_id, ctx.round_idx + 1),
            &[],
        ) {
            Ok(commit) => commit,
            Err(error) => return fail(ctx, "commit", error),
        };
        ctx.log(
            "git",
            &format!("Committed implementation branch {branch}"),
            Some(json_object(json!({
                "branch": branch,
                "commit": commit,
                "worktree": worktree_path
            }))),
        )?;

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

        ctx.agent_cwd = Some(agent_cwd);
        ctx.provider_output = Some(provider_output);
        ctx.commit = Some(commit);
        ctx.request_transition(GapStatus::InProgress, GapStatus::Qa)?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GapStatus::InProgress,
            to: GapStatus::Qa,
            reason: "Implementation completed".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowQa {
    fn observes(&self) -> GapStatus {
        GapStatus::Qa
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let quality = match run_workflow_quality(ctx) {
            Ok(result) => result,
            Err(error) => {
                record_quality_error(ctx, &error)?;
                return fail(ctx, "quality", error);
            }
        };
        record_quality(ctx, &quality)?;
        if !quality.ok {
            return fail(
                ctx,
                "quality",
                RefineError::Conflict("quality checks failed".to_string()),
            );
        }
        ctx.request_transition(GapStatus::Qa, GapStatus::ReadyMerge)?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GapStatus::Qa,
            to: GapStatus::ReadyMerge,
            reason: "Quality checks passed".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowReadyMerge {
    fn observes(&self) -> GapStatus {
        GapStatus::ReadyMerge
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let branch = ctx.require_branch()?.to_string();
        let commit = ctx.require_commit()?.to_string();
        let merger = FileMergerService::new(ctx.runtime_root, ctx.refine_dir());
        let merge = match merger.merge_branch_for_workflow(&branch) {
            Ok(result) if result.ok => result,
            Ok(result) => {
                let error = RefineError::Conflict(
                    result
                        .message
                        .clone()
                        .unwrap_or_else(|| "implementation merge failed".to_string()),
                );
                ctx.log(
                    "merge",
                    "Implementation merge failed",
                    Some(json_object(json!({"branch": branch, "merge": &result}))),
                )?;
                return fail(ctx, "merge", error);
            }
            Err(error) => return fail(ctx, "merge", error),
        };
        ctx.log(
            "merge",
            &format!("Merged implementation branch {branch}"),
            Some(json_object(json!({
                "branch": branch,
                "commit": commit,
                "merge": &merge
            }))),
        )?;
        ctx.merge = Some(merge);
        ctx.request_transition(GapStatus::ReadyMerge, GapStatus::Build)?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GapStatus::ReadyMerge,
            to: GapStatus::Build,
            reason: "Implementation branch merged".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowBuild {
    fn observes(&self) -> GapStatus {
        GapStatus::Build
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
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
        ctx.log(
            "build",
            "Target app build passed",
            Some(json_object(json!({"target_app": &build}))),
        )?;
        ctx.request_transition(GapStatus::Build, GapStatus::Review)?;
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GapStatus::Build,
            to: GapStatus::Review,
            reason: "Target app build passed".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowReview {
    fn observes(&self) -> GapStatus {
        GapStatus::Review
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        ctx.final_status = Some(GapStatus::Review);
        Ok(WorkflowAdvanceOutcome::Completed {
            final_status: GapStatus::Review,
            reason: "Workflow reached review".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowDone {
    fn observes(&self) -> GapStatus {
        GapStatus::Done
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        ctx.final_status = Some(GapStatus::Done);
        Ok(WorkflowAdvanceOutcome::Completed {
            final_status: GapStatus::Done,
            reason: "Workflow already done".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowFailed {
    fn observes(&self) -> GapStatus {
        GapStatus::Failed
    }

    fn advance(&self, _ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        Ok(WorkflowAdvanceOutcome::Failed {
            reason: "Workflow is failed".to_string(),
        })
    }
}

impl WorkflowBehavior for WorkflowCancelled {
    fn observes(&self) -> GapStatus {
        GapStatus::Cancelled
    }

    fn advance(&self, _ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        Ok(WorkflowAdvanceOutcome::Blocked {
            reason: "Workflow is cancelled".to_string(),
        })
    }
}

fn run_workflow_quality(ctx: &WorkflowContext<'_>) -> RefineResult<QualityCheckResult> {
    if setting_string(&ctx.settings, "quality_enabled", "0") != "1" {
        return Ok(QualityCheckResult {
            owner_id: ctx.gap_id.clone(),
            ok: true,
            diagnostics: vec!["Quality checks disabled.".to_string()],
        });
    }
    let service = FileQualityService::with_runtime_root(ctx.refine_dir(), ctx.runtime_root);
    let browser_required = setting_string(&ctx.settings, "quality_regressions_enabled", "0") == "1";
    service.run_checks(QualityCheckRequest {
        owner_id: ctx.gap_id.clone(),
        command: String::new(),
        browser_required,
        process_metadata: ctx.workflow_process_metadata("qa", "WorkflowQa"),
    })
}

fn record_quality(ctx: &WorkflowContext<'_>, result: &QualityCheckResult) -> RefineResult<()> {
    let message = if result.ok {
        "Quality checks passed"
    } else {
        "Quality checks failed"
    };
    ctx.work_items.update_latest_gap_round_evaluation_summary(
        &ctx.gap_id,
        &json!({
            "quality_state": if result.ok { "passed" } else { "failed" },
            "quality_message": message,
            "quality_details": {"diagnostics": result.diagnostics},
            "quality_checked_at": now_timestamp()
        }),
    )?;
    ctx.log(
        "quality",
        message,
        Some(json_object(json!({
            "ok": result.ok,
            "diagnostics": result.diagnostics
        }))),
    )
}

fn record_quality_error(ctx: &WorkflowContext<'_>, error: &RefineError) -> RefineResult<()> {
    ctx.work_items.update_latest_gap_round_evaluation_summary(
        &ctx.gap_id,
        &json!({
            "quality_state": "failed",
            "quality_message": "Quality checks failed.",
            "quality_details": {"error": error.to_string()},
            "quality_checked_at": now_timestamp()
        }),
    )?;
    Ok(())
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
        &ctx.gap_id,
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
    ctx.work_items.update_latest_gap_round_evaluation_summary(
        &ctx.gap_id,
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
