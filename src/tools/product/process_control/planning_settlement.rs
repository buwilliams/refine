use super::*;

pub(super) fn planning_stop_settlement(
    goal_id: &str,
    expectation: &GoalCancellationExpectation,
    ownership: &[WorkflowGoalOwnership],
    termination_intent: TerminationIntent,
) -> RefineResult<(
    GoalStopDisposition,
    Option<crate::model::goal::ImplementationPlan>,
)> {
    let Some(plan) = expectation.implementation_plan.as_ref() else {
        return Ok((termination_intent.disposition(), None));
    };
    let round_idx = expectation
        .round_count
        .checked_sub(1)
        .expect("implementation planning evidence requires a Goal round");
    if plan.binding.goal_id != goal_id || plan.binding.round_idx != round_idx {
        return Err(RefineError::Conflict(format!(
            "Goal {goal_id} process settlement found stale implementation planning evidence"
        )));
    }
    if !ownership.is_empty()
        && !ownership.iter().any(|owner| {
            owner.claim_id == plan.binding.claim_id
                && owner.execution_id.as_deref() == Some(plan.binding.execution_id.as_str())
                && owner.round_idx == Some(round_idx)
        })
    {
        return Err(RefineError::Conflict(format!(
            "Goal {goal_id} implementation planning binding does not match the stopped workflow claim and execution"
        )));
    }

    let disposition = if termination_intent == TerminationIntent::InteractiveStop {
        GoalStopDisposition::FailAttempt
    } else {
        termination_intent.disposition()
    };
    if expectation.status != GoalStatus::InProgress
        || plan.state != ImplementationPlanState::InProgress
    {
        return Ok((disposition, None));
    }

    let mut failed = plan.clone();
    let failed_at = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    failed.state = ImplementationPlanState::Failed;
    failed.updated_at = failed_at.clone();
    failed.failure = Some(ImplementationPlanningFailure {
        phase: plan.phase.clone(),
        category: if termination_intent == TerminationIntent::ExplicitCancellation {
            "cancelled"
        } else {
            "interrupted"
        }
        .to_string(),
        message: if termination_intent == TerminationIntent::ExplicitCancellation {
            "implementation planning ended because the Goal workflow was cancelled"
        } else {
            "implementation planning ended because its managed workflow process was stopped"
        }
        .to_string(),
        failed_at,
        operation_id: plan
            .active_process
            .as_ref()
            .map(|process| process.operation_id.clone()),
        process_id: plan
            .active_process
            .as_ref()
            .and_then(|process| process.process_id.clone()),
        git_before: None,
        git_after: None,
        process: plan.active_process.clone(),
    });
    failed.active_process = None;
    Ok((disposition, Some(failed)))
}
