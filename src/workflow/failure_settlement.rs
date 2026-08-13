use serde_json::json;

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::RefineError;
use crate::tools::product::work_items::FileWorkItemService;

use super::{WorkflowEngine, now_timestamp};

impl WorkflowEngine {
    pub(super) fn settle_goal_failure(
        &self,
        goal_id: &str,
        failure_stage: &str,
        error: &RefineError,
    ) -> Option<()> {
        let refine_dir = self.refine_dir().ok().flatten()?;
        let work_items = FileWorkItemService::new(refine_dir);
        let current = work_items.show_goal_summary(goal_id).ok()?;
        if matches!(
            current.goal.status,
            GoalStatus::Plan | GoalStatus::Implement | GoalStatus::Quality | GoalStatus::Governance
        ) {
            work_items.fail_automated_goal_if_active(goal_id).ok()?;
        }
        let _ = work_items.update_latest_goal_round_evaluation_summary(
            goal_id,
            &json!({
                "failure_category": failure_stage,
                "failure_message": error.to_string(),
                "failure_at": now_timestamp()
            }),
        );
        Some(())
    }
}
