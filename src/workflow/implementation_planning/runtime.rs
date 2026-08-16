use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationPlan, ImplementationPlanPhase,
    ImplementationPlanState, ImplementationPlanningFailure, PlanningGitObservation,
};
use crate::model::workflow::GoalStatus;
use crate::process::agent_sessions::{GoalAgentLaunch, run_goal_agent_with_settlement};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::agent_providers::ProviderSessionContinuity;
use crate::tools::host::git_worktrees::FileGitWorktreeService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::{now_timestamp, setting_usize};

use super::WorkflowContext;

pub(super) struct PlanningPhaseRun {
    pub started_at: String,
    pub completed_at: String,
    pub git_before: PlanningGitObservation,
    pub git_after: PlanningGitObservation,
    pub output: String,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_observational_phase(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    agent_cwd: &Path,
    branch: &str,
    phase_state: ImplementationPlanPhase,
    phase: &str,
    prompt: String,
    provider_session: Option<ProviderSessionContinuity>,
) -> RefineResult<PlanningPhaseRun> {
    ctx.revalidate_authority(GoalStatus::Plan)?;
    let git = FileGitWorktreeService::with_runtime_root(agent_cwd, ctx.runtime_root);
    let git_before = git.implementation_planning_observation()?;
    let started_at = now_timestamp();
    let operation_id = Uuid::new_v4().to_string();
    let mut metadata = ctx.workflow_process_metadata("plan", "WorkflowPlan");
    metadata.insert("implementation_phase".to_string(), json!(phase));
    metadata.insert("operation_id".to_string(), json!(&operation_id));
    metadata.insert("cwd".to_string(), json!(agent_cwd.display().to_string()));
    metadata.insert(
        "worktree".to_string(),
        json!({"path": agent_cwd, "branch": branch}),
    );
    metadata.insert("observational_phase".to_string(), json!(true));
    if cfg!(test)
        && ctx.provider == "smoke-ai"
        && std::env::var("REFINE_SMOKE_AI_GOVERNED_PLANNING").as_deref() != Ok("1")
    {
        // Fixtures are serialized from the durable contract types so they can
        // never drift from what the decoder accepts.
        let output = match phase {
            "criticize" => serde_json::to_string(&crate::model::goal::ImplementationCriticism {
                summary: "Smoke AI fixture found no material omissions.".to_string(),
                findings: Vec::new(),
            }),
            _ => serde_json::to_string(&crate::model::goal::ProposedImplementationPlan {
                summary: "Smoke AI fixture implementation plan.".to_string(),
                checklist: vec![crate::model::goal::ImplementationChecklistItem {
                    id: "P1".to_string(),
                    description: "Implement and verify the current Round request.".to_string(),
                    affected_behavior: Vec::new(),
                    governance_rationale: None,
                    verification: Vec::new(),
                }],
                criticism_resolutions: Vec::new(),
            }),
        }
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode smoke fixture: {error}"))
        })?;
        return Ok(PlanningPhaseRun {
            started_at,
            completed_at: now_timestamp(),
            git_before: git_before.clone(),
            git_after: git_before,
            output,
        });
    }
    let launch_with_session =
        |provider_session: Option<ProviderSessionContinuity>| GoalAgentLaunch {
            provider_session,
            runtime_root: ctx.runtime_root.to_path_buf(),
            cwd: agent_cwd.to_path_buf(),
            provider: ctx.provider.clone(),
            prompt: prompt.clone(),
            metadata: metadata.clone(),
            completion_timeout: Some(Duration::from_secs(setting_usize(
                &ctx.settings,
                "agent_hard_cap_seconds",
                7200,
            ) as u64)),
            idle_timeout: crate::workflow::agent_idle_timeout(&ctx.settings),
        };
    let on_attention = |attention: crate::process::agent_sessions::GoalAgentAttention| {
        let _ = ctx.log(
            "implementation_planning",
            &format!("Implementation {phase} agent is waiting for user input"),
            Some(crate::workflow::json_object(json!({
                "phase": phase,
                "message": attention.message
            }))),
        );
    };
    let invocation = run_goal_agent_with_settlement(
        launch_with_session(provider_session.clone()),
        on_attention,
        |_| Ok(()),
    );
    let invocation = match invocation {
        // The pinned provider session can be gone by now — pruned by the CLI
        // or created in an earlier daemon lifetime. Observational phases are
        // read-only, so a fresh launch preserves the phase instead of failing
        // the Round over lost provider-side session state.
        Err(resume_error)
            if matches!(provider_session, Some(ProviderSessionContinuity::Resume(_)))
                && git.implementation_planning_observation()? == git_before =>
        {
            let _ = ctx.log(
                "implementation_planning",
                &format!(
                    "Implementation {phase} agent could not resume its provider session; retrying fresh"
                ),
                Some(crate::workflow::json_object(json!({
                    "phase": phase,
                    "resume_error": resume_error.to_string()
                }))),
            );
            run_goal_agent_with_settlement(launch_with_session(None), on_attention, |_| Ok(()))
        }
        other => other,
    };
    let git_after = git.implementation_planning_observation()?;
    // A provider may finish after cancellation, node transfer, Undo, or a replacement Round.
    // Observe its checkout for audit purposes, but never persist or repair a superseded result.
    ctx.revalidate_authority(GoalStatus::Plan)?;
    if git_after != git_before {
        let error = RefineError::Conflict(format!(
            "implementation {phase} phase changed the worktree; changes were retained"
        ));
        let message = match &invocation {
            Ok(_) => error.to_string(),
            Err(provider_error) => format!("{error}; provider also failed: {provider_error}"),
        };
        let failure_result = record_failure(
            ctx,
            plan,
            ImplementationPlanningFailure {
                phase: phase_state,
                category: "worktree_mutation".to_string(),
                message,
                failed_at: now_timestamp(),
                git_before: Some(git_before),
                git_after: Some(git_after),
            },
        );
        return Err(failure_or_persistence_error(error, failure_result));
    }
    let result = match invocation {
        Ok(result) => result,
        Err(error) => {
            let failure_result = record_failure(
                ctx,
                plan,
                ImplementationPlanningFailure {
                    phase: phase_state,
                    category: "provider".to_string(),
                    message: error.to_string(),
                    failed_at: now_timestamp(),
                    git_before: Some(git_before),
                    git_after: Some(git_after),
                },
            );
            return Err(failure_or_persistence_error(error, failure_result));
        }
    };
    let output = match result.planning_result {
        Some(result) => serde_json::to_string(&result).map_err(|error| {
            RefineError::Serialization(format!("failed to encode planning result: {error}"))
        })?,
        // An accepted completed signal always carries `planning_result`, so its
        // absence means the agent exited without ever signalling completion.
        // Decoding the terminal transcript in its place is guaranteed to fail
        // the structured-output contract with a misleading plan-JSON diagnostic
        // and burns every repair attempt reproducing the same shape; fail as
        // the infrastructure fault it is instead.
        None => {
            let error = RefineError::Degraded(format!(
                "implementation {phase} agent exited without a structured completion signal; its terminal transcript is not a plan and was not decoded"
            ));
            let failure_result = record_failure(
                ctx,
                plan,
                ImplementationPlanningFailure {
                    phase: phase_state,
                    category: "provider".to_string(),
                    message: error.to_string(),
                    failed_at: now_timestamp(),
                    git_before: Some(git_before),
                    git_after: Some(git_after),
                },
            );
            return Err(failure_or_persistence_error(error, failure_result));
        }
    };
    Ok(PlanningPhaseRun {
        started_at,
        completed_at: now_timestamp(),
        git_before,
        git_after,
        output,
    })
}

