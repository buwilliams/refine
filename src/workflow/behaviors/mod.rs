use crate::model::workflow::GapStatus;
use crate::tools::product::project_state::GapSummaryProjection;
use crate::tools::supervisor::errors::RefineResult;
use crate::workflow::behavior::{WorkflowBehavior, WorkflowDecision};
use crate::workflow::context::WorkflowContext;

macro_rules! behavior {
    ($name:ident, $status:expr) => {
        #[derive(Clone, Debug, Default)]
        pub struct $name;

        impl WorkflowBehavior for $name {
            fn observes(&self) -> GapStatus {
                $status
            }

            fn evaluate(
                &self,
                gap: &GapSummaryProjection,
                _ctx: &mut WorkflowContext,
            ) -> RefineResult<WorkflowDecision> {
                Ok(WorkflowDecision::Noop {
                    reason: format!(
                        "{} behavior has no standalone decision",
                        gap.gap.status.as_str()
                    ),
                })
            }
        }
    };
}

behavior!(WorkflowBacklog, GapStatus::Backlog);
behavior!(WorkflowTodo, GapStatus::Todo);
behavior!(WorkflowImplementation, GapStatus::InProgress);
behavior!(WorkflowQa, GapStatus::Qa);
behavior!(WorkflowMerge, GapStatus::ReadyMerge);
behavior!(WorkflowBuild, GapStatus::Build);
behavior!(WorkflowReview, GapStatus::Review);
behavior!(WorkflowDone, GapStatus::Done);
behavior!(WorkflowFailed, GapStatus::Failed);
behavior!(WorkflowCancelled, GapStatus::Cancelled);
