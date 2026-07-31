use std::collections::BTreeSet;

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::host::quality::POST_BUILD;
use crate::tools::product::work_items::{FileWorkItemService, workflow_revision};
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::behaviors::{
    WorkflowBuild, WorkflowDone, WorkflowImplementation, WorkflowQa, WorkflowReadyMerge,
    WorkflowReview, WorkflowTodo,
};
use crate::workflow::context::WorkflowContext;
use crate::workflow::reconciliation::IntegratedTargetWorkflowLease;

use super::{
    ACTIVE_WORK_REPLENISH_INTERVAL, AUTOMATION_CONCURRENCY_LIMIT_REACHED, WorkflowAutomation,
    WorkflowClaim, WorkflowClaimState, WorkflowEngine, WorkflowPassResult, WorkflowStepResult,
    ensure_workflow_round, hydrate_retry_context, missing_workflow_artifact, now_timestamp,
};

enum PreparedClaim<'a> {
    Execute(Box<WorkflowContext<'a>>),
    Completed(Box<WorkflowStepResult>),
}

impl WorkflowEngine {
    pub fn evaluate_workflow(&self) -> RefineResult<WorkflowPassResult> {
        self.evaluate_workflow_locked()
    }

    pub(super) fn evaluate_workflow_locked(&self) -> RefineResult<WorkflowPassResult> {
        let promoted = self.promote()?;
        let steps = self.execute_claimed_work()?;
        let state = self.load_state()?;
        Ok(WorkflowPassResult {
            promoted,
            claims: state.claims,
            steps,
        })
    }

