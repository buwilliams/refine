use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationAgentEvidence,
    ImplementationCriticismArtifact, ImplementationExecutionEvidence, ImplementationPlan,
    ImplementationPlanArtifact, ImplementationPlanBinding, ImplementationPlanPhase,
    ImplementationPlanState, ImplementationPlanningOutputAttempt, ProposedImplementationPlan,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::structured_output::DIAGNOSTIC_REPAIR_ATTEMPTS;
use crate::workflow::{goal_agent_prompt, now_timestamp};

use super::context::WorkflowContext;

mod codec;
mod prompts;
mod runtime;

use codec::*;
use prompts::*;
pub(crate) use runtime::recover_interrupted_plan;
use runtime::*;

pub(super) fn run_governed_implementation_planning(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    agent_context: &Value,
    agent_cwd: &Path,
    implementation_branch: &str,
) -> RefineResult<ProposedImplementationPlan> {
    let spec = goal_agent_prompt(&ctx.goal_id, agent_context)?;
    let mut plan = load_or_initialize_plan(ctx, goal, agent_context, implementation_branch)?;
    if plan.state == ImplementationPlanState::Failed {
        return Err(RefineError::Conflict(format!(
            "Goal {} round {} implementation planning previously failed; its evidence was preserved",
            ctx.goal_id,
            ctx.round_idx + 1
        )));
    }

    if plan.proposal.is_none() {
        begin_phase(ctx, &mut plan, ImplementationPlanPhase::Plan)?;
        let (run, result) = run_observational_phase_with_repair(
            ctx,
            &mut plan,
            agent_cwd,
            implementation_branch,
            ImplementationPlanPhase::Plan,
            "plan",
            planning_prompt(&spec),
            "implementation plan",
            decode_plan,
        )?;
        let previous = plan.clone();
        plan.proposal = Some(ImplementationPlanArtifact {
            started_at: run.started_at,
            completed_at: run.completed_at,
            git_before: run.git_before,
            git_after: run.git_after,
            result,
        });
        plan.updated_at = now_timestamp();
        persist_plan(ctx, Some(&previous), &plan)?;
    }

    if plan.criticism.is_none() {
        begin_phase(ctx, &mut plan, ImplementationPlanPhase::Criticize)?;
        let proposal = plan
            .proposal
            .as_ref()
            .expect("proposal phase persisted")
            .result
            .clone();
        let prompt = criticism_prompt(&spec, &proposal)?;
        let (run, result) = run_observational_phase_with_repair(
            ctx,
            &mut plan,
            agent_cwd,
            implementation_branch,
            ImplementationPlanPhase::Criticize,
            "criticize",
            prompt,
            "implementation criticism",
            decode_criticism,
        )?;
        let previous = plan.clone();
        plan.criticism = Some(ImplementationCriticismArtifact {
            started_at: run.started_at,
            completed_at: run.completed_at,
            git_before: run.git_before,
            git_after: run.git_after,
            result,
        });
        plan.updated_at = now_timestamp();
        persist_plan(ctx, Some(&previous), &plan)?;
    }

    if plan.final_plan.is_none() {
        begin_phase(ctx, &mut plan, ImplementationPlanPhase::Revise)?;
        let proposal = plan
            .proposal
            .as_ref()
            .expect("proposal phase persisted")
            .result
            .clone();
        let criticism = plan
            .criticism
            .as_ref()
            .expect("criticism phase persisted")
            .result
            .clone();
        let prompt = revision_prompt(&spec, &proposal, &criticism)?;
        let (run, result) = run_observational_phase_with_repair(
            ctx,
            &mut plan,
            agent_cwd,
            implementation_branch,
            ImplementationPlanPhase::Revise,
            "revise",
            prompt,
            "revised implementation plan",
            |output| {
                let result = decode_plan(output)?;
                validate_revised_plan(&result, &criticism)?;
                Ok(result)
            },
        )?;
        let previous = plan.clone();
        plan.final_plan = Some(ImplementationPlanArtifact {
            started_at: run.started_at,
            completed_at: run.completed_at,
            git_before: run.git_before,
            git_after: run.git_after,
            result,
        });
        plan.updated_at = now_timestamp();
        persist_plan(ctx, Some(&previous), &plan)?;
    }

    Ok(plan.final_plan.expect("revision phase persisted").result)
}

pub(super) fn begin_implementation_phase(
    ctx: &WorkflowContext<'_>,
) -> RefineResult<ProposedImplementationPlan> {
    let mut plan = current_plan(ctx)?;
    let final_plan = plan
        .final_plan
        .as_ref()
        .map(|artifact| artifact.result.clone())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} round {} has no finalized plan",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    begin_phase(ctx, &mut plan, ImplementationPlanPhase::Implement)?;
    Ok(final_plan)
}

