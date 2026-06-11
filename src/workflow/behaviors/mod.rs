use crate::model::workflow::GapStatus;
use crate::process::supervisor::errors::RefineResult;
use crate::tools::product::project_state::GapSummaryProjection;
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::context::WorkflowContext;

macro_rules! behavior {
    ($name:ident, $status:expr) => {
        #[derive(Clone, Debug, Default)]
        pub struct $name;

        impl WorkflowBehavior for $name {
            fn observes(&self) -> GapStatus {
                $status
            }

            fn advance(
                &self,
                gap: &GapSummaryProjection,
                _ctx: &mut WorkflowContext,
            ) -> RefineResult<WorkflowAdvanceOutcome> {
                Ok(WorkflowAdvanceOutcome::Blocked {
                    reason: format!(
                        "{} behavior is coordinated by WorkflowEngine",
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
behavior!(WorkflowReadyMerge, GapStatus::ReadyMerge);
behavior!(WorkflowBuild, GapStatus::Build);
behavior!(WorkflowReview, GapStatus::Review);
behavior!(WorkflowDone, GapStatus::Done);
behavior!(WorkflowFailed, GapStatus::Failed);
behavior!(WorkflowCancelled, GapStatus::Cancelled);
