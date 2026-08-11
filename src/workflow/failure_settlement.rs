use serde_json::json;

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::product::work_items::{FileWorkItemService, workflow_revision};

use super::{WorkflowClaimState, WorkflowEngine, now_timestamp};

impl WorkflowEngine {
    pub(super) fn bind_started_claim_identity(
        &self,
        claim_id: &str,
        execution_id: &str,
        work_items: &FileWorkItemService,
        round_idx: usize,
    ) -> RefineResult<()> {
        let goal_id = self.claim_by_id(claim_id)?.goal_id;
        let goal_revision = workflow_revision(&work_items.show_goal_detail(&goal_id)?);
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let claim = state
            .claims
            .iter_mut()
            .find(|claim| claim.claim_id == claim_id)
            .ok_or_else(|| RefineError::NotFound(format!("claim {claim_id} was not found")))?;
        if claim.state != WorkflowClaimState::Running
            || claim.execution_id.as_deref() != Some(execution_id)
        {
            return Err(RefineError::Conflict(format!(
                "execution {execution_id} no longer owns claim {claim_id}"
            )));
        }
        if claim.round_idx == Some(round_idx) && claim.goal_revision == Some(goal_revision) {
            return Ok(());
        }
        claim.round_idx = Some(round_idx);
        claim.goal_revision = Some(goal_revision);
        claim.decision_version = claim.decision_version.saturating_add(1);
        claim.updated_at = now_timestamp();
        self.save_state(&mut state)
    }

    pub(super) fn settle_goal_failure(
        &self,
        goal_id: &str,
        failure_stage: &str,
        error: &RefineError,
    ) -> Option<(Option<usize>, Option<u64>)> {
        let refine_dir = self.refine_dir().ok().flatten()?;
        let work_items = FileWorkItemService::new(refine_dir);
        let current = work_items.show_goal_summary(goal_id).ok()?;
        if current.goal.status != GoalStatus::Failed {
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
        self.goal_identity(&work_items, goal_id)
    }

    pub(super) fn current_goal_identity(
        &self,
        goal_id: &str,
    ) -> Option<(Option<usize>, Option<u64>)> {
        let refine_dir = self.refine_dir().ok().flatten()?;
        self.goal_identity(&FileWorkItemService::new(refine_dir), goal_id)
    }

    fn goal_identity(
        &self,
        work_items: &FileWorkItemService,
        goal_id: &str,
    ) -> Option<(Option<usize>, Option<u64>)> {
        let detail = work_items.show_goal_detail(goal_id).ok()?;
        let round_idx = detail
            .get("rounds")
            .and_then(|rounds| rounds.as_array())
            .and_then(|rounds| rounds.len().checked_sub(1));
        Some((round_idx, Some(workflow_revision(&detail))))
    }
}
