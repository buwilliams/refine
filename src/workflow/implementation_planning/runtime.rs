use std::path::Path;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::model::goal::{
    ImplementationPlan, ImplementationPlanPhase, ImplementationPlanState,
    ImplementationPlanningFailure, PlanningGitObservation, PlanningProcessEvidence,
};
use crate::process::agent_sessions::{
    GoalAgentLaunch, GoalAgentSettlement, run_goal_agent_with_settlement,
};
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::FileGitWorktreeService;
use crate::workflow::{WorkflowClaimState, WorkflowEngine, now_timestamp};

use super::WorkflowContext;

pub(super) struct PlanningPhaseRun {
    pub started_at: String,
    pub completed_at: String,
    pub process: PlanningProcessEvidence,
    pub git_before: PlanningGitObservation,
    pub git_after: PlanningGitObservation,
    pub output: String,
}

pub(super) fn run_observational_phase(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    agent_cwd: &Path,
    branch: &str,
    phase_state: ImplementationPlanPhase,
    phase: &str,
    prompt: String,
) -> RefineResult<PlanningPhaseRun> {
    let git = FileGitWorktreeService::with_runtime_root(agent_cwd, ctx.runtime_root);
    let git_before = git.implementation_planning_observation()?;
    let started_at = now_timestamp();
    let operation_id = Uuid::new_v4().to_string();
    let process_evidence = PlanningProcessEvidence {
        operation_id: operation_id.clone(),
        process_id: None,
        provider: ctx.provider.clone(),
        state: None,
        exit_code: None,
        output: None,
        structured_output: None,
    };
    let previous = plan.clone();
    plan.active_process = Some(process_evidence.clone());
    plan.updated_at = now_timestamp();
    persist_plan(ctx, Some(&previous), plan)?;
    let mut metadata =
        ctx.workflow_process_metadata("in-progress", "WorkflowImplementationPlanning");
    metadata.insert("implementation_phase".to_string(), json!(phase));
    metadata.insert("operation_id".to_string(), json!(&operation_id));
    metadata.insert("provider".to_string(), json!(&ctx.provider));
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
        let output = match phase {
            "criticize" => json!({
                "summary": "Smoke AI fixture found no material omissions.",
                "findings": []
            }),
            _ => json!({
                "summary": "Smoke AI fixture implementation plan.",
                "checklist": [{
                    "id": "P1",
                    "description": "Implement and verify the current Round request.",
                    "affected_behavior": [],
                    "governance_rationale": null,
                    "verification": []
                }],
                "criticism_resolutions": []
            }),
        };
        return Ok(PlanningPhaseRun {
            started_at,
            completed_at: now_timestamp(),
            process: process_evidence,
            git_before: git_before.clone(),
            git_after: git_before,
            output: output.to_string(),
        });
    }
    let invocation = run_goal_agent_with_settlement(
        GoalAgentLaunch {
            runtime_root: ctx.runtime_root.to_path_buf(),
            cwd: agent_cwd.to_path_buf(),
            provider: ctx.provider.clone(),
            prompt,
            metadata,
        },
        |attention| {
            let _ = ctx.log(
                "implementation_planning",
                &format!("Implementation {phase} agent is waiting for user input"),
                Some(crate::workflow::json_object(json!({
                    "phase": phase,
                    "message": attention.message,
                    "operation_id": operation_id
                }))),
            );
        },
        |settlement| persist_active_process_settlement(ctx, plan, settlement),
    );
    let git_after = git.implementation_planning_observation()?;
    if git_after != git_before {
        let error = RefineError::Conflict(format!(
            "implementation {phase} phase changed the worktree; changes and process evidence were retained"
        ));
        let (process_id, message) = match &invocation {
            Ok(result) => (Some(result.process_id.clone()), error.to_string()),
            Err(provider_error) => (
                None,
                format!("{error}; provider also failed: {provider_error}"),
            ),
        };
        let failure_result = record_failure(
            ctx,
            plan,
            ImplementationPlanningFailure {
                phase: phase_state,
                category: "worktree_mutation".to_string(),
                message,
                failed_at: now_timestamp(),
                operation_id: Some(operation_id),
                process_id,
                git_before: Some(git_before),
                git_after: Some(git_after),
                process: plan.active_process.clone(),
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
                    operation_id: Some(operation_id),
                    process_id: None,
                    git_before: Some(git_before),
                    git_after: Some(git_after),
                    process: plan.active_process.clone(),
                },
            );
            return Err(failure_or_persistence_error(error, failure_result));
        }
    };
    let output = match result.planning_result {
        Some(result) => serde_json::to_string(&result).map_err(|error| {
            RefineError::Serialization(format!("failed to encode planning result: {error}"))
        })?,
        None => result.output,
    };
    Ok(PlanningPhaseRun {
        started_at,
        completed_at: now_timestamp(),
        process: PlanningProcessEvidence {
            operation_id,
            process_id: Some(result.process_id),
            provider: ctx.provider.clone(),
            state: plan
                .active_process
                .as_ref()
                .and_then(|process| process.state.clone()),
            exit_code: plan
                .active_process
                .as_ref()
                .and_then(|process| process.exit_code),
            output: plan
                .active_process
                .as_ref()
                .and_then(|process| process.output.clone()),
            structured_output: plan
                .active_process
                .as_ref()
                .and_then(|process| process.structured_output.clone()),
        },
        git_before,
        git_after,
        output,
    })
}

