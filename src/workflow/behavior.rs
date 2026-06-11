use crate::model::workflow::GapStatus;
use crate::tools::product::project_state::GapSummaryProjection;
use crate::tools::supervisor::errors::RefineResult;
use crate::workflow::context::WorkflowContext;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowDecision {
    Noop {
        reason: String,
    },
    Transition {
        from: GapStatus,
        to: GapStatus,
        reason: String,
    },
}

pub trait WorkflowBehavior {
    fn observes(&self) -> GapStatus;

    fn evaluate(
        &self,
        gap: &GapSummaryProjection,
        ctx: &mut WorkflowContext,
    ) -> RefineResult<WorkflowDecision>;
}
