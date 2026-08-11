use std::collections::BTreeSet;

use chrono::Utc;

use crate::model::feature::compare_feature_goal_order;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::product::process_control::FileProcessControlService;
use crate::tools::product::project_state::ActiveGoalIndex;
use crate::tools::product::work_items::FileWorkItemService;

use super::policy::ClaimEligibility;
use super::{
    AUTOMATION_CONCURRENCY_LIMIT_REACHED, WorkflowAutomation, WorkflowClaim, WorkflowClaimState,
    WorkflowEngine, new_claim_id, new_execution_id, now_timestamp, priority_rank,
};

impl WorkflowAutomation for WorkflowEngine {
    fn promote(&self) -> RefineResult<usize> {
        // Reconstructing the index reads every Goal record, which must not
        // happen while holding the lock that every other claim decision waits
        // on. Doing it first means the load inside the lock is a read of the
        // live set, whose size is bounded by work in flight.
        if let Some(refine_dir) = self.refine_dir()? {
            ActiveGoalIndex::ensure_built(&refine_dir)?;
        }
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let claim_history_needs_persistence = state.claim_history_needs_persistence();
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        let Some(refine_dir) = self.refine_dir()? else {
            let claimed = state
                .active_claims()
                .filter(|claim| claim.state == WorkflowClaimState::Claimed)
                .count();
            if claim_history_needs_persistence {
                self.save_state(&mut state)?;
            }
            return Ok(claimed);
        };
        self.promote_backlog_to_todo_for_refine_dir(&refine_dir)?;
        // Schedule from the active index rather than a projection of the whole
        // project. Its size is bounded by work in flight, so scheduling cost and
        // memory stop tracking how much work the project has ever contained.
        let active = ActiveGoalIndex::load_or_rebuild(&refine_dir)?;
        let work_items = FileWorkItemService::new(&refine_dir);
        let eligibility = ClaimEligibility::new(active.goals(), &BTreeSet::new());
        let mut eligible = active
            .goals()
            .filter(|goal| {
                matches!(
                    goal.status,
                    GoalStatus::Todo | GoalStatus::ReadyMerge | GoalStatus::Build | GoalStatus::Qa
                )
            })
            .filter(|goal| eligibility.feature_eligible(&goal.id))
            .filter(|goal| eligibility.priority_eligible(goal))
            .cloned()
            .collect::<Vec<_>>();
        eligible.sort_by(|a, b| {
            priority_rank(&b.priority)
                .cmp(&priority_rank(&a.priority))
                .then_with(|| compare_feature_goal_order(a.feature_order, b.feature_order))
                .then_with(|| a.created.cmp(&b.created))
                .then_with(|| a.id.cmp(&b.id))
        });

        let mut promoted = 0;
        for goal in eligible {
            if Self::active_claim(&state, &goal.id).is_some() {
                continue;
            }
            let metadata = match self.claim_metadata(Some(&goal), &policy) {
                Ok(metadata) => metadata,
                Err(RefineError::Conflict(_)) => continue,
                Err(error) => return Err(error),
            };
            if !Self::capacity_available(
                &state,
                &policy,
                &metadata.node_id,
                &metadata.provider,
                &metadata.target_app_id,
            ) {
                break;
            }
            let (round_idx, goal_revision) = claim_identity(&work_items, &goal)?;
            if state
                .claim_retry_not_before(&goal.id, round_idx, goal_revision, Utc::now())
                .is_some()
            {
                continue;
            }
            let now = now_timestamp();
            state.claims.push(WorkflowClaim {
                claim_id: new_claim_id(),
                goal_id: goal.id,
                node_id: metadata.node_id,
                provider: metadata.provider,
                target_app_id: metadata.target_app_id,
                execution_id: None,
                round_idx,
                goal_revision,
                failure_stage: None,
                failure_message: None,
                decision_version: 1,
                occurrences: 1,
                state: WorkflowClaimState::Claimed,
                created_at: now.clone(),
                updated_at: now,
            });
            promoted += 1;
        }
        if promoted > 0 || claim_history_needs_persistence {
            self.save_state(&mut state)?;
        }
        Ok(promoted)
    }

