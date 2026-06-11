use crate::model::workflow::GapStatus;
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
        from: GapStatus,
        to: GapStatus,
        reason: String,
    },
    Completed {
        final_status: GapStatus,
        reason: String,
    },
    Failed {
        reason: String,
    },
}

pub trait WorkflowBehavior {
    fn observes(&self) -> GapStatus;

    fn advance(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<WorkflowAdvanceOutcome>;
}
