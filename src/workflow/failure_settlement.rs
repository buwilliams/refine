use crate::process::supervisor::errors::RefineError;
use crate::tools::product::work_items::{FileWorkItemService, WorkflowAttemptAuthority};

use super::{WorkflowEngine, now_timestamp};

impl WorkflowEngine {
    pub(super) fn settle_goal_failure(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        failure_stage: &str,
        error: &RefineError,
    ) -> Option<bool> {
        let refine_dir = self.refine_dir().ok().flatten()?;
        let work_items = FileWorkItemService::new(refine_dir);
        work_items
            .settle_workflow_attempt_failure(
                goal_id,
                authority,
                failure_stage,
                &error.to_string(),
                &now_timestamp(),
            )
            .ok()
    }
}
