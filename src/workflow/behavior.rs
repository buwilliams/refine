use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::RefineResult;
use crate::workflow::context::WorkflowContext;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowAdvanceOutcome {
    Noop {
        reason: String,
    },
    Blocked {
        reason: String,
    },
    Transition {
        from: GoalStatus,
        to: GoalStatus,
        reason: String,
    },
    Completed {
        final_status: GoalStatus,
        reason: String,
    },
    Failed {
        reason: String,
    },
}

pub trait WorkflowBehavior {
    fn observes(&self) -> GoalStatus;

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome>;
}
