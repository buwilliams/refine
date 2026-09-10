use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::application::workflow::{json_object, now_timestamp};
use crate::error::{RefineError, RefineResult};
use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationAgentEvidence,
    ImplementationExecutionEvidence, ImplementationPlan, ImplementationPlanArtifact,
    ImplementationPlanBinding, ImplementationPlanPhase, ImplementationPlanState,
    ProposedImplementationPlan,
};

use crate::application::workflow::engine::context::WorkflowContext;

mod runtime;

pub(crate) use runtime::recover_interrupted_plan;
use runtime::*;

pub(crate) fn run_governed_implementation_planning(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    agent_context: &Value,
    agent_cwd: &Path,
    implementation_branch: &str,
) -> RefineResult<ProposedImplementationPlan> {
    let mut plan = load_or_initialize_plan(ctx, goal, agent_context, implementation_branch)?;
    if let Some(final_plan) = plan.final_plan.as_ref() {
        return Ok(final_plan.result.clone());
    }
    let git = crate::infrastructure::git::worktrees::FileGitWorktreeService::with_runtime_root(
        agent_cwd,
        ctx.runtime_root,
    );
    let before = git.implementation_planning_observation()?;
    let started_at = now_timestamp();
    let results = (|| {
        let results = crate::application::events::workflow::run(
            ctx,
            crate::model::workflow::GoalStatus::Plan,
            "enter",
            agent_cwd,
            agent_context.clone(),
            "plan",
        )?;
        crate::application::events::workflow::require_success(&results)?;
        Ok(results)
    })();
    let results = match results {
        Ok(results) => results,
        Err(error) => {
            let category = if matches!(error, RefineError::Serialization(_)) {
                "invalid_output"
            } else {
                "provider"
            };
            return Err(persist_phase_failure(
                ctx,
                &mut plan,
                ImplementationPlanPhase::Plan,
                category,
                error,
            ));
        }
    };
    let after = git.implementation_planning_observation()?;
    if before != after {
        return Err(RefineError::Conflict(
            "Plan Skills changed the workspace; changes were retained".into(),
        ));
    }
    let mut collected = ProposedImplementationPlan {
        summary: String::new(),
        checklist: Vec::new(),
        criticism_resolutions: Vec::new(),
    };
    for result in results.iter().filter(|r| r.role == "plan") {
        let value = result
            .artifacts
            .get("plan")
            .cloned()
            .ok_or_else(|| RefineError::InvalidInput("Plan Skill omitted its plan".into()))?;
        let mut next: ProposedImplementationPlan =
            crate::application::agent_io::structured_output::decode_persisted(value, "Skill plan")?;
        <ProposedImplementationPlan as crate::application::agent_io::structured_output::Contract>::validate(&next).map_err(|e| RefineError::InvalidInput(e.to_string()))?;
        if !collected.summary.is_empty() {
            collected.summary.push_str("; ");
        }
        collected
            .summary
            .push_str(&format!("{}: {}", result.binding_id, next.summary));
        for item in &mut next.checklist {
            item.id = format!("{}:{}", result.binding_id, item.id);
        }
        collected.checklist.extend(next.checklist);
    }
    if collected.checklist.is_empty() {
        return Err(RefineError::InvalidInput(
            "Plan Skills produced no actionable plan".into(),
        ));
    }
    let previous = plan.clone();
    let artifact = ImplementationPlanArtifact {
        started_at,
        completed_at: now_timestamp(),
        git_before: before,
        git_after: after,
        result: collected.clone(),
    };
    if plan.proposal.is_none() {
        plan.proposal = Some(artifact.clone());
    }
    plan.final_plan = Some(artifact);
    plan.phase = ImplementationPlanPhase::Plan;
    plan.updated_at = now_timestamp();
    persist_plan(ctx, Some(&previous), &plan)?;
    Ok(collected)
}

pub(crate) fn begin_implementation_phase(
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

pub(crate) fn complete_implementation_planning(
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
            crate::application::agent_io::structured_output::decode_persisted(
                raw.clone(),
                "implementation planning evidence",
            )?;
        // A persisted failure never blocks re-entry: the workflow determines
        // what happens, so a Goal re-queued after a planning failure enters
        // this step again and does the work over. The failed plan is replaced
        // wholesale rather than revived piecemeal — a retained governance
        // pre-check verdict would instantly re-settle the Round it already
        // failed, and a retained repair ledger would describe attempts this
        // fresh pass never made.
        if plan.state == ImplementationPlanState::Failed {
            let failure = plan.failure.clone();
            let fresh = initial_plan(binding);
            persist_plan(ctx, Some(&plan), &fresh)?;
            ctx.log(
                "plan",
                "Planning re-entry replaced the failed prior plan; the step is done over",
                Some(json_object(serde_json::json!({
                    "failed_phase": failure.as_ref().map(|failure| failure.phase.clone()),
                    "failed_category": failure.as_ref().map(|failure| failure.category.clone())
                }))),
            )?;
            return Ok(fresh);
        }
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
    let plan = initial_plan(binding);
    persist_plan(ctx, None, &plan)?;
    Ok(plan)
}

fn initial_plan(binding: ImplementationPlanBinding) -> ImplementationPlan {
    let now = now_timestamp();
    ImplementationPlan {
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
        provider_session_id: None,
        governance_precheck: None,
    }
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
