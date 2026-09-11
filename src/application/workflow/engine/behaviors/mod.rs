mod authoring;
mod scoped_recovery;
use scoped_recovery::begin_scoped_recovery_round;
pub mod contract;

use serde_json::{Value, json};

use crate::application::agent_io::prompts::{PromptEngine, PromptTemplate};
use crate::application::work_items::AlreadyMergedSettlement;
use crate::application::workflow::engine::behaviors::contract::{
    WorkflowAdvanceOutcome, WorkflowBehavior,
};
use crate::application::workflow::engine::context::WorkflowContext;
use crate::application::workflow::governance::integration::{
    AlreadyMergedResolutionDisposition, FileGovernanceIntegrationService, TargetRefresh,
    refresh_target_from_remote,
};
use crate::application::workflow::phases::implementation_planning::{
    planning_context, run_planning_skills,
};
use crate::application::workflow::phases::quality::{
    QualityCheckResult, QualityOperationRunner, is_quality_harness_fault,
    is_quality_output_contract_fault, quality_error_summary,
};
use crate::application::workflow::recovery::candidate_handoff::{
    find_candidate_handoff, record_candidate_handoff_commit, record_candidate_handoff_governance,
    register_candidate_handoff, retain_candidate_handoff_after_failure, settle_candidate_handoff,
};
use crate::application::workflow::{
    CandidateRefreshOutcome, GovernanceEvaluation, agent_worktree_cwd, implementation_branch_name,
    json_object, now_timestamp, refresh_candidate_for_target_advancement, round_agent_context,
    selected_agent_context, setting_string,
};
use crate::error::{MergeConflictStage, RefineError, RefineResult};
use crate::infrastructure::git::with_repository_git_lock;
use crate::infrastructure::git::worktrees::{
    FileGitWorktreeService, GitWorktreeService, MergeResult,
};
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::model::workflow::GoalStatus;

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
        ctx.revalidate_authority(GoalStatus::Todo)?;
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
        // The base is pinned from the local ref, so the local ref is brought up to
        // its remote first. Advisory: the Round starts on whatever the ref holds if
        // the remote is absent or unreachable. The remote itself is read from the
        // node setting rather than the Round's pin, because Todo may run before the
        // Round exists to carry one.
        refresh_todo_target_from_remote(ctx, &app_git, &target_branch)?;
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
        // The branch tip travels with the creation log so the Round branch's actual
        // birth commit is inspectable next to the base the Goal records. A silent
        // disagreement between the two is what made "stale" unreadable before.
        let branch_tip = app_git.resolve_commit(&branch).ok();
        ctx.log(
            "git",
            &format!("Created implementation worktree for {branch}"),
            Some(json_object(json!({
                "branch": branch,
                "worktree": worktree_path,
                "base_commit": base_commit,
                "branch_commit": branch_tip
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
        return fail(
            ctx,
            "reconciliation_candidate_absent",
            RefineError::Conflict(format!(
                "Candidate {candidate} is absent from {} at {target_commit}; an explicit workflow decision is required",
                integration.target_branch,
            )),
        );
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

/// Refresh the local target branch from its remote before the Round pins its
/// base, and record what happened on the Round.
///
/// The refresh itself never fails the Goal — every outcome, including an
/// unreachable remote, leaves a legible Round log and lets the Round proceed on
/// the local ref. Only writing that log can fail, and that is a durable-state
/// failure the caller must not swallow.
fn refresh_todo_target_from_remote(
    ctx: &WorkflowContext<'_>,
    app_git: &FileGitWorktreeService,
    target_branch: &str,
) -> RefineResult<()> {
    let remote = setting_string(&ctx.settings, "git_remote", "origin");
    let refresh = refresh_target_from_remote(app_git, ctx.target_root, &remote, target_branch);
    let (message, detail) = match &refresh {
        TargetRefresh::Unavailable { reason } => (
            format!("Pinned the base from local {target_branch}; its remote was unavailable"),
            json!({"remote": remote, "target_branch": target_branch, "reason": reason}),
        ),
        TargetRefresh::AlreadyCurrent { target_commit } => (
            format!("Local {target_branch} already carried the fetched tip"),
            json!({
                "remote": remote,
                "target_branch": target_branch,
                "target_commit": target_commit
            }),
        ),
        TargetRefresh::FastForwarded {
            from_commit,
            to_commit,
        } => (
            format!("Fast-forwarded {target_branch} to its remote before pinning the base"),
            json!({
                "remote": remote,
                "target_branch": target_branch,
                "from_commit": from_commit,
                "to_commit": to_commit
            }),
        ),
        TargetRefresh::Raced { target_commit } => (
            format!("Another writer advanced {target_branch} during the refresh"),
            json!({
                "remote": remote,
                "target_branch": target_branch,
                "target_commit": target_commit
            }),
        ),
        TargetRefresh::Diverged {
            local_commit,
            remote_commit,
        } => (
            format!(
                "Local {target_branch} carries commits its remote does not; integration owns the merge"
            ),
            json!({
                "remote": remote,
                "target_branch": target_branch,
                "local_commit": local_commit,
                "remote_commit": remote_commit
            }),
        ),
    };
    ctx.log("git", &message, Some(json_object(detail)))
}

fn materialize_plan_worktree(
    ctx: &WorkflowContext<'_>,
    app_git: &FileGitWorktreeService,
    branch: &str,
    base_commit: &str,
) -> RefineResult<(
    String,
    crate::infrastructure::process::supervisor::operations::OperationHandle,
)> {
    let status = ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status;
    if status != GoalStatus::Plan {
        return Err(RefineError::Conflict(format!(
            "refusing to create a workflow worktree for Goal {} while it is {}; worktrees are materialized only after admission to plan",
            ctx.goal_id,
            status.as_str()
        )));
    }
    crate::application::workflow::engine::context::validate_round_workspace_branch(
        &ctx.work_items.show_goal_detail(&ctx.goal_id)?,
        &ctx.goal_id,
        ctx.round_idx,
        branch,
        &setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}"),
    )?;
    let worktree_target = app_git.managed_worktree_path(branch)?;
    with_repository_git_lock(ctx.target_root, || {
        ctx.revalidate_authority(GoalStatus::Plan)?;
        // The branch is born at the recorded base, never at the shared checkout's
        // HEAD: a human sitting on any branch other than the merge target used to
        // decide where every Round branch started, which made the recorded base a
        // non-ancestor of the candidate and failed integration as "stale".
        let worktree_path =
            app_git.ensure_worktree_from_base(branch, &worktree_target, base_commit)?;
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
        if let Some(outcome) = refresh_candidate_at_quality_boundary(ctx)? {
            return Ok(outcome);
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
            return fail(ctx, "quality", RefineError::Conflict(
                "Quality findings require an explicit workflow action; automatic recovery is disabled".into()));
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

/// Refresh the candidate onto the current merge target at the Implement →
/// Quality boundary, before Quality judges anything.
///
/// `base_commit` is pinned once at Todo → Plan, so without this every
/// collision with a Goal that integrated in the meantime is compressed into
/// the single Governance-time refresh — one late rebase, minutes of Quality
/// later, with the implementing agent's context long gone. This is the same
/// machinery Governance uses, not a second implementation, and the
/// deterministic ladder makes the common case free: a target that has not
/// moved, or a candidate the moved target already contains, is `Unchanged`
/// after one ancestry classification with no worktree touched. When it does
/// rebase it repins `base_commit`/`candidate_commit` and clears the Round's
/// stale gate evidence, so Quality runs against the target the candidate will
/// actually merge into, and the Governance-time refresh — still the last line
/// — is then normally `Unchanged` itself.
///
/// Conflicts retain evidence and settle through Error handling. The repository
/// lock covers this single Git refresh; it never covers the Quality agent.
/// `Some(outcome)` ends the pass; `None` continues into Quality.
pub(crate) fn refresh_candidate_at_quality_boundary(
    ctx: &mut WorkflowContext<'_>,
) -> RefineResult<Option<WorkflowAdvanceOutcome>> {
    let refresh = match refresh_candidate_for_target_advancement(ctx, GoalStatus::Quality) {
        Ok(refresh) => refresh,
        Err(error) => return fail(ctx, "candidate_refresh", error),
    };
    match refresh {
        CandidateRefreshOutcome::Unchanged { .. } => Ok(None),
        CandidateRefreshOutcome::Refreshed {
            original_candidate,
            replacement_candidate,
            target_commit,
            evidence,
        } => {
            ctx.log(
                "candidate_refresh",
                "Refreshed the candidate onto the advanced target before Quality",
                Some(json_object(json!({
                    "original_candidate_commit": original_candidate,
                    "replacement_candidate_commit": replacement_candidate,
                    "replacement_base_commit": target_commit,
                    "refresh_evidence": evidence
                }))),
            )?;
            Ok(None)
        }
        CandidateRefreshOutcome::Stopped { reason, evidence } => {
            ctx.log(
                "candidate_refresh",
                "Pre-Quality candidate refresh failed; awaiting an explicit workflow decision",
                Some(json_object(json!({
                    "reason": reason,
                    "retained_evidence": evidence,

                }))),
            )?;
            ctx.final_status = Some(ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status);
            Ok(Some(WorkflowAdvanceOutcome::Completed {
                final_status: ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status,
                reason:
                    "Pre-Quality candidate refresh failed; awaiting an explicit workflow decision"
                        .to_string(),
            }))
        }
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
        let worktree_path = ctx.refresh_workspace()?.path.display().to_string();
        let agent_cwd = agent_worktree_cwd(
            &worktree_path,
            setting_string(&ctx.settings, "agent_subpath", "").as_str(),
        )?;
        let target_root = ctx.target_root.to_path_buf();

        let integration_service = FileGovernanceIntegrationService::with_target_root(
            ctx.runtime_root,
            ctx.refine_dir(),
            ctx.target_root,
        );
        // Only Git work holds the repository lock. Quality and Governance
        // agents run between the refresh and integration holds; a moved target
        // fails this attempt and requires an explicit workflow decision.
        let mut settled = None;
        'integration: {
            let step = (|| -> RefineResult<GovernanceIntegrationStep> {
                let refresh =
                    refresh_candidate_for_target_advancement(ctx, GoalStatus::Governance)?;
                let expected_target = match refresh {
                    CandidateRefreshOutcome::Stopped { reason, evidence } => {
                        ctx.log(
                            "governance_integration",
                            "Candidate refresh failed; awaiting an explicit workflow decision",
                            Some(json_object(json!({
                                "reason": reason,
                                "retained_evidence": evidence,

                            }))),
                        )?;
                        ctx.final_status =
                            Some(ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status);
                        return Ok(GovernanceIntegrationStep::Outcome(
                            WorkflowAdvanceOutcome::Completed {
                                final_status: ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status,
                                reason:
                                    "Candidate refresh failed; awaiting an explicit workflow decision"
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
                            let recovery = ctx.work_items.settle_integration_failure_summary(
                                &ctx.goal_id,
                                ctx.attempt_authority,
                                &GoalStatus::Governance,
                                &ctx.node_id,
                                "replacement candidate failed exact-candidate Quality",
                                retained.clone(),
                            )?;
                            let final_status = recovery.goal.status;
                            ctx.final_status = Some(final_status.clone());
                            return Ok(GovernanceIntegrationStep::Outcome(
                                WorkflowAdvanceOutcome::Completed {
                                    final_status: final_status.clone(),
                                    reason: "Integration did not complete; an explicit workflow decision is required".to_string(),
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

                crate::application::events::workflow::exit(
                    ctx,
                    GoalStatus::Governance,
                    GoalStatus::Review,
                )?;

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
                    let worktree_git = ctx.candidate_git()?;
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
                // retain the failure for an explicit workflow decision.
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
                // A candidate the target branch no longer descends from is the
                // same fenced-recovery shape as the race above, not a dead end:
                // when the target genuinely moved, a fresh Round from a fresh
                // base is exactly the recovery. `settle_stale_candidate` keeps
                // the one case a fresh Round cannot help terminal.
                Err(stale @ RefineError::StaleCandidate { .. }) => {
                    return settle_stale_candidate(ctx, stale);
                }
                // A conflicted `merge --no-ff` is the rebase conflict's twin:
                // the candidate no longer applies to the advanced target.
                // Route it into the same fenced integration recovery instead
                // of failing the Goal, retaining the conflicted paths the way
                // the rebase path retains `rebase.conflicts`.
                Err(RefineError::MergeConflict {
                    stage: MergeConflictStage::CandidateIntegration,
                    conflicts,
                    message,
                }) => {
                    return settle_integration_merge_conflict(ctx, conflicts, message);
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
                    break 'integration;
                }
                GovernanceIntegrationStep::TargetAdvanced { expected_target } => {
                    ctx.log(
                        "governance_integration",
                        "Target advanced between candidate refresh and integration; stopping this attempt",
                        Some(json_object(json!({
                            "expected_target_commit": expected_target
                        }))),
                    )?;
                }
            }
        }
        let Some((integration, transitioned, candidate_commit)) = settled else {
            let recovery = ctx.work_items.settle_integration_failure_summary(
                &ctx.goal_id,
                ctx.attempt_authority,
                &GoalStatus::Governance,
                &ctx.node_id,
                "target branch advanced during integration",
                json!({ "integration_attempts": 1 }),
            )?;
            let final_status = recovery.goal.status;
            ctx.final_status = Some(final_status.clone());
            return Ok(WorkflowAdvanceOutcome::Completed {
                final_status: final_status.clone(),
                reason: "Integration did not complete; an explicit workflow decision is required"
                    .to_string(),
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

/// Retain stale-candidate evidence and stop the attempt. Distinguish an
/// advanced target from invalid original lineage for the Error handler.
pub(crate) fn settle_stale_candidate(
    ctx: &mut WorkflowContext<'_>,
    error: RefineError,
) -> RefineResult<WorkflowAdvanceOutcome> {
    let RefineError::StaleCandidate {
        candidate_commit,
        recorded_base,
        target_branch,
        target_commit,
    } = &error
    else {
        return fail(ctx, "governance_integration", error);
    };
    let retained = json!({
        "candidate_commit": candidate_commit,
        "recorded_base": recorded_base,
        "target_branch": target_branch,
        "target_commit": target_commit
    });
    let target_never_moved = target_commit == recorded_base;
    if target_never_moved {
        ctx.log(
            "governance_integration",
            "Candidate does not descend from its recorded base and the target never moved; \
             a recovery Round would reproduce this exactly",
            Some(json_object(retained)),
        )?;
        return fail(ctx, "governance_candidate_lineage", error);
    }
    let reason = "candidate is stale against the advanced target";
    let recovery = ctx.work_items.settle_integration_failure_summary(
        &ctx.goal_id,
        ctx.attempt_authority,
        &GoalStatus::Governance,
        &ctx.node_id,
        reason,
        retained.clone(),
    )?;
    let final_status = recovery.goal.status;
    ctx.log(
        "governance_integration",
        "Integration failed; candidate evidence was retained",
        Some(json_object(
            json!({"reason":reason,"retained_evidence":retained}),
        )),
    )?;
    ctx.final_status = Some(final_status.clone());
    Ok(WorkflowAdvanceOutcome::Completed {
        final_status: final_status.clone(),
        reason: "Integration did not complete; an explicit workflow decision is required"
            .to_string(),
    })
}

/// Retain conflicted paths and settle through Error handling without another Round.
pub(crate) fn settle_integration_merge_conflict(
    ctx: &mut WorkflowContext<'_>,
    conflicts: Vec<String>,
    message: String,
) -> RefineResult<WorkflowAdvanceOutcome> {
    let reason = "candidate integration merge conflicted";
    let retained = json!({
        "merge": MergeResult {
            ok: false,
            conflicts,
            message: Some(message),
        }
    });
    let recovery = ctx.work_items.settle_integration_failure_summary(
        &ctx.goal_id,
        ctx.attempt_authority,
        &GoalStatus::Governance,
        &ctx.node_id,
        reason,
        retained.clone(),
    )?;
    let final_status = recovery.goal.status;
    ctx.log(
        "governance_integration",
        "Integration failed; candidate evidence was retained",
        Some(json_object(
            json!({"reason":reason,"retained_evidence":retained}),
        )),
    )?;
    ctx.final_status = Some(final_status.clone());
    Ok(WorkflowAdvanceOutcome::Completed {
        final_status: final_status.clone(),
        reason: "Integration did not complete; an explicit workflow decision is required"
            .to_string(),
    })
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
    let plan = planning_context(ctx)?;
    let implementation_report = round
        .get("implementation_report")
        .and_then(Value::as_str)
        .unwrap_or("No implementation report was recorded.");

    let agent_cwd = agent_worktree_cwd(
        &worktree_path,
        setting_string(&ctx.settings, "agent_subpath", "").as_str(),
    )?;
    let results = crate::application::events::workflow::run(
        ctx,
        GoalStatus::Quality,
        "enter",
        &agent_cwd,
        json!({"agent_context": agent_context, "plans": plan, "implementation_report": implementation_report}),
        &format!("correct-{}", ctx.commit.as_deref().unwrap_or("")),
    )?;
    let report = if results.is_empty() {
        "No required Quality Skills apply; passed without agent checks.".to_string()
    } else {
        results
            .iter()
            .map(|r| format!("{}: {}", r.binding_id, r.summary))
            .collect::<Vec<_>>()
            .join("\n")
    };
    ctx.work_items.update_goal_round_evaluation_summary(
        &ctx.goal_id,
        ctx.round_idx,
        &json!({"quality_skill_results": results}),
    )?;
    let target_branch = setting_string(&ctx.settings, "merge_target_branch", "main");
    let worktree_git = ctx.candidate_git()?;
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
        if results.is_empty() {
            "Quality passed without agent checks"
        } else {
            "Quality Agent completed candidate review and corrections"
        },
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

    let context = goal_agent_context(&json!({}), &json!({"guidance": []}), goal, ctx.round_idx)?;
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

fn evaluate_workflow_governance(
    ctx: &WorkflowContext<'_>,
    worktree_path: &str,
    provider_cwd: &std::path::Path,
    agent_context: &Value,
) -> RefineResult<GovernanceEvaluation> {
    let results = crate::application::events::workflow::run(
        ctx,
        GoalStatus::Governance,
        "enter",
        provider_cwd,
        agent_context.clone(),
        ctx.commit.as_deref().unwrap_or(""),
    )?;
    let failures: Vec<_> = results.iter().filter(|r| r.outcome != "success").collect();
    let actions = failures.iter().flat_map(|result| result.artifacts.get("violations").and_then(Value::as_array).into_iter().flatten().map(move |violation| json!({"rule_id": format!("{}:{}", result.binding_id, violation.get("rule_id").and_then(Value::as_str).unwrap_or("finding")), "state":"failed", "message":violation.get("message"), "evidence":violation.get("evidence")}))).collect::<Vec<_>>();
    let analysis = failures
        .iter()
        .map(|r| {
            r.artifacts
                .get("recovery_analysis")
                .and_then(Value::as_str)
                .unwrap_or(&r.summary)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let recovery = failures
        .iter()
        .filter_map(|r| {
            r.artifacts
                .get("recovery_round_prompt")
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(GovernanceEvaluation {
        failed: !failures.is_empty(),
        message: (!failures.is_empty()).then(|| {
            let messages = failures
                .iter()
                .map(|r| r.summary.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            if messages.is_empty() {
                "Governance agent reported failure.".into()
            } else {
                messages
            }
        }),
        recovery_analysis: (!analysis.trim().is_empty()).then_some(analysis),
        recovery_round_prompt: (!recovery.trim().is_empty()).then_some(recovery),
        details: json_object(
            json!({"phase": "post_implementation", "configured": !results.is_empty(), "skill_results": results, "failed_actions": actions, "worktree": worktree_path, "candidate_commit": ctx.commit}),
        ),
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
            "No required Governance Skills apply; passed without agent checks.".to_string()
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

fn handle_governance_finding(
    ctx: &mut WorkflowContext<'_>,
    evaluation: &GovernanceEvaluation,
) -> RefineResult<WorkflowAdvanceOutcome> {
    fail(
        ctx,
        "governance",
        RefineError::Conflict(
            evaluation.message.clone().unwrap_or_else(|| {
                "Governance findings require an explicit workflow action".into()
            }),
        ),
    )
}

fn quality_failure_category(error: &RefineError) -> &'static str {
    if crate::application::workflow::phases::quality::is_quality_candidate_infrastructure(error) {
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
