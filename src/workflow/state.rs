use std::fs;
use std::fs::OpenOptions;
use std::path::PathBuf;

use fs2::FileExt;

use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::product::work_items::{FileWorkItemService, workflow_revision};
use crate::workflow::capacity::{AgentCapacityRequest, AgentCapacityService};

use super::{
    WORKFLOW_AUTOMATION_STATE_FILE, WORKFLOW_AUTOMATION_STATE_LOCK_FILE, WorkflowAutomationState,
    WorkflowClaim, WorkflowClaimState, WorkflowEngine, WorkflowStateMutationLock, now_timestamp,
    read_state, write_state,
};

impl WorkflowEngine {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        let runtime_root = runtime_root.into();
        Self {
            runtime_root,
            target_root: None,
            #[cfg(test)]
            before_worker_prepare_hook: None,
        }
    }

    pub fn with_target_root(
        runtime_root: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        let runtime_root = runtime_root.into();
        Self {
            runtime_root,
            target_root: Some(target_root.into()),
            #[cfg(test)]
            before_worker_prepare_hook: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_before_worker_prepare_hook(
        mut self,
        hook: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        self.before_worker_prepare_hook = Some(std::sync::Arc::new(hook));
        self
    }

    pub fn state_path(&self) -> PathBuf {
        self.runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE)
    }

    pub(super) fn capacity_service(&self) -> AgentCapacityService {
        AgentCapacityService::new(&self.runtime_root)
    }

    pub(crate) fn capacity_service_for_settlement(&self) -> AgentCapacityService {
        self.capacity_service()
    }

    pub(super) fn claim_capacity_request(claim: &WorkflowClaim) -> AgentCapacityRequest {
        AgentCapacityRequest {
            owner_id: format!("workflow:{}", claim.claim_id),
            role: "workflow".to_string(),
            node_id: claim.node_id.clone(),
            provider: claim.provider.clone(),
            target_app_id: claim.target_app_id.clone(),
        }
    }

    pub(super) fn release_claim_capacity(&self, claim_id: &str) -> RefineResult<bool> {
        self.capacity_service()
            .release(&format!("workflow:{claim_id}"))
    }

    pub(crate) fn persist_claims_cancelled_locked(
        &self,
        state: &mut WorkflowAutomationState,
        claim_ids: &[String],
    ) -> RefineResult<()> {
        let cancelled = self.claims_cancelled_state(state, claim_ids)?;
        self.persist_state_preserving_policy_locked(&cancelled)?;
        *state = cancelled;
        Ok(())
    }

    pub(crate) fn claims_cancelled_state(
        &self,
        state: &WorkflowAutomationState,
        claim_ids: &[String],
    ) -> RefineResult<WorkflowAutomationState> {
        if claim_ids.is_empty() {
            return Ok(state.clone());
        }
        let mut cancelled = state.clone();
        let now = now_timestamp();
        for claim_id in claim_ids {
            let claim = cancelled
                .claims
                .iter_mut()
                .find(|claim| claim.claim_id == *claim_id)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "workflow claim {claim_id} disappeared before cancellation settlement"
                    ))
                })?;
            claim.decision_version = claim.decision_version.saturating_add(1);
            claim.state = WorkflowClaimState::Cancelled;
            claim.updated_at = now.clone();
        }
        cancelled.updated_at = Some(now);
        cancelled.version = cancelled.version.saturating_add(1);
        Ok(cancelled)
    }

    pub(crate) fn persist_state_preserving_policy_locked(
        &self,
        state: &WorkflowAutomationState,
    ) -> RefineResult<()> {
        let current = read_state(&self.state_path())?;
        if current == *state {
            return Ok(());
        }
        let expected_version = current.version.saturating_add(1);
        if state.version != expected_version {
            return Err(RefineError::Conflict(format!(
                "workflow cancellation settlement expected to advance version {} to {}, received {}",
                current.version, expected_version, state.version
            )));
        }
        write_state(&self.state_path(), state)
    }

    pub(crate) fn restore_state_locked(&self, state: &WorkflowAutomationState) -> RefineResult<()> {
        let current = read_state(&self.state_path())?;
        let mut restored = state.clone();
        restored.version = current.version.saturating_add(1);
        self.persist_state_preserving_policy_locked(&restored)
    }

    pub(super) fn refine_dir(&self) -> RefineResult<Option<PathBuf>> {
        self.target_root
            .as_ref()
            .map(|target_root| prepare_refine_dir(target_root))
            .transpose()
    }

    pub(super) fn coordination_root(&self) -> RefineResult<PathBuf> {
        Ok(self
            .refine_dir()?
            .unwrap_or_else(|| self.runtime_root.clone()))
    }

    pub fn load_state(&self) -> RefineResult<WorkflowAutomationState> {
        read_state(&self.state_path())
    }

    pub fn preparation_failures_needing_attention(&self) -> RefineResult<Vec<WorkflowClaim>> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(Vec::new());
        };
        let state = self.load_state()?;
        let work_items = FileWorkItemService::new(refine_dir);
        let mut seen = std::collections::BTreeSet::new();
        let mut failures = Vec::new();
        for claim in state.claims.iter().rev() {
            if !seen.insert(claim.goal_id.clone()) {
                continue;
            }
            if claim.state != WorkflowClaimState::Failed
                || claim.failure_stage.as_deref() != Some("preparation")
            {
                continue;
            }
            let still_current = match claim.goal_revision {
                None => true,
                Some(failed_revision) => work_items
                    .show_goal_detail(&claim.goal_id)
                    .map(|detail| workflow_revision(&detail) == failed_revision)
                    .unwrap_or(true),
            };
            if still_current {
                failures.push(claim.clone());
            }
        }
        failures.reverse();
        Ok(failures)
    }

    pub(crate) fn acquire_state_mutation_lock(&self) -> RefineResult<WorkflowStateMutationLock> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create workflow runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let path = self.runtime_root.join(WORKFLOW_AUTOMATION_STATE_LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open workflow state mutation lock {}: {error}",
                    path.display()
                ))
            })?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to lock workflow state mutations {}: {error}",
                path.display()
            ))
        })?;
        Ok(WorkflowStateMutationLock { file })
    }

    pub(super) fn save_state(&self, state: &mut WorkflowAutomationState) -> RefineResult<()> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let current = read_state(&self.state_path())?;
        if current.version != state.version {
            return Err(RefineError::Conflict(format!(
                "workflow authority changed after it was read (expected version {}, current version {})",
                state.version, current.version
            )));
        }
        state.policy = self.policy()?;
        state.updated_at = Some(now_timestamp());
        state.version = state.version.saturating_add(1);
        write_state(&self.state_path(), state)
    }
}
