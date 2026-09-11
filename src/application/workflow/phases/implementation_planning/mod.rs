//! Planning is a Skill decision and a handoff, not a second workflow.
use crate::application::workflow::engine::context::WorkflowContext;
use crate::error::{RefineError, RefineResult};
use crate::model::workflow::GoalStatus;
use serde_json::Value;
use std::path::Path;

pub(crate) fn run_planning_skills(
    ctx: &WorkflowContext<'_>,
    context: &Value,
    cwd: &Path,
) -> RefineResult<()> {
    ctx.revalidate_authority(GoalStatus::Plan)?;
    let results = crate::application::events::workflow::run(
        ctx,
        GoalStatus::Plan,
        "enter",
        cwd,
        context.clone(),
        "plan",
    )?;
    crate::application::events::workflow::require_success(&results)?;
    Ok(())
}

/// Pass along the agent's own report. Older structured plans remain readable
/// context; neither format is a prerequisite for the authorized Implement step.
pub(crate) fn planning_context(ctx: &WorkflowContext<'_>) -> RefineResult<Value> {
    let goal = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let round = goal["rounds"]
        .as_array()
        .and_then(|r| r.get(ctx.round_idx))
        .ok_or_else(|| RefineError::Conflict("Current Round is unavailable".into()))?;
    // Event completion already records reports under the workflow's lock. Reuse
    // that history instead of maintaining another planning lifecycle or copy.
    let reports = round["event_results"]
        .as_object()
        .into_iter()
        .flat_map(|r| r.values())
        .filter(|r| r["source"] == "workflow.plan.enter")
        .flat_map(|r| {
            r["results"]
                .as_object()
                .into_iter()
                .flat_map(|rs| rs.values())
        })
        .cloned()
        .collect::<Vec<_>>();
    if !reports.is_empty() {
        return Ok(Value::Array(reports));
    }
    Ok(round
        .get("implementation_plan")
        .and_then(|p| p.get("final_plan"))
        .and_then(|p| p.get("result"))
        .cloned()
        .unwrap_or(Value::Null))
}