    fn claim(&self, goal_id: &str) -> RefineResult<String> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let goal_id = goal_id.trim();
        if goal_id.is_empty() {
            return Err(RefineError::InvalidInput("Goal id is required".to_string()));
        }
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        if let Some(existing) = Self::active_claim(&state, goal_id) {
            return Ok(existing.claim_id.clone());
        }
        let (goal, round_idx, goal_revision) = if let Some(refine_dir) = self.refine_dir()? {
            // Claiming decides against the same set scheduling does, so it reads
            // the scheduler index rather than a projection of every Goal. A Goal
            // absent from it is one no claim could succeed for.
            let active = ActiveGoalIndex::load_or_rebuild(&refine_dir)?;
            let goal = active
                .goals()
                .find(|goal| goal.id == goal_id)
                .cloned()
                .ok_or_else(|| {
                    RefineError::NotFound(format!("Goal {goal_id} was not found in target state"))
                })?;
            let eligibility = ClaimEligibility::new(active.goals(), &BTreeSet::new());
            if !eligibility.feature_eligible(&goal.id) {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} is blocked by Feature order"
                )));
            }
            if !eligibility.priority_eligible(&goal) {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} is blocked by higher priority work"
                )));
            }
            let (round_idx, goal_revision) =
                claim_identity(&FileWorkItemService::new(&refine_dir), &goal)?;
            (Some(goal), round_idx, goal_revision)
        } else {
            (None, None, None)
        };
        if let Some(not_before) =
            state.claim_retry_not_before(goal_id, round_idx, goal_revision, Utc::now())
        {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is waiting for execution retry backoff until {not_before}"
            )));
        }
        let metadata = self.claim_metadata(goal.as_ref(), &policy)?;
        if !Self::capacity_available(
            &state,
            &policy,
            &metadata.node_id,
            &metadata.provider,
            &metadata.target_app_id,
        ) {
            return Err(RefineError::Conflict(
                AUTOMATION_CONCURRENCY_LIMIT_REACHED.to_string(),
            ));
        }
        let now = now_timestamp();
        let claim = WorkflowClaim {
            claim_id: new_claim_id(),
            goal_id: goal_id.to_string(),
            node_id: metadata.node_id,
            provider: metadata.provider,
            target_app_id: metadata.target_app_id,
            execution_id: None,
            round_idx,
            goal_revision,
            failure_stage: None,
            failure_message: None,
            decision_version: 1,
            occurrences: 1,
            state: WorkflowClaimState::Claimed,
            created_at: now.clone(),
            updated_at: now,
        };
        let id = claim.claim_id.clone();
        state.claims.push(claim);
        self.save_state(&mut state)?;
        Ok(id)
    }

    fn start_claim(&self, claim_id: &str) -> RefineResult<String> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let claim_id = claim_id.trim();
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        let Some(claim_index) = state
            .claims
            .iter()
            .position(|claim| claim.claim_id == claim_id)
        else {
            return Err(RefineError::NotFound(format!(
                "claim {claim_id} was not found"
            )));
        };
        let claim = &state.claims[claim_index];
        if claim.state != WorkflowClaimState::Claimed {
            return Err(RefineError::Conflict(format!(
                "claim {claim_id} is not claimed"
            )));
        }
        if let Some(refine_dir) = self.refine_dir()? {
            let active = ActiveGoalIndex::load_or_rebuild(&refine_dir)?;
            let goal = active
                .goals()
                .find(|goal| goal.id == claim.goal_id)
                .ok_or_else(|| {
                    RefineError::NotFound(format!(
                        "Goal {} was not found in target state",
                        claim.goal_id
                    ))
                })?;
            self.claim_metadata(Some(goal), &policy)?;
            let eligibility = ClaimEligibility::new(active.goals(), &BTreeSet::new());
            if !eligibility.feature_eligible(&goal.id) {
                return Err(RefineError::Conflict(format!(
                    "Goal {} is blocked by Feature order",
                    claim.goal_id
                )));
            }
            if !eligibility.priority_eligible(goal) {
                return Err(RefineError::Conflict(format!(
                    "Goal {} is blocked by higher priority work",
                    claim.goal_id
                )));
            }
        }
        let capacity_request = Self::claim_capacity_request(&state.claims[claim_index]);
        if !self
            .capacity_service()
            .try_acquire(&policy, capacity_request)?
        {
            return Err(RefineError::Conflict(
                AUTOMATION_CONCURRENCY_LIMIT_REACHED.to_string(),
            ));
        }
        let execution_id = new_execution_id();
        let claim = &mut state.claims[claim_index];
        claim.execution_id = Some(execution_id.clone());
        claim.decision_version = claim.decision_version.saturating_add(1);
        claim.state = WorkflowClaimState::Running;
        claim.updated_at = now_timestamp();
        if let Err(error) = self.save_state(&mut state) {
            let _ = self.release_claim_capacity(claim_id);
            return Err(error);
        }
        Ok(execution_id)
    }

    fn cancel(&self, execution_id: &str) -> RefineResult<()> {
        let control = match self.refine_dir()? {
            Some(refine_dir) => {
                FileProcessControlService::with_refine_dir(&self.runtime_root, refine_dir)
            }
            None => FileProcessControlService::new(&self.runtime_root),
        };
        control.cancel_workflow_execution_managed(execution_id)?;
        Ok(())
    }

    fn retry(&self, execution_id: &str) -> RefineResult<String> {
        let execution_id = execution_id.trim();
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        let Some(claim_index) = state
            .claims
            .iter()
            .position(|claim| claim.execution_id.as_deref() == Some(execution_id))
        else {
            return Err(RefineError::NotFound(format!(
                "claim for execution {execution_id} was not found"
            )));
        };
        if let Some(refine_dir) = self.refine_dir()? {
            let goal_id = state.claims[claim_index].goal_id.clone();
            match FileWorkItemService::new(refine_dir).show_goal_summary(&goal_id) {
                Ok(goal)
                    if matches!(goal.goal.status, GoalStatus::Cancelled | GoalStatus::Done) =>
                {
                    return Err(RefineError::Conflict(format!(
                        "Goal {goal_id} is {}; its workflow execution cannot be retried",
                        goal.goal.status.as_str()
                    )));
                }
                Ok(_) | Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        if self.signal_workflow_subprocesses(execution_id, "terminate")? > 0 {
            return Err(RefineError::Conflict(format!(
                "execution {execution_id} is still stopping; retry after its managed agent process exits"
            )));
        }
        let request = Self::claim_capacity_request(&state.claims[claim_index]);
        if !self.capacity_service().try_acquire(&policy, request)? {
            return Err(RefineError::Conflict(
                AUTOMATION_CONCURRENCY_LIMIT_REACHED.to_string(),
            ));
        }
        let retried_execution_id = new_execution_id();
        let claim = &mut state.claims[claim_index];
        let claim_id = claim.claim_id.clone();
        claim.execution_id = Some(retried_execution_id.clone());
        claim.decision_version = claim.decision_version.saturating_add(1);
        claim.state = WorkflowClaimState::Running;
        claim.updated_at = now_timestamp();
        if let Err(error) = self.save_state(&mut state) {
            let _ = self.release_claim_capacity(&claim_id);
            return Err(error);
        }
        Ok(retried_execution_id)
    }
}

fn claim_identity(
    work_items: &FileWorkItemService,
    goal: &crate::model::goal::GoalIndexProjection,
) -> RefineResult<(Option<usize>, Option<u64>)> {
    let revision = work_items.workflow_revision_for_goal_projection(goal)?;
    Ok((goal.round_count.checked_sub(1), Some(revision)))
}