pub(super) fn persist_phase_failure(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    phase: ImplementationPlanPhase,
    category: &str,
    error: RefineError,
) -> RefineError {
    let failure_result = record_failure(
        ctx,
        plan,
        ImplementationPlanningFailure {
            phase,
            category: category.to_string(),
            message: error.to_string(),
            failed_at: now_timestamp(),
            git_before: None,
            git_after: None,
        },
    );
    failure_or_persistence_error(error, failure_result)
}

pub(super) fn record_failure(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    failure: ImplementationPlanningFailure,
) -> RefineResult<()> {
    let previous = plan.clone();
    plan.phase = failure.phase.clone();
    plan.state = ImplementationPlanState::Failed;
    plan.updated_at = now_timestamp();
    plan.failure = Some(failure);
    persist_plan(ctx, Some(&previous), plan)
}

pub(super) fn failure_or_persistence_error(
    original: RefineError,
    persistence: RefineResult<()>,
) -> RefineError {
    match persistence {
        Ok(()) => original,
        Err(persistence) => RefineError::Conflict(format!(
            "{original}; additionally failed to persist implementation planning failure evidence: {persistence}"
        )),
    }
}

pub(super) fn current_plan(ctx: &WorkflowContext<'_>) -> RefineResult<ImplementationPlan> {
    let value = ctx
        .work_items
        .show_goal_detail(&ctx.goal_id)?
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .and_then(|round| round.get("implementation_plan"))
        .cloned()
        .filter(|value| !value.is_null())
        .ok_or_else(|| {
            RefineError::NotFound("implementation planning evidence is missing".to_string())
        })?;
    decode_plan(value)
}

