use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::application::projects::projection::ActiveGoalIndex;
use crate::application::work_items::FileWorkItemService;
use crate::application::work_items::WorkflowAttemptAuthority;
use crate::application::workflow::engine::behaviors::contract::{
    WorkflowAdvanceOutcome, WorkflowBehavior,
};
use crate::application::workflow::engine::behaviors::{
    WorkflowDone, WorkflowGovernance, WorkflowImplementation, WorkflowPlan, WorkflowQuality,
    WorkflowReview, WorkflowTodo,
};
use crate::application::workflow::engine::context::WorkflowContext;
use crate::application::workflow::engine::policy::SchedulingEligibility;
use crate::application::workflow::recovery::reconciliation::IntegratedTargetWorkflowLease;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::infrastructure::storage::project_layout::prepare_refine_dir;
use crate::model::feature::compare_feature_goal_order;
use crate::model::workflow::GoalStatus;

use crate::application::workflow::engine::{
    authored_workflow_commitment, hydrate_plan_or_implement_context, hydrate_retry_context,
    setting_string,
};
use crate::application::workflow::{
    ACTIVE_WORK_REPLENISH_INTERVAL, WorkflowEngine, WorkflowPassResult, WorkflowStepResult,
    missing_workflow_artifact, priority_rank,
};

static RETRY_STATE: OnceLock<Mutex<BTreeMap<String, RetryState>>> = OnceLock::new();

#[derive(Clone, Copy)]
struct RetryState {
    failures: u32,
    not_before: Instant,
}

enum PreparedGoal<'a> {
    Execute(Box<WorkflowContext<'a>>),
    Completed(Box<WorkflowStepResult>),
}

struct PreparedGoalError {
    error: RefineError,
    authority: Option<WorkflowAttemptAuthority>,
}

impl WorkflowEngine {
    pub fn evaluate_workflow(&self) -> RefineResult<WorkflowPassResult> {
        let promoted = self.promote()?;
        let steps = self.execute_work()?;
        Ok(WorkflowPassResult { promoted, steps })
    }

    /// Promotes backlog work into the state-only todo queue. Worker admission happens separately
    /// from this durable lifecycle mutation.
    pub fn promote(&self) -> RefineResult<usize> {
        self.ensure_automation_running()?;
        self.promote_backlog_to_todo()
    }

