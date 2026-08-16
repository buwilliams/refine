use std::time::Duration;

use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::agent_sessions::{GoalAgentLaunch, run_goal_agent_with_settlement};
use crate::process::supervisor::config::{FileGovernanceService, FileGuidanceService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::prompts::{PromptEngine, PromptTemplate};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::host::quality::{
    QualityCheckResult, QualityOperationRunner, is_quality_harness_fault,
    is_quality_output_contract_fault, quality_error_summary,
};
use crate::tools::product::governance_integration::{
    AlreadyMergedResolutionDisposition, FileGovernanceIntegrationService,
};
use crate::tools::product::work_items::AlreadyMergedSettlement;
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::candidate_handoff::{
    find_candidate_handoff, record_candidate_handoff_commit, record_candidate_handoff_governance,
    register_candidate_handoff, retain_candidate_handoff_after_failure, settle_candidate_handoff,
};
use crate::workflow::context::WorkflowContext;
use crate::workflow::implementation_planning::begin_implementation_phase;
use crate::workflow::{
    CandidateRefreshOutcome, GovernanceEvaluation, QualityRecoveryInvestigation,
    agent_idle_timeout, agent_worktree_cwd, complete_implementation_planning,
    fail_implementation_phase, governed_implementation_prompt, implementation_branch_name,
    implementation_resume_session, json_object, now_timestamp, parse_governance_provider_output,
    parse_quality_recovery_provider_output, post_implementation_governance_prompt,
    quality_recovery_prompt, refresh_candidate_for_target_advancement, round_agent_context,
    run_governed_implementation_planning, selected_agent_context, setting_string, setting_usize,
};

#[derive(Clone, Debug, Default)]
pub struct WorkflowBacklog;

#[derive(Clone, Debug, Default)]
pub struct WorkflowTodo;

#[derive(Clone, Debug, Default)]
pub struct WorkflowPlan;

#[derive(Clone, Debug, Default)]
pub struct WorkflowImplementation;

#[derive(Clone, Debug, Default)]
pub struct WorkflowQuality;

#[derive(Clone, Debug, Default)]
pub struct WorkflowGovernance;

enum GovernanceIntegrationStep {
    Outcome(WorkflowAdvanceOutcome),
    Integrated {
        integration: crate::model::goal::RoundIntegration,
        transitioned: bool,
        candidate_commit: String,
    },
    /// The target tip moved between the refresh hold and the integrate hold;
    /// the pass must refresh again before it may merge.
    TargetAdvanced {
        expected_target: String,
    },
}

/// How many refresh → prove → integrate passes may observe the target
/// advancing before the race settles through a fresh recovery Round. The
/// integration lease already serializes other Goals, so only repository sync
/// or an operator can move the target between the two repository-lock holds.
const INTEGRATION_REFRESH_ATTEMPTS: u32 = 3;

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
        if let Some(outcome) = begin_scoped_recovery_round(ctx, &app_git)? {
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
        // selected this Goal using observed local capacity, and durable Goal state
        // must cross into Plan before Git may materialize a repository copy.
        ctx.request_transition(GoalStatus::Todo, GoalStatus::Plan)?;
        let (worktree_path, handoff) =
            match materialize_plan_worktree(ctx, &app_git, &branch, &base_commit) {
                Ok(materialized) => materialized,
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
        ctx.candidate_handoff_operation_id = Some(handoff.id);
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Todo,
            to: GoalStatus::Plan,
            reason: "Goal entered planning".to_string(),
        })
    }
}

/// The `automatic_retry` marker of a Round, when it is a recovery drafted from
/// Quality or Governance findings: `(kind, zero-based source round index)`.
///
/// Integration-race recoveries are excluded on purpose — their candidate
/// itself is stale, so they replay the full pipeline from a fresh base.
fn round_scoped_recovery_retry(round: &Value) -> Option<(String, usize)> {
    let retry = round.get("automatic_retry")?;
    let kind = retry.get("kind").and_then(Value::as_str)?;
    if !matches!(kind, "quality" | "governance") {
        return None;
    }
    let source_round = retry.get("source_round").and_then(Value::as_u64)?;
    (source_round >= 1).then(|| (kind.to_string(), source_round as usize - 1))
}