fn decode_plan(value: Value) -> RefineResult<ImplementationPlan> {
    crate::structured_output::decode_persisted(value, "implementation planning evidence")
}

pub(in crate::workflow) fn persist_plan(
    ctx: &WorkflowContext<'_>,
    expected: Option<&ImplementationPlan>,
    plan: &ImplementationPlan,
) -> RefineResult<()> {
    let summary = ctx.work_items.show_goal_summary(&ctx.goal_id)?;
    let node = summary.goal.node_id.as_deref().unwrap_or("default");
    if !matches!(
        summary.goal.status,
        GoalStatus::Plan | GoalStatus::Implement
    ) || node != ctx.node_id
        || summary.goal.round_count != ctx.round_idx + 1
    {
        return Err(RefineError::Conflict(format!(
            "Goal {} no longer authorizes implementation planning on node {} round {}",
            ctx.goal_id,
            ctx.node_id,
            ctx.round_idx + 1
        )));
    }
    ctx.work_items.replace_goal_round_implementation_plan(
        &ctx.goal_id,
        ctx.round_idx,
        expected,
        plan,
    )?;
    Ok(())
}

pub(crate) fn recover_interrupted_plan(
    work_items: &FileWorkItemService,
    goal_id: &str,
    round_idx: usize,
) -> RefineResult<()> {
    let detail = work_items.show_goal_detail(goal_id)?;
    let Some(raw) = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(round_idx))
        .and_then(|round| round.get("implementation_plan"))
        .cloned()
        .filter(|value| !value.is_null())
    else {
        return Ok(());
    };
    let mut plan = decode_plan(raw)?;
    let previous = plan.clone();
    let interrupted_failure = plan
        .failure
        .as_ref()
        .is_some_and(|failure| failure.category == "interrupted");
    if plan.schema_version == IMPLEMENTATION_PLAN_SCHEMA_VERSION && !interrupted_failure {
        return Ok(());
    }
    plan.schema_version = IMPLEMENTATION_PLAN_SCHEMA_VERSION;
    if interrupted_failure {
        plan.state = ImplementationPlanState::InProgress;
        plan.failure = None;
    }
    plan.updated_at = now_timestamp();
    work_items.replace_goal_round_implementation_plan(
        goal_id,
        round_idx,
        Some(&previous),
        &plan,
    )?;
    Ok(())
}
