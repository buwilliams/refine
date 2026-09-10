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

mod admission;
mod attempt;
mod retry;
#[cfg(test)]
pub(crate) mod test_hooks;
use attempt::contain;

enum PreparedGoal<'a> {
    Execute(Box<WorkflowContext<'a>>),
    Completed(Box<WorkflowStepResult>),
}
struct PreparedGoalError {
    error: RefineError,
}

impl WorkflowEngine {
    pub fn evaluate_workflow(&self) -> RefineResult<WorkflowPassResult> {
        self.execute_pass(None)
    }
    pub fn promote(&self) -> RefineResult<usize> {
        self.ensure_automation_running()?;
        self.promote_backlog_to_todo()
    }

    pub fn execute_work(&self) -> RefineResult<Vec<WorkflowStepResult>> {
        self.execute_pass(None).map(|pass| pass.steps)
    }
    pub(crate) fn evaluate_worker_workflow(
        &self,
        registry: &std::path::Path,
    ) -> RefineResult<WorkflowPassResult> {
        self.execute_pass(Some(registry))
    }
    fn execute_pass(
        &self,
        worker_registry: Option<&std::path::Path>,
    ) -> RefineResult<WorkflowPassResult> {
        let mut promoted = 0;
        let mut results = Vec::new();
        let mut errors = Vec::new();
        std::thread::scope(|scope| {
            let mut active = BTreeMap::new();
            let mut discovery = None;
            let mut next_cycle = Instant::now();
            let mut order = 0usize;
            let mut cycle_failures = 0;
            let mut rescan_required = false;
            loop {
                // This is the admission controller itself, never a heartbeat helper thread.
                crate::application::workflow::health::scheduler_tick(
                    &self.runtime_root,
                    self.target_root.as_deref(),
                    &active.keys().cloned().collect(),
                    false,
                    None,
                );
                let finished = active
                    .iter()
                    .filter_map(
                        |(id, (_, handle)): (
                            &String,
                            &(
                                usize,
                                std::thread::ScopedJoinHandle<'_, RefineResult<WorkflowStepResult>>,
                            ),
                        )| handle.is_finished().then_some(id.clone()),
                    )
                    .collect::<Vec<_>>();
                for id in finished {
                    let (order, handle) = active.remove(&id).unwrap();
                    // The handle is the completion delivery mechanism and fallback. No sender
                    // can disappear while leaving an ended attempt in the active set.
                    let outcome = handle.join().unwrap_or_else(|_| {
                        Err(RefineError::Conflict(format!(
                            "Goal {id} completion panicked"
                        )))
                    });
                    match outcome {
                        Ok(result) => {
                            self.clear_retry(&id);
                            results.push((order, result));
                        }
                        Err(error) => {
                            self.record_retry(&id);
                            errors.push(error);
                        }
                    }
                    rescan_required = true;
                    next_cycle = Instant::now();
                }
                if discovery
                    .as_ref()
                    .is_some_and(std::thread::ScopedJoinHandle::is_finished)
                {
                    let handle: std::thread::ScopedJoinHandle<
                        '_,
                        RefineResult<(usize, Vec<(String, super::admission::AdmissionLease)>)>,
                    > = discovery.take().unwrap();
                    let outcome = handle.join().unwrap_or_else(|_| {
                        Err(RefineError::Conflict("workflow discovery panicked".into()))
                    });
                    let failure = outcome.as_ref().err().map(ToString::to_string);
                    crate::application::workflow::health::scheduler_tick(
                        &self.runtime_root,
                        self.target_root.as_deref(),
                        &active.keys().cloned().collect(),
                        true,
                        failure.as_deref(),
                    );
                    match outcome {
                        Ok((count, ids)) => {
                            promoted += count;
                            cycle_failures = 0;
                            let empty = ids.is_empty();
                            for (id, lease) in ids {
                                if active.contains_key(&id) {
                                    continue;
                                }
                                let goal_id = id.clone();
                                active.insert(
                                    id,
                                    (
                                        order,
                                        scope.spawn(move || {
                                            let _admission = lease;
                                            let outcome = contain(&goal_id, || {
                                                self.ensure_automation_running()?;
                                                if !self.target_still_attached(worker_registry)? {
                                                    return Err(RefineError::Conflict(
                                                        "workflow target detached before launch"
                                                            .into(),
                                                    ));
                                                }
                                                self.run_goal_attempt(&goal_id)
                                            });
                                            #[cfg(test)]
                                            test_hooks::run(
                                                self,
                                                &goal_id,
                                                "delivery",
                                                WorkflowAttemptAuthority {
                                                    round_idx: 0,
                                                    workflow_revision: 0,
                                                },
                                            )?;
                                            outcome
                                        }),
                                    ),
                                );
                                order += 1;
                            }
                            if empty && active.is_empty() && !rescan_required {
                                break;
                            }
                        }
                        Err(error) => {
                            cycle_failures += 1;
                            errors.push(error);
                            if active.is_empty() && cycle_failures >= 3 {
                                break;
                            }
                        }
                    }
                    next_cycle = if rescan_required {
                        Instant::now()
                    } else {
                        Instant::now() + ACTIVE_WORK_REPLENISH_INTERVAL
                    };
                }
                if discovery.is_none() && Instant::now() >= next_cycle {
                    let ids = active.keys().cloned().collect::<BTreeSet<_>>();
                    // An empty discovery captured before a completion cannot prove the queue
                    // is empty afterward: completion can free capacity or author a recovery Round.
                    rescan_required = false;
                    discovery = Some(scope.spawn(move || contain("admission", || {
                        crate::infrastructure::process::supervisor::coordination::with_lock_timeout(Duration::from_millis(200), || {
                            if self.workflow_paused()? || !self.target_still_attached(worker_registry)? { return Ok((0, Vec::new())); }
                            #[cfg(test)]
                            test_hooks::scheduler(self)?;
                            let skills_first = super::admission::skills_first(&self.runtime_root);
                            self.service_pending_skills(usize::from(skills_first));
                            let promoted = self.promote_backlog_to_todo()?;
                            let mut candidates = Vec::new();
                            for id in self.launchable_goals(&ids)? {
                                if let Some(lease) = self.reserve_goal(&id)? {
                                    candidates.push((id, lease));
                                }
                            }
                            self.service_pending_skills(32);
                            Ok((promoted, candidates))
                        })
                    })));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        if let Some(error) = errors.into_iter().next() {
            return Err(error);
        }
        results.sort_by_key(|(order, _)| *order);
        Ok(WorkflowPassResult {
            promoted,
            steps: results.into_iter().map(|(_, result)| result).collect(),
        })
    }
}