    pub fn execute_claimed_work(&self) -> RefineResult<Vec<WorkflowStepResult>> {
        let state = self.load_state()?;
        self.ensure_automation_running(&state)?;
        // Reclaim slots held by claims whose executor died with an earlier daemon,
        // before this tick decides how much capacity is available.
        self.reconcile_orphaned_running_claims()?;
        let mut results = Vec::new();
        let mut errors = Vec::new();
        let mut scheduler_error = None;
        std::thread::scope(|scope| {
            let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();
            let mut running = 0usize;
            let mut launch_order = 0usize;
            let mut launched_any = false;
            let mut next_replenish = std::time::Instant::now();

            loop {
                if launched_any && running == 0 {
                    break;
                }
                let workflow_paused = match self.workflow_paused() {
                    Ok(paused) => paused,
                    Err(error) => {
                        scheduler_error = Some(error);
                        false
                    }
                };
                if workflow_paused {
                    // A pause is an admission gate, not a scheduler failure. Keep active
                    // executions draining and retry promotion immediately after resume.
                    next_replenish = std::time::Instant::now();
                } else if scheduler_error.is_none() && std::time::Instant::now() >= next_replenish {
                    if let Err(error) = self.promote() {
                        scheduler_error = Some(error);
                    }
                    next_replenish = std::time::Instant::now() + ACTIVE_WORK_REPLENISH_INTERVAL;
                }
                if scheduler_error.is_none() && !workflow_paused {
                    let launchable = self.load_state().and_then(|state| {
                        self.policy()
                            .map(|policy| Self::launchable_claim_ids(&state, &policy))
                    });
                    match launchable {
                        Ok(claim_ids) => {
                            for claim_id in claim_ids {
                                let order = launch_order;
                                launch_order += 1;
                                let execution_id = match self.start_claim(&claim_id) {
                                    Ok(execution_id) => execution_id,
                                    Err(RefineError::Conflict(message))
                                        if message == AUTOMATION_CONCURRENCY_LIMIT_REACHED =>
                                    {
                                        continue;
                                    }
                                    Err(error) => {
                                        errors.push((order, error));
                                        continue;
                                    }
                                };
                                #[cfg(test)]
                                if let Some(hook) = &self.before_worker_prepare_hook {
                                    hook(&claim_id, &execution_id);
                                }
                                let preparation =
                                    self.prepare_started_claim(&claim_id, &execution_id);
                                match preparation {
                                    Ok(PreparedClaim::Execute(ctx)) => {
                                        running += 1;
                                        launched_any = true;
                                        let outcome_tx = outcome_tx.clone();
                                        let worker_execution_id = execution_id.clone();
                                        scope.spawn(move || {
                                            let outcome = std::panic::catch_unwind(
                                                std::panic::AssertUnwindSafe(|| {
                                                    self.execute_prepared_claim(*ctx)
                                                }),
                                            )
                                            .unwrap_or_else(|_| {
                                                Err(RefineError::Conflict(format!(
                                                    "workflow worker panicked for claim {claim_id}"
                                                )))
                                            });
                                            let _ = outcome_tx.send((
                                                order,
                                                claim_id,
                                                worker_execution_id,
                                                outcome,
                                            ));
                                        });
                                    }
                                    Ok(PreparedClaim::Completed(result)) => {
                                        launched_any = true;
                                        if let Err(error) = self.mark_claim_state(
                                            &claim_id,
                                            Some(&execution_id),
                                            WorkflowClaimState::Completed,
                                        ) {
                                            errors.push((order, error));
                                        } else {
                                            results.push((order, *result));
                                        }
                                    }
                                    Err(error) => {
                                        let _ = self.mark_claim_preparation_failed(
                                            &claim_id,
                                            &execution_id,
                                            &error,
                                        );
                                        errors.push((order, error));
                                    }
                                }
                            }
                        }
                        Err(error) => scheduler_error = Some(error),
                    }
                }

                if running == 0 {
                    break;
                }

                match outcome_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok((order, claim_id, execution_id, outcome)) => {
                        running -= 1;
                        next_replenish = std::time::Instant::now();
                        match outcome {
                            Ok(result) => {
                                if let Err(error) = self.mark_claim_state(
                                    &claim_id,
                                    Some(&execution_id),
                                    WorkflowClaimState::Completed,
                                ) {
                                    errors.push((order, error));
                                } else {
                                    results.push((order, result));
                                }
                            }
                            Err(error) => {
                                let _ = self.mark_claim_state(
                                    &claim_id,
                                    Some(&execution_id),
                                    WorkflowClaimState::Failed,
                                );
                                errors.push((order, error));
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        scheduler_error = Some(RefineError::Conflict(
                            "workflow worker result channel disconnected".to_string(),
                        ));
                    }
                }
            }
        });
        errors.sort_by_key(|(order, _)| *order);
        if let Some((_, error)) = errors.into_iter().next() {
            return Err(error);
        }
        if let Some(error) = scheduler_error {
            return Err(error);
        }
        results.sort_by_key(|(order, _)| *order);
        Ok(results.into_iter().map(|(_, result)| result).collect())
    }

    fn prepare_started_claim<'a>(
        &'a self,
        claim_id: &str,
        execution_id: &str,
    ) -> RefineResult<PreparedClaim<'a>> {
        let claim = self.claim_by_id(claim_id)?;
        let target_root = self.target_root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput(
                "target root is required to execute claimed workflow work".to_string(),
            )
        })?;
        let refine_dir = prepare_refine_dir(target_root)?;
        // One shared projection cache, as every other caller uses. Scoping it per
        // claim wrote a full copy of project state per claim into
        // `cache/workflow/<claim id>`, and nothing ever removed those, so disk
        // grew without bound in proportion to claims ever created. Snapshots are
        // published by atomic rename over a temp file, so sharing is safe under
        // concurrent workers: a reader sees one complete snapshot, and writers
        // rebuilding from the same sources converge.
        let work_items = FileWorkItemService::with_projection_cache(
            &refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let round_idx = ensure_workflow_round(&work_items, &claim.goal_id)?;
        let settings =
            FileSettingsService::with_active_root(&refine_dir, &self.runtime_root).load()?;
        let mut ctx = WorkflowContext::new(
            &self.runtime_root,
            target_root,
            claim,
            execution_id,
            round_idx,
            settings,
            work_items,
        );
        let current = ctx.work_items.show_goal_summary(&ctx.goal_id)?.goal.status;
        if current == GoalStatus::Todo {
            return match WorkflowTodo.advance(&mut ctx)? {
                WorkflowAdvanceOutcome::Transition {
                    to: GoalStatus::InProgress,
                    ..
                }
                | WorkflowAdvanceOutcome::Transition {
                    to: GoalStatus::Qa, ..
                } => Ok(PreparedClaim::Execute(Box::new(ctx))),
                WorkflowAdvanceOutcome::Completed { final_status, .. } => {
                    ctx.final_status = Some(final_status);
                    Ok(PreparedClaim::Completed(Box::new(
                        Self::workflow_step_result(ctx)?,
                    )))
                }
                WorkflowAdvanceOutcome::Noop { reason }
                | WorkflowAdvanceOutcome::Blocked { reason }
                | WorkflowAdvanceOutcome::Failed { reason }
                | WorkflowAdvanceOutcome::Transition { reason, .. } => {
                    Err(RefineError::Conflict(reason))
                }
            };
        }
        if !matches!(
            current,
            GoalStatus::ReadyMerge | GoalStatus::Build | GoalStatus::Qa
        ) {
            return Err(RefineError::Conflict(format!(
                "Goal {} cannot resume workflow from {}",
                ctx.goal_id,
                current.as_str()
            )));
        }
        hydrate_retry_context(&mut ctx, current)?;
        Ok(PreparedClaim::Execute(Box::new(ctx)))
    }

    pub(super) fn execute_prepared_claim(
        &self,
        mut ctx: WorkflowContext<'_>,
    ) -> RefineResult<WorkflowStepResult> {
        let start_status = ctx.start_status.clone();
        self.advance_claim_behaviors(&mut ctx, start_status)?;
        Self::workflow_step_result(ctx)
    }

    fn workflow_step_result(ctx: WorkflowContext<'_>) -> RefineResult<WorkflowStepResult> {
        let execution_id = ctx.execution_id.clone();
        let branch = ctx
            .branch
            .clone()
            .ok_or_else(|| missing_workflow_artifact("branch", &ctx.goal_id))?;
        let commit = ctx
            .commit
            .clone()
            .ok_or_else(|| missing_workflow_artifact("commit", &ctx.goal_id))?;
        let merge = ctx.merge.clone();
        let provider_output = ctx
            .provider_output
            .clone()
            .ok_or_else(|| missing_workflow_artifact("provider output", &ctx.goal_id))?;
        let final_status = ctx
            .final_status
            .clone()
            .unwrap_or(GoalStatus::Review)
            .as_str()
            .to_string();

        Ok(WorkflowStepResult {
            claim_id: ctx.claim_id,
            goal_id: ctx.goal_id,
            execution_id,
            provider: ctx.provider,
            branch,
            commit,
            merge,
            final_status,
            provider_output,
        })
    }

    pub(super) fn advance_claim_behaviors(
        &self,
        ctx: &mut WorkflowContext<'_>,
        mut current: GoalStatus,
    ) -> RefineResult<()> {
        let implementation = WorkflowImplementation;
        let ready_merge = WorkflowReadyMerge;
        let build = WorkflowBuild;
        let qa = WorkflowQa;
        let review = WorkflowReview;
        let done = WorkflowDone;
        let behaviors: [&dyn WorkflowBehavior; 6] =
            [&implementation, &ready_merge, &build, &qa, &review, &done];
        let mut integrated_target_lane = None;
        loop {
            if integrated_target_lane.is_none()
                && workflow_status_uses_integrated_target(ctx, &current)?
            {
                let lane = match IntegratedTargetWorkflowLease::acquire(
                    ctx.target_root,
                    &ctx.goal_id,
                    ctx.round_idx,
                ) {
                    Ok(lane) => lane,
                    Err(error) => {
                        ctx.log(
                            "workflow",
                            "Integrated-target workflow lane could not start; Goal status was preserved",
                            Some(crate::workflow::json_object(serde_json::json!({
                                "error": error.to_string()
                            }))),
                        )?;
                        return Err(error);
                    }
                };
                ctx.log(
                    "workflow",
                    "Acquired exclusive integrated-target workflow lane",
                    None,
                )?;
                integrated_target_lane = Some(lane);
            }
            let Some(behavior) = behaviors
                .iter()
                .copied()
                .find(|behavior| behavior.observes() == current)
            else {
                return Err(RefineError::Conflict(format!(
                    "No workflow behavior registered for {}",
                    current.as_str()
                )));
            };
            let outcome = match behavior.advance(ctx) {
                Ok(outcome) => outcome,
                Err(error) => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish_if_clean();
                    }
                    return Err(error);
                }
            };
            match outcome {
                WorkflowAdvanceOutcome::Transition { to, .. } => {
                    current = to;
                }
                WorkflowAdvanceOutcome::Completed { .. } => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish()?;
                    }
                    return Ok(());
                }
                WorkflowAdvanceOutcome::Noop { reason }
                | WorkflowAdvanceOutcome::Blocked { reason }
                | WorkflowAdvanceOutcome::Failed { reason } => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish_if_clean();
                    }
                    return Err(RefineError::Conflict(reason));
                }
            }
        }
    }

    pub(super) fn claim_by_id(&self, claim_id: &str) -> RefineResult<WorkflowClaim> {
        self.load_state()?
            .claims
            .into_iter()
            .find(|claim| claim.claim_id == claim_id)
            .ok_or_else(|| RefineError::NotFound(format!("claim {claim_id} was not found")))
    }

    pub(super) fn mark_claim_state(
        &self,
        claim_id: &str,
        expected_execution_id: Option<&str>,
        claim_state: WorkflowClaimState,
    ) -> RefineResult<()> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let Some(claim) = state
            .claims
            .iter_mut()
            .find(|claim| claim.claim_id == claim_id)
        else {
            return Err(RefineError::NotFound(format!(
                "claim {claim_id} was not found"
            )));
        };
        if !matches!(
            claim.state,
            WorkflowClaimState::Claimed | WorkflowClaimState::Running
        ) {
            return Ok(());
        }
        if let Some(expected_execution_id) = expected_execution_id
            && (claim.execution_id.as_deref() != Some(expected_execution_id)
                || claim.state != WorkflowClaimState::Running)
        {
            return Err(RefineError::Conflict(format!(
                "execution {expected_execution_id} no longer owns claim {claim_id}"
            )));
        }
        claim.decision_version = claim.decision_version.saturating_add(1);
        claim.state = claim_state;
        claim.updated_at = now_timestamp();
        let terminal = !matches!(
            claim.state,
            WorkflowClaimState::Claimed | WorkflowClaimState::Running
        );
        self.save_state(&mut state)?;
        if terminal {
            self.release_claim_capacity(claim_id)?;
        }
        Ok(())
    }

    fn mark_claim_preparation_failed(
        &self,
        claim_id: &str,
        expected_execution_id: &str,
        error: &RefineError,
    ) -> RefineResult<()> {
        let goal_id = self.claim_by_id(claim_id)?.goal_id;
        // Preparation can fail because the target or Goal record itself cannot
        // be read. Terminalize the claim even then; an unavailable revision is
        // conservatively quarantined until an operator explicitly retries it.
        let goal_revision = self
            .target_root
            .as_ref()
            .and_then(|target_root| prepare_refine_dir(target_root).ok())
            .and_then(|refine_dir| {
                FileWorkItemService::new(refine_dir)
                    .show_goal_detail(&goal_id)
                    .ok()
            })
            .map(|detail| workflow_revision(&detail));

        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let Some(claim) = state
            .claims
            .iter_mut()
            .find(|claim| claim.claim_id == claim_id)
        else {
            return Err(RefineError::NotFound(format!(
                "claim {claim_id} was not found"
            )));
        };
        if claim.state != WorkflowClaimState::Running
            || claim.execution_id.as_deref() != Some(expected_execution_id)
        {
            return Err(RefineError::Conflict(format!(
                "execution {expected_execution_id} no longer owns claim {claim_id}"
            )));
        }
        claim.goal_revision = goal_revision;
        claim.failure_stage = Some("preparation".to_string());
        claim.failure_message = Some(error.to_string());
        claim.decision_version = claim.decision_version.saturating_add(1);
        claim.state = WorkflowClaimState::Failed;
        claim.updated_at = now_timestamp();
        self.save_state(&mut state)?;
        self.release_claim_capacity(claim_id)?;
        Ok(())
    }

    pub(super) fn interrupt_active_claims(&self, goal_ids: &[String]) -> RefineResult<()> {
        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let goal_ids = goal_ids.iter().collect::<BTreeSet<_>>();
        let mut state = self.load_state()?;
        let mut changed = false;
        let mut released_claim_ids = Vec::new();
        let now = now_timestamp();
        for claim in &mut state.claims {
            if goal_ids.contains(&claim.goal_id)
                && matches!(
                    claim.state,
                    WorkflowClaimState::Claimed | WorkflowClaimState::Running
                )
            {
                claim.decision_version = claim.decision_version.saturating_add(1);
                claim.state = WorkflowClaimState::Interrupted;
                claim.updated_at = now.clone();
                released_claim_ids.push(claim.claim_id.clone());
                changed = true;
            }
        }
        if changed {
            self.save_state(&mut state)?;
            for claim_id in released_claim_ids {
                self.release_claim_capacity(&claim_id)?;
            }
        }
        Ok(())
    }
}

fn workflow_status_uses_integrated_target(
    ctx: &mut WorkflowContext<'_>,
    status: &GoalStatus,
) -> RefineResult<bool> {
    match status {
        GoalStatus::ReadyMerge | GoalStatus::Build => Ok(true),
        GoalStatus::Qa if ctx.reconciliation.is_some() => Ok(true),
        GoalStatus::Qa => Ok(ctx.quality_timing(GoalStatus::Qa)? == POST_BUILD),
        _ => Ok(false),
    }
}
