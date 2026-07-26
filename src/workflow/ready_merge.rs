use super::*;

impl WorkflowEngine {
    /// Atomically commits the exact execution and Goal revision allowed to perform Ready Merge.
    pub fn commit_ready_merge_fence(
        &self,
        claim_id: &str,
        execution_id: &str,
        goal_id: &str,
        node_id: &str,
        round_idx: usize,
        goal_revision: u64,
    ) -> RefineResult<WorkflowExecutionFence> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        if let Some(other) = state.claims.iter().find(|claim| {
            claim.goal_id == goal_id
                && claim.claim_id != claim_id
                && matches!(
                    claim.state,
                    WorkflowClaimState::Claimed | WorkflowClaimState::Running
                )
        }) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} has unequal concurrent claims {claim_id} at revision {goal_revision} and {} at revision {}",
                other.claim_id,
                other.goal_revision.unwrap_or(0)
            )));
        }
        let claim = state
            .claims
            .iter_mut()
            .find(|claim| claim.claim_id == claim_id)
            .ok_or_else(|| RefineError::NotFound(format!("claim {claim_id} was not found")))?;
        if claim.goal_id != goal_id
            || claim.node_id != node_id
            || claim.execution_id.as_deref() != Some(execution_id)
            || claim.state != WorkflowClaimState::Running
        {
            return Err(RefineError::Conflict(format!(
                "execution {execution_id} no longer owns active claim {claim_id} for Goal {goal_id}"
            )));
        }
        let commitment_changed =
            claim.round_idx != Some(round_idx) || claim.goal_revision != Some(goal_revision);
        if commitment_changed {
            claim.round_idx = Some(round_idx);
            claim.goal_revision = Some(goal_revision);
            claim.decision_version = claim.decision_version.saturating_add(1);
            claim.updated_at = now_timestamp();
        }
        let fence = WorkflowExecutionFence {
            claim_id: claim.claim_id.clone(),
            execution_id: execution_id.to_string(),
            goal_id: goal_id.to_string(),
            node_id: node_id.to_string(),
            round_idx,
            goal_revision,
            decision_version: claim.decision_version,
        };
        if commitment_changed {
            self.save_state(&mut state)?;
        }
        Ok(fence)
    }

    /// Revalidates Ready Merge authority immediately before a side effect or settlement.
    pub fn verify_ready_merge_fence(&self, fence: &WorkflowExecutionFence) -> RefineResult<()> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let state = self.load_state()?;
        if let Some(other) = state.claims.iter().find(|claim| {
            claim.goal_id == fence.goal_id
                && claim.claim_id != fence.claim_id
                && matches!(
                    claim.state,
                    WorkflowClaimState::Claimed | WorkflowClaimState::Running
                )
        }) {
            return Err(RefineError::Conflict(format!(
                "Goal {} has unequal concurrent claims {} at revision {} and {} at revision {}",
                fence.goal_id,
                fence.claim_id,
                fence.goal_revision,
                other.claim_id,
                other.goal_revision.unwrap_or(0)
            )));
        }
        let claim = state
            .claims
            .iter()
            .find(|claim| claim.claim_id == fence.claim_id)
            .ok_or_else(|| {
                RefineError::Conflict(format!("Ready Merge claim {} was replaced", fence.claim_id))
            })?;
        if claim.goal_id != fence.goal_id
            || claim.node_id != fence.node_id
            || claim.execution_id.as_deref() != Some(fence.execution_id.as_str())
            || claim.round_idx != Some(fence.round_idx)
            || claim.goal_revision != Some(fence.goal_revision)
            || claim.decision_version != fence.decision_version
            || claim.state != WorkflowClaimState::Running
        {
            return Err(RefineError::Conflict(format!(
                "execution {} no longer owns Ready Merge claim {}",
                fence.execution_id, fence.claim_id
            )));
        }
        Ok(())
    }

    /// Advances a Ready Merge fence after the integration owner writes its durable evidence.
    ///
    /// The workflow coordination lease must cover both the evidence write and this update. That
    /// keeps the claim's committed Goal revision equal to the actual record revision before
    /// operation settlement and the Ready Merge transition.
    pub fn advance_ready_merge_fence_revision(
        &self,
        fence: &mut WorkflowExecutionFence,
        goal_revision: u64,
    ) -> RefineResult<()> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let claim = state
            .claims
            .iter_mut()
            .find(|claim| claim.claim_id == fence.claim_id)
            .ok_or_else(|| {
                RefineError::Conflict(format!("Ready Merge claim {} was replaced", fence.claim_id))
            })?;
        if claim.goal_id != fence.goal_id
            || claim.node_id != fence.node_id
            || claim.execution_id.as_deref() != Some(fence.execution_id.as_str())
            || claim.round_idx != Some(fence.round_idx)
            || claim.goal_revision != Some(fence.goal_revision)
            || claim.decision_version != fence.decision_version
            || claim.state != WorkflowClaimState::Running
        {
            return Err(RefineError::Conflict(format!(
                "execution {} no longer owns Ready Merge claim {}",
                fence.execution_id, fence.claim_id
            )));
        }
        claim.goal_revision = Some(goal_revision);
        claim.decision_version = claim.decision_version.saturating_add(1);
        claim.updated_at = now_timestamp();
        fence.goal_revision = goal_revision;
        fence.decision_version = claim.decision_version;
        self.save_state(&mut state)
    }
}
