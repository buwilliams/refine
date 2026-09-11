use serde_json::Value;

use crate::application::workflow::now_timestamp;
use crate::error::{RefineError, RefineResult};
use crate::model::goal::{
    ImplementationPlan, ImplementationPlanPhase, ImplementationPlanState,
    ImplementationPlanningFailure,
};
use crate::model::workflow::GoalStatus;

use super::WorkflowContext;

pub(crate) fn persist_phase_failure(
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

pub(crate) fn record_failure(
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

pub(crate) fn failure_or_persistence_error(
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

pub(crate) fn current_plan(ctx: &WorkflowContext<'_>) -> RefineResult<ImplementationPlan> {
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
    crate::application::agent_io::structured_output::decode_persisted(
        value,
        "implementation planning evidence",
    )
}

pub(in crate::application::workflow) fn persist_plan(
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