pub(super) fn governed_implementation_prompt(
    goal_id: &str,
    agent_context: &Value,
    final_plan: &ProposedImplementationPlan,
) -> RefineResult<String> {
    implementation_prompt(&goal_agent_prompt(goal_id, agent_context)?, final_plan)
}

pub(super) fn complete_implementation_planning(
    ctx: &WorkflowContext<'_>,
    started_at: String,
    report: String,
    evidence: Option<ImplementationExecutionEvidence>,
) -> RefineResult<()> {
    let mut plan = current_plan(ctx)?;
    if plan.phase != ImplementationPlanPhase::Implement || plan.final_plan.is_none() {
        return Err(RefineError::Conflict(format!(
            "Goal {} round {} is not ready to record implementation evidence",
            ctx.goal_id,
            ctx.round_idx + 1
        )));
    }
    let evidence = evidence.or_else(|| {
        (cfg!(test) && ctx.provider == "smoke-ai").then(|| ImplementationExecutionEvidence {
            checklist: plan
                .final_plan
                .as_ref()
                .into_iter()
                .flat_map(|artifact| &artifact.result.checklist)
                .map(|item| crate::model::goal::ImplementationChecklistResult {
                    id: item.id.clone(),
                    outcome: crate::model::goal::ImplementationChecklistOutcome::Completed,
                    evidence: "Completed by the smoke-ai provider fixture".to_string(),
                })
                .collect(),
            verification: Vec::new(),
        })
    });
    let evidence = match evidence.ok_or_else(|| {
        RefineError::Serialization(
            "Goal Agent completion omitted required implementation_evidence".to_string(),
        )
    }) {
        Ok(evidence) => evidence,
        Err(error) => {
            return Err(persist_phase_failure(
                ctx,
                &mut plan,
                ImplementationPlanPhase::Implement,
                "invalid_output",
                error,
            ));
        }
    };
    if let Err(error) = validate_implementation_evidence(&plan, &evidence) {
        return Err(persist_phase_failure(
            ctx,
            &mut plan,
            ImplementationPlanPhase::Implement,
            "invalid_output",
            error,
        ));
    }
    let previous = plan.clone();
    let completed_at = now_timestamp();
    plan.state = ImplementationPlanState::Completed;
    plan.phase_started_at = started_at.clone();
    plan.updated_at = completed_at.clone();
    plan.completed_at = Some(completed_at.clone());
    plan.implementation = Some(ImplementationAgentEvidence {
        started_at,
        completed_at,
        report,
        execution: evidence,
    });
    persist_plan(ctx, Some(&previous), &plan)
}

pub(super) fn fail_implementation_phase(
    ctx: &WorkflowContext<'_>,
    category: &str,
    error: &RefineError,
) -> RefineError {
    if let Ok(mut plan) = current_plan(ctx) {
        return persist_phase_failure(
            ctx,
            &mut plan,
            ImplementationPlanPhase::Implement,
            category,
            RefineError::Conflict(error.to_string()),
        );
    }
    RefineError::Conflict(error.to_string())
}