/// Continue a Quality/Governance recovery Round on the source Round's retained
/// candidate: same worktree (warm build caches), a fresh Round branch created
/// at the exact candidate commit, and no replanning — the drafted recovery
/// request becomes the implementation instruction, and the full Quality and
/// Governance gates still judge whatever the Round produces. Any precondition
/// that does not hold falls back to the ordinary fresh-worktree path.
fn begin_scoped_recovery_round(
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
    let Some((kind, _)) = round_scoped_recovery_retry(round) else {
        return Ok(None);
    };
    let recorded = |key: &str| {
        detail
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let (Some(source_branch), Some(candidate), Some(base_commit)) = (
        recorded("branch_name"),
        recorded("candidate_commit"),
        recorded("base_commit"),
    ) else {
        return Ok(None);
    };
    let Some(worktree_path) = app_git.existing_worktree_for_branch(&source_branch)? else {
        ctx.log(
            "git",
            "Scoped recovery fell back to a fresh worktree; the source Round worktree is gone",
            Some(json_object(json!({"source_branch": source_branch}))),
        )?;
        return Ok(None);
    };
    let worktree_path = worktree_path.display().to_string();
    let worktree_git = FileGitWorktreeService::with_runtime_root(&worktree_path, ctx.runtime_root);
    let retained_candidate_intact = |worktree_git: &FileGitWorktreeService| -> RefineResult<bool> {
        let head = worktree_git.head_ref()?;
        let status = worktree_git.inspect("")?;
        Ok(head.commit.as_deref() == Some(candidate.as_str()) && status.is_pristine())
    };
    if !retained_candidate_intact(&worktree_git)? {
        ctx.log(
            "git",
            "Scoped recovery fell back to a fresh worktree; the retained candidate checkout changed",
            Some(json_object(json!({
                "source_branch": source_branch,
                "candidate_commit": candidate
            }))),
        )?;
        return Ok(None);
    }
    let branch = implementation_branch_name(
        setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}").as_str(),
        &ctx.goal_id,
        ctx.round_idx,
    );
    let target_branch = setting_string(&ctx.settings, "merge_target_branch", "main");
    // Same invariant as the ordinary path: the durable Todo→Plan status write
    // lands before any Git mutation.
    ctx.request_transition(GoalStatus::Todo, GoalStatus::Plan)?;
    let handoff = match with_repository_git_lock(ctx.target_root, || {
        if !retained_candidate_intact(&worktree_git)? {
            return Err(RefineError::Conflict(format!(
                "Goal {} retained candidate changed before scoped recovery could begin",
                ctx.goal_id
            )));
        }
        // The recovery Round may be a reused slot whose previous attempt already
        // created this branch; reuse or reset it instead of demanding a fresh name.
        worktree_git.ensure_branch_at_head(&branch)?;
        register_candidate_handoff(
            ctx.runtime_root,
            ctx.target_root,
            &ctx.goal_id,
            ctx.round_idx,
            &ctx.node_id,
            &branch,
            &worktree_path,
            &base_commit,
        )
    }) {
        Ok(handoff) => handoff,
        Err(error) => return fail(ctx, "branch", error),
    };
    if let Err(error) = ctx.work_items.update_goal_git_refs(
        &ctx.goal_id,
        &branch,
        &target_branch,
        &base_commit,
        Some(&candidate),
    ) {
        return fail(ctx, "branch", error);
    }
    ctx.log(
        "git",
        &format!("Scoped {kind} recovery continues on the retained candidate worktree"),
        Some(json_object(json!({
            "branch": branch,
            "worktree": worktree_path,
            "candidate_commit": candidate,
            "source_branch": source_branch
        }))),
    )?;
    ctx.branch = Some(branch);
    ctx.worktree_path = Some(worktree_path);
    ctx.candidate_handoff_operation_id = Some(handoff.id);
    Ok(Some(WorkflowAdvanceOutcome::Transition {
        from: GoalStatus::Todo,
        to: GoalStatus::Plan,
        reason: "Scoped recovery Round entered planning on the retained candidate".to_string(),
    }))
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
                "Goal {} has invalid Governance evidence: {error}",
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
    if recorded_reconciliation_state.is_none() {
        ctx.log(
            "reconcile",
            "Admitted current-Round integration evidence for terminal already-merged resolution",
            Some(json_object(json!({
                "candidate_commit": candidate,
                "target_branch": integration.target_branch,
                "integration_target_commit": integration.target_commit
            }))),
        )?;
        return enter_already_merged_quality(ctx, &detail, round, integration, candidate);
    }
    let (target_commit, published_commit, candidate_present) = with_repository_git_lock(
        ctx.target_root,
        || -> RefineResult<(String, Option<String>, bool)> {
            let target_commit = app_git.resolve_commit(&integration.target_branch)?;
            if !app_git.commit_is_ancestor(candidate, &target_commit)? {
                return Ok((target_commit, None, false));
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
        let recovered = ctx
            .work_items
            .queue_missing_reconciled_candidate_recovery_summary(
                &ctx.goal_id,
                ctx.round_idx,
                Some(ctx.attempt_authority),
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
                // A reused inert trailing Round keeps its index, so the
                // successor is wherever the queue actually left it.
                "successor_round": recovered.goal.round_count
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
    enter_already_merged_quality(ctx, &detail, round, integration, candidate)
}

fn enter_already_merged_quality(
    ctx: &mut WorkflowContext<'_>,
    detail: &Value,
    round: &Value,
    integration: crate::model::goal::RoundIntegration,
    candidate: &str,
) -> RefineResult<Option<WorkflowAdvanceOutcome>> {
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
    ctx.start_status = GoalStatus::Quality;
    ctx.request_transition(GoalStatus::Todo, GoalStatus::Quality)?;
    Ok(Some(WorkflowAdvanceOutcome::Transition {
        from: GoalStatus::Todo,
        to: GoalStatus::Quality,
        reason: "Already-merged candidate requires terminal resolution".to_string(),
    }))
}

fn materialize_plan_worktree(
    ctx: &WorkflowContext<'_>,
    app_git: &FileGitWorktreeService,
    branch: &str,
    base_commit: &str,
) -> RefineResult<(
    String,
    crate::process::supervisor::operations::OperationHandle,
)> {
    let status = ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status;
    if status != GoalStatus::Plan {
        return Err(RefineError::Conflict(format!(
            "refusing to create a workflow worktree for Goal {} while it is {}; worktrees are materialized only after admission to plan",
            ctx.goal_id,
            status.as_str()
        )));
    }
    let worktree_target = app_git
        .git_path("refine-worktrees")?
        .join(branch.replace('/', "-"));
    with_repository_git_lock(ctx.target_root, || {
        let worktree_path = app_git.ensure_worktree(branch, &worktree_target)?;
        let handoff = register_candidate_handoff(
            ctx.runtime_root,
            ctx.target_root,
            &ctx.goal_id,
            ctx.round_idx,
            &ctx.node_id,
            branch,
            &worktree_path,
            base_commit,
        )?;
        Ok((worktree_path, handoff))
    })
}

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
            reason: "Plan was proposed, criticized, and finalized".to_string(),
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
        let prompt = match governed_implementation_prompt(&ctx.goal_id, &agent_context, &final_plan)
        {
            Ok(prompt) => prompt,
            Err(error) => return fail(ctx, "implementation_planning", error),
        };
        let mut process_metadata =
            ctx.workflow_process_metadata("implement", "WorkflowImplementation");
        process_metadata.insert("implementation_phase".to_string(), json!("implement"));
        process_metadata.insert(
            "operation_id".to_string(),
            json!(uuid::Uuid::new_v4().to_string()),
        );
        process_metadata.insert(
            "worktree".to_string(),
            json!({"path": worktree_path, "branch": branch}),
        );
        let implementation_started_at = now_timestamp();
        let resume_session = implementation_resume_session(ctx);
        let observation_before_launch = resume_session
            .as_ref()
            .map(|_| {
                FileGitWorktreeService::with_runtime_root(&agent_cwd, ctx.runtime_root)
                    .implementation_planning_observation()
            })
            .transpose()
            .ok()
            .flatten();
        let launch_with_session = |provider_session| GoalAgentLaunch {
            provider_session,
            runtime_root: ctx.runtime_root.to_path_buf(),
            cwd: agent_cwd.clone(),
            provider: ctx.provider.clone(),
            prompt: prompt.clone(),
            metadata: process_metadata.clone(),
            completion_timeout: Some(Duration::from_secs(setting_usize(
                &ctx.settings,
                "agent_hard_cap_seconds",
                7200,
            ) as u64)),
            idle_timeout: crate::workflow::agent_idle_timeout(&ctx.settings),
        };
        let on_attention = |attention: crate::process::agent_sessions::GoalAgentAttention| {
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
        };
        let launch_result = run_goal_agent_with_settlement(
            launch_with_session(resume_session.clone()),
            on_attention,
            |_| Ok(()),
        );
        let launch_result = match launch_result {
            // A resumed plan-phase session can be gone by now — pruned by the
            // provider or from an earlier daemon lifetime. Retry fresh only
            // when the worktree is provably untouched, so a genuine mid-work
            // agent failure still fails the Round exactly as before.
            Err(resume_error)
                if resume_session.is_some()
                    && observation_before_launch.is_some()
                    && FileGitWorktreeService::with_runtime_root(&agent_cwd, ctx.runtime_root)
                        .implementation_planning_observation()
                        .ok()
                        == observation_before_launch =>
            {
                let _ = ctx.log(
                    "agent",
                    "Goal Agent could not resume the plan-phase provider session; retrying fresh",
                    Some(json_object(json!({
                        "provider": ctx.provider,
                        "resume_error": resume_error.to_string(),
                        "branch": branch
                    }))),
                );
                run_goal_agent_with_settlement(launch_with_session(None), on_attention, |_| Ok(()))
            }
            other => other,
        };
        let agent_result = match launch_result {
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

impl WorkflowBehavior for WorkflowQuality {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Quality
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        if ctx.reconciliation.is_some() {
            return resolve_already_merged_quality(ctx);
        }
        match reuse_durable_quality_proof(ctx) {
            Ok(Some(outcome)) => return Ok(outcome),
            Ok(None) => {}
            Err(error) => return fail(ctx, "quality", error),
        }
        let reusable_correction = match persisted_quality_correction_evidence(ctx) {
            Ok(evidence) => evidence.filter(|(_, commit)| {
                ctx.commit.as_deref() == Some(commit.as_str())
                    && FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root)
                        .resolve_commit(commit)
                        .is_ok()
            }),
            Err(error) => return fail(ctx, "quality", error),
        };
        let (quality_report, quality_commit) = match reusable_correction {
            Some((report, commit)) => {
                ctx.log(
                    "quality",
                    "Reused persisted Quality correction evidence; re-running only the gate",
                    Some(json_object(json!({"candidate_commit": commit}))),
                )?;
                (report, commit)
            }
            None => match run_quality_correction_agent(ctx) {
                Ok(result) => result,
                Err(error) => return fail(ctx, "quality", error),
            },
        };
        ctx.commit = Some(quality_commit.clone());
        if let Err(error) = ctx
            .work_items
            .update_goal_candidate_commit(&ctx.goal_id, &quality_commit)
        {
            return fail(ctx, "quality", error);
        }
        ctx.work_items.update_goal_round_evaluation_summary(
            &ctx.goal_id,
            ctx.round_idx,
            &json!({
                "quality_agent_report": quality_report,
                "quality_candidate_commit": quality_commit
            }),
        )?;
        let quality = match run_workflow_quality(ctx, GoalStatus::Quality) {
            Ok(result) => result,
            Err(error) => {
                let category = quality_failure_category(&error);
                return fail(
                    ctx,
                    category,
                    RefineError::Conflict(quality_error_summary(&error)),
                );
            }
        };
        if !quality.ok {
            let recovery = match investigate_quality_failure(ctx, &quality_report, &quality) {
                Ok(recovery) => recovery,
                Err(error) => return fail(ctx, "quality_recovery", error),
            };
            return handle_quality_finding(ctx, &recovery);
        }
        ctx.request_transition(GoalStatus::Quality, GoalStatus::Governance)?;
        if let Some(handoff) = find_candidate_handoff(
            ctx.runtime_root,
            ctx.target_root,
            &ctx.goal_id,
            ctx.round_idx,
        )? {
            record_candidate_handoff_governance(
                ctx.runtime_root,
                &handoff.id,
                &quality.candidate_commit,
            )?;
        }
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Quality,
            to: GoalStatus::Governance,
            reason: "Quality checks passed".to_string(),
        })
    }
}

/// Resume skip for a fully completed Quality phase: a durable proof for the exact current
/// candidate means the phase already succeeded, so an interrupted attempt only has to replay
/// the Quality -> Governance transition instead of re-running the correction agent and gate.
/// The predicate is the canonical proof check — never anything weaker.
fn reuse_durable_quality_proof(
    ctx: &mut WorkflowContext<'_>,
) -> RefineResult<Option<WorkflowAdvanceOutcome>> {
    let Some(candidate) = ctx.commit.clone() else {
        return Ok(None);
    };
    if ctx
        .work_items
        .current_round_quality_proof(&ctx.goal_id, ctx.round_idx, &candidate)?
        .is_none()
    {
        return Ok(None);
    }
    let app_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
    if app_git.resolve_commit(&candidate).is_err() {
        return Ok(None);
    }
    ctx.log(
        "quality",
        "Reused durable Quality proof on resume",
        Some(json_object(json!({"candidate_commit": candidate}))),
    )?;
    ctx.request_transition(GoalStatus::Quality, GoalStatus::Governance)?;
    if let Some(handoff) = find_candidate_handoff(
        ctx.runtime_root,
        ctx.target_root,
        &ctx.goal_id,
        ctx.round_idx,
    )? {
        record_candidate_handoff_governance(ctx.runtime_root, &handoff.id, &candidate)?;
    }
    Ok(Some(WorkflowAdvanceOutcome::Transition {
        from: GoalStatus::Quality,
        to: GoalStatus::Governance,
        reason: "Reused durable Quality proof from an interrupted attempt".to_string(),
    }))
}

/// Resume skip for a completed correction agent: `quality_agent_report` and
/// `quality_candidate_commit` are persisted before the gate runs, so their presence proves
/// the expensive correction agent finished even when no gate proof exists yet.
fn persisted_quality_correction_evidence(
    ctx: &WorkflowContext<'_>,
) -> RefineResult<Option<(String, String)>> {
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let Some(round) = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
    else {
        return Ok(None);
    };
    let report = round
        .get("quality_agent_report")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let commit = round
        .get("quality_candidate_commit")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Ok(report
        .zip(commit)
        .map(|(report, commit)| (report.to_string(), commit.to_string())))
}

fn resolve_already_merged_quality(
    ctx: &mut WorkflowContext<'_>,
) -> RefineResult<WorkflowAdvanceOutcome> {
    let candidate = ctx.require_commit()?.to_string();
    if let Some(outcome) = recover_failed_already_merged_quality(ctx, &candidate)? {
        return Ok(outcome);
    }
    if ctx
        .work_items
        .current_round_quality_proof(&ctx.goal_id, ctx.round_idx, &candidate)?
        .is_none()
    {
        let quality = match run_workflow_quality(ctx, GoalStatus::Quality) {
            Ok(quality) => quality,
            Err(error) => {
                return fail(
                    ctx,
                    "already_merged_quality_regeneration",
                    RefineError::Conflict(format!(
                        "Exact-candidate Quality proof could not be regenerated: {error}"
                    )),
                );
            }
        };
        ctx.log(
            "reconcile",
            if quality.ok {
                "Regenerated passed isolated Quality proof for the exact integrated candidate"
            } else {
                "Regenerated isolated Quality proof rejected the exact integrated candidate"
            },
            Some(json_object(json!({
                "candidate_commit": candidate,
                "quality_ok": quality.ok,
                "provider_attempts": quality.provider_attempts.len()
            }))),
        )?;
        if !quality.ok {
            let settlement = ctx
                .work_items
                .settle_already_merged_quality_failure(
                    &ctx.goal_id,
                    ctx.attempt_authority,
                    &candidate,
                    None,
                )?
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "Exact-candidate failed Quality result for Goal {} was not durably recoverable",
                        ctx.goal_id
                    ))
                })?;
            return failed_already_merged_quality_outcome(ctx, settlement);
        }
    }
    let service = FileGovernanceIntegrationService::with_target_root(
        ctx.runtime_root,
        ctx.refine_dir(),
        ctx.target_root,
    );
    let resolution =
        service.resolve_already_merged_goal_with_authority(&ctx.goal_id, ctx.attempt_authority)?;
    ctx.log(
        "reconcile",
        match resolution.disposition {
            AlreadyMergedResolutionDisposition::Resolved => {
                "Already-merged candidate resolved to Review from exact passed gate evidence"
            }
            AlreadyMergedResolutionDisposition::AlreadyResolved => {
                "Already-merged candidate was already resolved to Review"
            }
            AlreadyMergedResolutionDisposition::Failed => {
                "Already-merged candidate could not be resolved safely and was settled as Failed"
            }
            AlreadyMergedResolutionDisposition::AlreadyFailed => {
                "Already-merged candidate was already settled as Failed"
            }
        },
        Some(json_object(json!({
            "disposition": resolution.disposition.as_str(),
            "evidence": resolution.evidence
        }))),
    )?;
    match resolution.goal.goal.status {
        GoalStatus::Review => Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Quality,
            to: GoalStatus::Review,
            reason: "Already-merged candidate resolved from exact passed Quality and Governance evidence"
                .to_string(),
        }),
        GoalStatus::Failed => {
            ctx.final_status = Some(GoalStatus::Failed);
            Ok(WorkflowAdvanceOutcome::Completed {
                final_status: GoalStatus::Failed,
                reason: "Already-merged reconciliation failed closed with durable recovery evidence"
                    .to_string(),
            })
        }
        status => Err(RefineError::Conflict(format!(
            "already-merged resolver returned unexpected Goal status {}",
            status.as_str()
        ))),
    }
}