    pub fn execute_work(&self) -> RefineResult<Vec<WorkflowStepResult>> {
        self.ensure_automation_running()?;
        let mut results = Vec::new();
        let mut errors = Vec::new();
        let mut scheduler_error = None;
        std::thread::scope(|scope| {
            let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();
            let mut active = BTreeSet::new();
            let mut launch_order = 0usize;
            let mut next_replenish = Instant::now();

            loop {
                let paused = match self.workflow_paused() {
                    Ok(paused) => paused,
                    Err(error) => {
                        scheduler_error = Some(error);
                        false
                    }
                };
                if !paused && scheduler_error.is_none() && Instant::now() >= next_replenish {
                    if let Err(error) = self.promote_backlog_to_todo() {
                        scheduler_error = Some(error);
                    }
                    next_replenish = Instant::now() + ACTIVE_WORK_REPLENISH_INTERVAL;
                }

                let mut launched = false;
                if !paused && scheduler_error.is_none() {
                    match self.launchable_goals(&active) {
                        Ok(goal_ids) => {
                            for goal_id in goal_ids {
                                if active.contains(&goal_id) {
                                    continue;
                                }
                                let order = launch_order;
                                launch_order += 1;
                                match self.prepare_goal(&goal_id) {
                                    Ok(PreparedGoal::Execute(ctx)) => {
                                        active.insert(goal_id.clone());
                                        launched = true;
                                        let outcome_tx = outcome_tx.clone();
                                        let authority = ctx.attempt_authority;
                                        scope.spawn(move || {
                                            let outcome = std::panic::catch_unwind(
                                                std::panic::AssertUnwindSafe(|| {
                                                    self.execute_prepared_goal(*ctx)
                                                }),
                                            )
                                            .unwrap_or_else(|_| {
                                                let error = RefineError::Conflict(format!(
                                                    "workflow worker panicked for Goal {goal_id}"
                                                ));
                                                let _ = self.settle_goal_failure(
                                                    &goal_id,
                                                    authority,
                                                    "workflow_panic",
                                                    &error,
                                                );
                                                Err(error)
                                            });
                                            let _ = outcome_tx.send((order, goal_id, outcome));
                                        });
                                    }
                                    Ok(PreparedGoal::Completed(result)) => {
                                        self.clear_retry(&goal_id);
                                        results.push((order, *result));
                                        launched = true;
                                    }
                                    Err(failure) if is_stale_authority(&failure.error) => {}
                                    Err(failure) => {
                                        if let Some(authority) = failure.authority {
                                            let _ = self.settle_goal_failure(
                                                &goal_id,
                                                authority,
                                                "preparation",
                                                &failure.error,
                                            );
                                        }
                                        self.record_retry(&goal_id);
                                        errors.push((order, failure.error));
                                    }
                                }
                            }
                        }
                        Err(error) => scheduler_error = Some(error),
                    }
                }

                if active.is_empty() {
                    if !launched {
                        break;
                    }
                    continue;
                }
                match outcome_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok((order, goal_id, outcome)) => {
                        active.remove(&goal_id);
                        next_replenish = Instant::now();
                        match outcome {
                            Ok(result) => {
                                self.clear_retry(&goal_id);
                                results.push((order, result));
                            }
                            Err(error) => {
                                self.record_retry(&goal_id);
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

    fn launchable_goals(&self, active: &BTreeSet<String>) -> RefineResult<Vec<String>> {
        let target_root = self.target_root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput(
                "target root is required to execute workflow work".to_string(),
            )
        })?;
        let refine_dir = prepare_refine_dir(target_root)?;
        ActiveGoalIndex::ensure_built(&refine_dir)?;
        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir)?;
        let policy = self.policy()?;
        let eligibility = SchedulingEligibility::new(index.goals());
        let mut goals = index
            .goals()
            .filter(|goal| {
                matches!(
                    goal.status,
                    GoalStatus::Todo
                        | GoalStatus::Plan
                        | GoalStatus::Implement
                        | GoalStatus::Governance
                        | GoalStatus::Quality
                )
            })
            .filter(|goal| {
                crate::application::fleet::nodes::node_ids_match(
                    goal.node_id.as_deref().unwrap_or("default"),
                    &policy.active_node_id,
                )
            })
            .filter(|goal| goal.round_count > 0)
            .filter(|goal| !active.contains(&goal.id))
            .filter(|goal| !self.retry_delayed(&goal.id))
            .filter(|goal| eligibility.feature_eligible(&goal.id))
            .filter(|goal| eligibility.priority_eligible(goal))
            .cloned()
            .collect::<Vec<_>>();
        goals.sort_by(|a, b| {
            priority_rank(&b.priority)
                .cmp(&priority_rank(&a.priority))
                .then_with(|| compare_feature_goal_order(a.feature_order, b.feature_order))
                .then_with(|| a.created.cmp(&b.created))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut load = self.observed_execution_load()?;
        let mut result = Vec::new();
        for goal in goals {
            if !load.available(
                &policy,
                &policy.active_node_id,
                &policy.provider,
                &policy.target_app_id,
            ) {
                break;
            }
            load.record(
                &policy.active_node_id,
                &policy.provider,
                &policy.target_app_id,
            );
            result.push(goal.id);
        }
        Ok(result)
    }

    fn prepare_goal<'a>(&'a self, goal_id: &str) -> Result<PreparedGoal<'a>, PreparedGoalError> {
        let unclaimed = |error| PreparedGoalError {
            error,
            authority: None,
        };
        let target_root = self
            .target_root
            .as_ref()
            .ok_or_else(|| {
                RefineError::InvalidInput(
                    "target root is required to execute workflow work".to_string(),
                )
            })
            .map_err(unclaimed)?;
        let refine_dir = prepare_refine_dir(target_root).map_err(unclaimed)?;
        let work_items = FileWorkItemService::with_projection_cache(
            &refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let policy = self.policy().map_err(unclaimed)?;
        let summary = work_items.show_goal_summary(goal_id).map_err(unclaimed)?;
        let node_id = summary
            .goal
            .node_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        if node_id != policy.active_node_id {
            return Err(unclaimed(RefineError::Conflict(format!(
                "Goal {goal_id} is owned by node {node_id}, not active node {}",
                policy.active_node_id
            ))));
        }
        if !matches!(
            summary.goal.status,
            GoalStatus::Todo
                | GoalStatus::Plan
                | GoalStatus::Implement
                | GoalStatus::Governance
                | GoalStatus::Quality
        ) {
            return Err(unclaimed(RefineError::Conflict(format!(
                "Goal {goal_id} is no longer eligible from {}",
                summary.goal.status.as_str()
            ))));
        }
        let (round_idx, authored_revision, authored_request) =
            authored_workflow_commitment(&work_items, goal_id).map_err(unclaimed)?;
        let authority = work_items
            .claim_workflow_attempt(
                goal_id,
                summary.goal.status.clone(),
                round_idx,
                authored_revision,
                &authored_request,
            )
            .map_err(unclaimed)?;
        let claimed = |error| PreparedGoalError {
            error,
            authority: Some(authority),
        };
        let settings = FileSettingsService::with_active_root(&refine_dir, &self.runtime_root)
            .load()
            .map_err(claimed)?;
        let mut ctx = WorkflowContext::new(
            &self.runtime_root,
            target_root,
            goal_id.to_string(),
            node_id,
            policy.provider,
            round_idx,
            authority,
            settings,
            work_items,
        );
        match summary.goal.status {
            GoalStatus::Todo => match WorkflowTodo.advance(&mut ctx).map_err(claimed)? {
                WorkflowAdvanceOutcome::Transition {
                    to: GoalStatus::Plan,
                    ..
                }
                | WorkflowAdvanceOutcome::Transition {
                    to: GoalStatus::Quality,
                    ..
                } => Ok(PreparedGoal::Execute(Box::new(ctx))),
                WorkflowAdvanceOutcome::Completed { final_status, .. } => {
                    ctx.final_status = Some(final_status);
                    Ok(PreparedGoal::Completed(Box::new(
                        Self::workflow_step_result(ctx).map_err(claimed)?,
                    )))
                }
                outcome => Err(claimed(RefineError::Conflict(outcome_reason(outcome)))),
            },
            GoalStatus::Plan | GoalStatus::Implement => {
                let start_status = summary.goal.status.clone();
                let pattern =
                    setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}");
                let target = setting_string(&ctx.settings, "merge_target_branch", "main");
                hydrate_plan_or_implement_context(&mut ctx, &pattern, &target).map_err(claimed)?;
                ctx.start_status = start_status;
                Ok(PreparedGoal::Execute(Box::new(ctx)))
            }
            current => {
                hydrate_retry_context(&mut ctx, current).map_err(claimed)?;
                Ok(PreparedGoal::Execute(Box::new(ctx)))
            }
        }
    }

    pub(crate) fn execute_prepared_goal(
        &self,
        mut ctx: WorkflowContext<'_>,
    ) -> RefineResult<WorkflowStepResult> {
        let start_status = ctx.start_status.clone();
        if let Err(error) = self.advance_behaviors(&mut ctx, start_status) {
            let _ =
                self.settle_goal_failure(&ctx.goal_id, ctx.attempt_authority, "workflow", &error);
            return Err(error);
        }
        Self::workflow_step_result(ctx)
    }

    fn workflow_step_result(ctx: WorkflowContext<'_>) -> RefineResult<WorkflowStepResult> {
        let branch = ctx
            .branch
            .clone()
            .ok_or_else(|| missing_workflow_artifact("branch", &ctx.goal_id))?;
        let commit = ctx
            .commit
            .clone()
            .ok_or_else(|| missing_workflow_artifact("commit", &ctx.goal_id))?;
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
            goal_id: ctx.goal_id,
            provider: ctx.provider,
            branch,
            commit,
            merge: ctx.merge,
            final_status,
            provider_output,
        })
    }

    pub(crate) fn advance_behaviors(
        &self,
        ctx: &mut WorkflowContext<'_>,
        mut current: GoalStatus,
    ) -> RefineResult<()> {
        let plan = WorkflowPlan;
        let implementation = WorkflowImplementation;
        let quality = WorkflowQuality;
        let governance = WorkflowGovernance;
        let review = WorkflowReview;
        let done = WorkflowDone;
        let behaviors: [&dyn WorkflowBehavior; 6] = [
            &plan,
            &implementation,
            &quality,
            &governance,
            &review,
            &done,
        ];
        let mut integrated_target_lane = None;
        loop {
            if integrated_target_lane.is_none()
                && workflow_status_uses_integrated_target(ctx, &current)?
            {
                integrated_target_lane = Some(IntegratedTargetWorkflowLease::acquire(
                    ctx.target_root,
                    &ctx.goal_id,
                    ctx.round_idx,
                )?);
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
            match behavior.advance(ctx) {
                Ok(WorkflowAdvanceOutcome::Transition { to, .. }) => current = to,
                Ok(WorkflowAdvanceOutcome::Completed { .. }) => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish()?;
                    }
                    return Ok(());
                }
                Ok(outcome) => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish_if_clean();
                    }
                    return Err(RefineError::Conflict(outcome_reason(outcome)));
                }
                Err(error) => {
                    if let Some(lane) = integrated_target_lane.as_mut() {
                        lane.finish_if_clean();
                    }
                    return Err(error);
                }
            }
        }
    }

    fn retry_key(&self, goal_id: &str) -> String {
        format!(
            "{}:{}:{goal_id}",
            self.runtime_root.display(),
            self.target_root
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        )
    }

    fn retry_delayed(&self, goal_id: &str) -> bool {
        RETRY_STATE
            .get_or_init(Default::default)
            .lock()
            .ok()
            .and_then(|state| state.get(&self.retry_key(goal_id)).copied())
            .is_some_and(|state| state.not_before > Instant::now())
    }

    fn record_retry(&self, goal_id: &str) {
        if let Ok(mut retries) = RETRY_STATE.get_or_init(Default::default).lock() {
            let key = self.retry_key(goal_id);
            let failures = retries
                .get(&key)
                .map(|state| state.failures.saturating_add(1))
                .unwrap_or(1);
            let delay = 5_u64.saturating_mul(1_u64 << failures.saturating_sub(1).min(6));
            retries.insert(
                key,
                RetryState {
                    failures,
                    not_before: Instant::now() + Duration::from_secs(delay.min(300)),
                },
            );
        }
    }

    fn clear_retry(&self, goal_id: &str) {
        if let Ok(mut retries) = RETRY_STATE.get_or_init(Default::default).lock() {
            retries.remove(&self.retry_key(goal_id));
        }
    }
}

fn outcome_reason(outcome: WorkflowAdvanceOutcome) -> String {
    match outcome {
        WorkflowAdvanceOutcome::Transition { reason, .. }
        | WorkflowAdvanceOutcome::Completed { reason, .. }
        | WorkflowAdvanceOutcome::Noop { reason }
        | WorkflowAdvanceOutcome::Blocked { reason }
        | WorkflowAdvanceOutcome::Failed { reason } => reason,
    }
}

fn is_stale_authority(error: &RefineError) -> bool {
    matches!(error, RefineError::Conflict(message) if message.contains("owned by node")
        || message.contains("no longer eligible")
        || message.contains("changed from expected")
        || message.contains("changed before workflow attempt claim")
        || message.contains("workflow attempt") && message.contains("was superseded"))
}

fn workflow_status_uses_integrated_target(
    ctx: &mut WorkflowContext<'_>,
    status: &GoalStatus,
) -> RefineResult<bool> {
    match status {
        GoalStatus::Governance => Ok(true),
        GoalStatus::Quality if ctx.reconciliation.is_some() => Ok(true),
        _ => Ok(false),
    }
}