pub(super) fn arm_implementation_phase(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    _agent_cwd: &Path,
) -> RefineResult<()> {
    if plan.active_process.is_some() {
        return Ok(());
    }
    let previous = plan.clone();
    plan.active_process = Some(PlanningProcessEvidence {
        operation_id: Uuid::new_v4().to_string(),
        process_id: None,
        provider: ctx.provider.clone(),
        state: None,
        exit_code: None,
        output: None,
        structured_output: None,
    });
    plan.updated_at = now_timestamp();
    persist_plan(ctx, Some(&previous), plan)
}

pub(super) fn persist_run_failure(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    phase: ImplementationPlanPhase,
    category: &str,
    run: &PlanningPhaseRun,
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
            operation_id: Some(run.process.operation_id.clone()),
            process_id: run.process.process_id.clone(),
            git_before: Some(run.git_before.clone()),
            git_after: Some(run.git_after.clone()),
            process: Some(run.process.clone()),
        },
    );
    failure_or_persistence_error(error, failure_result)
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
            operation_id: None,
            process_id: None,
            git_before: None,
            git_after: None,
            process: plan.active_process.clone(),
        },
    );
    failure_or_persistence_error(error, failure_result)
}

fn record_failure(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    failure: ImplementationPlanningFailure,
) -> RefineResult<()> {
    let previous = plan.clone();
    plan.phase = failure.phase.clone();
    plan.state = ImplementationPlanState::Failed;
    plan.updated_at = now_timestamp();
    plan.failure = Some(failure);
    plan.active_process = None;
    persist_plan(ctx, Some(&previous), plan)
}

fn failure_or_persistence_error(
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

pub(super) fn persist_active_process_settlement(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    settlement: &GoalAgentSettlement,
) -> RefineResult<()> {
    let previous = plan.clone();
    let process = plan.active_process.as_mut().ok_or_else(|| {
        RefineError::Conflict(
            "implementation planning phase has no active process fence".to_string(),
        )
    })?;
    process.process_id = Some(settlement.process_id.clone());
    process.state = Some(settlement.state.clone());
    process.exit_code = settlement.exit_code;
    process.output = Some(settlement.output.clone());
    process.structured_output = settlement.planning_result.clone();
    plan.updated_at = now_timestamp();
    persist_plan(ctx, Some(&previous), plan)
}

pub(in crate::workflow) fn record_current_process_settlement(
    ctx: &WorkflowContext<'_>,
    settlement: &GoalAgentSettlement,
) -> RefineResult<()> {
    let mut plan = current_plan(ctx)?;
    persist_active_process_settlement(ctx, &mut plan, settlement)
}

pub(super) fn current_plan(ctx: &WorkflowContext<'_>) -> RefineResult<ImplementationPlan> {
    ctx.work_items
        .show_goal_detail(&ctx.goal_id)?
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .and_then(|round| round.get("implementation_plan"))
        .cloned()
        .filter(|value| !value.is_null())
        .ok_or_else(|| {
            RefineError::NotFound("implementation planning evidence is missing".to_string())
        })
        .and_then(|value| {
            serde_json::from_value(value).map_err(|error| {
                RefineError::Serialization(format!(
                    "invalid implementation planning evidence: {error}"
                ))
            })
        })
}

pub(in crate::workflow) fn persist_plan(
    ctx: &WorkflowContext<'_>,
    expected: Option<&ImplementationPlan>,
    plan: &ImplementationPlan,
) -> RefineResult<()> {
    let _coordination = acquire_workflow_coordination(&ctx.refine_dir())?;
    let state = WorkflowEngine::with_target_root(ctx.runtime_root, ctx.target_root).load_state()?;
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.claim_id == ctx.claim_id)
        .ok_or_else(|| {
            RefineError::Conflict(format!("workflow claim {} disappeared", ctx.claim_id))
        })?;
    if claim.goal_id != ctx.goal_id
        || claim.execution_id.as_deref() != Some(ctx.execution_id.as_str())
        || claim.state != WorkflowClaimState::Running
    {
        return Err(RefineError::Conflict(format!(
            "execution {} no longer owns implementation planning claim {} for Goal {}",
            ctx.execution_id, ctx.claim_id, ctx.goal_id
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