fn recover_failed_already_merged_quality(
    ctx: &mut WorkflowContext<'_>,
    candidate: &str,
) -> RefineResult<Option<WorkflowAdvanceOutcome>> {
    if let Some(settlement) = ctx.work_items.settle_already_merged_quality_failure(
        &ctx.goal_id,
        ctx.attempt_authority,
        candidate,
        None,
    )? {
        return failed_already_merged_quality_outcome(ctx, settlement).map(Some);
    }

    let owner = format!("quality:{}:{candidate}", ctx.goal_id);
    for operation in FileOperationRegistry::new(ctx.runtime_root)
        .recover()?
        .into_iter()
        .filter(|operation| operation.owner == owner && operation.state == OperationState::Failed)
    {
        if let Some(settlement) = ctx.work_items.settle_already_merged_quality_failure(
            &ctx.goal_id,
            ctx.attempt_authority,
            candidate,
            Some(&operation),
        )? {
            return failed_already_merged_quality_outcome(ctx, settlement).map(Some);
        }
    }
    Ok(None)
}

fn failed_already_merged_quality_outcome(
    ctx: &mut WorkflowContext<'_>,
    settlement: AlreadyMergedSettlement,
) -> RefineResult<WorkflowAdvanceOutcome> {
    let (disposition, evidence) = match settlement {
        AlreadyMergedSettlement::Failed(_, evidence) => ("failed", evidence),
        AlreadyMergedSettlement::AlreadyFailed(_, evidence) => ("already_failed", evidence),
        AlreadyMergedSettlement::Resolved(_, _)
        | AlreadyMergedSettlement::AlreadyResolved(_, _) => {
            return Err(RefineError::Conflict(
                "failed already-merged Quality settlement produced approval evidence".to_string(),
            ));
        }
    };
    ctx.log(
        "reconcile",
        "Already-merged exact candidate failed isolated Quality and was settled without regeneration",
        Some(json_object(json!({
            "disposition": disposition,
            "evidence": evidence
        }))),
    )?;
    ctx.final_status = Some(GoalStatus::Failed);
    Ok(WorkflowAdvanceOutcome::Completed {
        final_status: GoalStatus::Failed,
        reason: "Already-merged exact candidate failed its first isolated Quality evaluation"
            .to_string(),
    })
}