fn validate_implementation_evidence(
    plan: &ImplementationPlan,
    evidence: &ImplementationExecutionEvidence,
) -> RefineResult<()> {
    let expected = plan
        .final_plan
        .as_ref()
        .expect("validated above")
        .result
        .checklist
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let observed = evidence
        .checklist
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if expected != observed || observed.len() != evidence.checklist.len() {
        return Err(RefineError::Serialization(format!(
            "implementation evidence checklist IDs did not match accepted plan (expected {}, observed {})",
            expected.into_iter().collect::<Vec<_>>().join(", "),
            observed.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(())
}

fn load_or_initialize_plan(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    agent_context: &Value,
    implementation_branch: &str,
) -> RefineResult<ImplementationPlan> {
    let binding = plan_binding(ctx, goal, agent_context, implementation_branch)?;
    let round = goal
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .ok_or_else(|| {
            RefineError::NotFound(format!("Goal {} has no current round", ctx.goal_id))
        })?;
    if let Some(raw) = round
        .get("implementation_plan")
        .filter(|value| !value.is_null())
    {
        let mut plan: ImplementationPlan =
            serde_json::from_value(raw.clone()).map_err(|error| {
                RefineError::Serialization(format!(
                    "invalid implementation planning evidence: {error}"
                ))
            })?;
        if plan.binding != binding {
            return Err(RefineError::Conflict(format!(
                "Goal {} round {} implementation planning binding changed",
                ctx.goal_id,
                ctx.round_idx + 1
            )));
        }
        if plan.schema_version != IMPLEMENTATION_PLAN_SCHEMA_VERSION {
            let previous = plan.clone();
            plan.schema_version = IMPLEMENTATION_PLAN_SCHEMA_VERSION;
            if plan
                .failure
                .as_ref()
                .is_some_and(|failure| failure.category == "interrupted")
            {
                plan.state = ImplementationPlanState::InProgress;
                plan.failure = None;
            }
            plan.updated_at = now_timestamp();
            persist_plan(ctx, Some(&previous), &plan)?;
        }
        return Ok(plan);
    }
    let now = now_timestamp();
    let plan = ImplementationPlan {
        schema_version: IMPLEMENTATION_PLAN_SCHEMA_VERSION,
        state: ImplementationPlanState::InProgress,
        phase: ImplementationPlanPhase::Plan,
        binding,
        started_at: now.clone(),
        phase_started_at: now.clone(),
        updated_at: now,
        completed_at: None,
        proposal: None,
        criticism: None,
        final_plan: None,
        implementation: None,
        failure: None,
        invalid_output_attempts: Vec::new(),
    };
    persist_plan(ctx, None, &plan)?;
    Ok(plan)
}

#[allow(clippy::too_many_arguments)]
fn run_observational_phase_with_repair<T>(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    agent_cwd: &Path,
    branch: &str,
    phase_state: ImplementationPlanPhase,
    phase: &str,
    initial_prompt: String,
    output_label: &str,
    decode: impl Fn(&str) -> RefineResult<T>,
) -> RefineResult<(PlanningPhaseRun, T)> {
    let mut prompt = initial_prompt.clone();
    for attempt in 1..=DIAGNOSTIC_REPAIR_ATTEMPTS + 1 {
        let run = run_observational_phase(
            ctx,
            plan,
            agent_cwd,
            branch,
            phase_state.clone(),
            phase,
            prompt,
        )?;
        match decode(&run.output) {
            Ok(result) => return Ok((run, result)),
            Err(error) => {
                ctx.revalidate_authority(crate::model::workflow::GoalStatus::Plan)?;
                let diagnostics = error.to_string();
                let previous = plan.clone();
                plan.invalid_output_attempts
                    .push(ImplementationPlanningOutputAttempt {
                        phase: phase_state.clone(),
                        attempt,
                        raw_output: run.output.clone(),
                        diagnostics: diagnostics.clone(),
                        recorded_at: now_timestamp(),
                    });
                plan.updated_at = now_timestamp();
                persist_plan(ctx, Some(&previous), plan)?;
                if attempt > DIAGNOSTIC_REPAIR_ATTEMPTS {
                    return Err(persist_run_failure(
                        ctx,
                        plan,
                        phase_state,
                        "invalid_output_repair_exhausted",
                        &run,
                        error,
                    ));
                }
                prompt = structured_output_repair_prompt(
                    &initial_prompt,
                    output_label,
                    attempt,
                    DIAGNOSTIC_REPAIR_ATTEMPTS,
                    &diagnostics,
                    &run.output,
                );
            }
        }
    }
    unreachable!("bounded structured-output loop always returns")
}

fn plan_binding(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    agent_context: &Value,
    implementation_branch: &str,
) -> RefineResult<ImplementationPlanBinding> {
    let required = |key: &str| {
        goal.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| {
                RefineError::Conflict(format!("Goal {} has no pinned {key}", ctx.goal_id))
            })
    };
    let encoded = serde_json::to_vec(agent_context).map_err(|error| {
        RefineError::Serialization(format!("failed to encode pinned agent context: {error}"))
    })?;
    Ok(ImplementationPlanBinding {
        goal_id: ctx.goal_id.clone(),
        round_idx: ctx.round_idx,
        context_version: agent_context
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        context_digest: format!("{:x}", Sha256::digest(encoded)),
        implementation_branch: implementation_branch.to_string(),
        target_branch: required("target_branch")?,
        base_commit: required("base_commit")?,
    })
}

fn begin_phase(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    phase: ImplementationPlanPhase,
) -> RefineResult<()> {
    if plan.phase == phase {
        return Ok(());
    }
    let previous = plan.clone();
    let now = now_timestamp();
    plan.phase = phase;
    plan.state = ImplementationPlanState::InProgress;
    plan.phase_started_at = now.clone();
    plan.updated_at = now;
    plan.failure = None;
    persist_plan(ctx, Some(&previous), plan)
}
