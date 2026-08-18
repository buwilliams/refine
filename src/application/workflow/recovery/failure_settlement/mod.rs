use serde_json::json;

use crate::application::work_items::{FileWorkItemService, WorkflowAttemptAuthority};
use crate::error::RefineError;
use crate::infrastructure::observability::logs::FileLogService;
use crate::model::log::LogEntry;

use crate::application::workflow::{WorkflowEngine, json_object, now_timestamp};

impl WorkflowEngine {
    pub(crate) fn settle_goal_failure(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        failure_stage: &str,
        error: &RefineError,
    ) -> Option<bool> {
        let refine_dir = self.refine_dir().ok().flatten()?;
        let work_items = FileWorkItemService::new(&refine_dir);
        match work_items.settle_workflow_attempt_failure(
            goal_id,
            authority,
            failure_stage,
            &error.to_string(),
            &now_timestamp(),
        ) {
            Ok(true) => Some(true),
            // The attempt was superseded, so settlement was a no-op; leave a
            // durable trace instead of dropping the failure silently.
            Ok(false) => {
                let _ = FileLogService::new(&refine_dir).append_round_log(
                    goal_id,
                    authority.round_idx,
                    LogEntry {
                        datetime: now_timestamp(),
                        severity: "warning".to_string(),
                        category: "workflow".to_string(),
                        message: format!(
                            "Goal failure was not durably settled (attempt superseded); original error: {error}"
                        ),
                        details: Some(json_object(json!({
                            "failure_stage": failure_stage
                        }))),
                        actions: Vec::new(),
                        actor: Some("refine".to_string()),
                        goal_id: Some(goal_id.to_string()),
                    },
                );
                Some(false)
            }
            Err(settle_error) => {
                eprintln!(
                    "refine workflow failure settlement: Goal {goal_id} {failure_stage} failure was not persisted: {settle_error}; original error: {error}"
                );
                None
            }
        }
    }
}