impl WorkflowBehavior for WorkflowGovernance {
    fn observes(&self) -> GoalStatus {
        GoalStatus::Governance
    }

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome> {
        let branch = ctx.require_branch()?.to_string();
        let worktree_path = ctx.require_worktree_path()?.to_string();
        let agent_cwd = agent_worktree_cwd(
            &worktree_path,
            setting_string(&ctx.settings, "agent_subpath", "").as_str(),
        )?;
        let target_root = ctx.target_root.to_path_buf();
        let max_retries = max_automatic_round_retries(ctx)?;
        let integration_service = FileGovernanceIntegrationService::with_target_root(
            ctx.runtime_root,
            ctx.refine_dir(),
            ctx.target_root,
        );
        // The repository lock covers only Git work: one hold refreshes the
        // candidate onto the target tip, and a second hold proves that tip is
        // unchanged and merges. The quality proof and the governance verdict —
        // agent invocations that can run for minutes — execute between the two
        // holds so they can never stall every other Goal's commits and
        // worktree operations behind this Goal's review.
        let mut settled = None;
        for _ in 0..INTEGRATION_REFRESH_ATTEMPTS {
            let step = (|| -> RefineResult<GovernanceIntegrationStep> {
                let refresh = with_repository_git_lock(&target_root, || {
                    refresh_candidate_for_target_advancement(ctx, max_retries)
                })?;
                let expected_target = match refresh {
                    CandidateRefreshOutcome::RecoveryQueued { reason, evidence } => {
                        ctx.log(
                            "governance_integration",
                            "Queued a fresh recovery Round for an ambiguous or conflicted candidate refresh",
                            Some(json_object(json!({
                                "reason": reason,
                                "retained_evidence": evidence
                            }))),
                        )?;
                        ctx.final_status = Some(GoalStatus::Todo);
                        return Ok(GovernanceIntegrationStep::Outcome(
                            WorkflowAdvanceOutcome::Completed {
                                final_status: GoalStatus::Todo,
                                reason: "Integration race queued a fresh fenced recovery Round"
                                    .to_string(),
                            },
                        ));
                    }
                    CandidateRefreshOutcome::RecoveryExhausted { reason, evidence } => {
                        ctx.log(
                            "governance_integration",
                            "Integration recovery exhausted the shared automatic Round budget",
                            Some(json_object(json!({
                                "reason": reason,
                                "retained_evidence": evidence,
                                "max_automatic_round_retries": max_retries
                            }))),
                        )?;
                        ctx.final_status = Some(GoalStatus::Failed);
                        return Ok(GovernanceIntegrationStep::Outcome(
                            WorkflowAdvanceOutcome::Completed {
                                final_status: GoalStatus::Failed,
                                reason:
                                    "Integration recovery exhausted the shared automatic Round budget"
                                        .to_string(),
                            },
                        ));
                    }
                    CandidateRefreshOutcome::Refreshed {
                        original_candidate,
                        replacement_candidate,
                        target_commit,
                        evidence,
                    } => {
                        ctx.log(
                            "governance_integration",
                            "Refreshed a provable candidate delta onto the advanced target",
                            Some(json_object(json!({
                                "original_candidate_commit": original_candidate,
                                "replacement_candidate_commit": replacement_candidate,
                                "replacement_base_commit": target_commit,
                                "refresh_evidence": evidence
                            }))),
                        )?;
                        ctx.revalidate_authority(GoalStatus::Governance)?;
                        let quality = run_workflow_quality(ctx, GoalStatus::Governance)?;
                        if !quality.ok {
                            let retained = json!({
                                "candidate_refresh": evidence,
                                "replacement_candidate_commit": replacement_candidate,
                                "quality_summary": quality.summary,
                                "quality_results": quality.results,
                                "quality_diagnostics": quality.diagnostics
                            });
                            let recovery = ctx.work_items.queue_integration_recovery_summary(
                                &ctx.goal_id,
                                ctx.attempt_authority,
                                &ctx.node_id,
                                "replacement candidate failed exact-candidate Quality",
                                retained.clone(),
                                max_retries,
                            )?;
                            let final_status = recovery.goal.status;
                            ctx.final_status = Some(final_status.clone());
                            return Ok(GovernanceIntegrationStep::Outcome(
                                WorkflowAdvanceOutcome::Completed {
                                    final_status: final_status.clone(),
                                    reason: if final_status == GoalStatus::Todo {
                                        "Replacement candidate Quality queued a fresh recovery Round"
                                            .to_string()
                                    } else {
                                        "Replacement candidate Quality exhausted the shared automatic Round budget"
                                            .to_string()
                                    },
                                },
                            ));
                        }
                        ctx.log(
                            "quality",
                            "Replacement candidate passed exact-candidate Quality under the integration lease",
                            Some(json_object(json!({
                                "candidate_commit": replacement_candidate
                            }))),
                        )?;
                        target_commit
                    }
                    CandidateRefreshOutcome::Unchanged { target_commit } => target_commit,
                };

                ctx.revalidate_authority(GoalStatus::Governance)?;
                let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
                let agent_context = ensure_goal_agent_context(ctx, &goal)?;
                let governance =
                    evaluate_workflow_governance(ctx, &worktree_path, &agent_cwd, &agent_context)?;
                ctx.revalidate_authority(GoalStatus::Governance)?;
                record_governance(ctx, &governance)?;
                if governance.failed {
                    return handle_governance_finding(ctx, &governance)
                        .map(GovernanceIntegrationStep::Outcome);
                }

                with_repository_git_lock(&target_root, || {
                    ctx.revalidate_authority(GoalStatus::Governance)?;
                    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
                    let target_branch = detail
                        .get("target_branch")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            RefineError::Conflict(format!(
                                "Goal {} has no pinned target_branch to integrate",
                                ctx.goal_id
                            ))
                        })?;
                    let target_git =
                        FileGitWorktreeService::with_runtime_root(&target_root, ctx.runtime_root);
                    if target_git.resolve_commit(target_branch)? != expected_target {
                        return Ok(GovernanceIntegrationStep::TargetAdvanced { expected_target });
                    }
                    let remote = ctx.git_remote()?;
                    let commit = ctx.require_commit()?.to_string();
                    let worktree_git =
                        FileGitWorktreeService::with_runtime_root(&worktree_path, ctx.runtime_root);
                    if worktree_git.remote_exists(&remote)? {
                        worktree_git.push(&remote, &branch)?;
                    }
                    ctx.revalidate_authority(GoalStatus::Governance)?;
                    let goal_id = ctx.goal_id.clone();
                    let node_id = ctx.node_id.clone();
                    let round_idx = ctx.round_idx;
                    let (integration, transitioned) = integration_service
                        .integrate_workflow_candidate_and_settle_under_repository_lease(
                            &goal_id,
                            round_idx,
                            &node_id,
                            &branch,
                            &commit,
                            &remote,
                            |integration| {
                                ctx.log(
                                    "governance_integration",
                                    &format!(
                                        "Governance integrated approved implementation candidate {branch}"
                                    ),
                                    Some(json_object(json!({
                                        "branch": branch,
                                        "integration": integration
                                    }))),
                                )?;
                                let current = ctx.work_items.show_goal_summary(&ctx.goal_id)?;
                                if current.goal.status == GoalStatus::Cancelled {
                                    Ok(false)
                                } else {
                                    ctx.request_transition(
                                        GoalStatus::Governance,
                                        GoalStatus::Review,
                                    )?;
                                    Ok(true)
                                }
                            },
                        )?;
                    Ok(GovernanceIntegrationStep::Integrated {
                        integration,
                        transitioned,
                        candidate_commit: commit,
                    })
                })
            })();
            let step = match step {
                Ok(step) => step,
                // Losing the target-ref compare-and-swap inside the
                // integration is the same race as the pre-merge tip check:
                // refresh against the moved target and try again.
                Err(RefineError::TargetAdvanced { expected, .. }) => {
                    GovernanceIntegrationStep::TargetAdvanced {
                        expected_target: expected,
                    }
                }
                Err(error)
                    if ctx
                        .work_items
                        .show_goal_summary(&ctx.goal_id)
                        .is_ok_and(|goal| goal.goal.status == GoalStatus::Cancelled) =>
                {
                    return Err(error);
                }
                Err(error) => return fail(ctx, "governance_integration", error),
            };
            match step {
                GovernanceIntegrationStep::Outcome(outcome) => return Ok(outcome),
                GovernanceIntegrationStep::Integrated {
                    integration,
                    transitioned,
                    candidate_commit,
                } => {
                    settled = Some((integration, transitioned, candidate_commit));
                    break;
                }
                GovernanceIntegrationStep::TargetAdvanced { expected_target } => {
                    ctx.log(
                        "governance_integration",
                        "Target advanced between candidate refresh and integration; refreshing again",
                        Some(json_object(json!({
                            "expected_target_commit": expected_target
                        }))),
                    )?;
                }
            }
        }
        let Some((integration, transitioned, candidate_commit)) = settled else {
            let recovery = ctx.work_items.queue_integration_recovery_summary(
                &ctx.goal_id,
                ctx.attempt_authority,
                &ctx.node_id,
                "target branch advanced ahead of every integration attempt",
                json!({ "integration_attempts": INTEGRATION_REFRESH_ATTEMPTS }),
                max_retries,
            )?;
            let final_status = recovery.goal.status;
            ctx.final_status = Some(final_status.clone());
            return Ok(WorkflowAdvanceOutcome::Completed {
                final_status: final_status.clone(),
                reason: if final_status == GoalStatus::Todo {
                    "Repeated integration races queued a fresh recovery Round".to_string()
                } else {
                    "Repeated integration races exhausted the shared automatic Round budget"
                        .to_string()
                },
            });
        };
        ctx.merge = Some(integration.merge);
        if let Some(handoff) = find_candidate_handoff(
            ctx.runtime_root,
            ctx.target_root,
            &ctx.goal_id,
            ctx.round_idx,
        )? {
            settle_candidate_handoff(
                ctx.runtime_root,
                &handoff.id,
                "candidate_integrated",
                json!({
                    "candidate_commit": candidate_commit,
                    "round_idx": ctx.round_idx
                }),
            )?;
        }
        if !transitioned {
            ctx.final_status = Some(GoalStatus::Cancelled);
            return Ok(WorkflowAdvanceOutcome::Completed {
                final_status: GoalStatus::Cancelled,
                reason: "Governance finished after Goal cancellation; integration evidence was preserved"
                    .to_string(),
            });
        }
        Ok(WorkflowAdvanceOutcome::Transition {
            from: GoalStatus::Governance,
            to: GoalStatus::Review,
            reason: "Governance passed and integrated the implementation candidate".to_string(),
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

fn run_quality_correction_agent(ctx: &mut WorkflowContext<'_>) -> RefineResult<(String, String)> {
    ctx.revalidate_authority(GoalStatus::Quality)?;
    let worktree_path = ctx.require_worktree_path()?.to_string();
    let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let agent_context = ensure_goal_agent_context(ctx, &goal)?;
    let round = goal
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .ok_or_else(|| {
            RefineError::NotFound(format!(
                "Goal {} has no round {} for Quality",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    let plan = round
        .get("implementation_plan")
        .and_then(|plan| plan.get("final_plan"))
        .and_then(|artifact| artifact.get("result"))
        .cloned()
        .unwrap_or_else(|| json!({
            "summary": "Validate an imported implementation candidate.",
            "checklist": [{
                "id": "IMPORTED",
                "description": round.get("prompt").and_then(Value::as_str).unwrap_or("Review the imported candidate")
            }]
        }));
    let implementation_report = round
        .get("implementation_report")
        .and_then(Value::as_str)
        .unwrap_or("No implementation report was recorded.");
    let context_json = serde_json::to_string_pretty(&agent_context).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Quality context: {error}"))
    })?;
    let plan_json = serde_json::to_string_pretty(&plan).map_err(|error| {
        RefineError::Serialization(format!("failed to encode finalized plan: {error}"))
    })?;
    let prompt = PromptEngine::render(
        PromptTemplate::GoalWorkflowQualityAgent,
        &[
            ("context_json", &context_json),
            ("plan_json", &plan_json),
            ("implementation_report", implementation_report),
        ],
    )
    .map_err(|error| RefineError::Serialization(format!("invalid Quality prompt: {error}")))?;
    let agent_cwd = agent_worktree_cwd(
        &worktree_path,
        setting_string(&ctx.settings, "agent_subpath", "").as_str(),
    )?;
    let report = if cfg!(test) && ctx.provider == "smoke-ai" {
        "Smoke AI Quality fixture reviewed the candidate and retained existing tests.".to_string()
    } else {
        let result = run_goal_agent_with_settlement(
            GoalAgentLaunch {
                provider_session: None,
                runtime_root: ctx.runtime_root.to_path_buf(),
                cwd: agent_cwd,
                provider: ctx.provider.clone(),
                prompt,
                metadata: ctx.workflow_process_metadata("quality", "WorkflowQualityAgent"),
                completion_timeout: Some(Duration::from_secs(setting_usize(
                    &ctx.settings,
                    "agent_hard_cap_seconds",
                    7200,
                ) as u64)),
                idle_timeout: crate::workflow::agent_idle_timeout(&ctx.settings),
            },
            |attention| {
                let _ = ctx.log(
                    "quality",
                    "Quality Agent is waiting for user input",
                    Some(json_object(json!({"message": attention.message}))),
                );
            },
            |_| Ok(()),
        )?;
        let output = result.output;
        ctx.revalidate_authority(GoalStatus::Quality)?;
        output
    };
    let target_branch = setting_string(&ctx.settings, "merge_target_branch", "main");
    let worktree_git = FileGitWorktreeService::with_runtime_root(&worktree_path, ctx.runtime_root);
    ctx.revalidate_authority(GoalStatus::Quality)?;
    let commit = with_repository_git_lock(ctx.target_root, || {
        ctx.revalidate_authority(GoalStatus::Quality)?;
        worktree_git.commit_or_clean_noop_since(
            &format!("Quality {} round {}", ctx.goal_id, ctx.round_idx + 1),
            &[],
            &target_branch,
        )
    })?;
    ctx.log(
        "quality",
        "Quality Agent completed candidate review and corrections",
        Some(json_object(json!({
            "candidate_commit": commit.commit,
            "report": report
        }))),
    )?;
    Ok((report, commit.commit))
}

fn run_workflow_quality(
    ctx: &mut WorkflowContext<'_>,
    authority_status: GoalStatus,
) -> RefineResult<QualityCheckResult> {
    let runner = QualityOperationRunner::new(ctx.refine_dir(), ctx.runtime_root, ctx.target_root);
    if let (Some(operation_id), Some(request)) =
        (ctx.quality_operation_id.take(), ctx.quality_request.take())
    {
        runner
            .run_registered(&operation_id, request)
            .map(|operation| operation.result)
    } else {
        let mut metadata =
            ctx.workflow_process_metadata(authority_status.as_str(), "WorkflowQuality");
        if ctx.reconciliation.is_some() {
            metadata.insert("quality_proof_mode".to_string(), json!("regenerated"));
        }
        runner
            .run_goal_checks(&ctx.goal_id, &ctx.provider, metadata)
            .map(|operation| operation.result)
    }
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

fn investigate_quality_failure(
    ctx: &WorkflowContext<'_>,
    quality_agent_report: &str,
    quality: &QualityCheckResult,
) -> RefineResult<QualityRecoveryInvestigation> {
    ctx.revalidate_authority(GoalStatus::Quality)?;
    let worktree_path = ctx.require_worktree_path()?.to_string();
    let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let agent_context = ensure_goal_agent_context(ctx, &goal)?;
    let provider_cwd = agent_worktree_cwd(
        &worktree_path,
        setting_string(&ctx.settings, "agent_subpath", "").as_str(),
    )?;
    let prompt = quality_recovery_prompt(
        &ctx.goal_id,
        ctx.round_idx,
        &worktree_path,
        &agent_context,
        quality_agent_report,
        quality,
    )?;
    let worktree_git = FileGitWorktreeService::with_runtime_root(&worktree_path, ctx.runtime_root);
    let before = worktree_git.implementation_planning_observation()?;
    if before.head_commit != quality.candidate_commit {
        return Err(RefineError::Conflict(format!(
            "Quality recovery expected candidate {} but worktree HEAD is {}",
            quality.candidate_commit, before.head_commit
        )));
    }
    if !before.status_porcelain.is_empty() {
        return Err(RefineError::Conflict(
            "Quality recovery requires a clean candidate worktree".to_string(),
        ));
    }

    let provider = HostAgentProviderService::with_runtime_root(ctx.runtime_root.join("agents"));
    let output = provider.invoke(ProviderInvocation {
        provider: ctx.provider.clone(),
        prompt,
        session_id: None,
        cwd: Some(provider_cwd.display().to_string()),
        stall_timeout_seconds: agent_idle_timeout(&ctx.settings).map(|timeout| timeout.as_secs()),
        process_metadata: ctx
            .workflow_process_metadata("quality_recovery", "WorkflowQualityRecovery"),
    })?;
    ctx.revalidate_authority(GoalStatus::Quality)?;

    let after = worktree_git.implementation_planning_observation()?;
    if after != before {
        return Err(RefineError::Conflict(
            "Quality recovery investigation modified the candidate worktree; recovery analysis must be read-only"
                .to_string(),
        ));
    }
    let mut recovery = parse_quality_recovery_provider_output(&output)?;
    recovery
        .details
        .insert("provider".to_string(), Value::String(ctx.provider.clone()));
    recovery
        .details
        .insert("worktree".to_string(), Value::String(worktree_path));
    recovery.details.insert(
        "cwd".to_string(),
        Value::String(provider_cwd.display().to_string()),
    );
    recovery.details.insert(
        "candidate_commit".to_string(),
        Value::String(quality.candidate_commit.clone()),
    );
    Ok(recovery)
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
    let guidance = agent_context
        .get("guidance_candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let product_or_constitution_configured = governance
        .get("product")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || governance
            .get("constitution")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
    if rules.is_empty() && guidance.is_empty() && !product_or_constitution_configured {
        return Ok(GovernanceEvaluation {
            failed: false,
            message: None,
            recovery_analysis: None,
            recovery_round_prompt: None,
            details: json_object(json!({
                "phase": "post_implementation",
                "configured": false,
                "governance_configured": governance.get("configured").and_then(Value::as_bool).unwrap_or(false),
                "rules_checked": 0,
                "guidance_checked": 0,
                "failed_actions": []
            })),
        });
    }
    let prompt = post_implementation_governance_prompt(
        &governance,
        &rules,
        &guidance,
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
        stall_timeout_seconds: agent_idle_timeout(&ctx.settings).map(|timeout| timeout.as_secs()),
        process_metadata: ctx.workflow_process_metadata("governance", "WorkflowGovernance"),
    })?;
    let mut evaluation = parse_governance_provider_output(&output, rules.len());
    evaluation
        .details
        .insert("provider".to_string(), Value::String(ctx.provider.clone()));
    evaluation
        .details
        .insert("guidance_checked".to_string(), json!(guidance.len()));
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
            "governance_candidate_commit": ctx.require_commit()?,
            "governance_rule_actions": evaluation.details
                .get("failed_actions")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "governance_recovery_analysis": evaluation.recovery_analysis,
            "governance_recovery_round_prompt": evaluation.recovery_round_prompt
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

fn handle_quality_finding(
    ctx: &mut WorkflowContext<'_>,
    recovery: &QualityRecoveryInvestigation,
) -> RefineResult<WorkflowAdvanceOutcome> {
    ctx.work_items.update_goal_round_evaluation_summary(
        &ctx.goal_id,
        ctx.round_idx,
        &json!({
            "quality_recovery_analysis": recovery.analysis,
            "quality_recovery_round_prompt": recovery.round_prompt,
            "quality_recovery_details": recovery.details,
            "quality_recovery_checked_at": now_timestamp()
        }),
    )?;
    let current_attempt = current_automatic_retry_attempt(ctx)?;
    let max_retries = max_automatic_round_retries(ctx)?;
    if current_attempt >= max_retries {
        return fail(
            ctx,
            "quality_retry_exhausted",
            RefineError::Conflict(format!(
                "Quality findings remain after {max_retries} automatic recovery Rounds"
            )),
        );
    }
    let next_attempt = current_attempt + 1;
    ctx.work_items.queue_quality_recovery_summary(
        &ctx.goal_id,
        ctx.round_idx,
        Some(ctx.attempt_authority),
        next_attempt,
        &recovery.analysis,
        &recovery.round_prompt,
    )?;
    ctx.log(
        "quality",
        "Quality drafted a fresh automatic recovery Round",
        Some(json_object(json!({
            "attempt": next_attempt,
            "max_automatic_round_retries": max_retries,
            "source_round": ctx.round_idx + 1
        }))),
    )?;
    ctx.final_status = Some(GoalStatus::Todo);
    Ok(WorkflowAdvanceOutcome::Completed {
        final_status: GoalStatus::Todo,
        reason: "Quality findings queued a fresh recovery Round".to_string(),
    })
}

fn handle_governance_finding(
    ctx: &mut WorkflowContext<'_>,
    evaluation: &GovernanceEvaluation,
) -> RefineResult<WorkflowAdvanceOutcome> {
    if evaluation
        .details
        .get("verdict_parse_error")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return fail(
            ctx,
            "governance",
            RefineError::Conflict(
                evaluation
                    .message
                    .clone()
                    .unwrap_or_else(|| "Governance verdict could not be parsed".to_string()),
            ),
        );
    }
    let analysis = evaluation.recovery_analysis.as_deref().ok_or_else(|| {
        RefineError::Serialization(
            "failing Governance verdict omitted recovery_analysis".to_string(),
        )
    });
    let prompt = evaluation.recovery_round_prompt.as_deref().ok_or_else(|| {
        RefineError::Serialization(
            "failing Governance verdict omitted recovery_round_prompt".to_string(),
        )
    });
    let (analysis, prompt) = match (analysis, prompt) {
        (Ok(analysis), Ok(prompt)) => (analysis, prompt),
        (Err(error), _) | (_, Err(error)) => return fail(ctx, "governance", error),
    };
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    if let Some(source_signature) = source_governance_failure_signature(&detail, ctx.round_idx) {
        let current_signature = governance_failure_signature(
            evaluation
                .details
                .get("failed_actions")
                .and_then(Value::as_array),
        );
        if !current_signature.is_empty() && current_signature == source_signature {
            return fail(
                ctx,
                "governance_retry_exhausted",
                RefineError::Conflict(
                    "consecutive Rounds failed Governance with identical findings; automatic retries were stopped early"
                        .to_string(),
                ),
            );
        }
    }
    let current_attempt = current_automatic_retry_attempt(ctx)?;
    let max_retries = max_automatic_round_retries(ctx)?;
    if current_attempt >= max_retries {
        return fail(
            ctx,
            "governance_retry_exhausted",
            RefineError::Conflict(format!(
                "Governance findings remain after {max_retries} automatic recovery Rounds"
            )),
        );
    }
    let next_attempt = current_attempt + 1;
    ctx.work_items.queue_governance_recovery_summary(
        &ctx.goal_id,
        ctx.round_idx,
        Some(ctx.attempt_authority),
        next_attempt,
        analysis,
        prompt,
    )?;
    ctx.log(
        "governance",
        "Governance drafted a fresh automatic recovery Round",
        Some(json_object(json!({
            "attempt": next_attempt,
            "max_automatic_round_retries": max_retries,
            "source_round": ctx.round_idx + 1
        }))),
    )?;
    ctx.final_status = Some(GoalStatus::Todo);
    Ok(WorkflowAdvanceOutcome::Completed {
        final_status: GoalStatus::Todo,
        reason: "Governance findings queued a fresh recovery Round".to_string(),
    })
}

/// The source Round's Governance failure signature, when the current Round is
/// itself a governance-finding recovery. A recovery Round that reproduces the
/// exact signature it was drafted to fix is stopped early instead of burning
/// the remaining automatic Round budget on identical attempts.
fn source_governance_failure_signature(detail: &Value, round_idx: usize) -> Option<Vec<String>> {
    let rounds = detail.get("rounds").and_then(Value::as_array)?;
    let (kind, source_idx) = round_scoped_recovery_retry(rounds.get(round_idx)?)?;
    if kind != "governance" {
        return None;
    }
    let signature = governance_failure_signature(
        rounds
            .get(source_idx)?
            .get("governance_details")
            .and_then(|details| details.get("failed_actions"))
            .and_then(Value::as_array),
    );
    (!signature.is_empty()).then_some(signature)
}

fn governance_failure_signature(actions: Option<&Vec<Value>>) -> Vec<String> {
    let mut signature = actions
        .into_iter()
        .flatten()
        .filter_map(|action| {
            action
                .get("rule_id")
                .or_else(|| action.get("rule"))
                .or_else(|| action.get("action"))
                .or_else(|| action.get("message"))
                .and_then(Value::as_str)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .collect::<Vec<_>>();
    signature.sort();
    signature.dedup();
    signature
}

fn current_automatic_retry_attempt(ctx: &WorkflowContext<'_>) -> RefineResult<u32> {
    Ok(ctx
        .work_items
        .show_goal_detail(&ctx.goal_id)?
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .and_then(|round| round.get("automatic_retry"))
        .and_then(|retry| retry.get("attempt"))
        .and_then(Value::as_u64)
        .and_then(|attempt| u32::try_from(attempt).ok())
        .unwrap_or(0))
}

fn max_automatic_round_retries(ctx: &WorkflowContext<'_>) -> RefineResult<u32> {
    Ok(FileGovernanceService::new(ctx.refine_dir())
        .load()?
        .get("max_automatic_round_retries")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(5))
}

fn quality_failure_category(error: &RefineError) -> &'static str {
    if crate::tools::host::quality::is_quality_candidate_infrastructure(error) {
        "quality_candidate_infrastructure"
    } else if is_quality_harness_fault(error) {
        "quality_harness"
    } else if is_quality_output_contract_fault(error) {
        "quality_output_contract"
    } else {
        "quality"
    }
}

fn fail<T>(ctx: &WorkflowContext<'_>, category: &str, error: RefineError) -> RefineResult<T> {
    if let Ok(Some(handoff)) = find_candidate_handoff(
        ctx.runtime_root,
        ctx.target_root,
        &ctx.goal_id,
        ctx.round_idx,
    ) {
        retain_candidate_handoff_after_failure(
            ctx.runtime_root,
            &handoff.id,
            "candidate_handoff_workflow_failed",
            ctx.commit.as_deref(),
            &error,
        );
    }
    let _ = ctx.fail(category, &error);
    Err(error)
}

#[cfg(test)]
mod tests;
