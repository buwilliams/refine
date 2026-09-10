use crate::application::work_items::{
    FailureSettlement, FileWorkItemService, WorkflowAttemptAuthority,
};
use crate::application::workflow::{WorkflowEngine, now_timestamp};
use crate::error::RefineError;
use crate::infrastructure::process::supervisor::coordination::with_lock_timeout;
use std::time::Duration;
mod evidence;

impl WorkflowEngine {
    pub(crate) fn settle_goal_failure(
        &self,
        goal_id: &str,
        authority: WorkflowAttemptAuthority,
        failure_stage: &str,
        error: &RefineError,
    ) -> FailureSettlement {
        let evidence = evidence::OriginatingFailure::new(goal_id, authority, failure_stage, error);
        let outcome = evidence::contain("settlement", || {
            self.persist_goal_failure(&evidence, authority)
        })
        .unwrap_or_else(FailureSettlement::UnpersistedEvidence);
        evidence.finalize(self, outcome)
    }

    fn persist_goal_failure(
        &self,
        evidence: &evidence::OriginatingFailure,
        authority: WorkflowAttemptAuthority,
    ) -> FailureSettlement {
        let mut outcome = FailureSettlement::UnpersistedEvidence("settlement did not run".into());
        for attempt in 0..3 {
            let result = with_lock_timeout(Duration::from_millis(200), || {
                #[cfg(test)]
                crate::application::workflow::engine::test_hooks::run(
                    self,
                    &evidence.goal_id,
                    "settlement",
                    authority,
                )?;
                let refine_dir = self
                    .refine_dir()?
                    .ok_or_else(|| RefineError::InvalidInput("missing target".into()))?;
                // Each retry reads Round, claim, node and cancellation again under its record lock.
                FileWorkItemService::with_projection_cache(
                    &refine_dir,
                    &self.runtime_root,
                    self.runtime_root.join("cache"),
                )
                .settle_workflow_attempt_failure(
                    &evidence.goal_id,
                    authority,
                    &evidence.failure_stage,
                    &evidence.original_error,
                    &evidence.failure_at,
                )
            });
            match result {
                Ok(settled) => {
                    outcome = settled;
                    break;
                }
                Err(fault) => {
                    let transient = matches!(&fault, RefineError::Io(_) | RefineError::Degraded(_));
                    outcome = FailureSettlement::UnpersistedEvidence(fault.to_string());
                    if !transient || attempt == 2 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50 * (attempt + 1)));
                }
            }
        }
        outcome
    }
}
