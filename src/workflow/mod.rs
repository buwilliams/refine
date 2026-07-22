use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub mod behavior;
pub mod behaviors;
pub mod context;
pub mod promotion;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::model::JsonObject;
use crate::model::feature::{compare_feature_goal_order, is_ordered_feature_goal};
use crate::model::goal::GoalPriority;
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{FileProcessSupervisor, ProcessPauseState, ProcessSupervisor};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::MergeResult;
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{
    FileProjectStateStore, GoalSummaryProjection, ProjectionSnapshot,
};
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::behaviors::{
    WorkflowBuild, WorkflowImplementation, WorkflowQa, WorkflowReadyMerge, WorkflowReview,
    WorkflowTodo,
};
use crate::workflow::context::WorkflowContext;
use crate::workflow::promotion::BacklogPromotionService;

pub const WORKFLOW_AUTOMATION_STATE_FILE: &str = "workflow-automation-state.json";
const AUTOMATION_CONCURRENCY_LIMIT_REACHED: &str = "automation concurrency limit reached";

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPauseControl {
    Agents,
    TargetApp,
    AllAutomation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowClaimState {
    Claimed,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowClaim {
    pub claim_id: String,
    #[serde(alias = "gap_id")]
    pub goal_id: String,
    #[serde(default = "default_node_id")]
    pub node_id: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_target_app_id")]
    pub target_app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    pub state: WorkflowClaimState,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowPolicy {
    pub global_limit: usize,
    pub per_node_limit: usize,
    pub per_provider_limit: usize,
    pub per_target_app_limit: usize,
    pub active_node_id: String,
    pub provider: String,
    pub target_app_id: String,
}

impl Default for WorkflowPolicy {
    fn default() -> Self {
        Self {
            global_limit: 2,
            per_node_limit: 1,
            per_provider_limit: 2,
            per_target_app_limit: 2,
            active_node_id: default_node_id(),
            provider: default_provider(),
            target_app_id: default_target_app_id(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAutomationState {
    pub paused: BTreeSet<WorkflowPauseControl>,
    #[serde(default)]
    pub policy: WorkflowPolicy,
    pub claims: Vec<WorkflowClaim>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowPassResult {
    pub promoted: usize,
    pub claims: Vec<WorkflowClaim>,
    pub steps: Vec<WorkflowStepResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowStepResult {
    pub claim_id: String,
    pub goal_id: String,
    pub execution_id: String,
    pub provider: String,
    pub branch: String,
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeResult>,
    pub final_status: String,
    pub provider_output: String,
}

pub trait WorkflowAutomation {
    fn promote(&self) -> RefineResult<usize>;
    fn claim(&self, goal_id: &str) -> RefineResult<String>;
    fn start_claim(&self, claim_id: &str) -> RefineResult<String>;
    fn pause(&self, control: WorkflowPauseControl) -> RefineResult<()>;
    fn resume(&self, control: WorkflowPauseControl) -> RefineResult<()>;
    fn cancel(&self, execution_id: &str) -> RefineResult<()>;
    fn retry(&self, execution_id: &str) -> RefineResult<String>;
}

#[derive(Clone, Debug)]
pub struct WorkflowEngine {
    pub runtime_root: PathBuf,
    pub target_root: Option<PathBuf>,
}

impl WorkflowEngine {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        let runtime_root = runtime_root.into();
        Self {
            runtime_root,
            target_root: None,
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
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE)
    }

    fn refine_dir(&self) -> RefineResult<Option<PathBuf>> {
        self.target_root
            .as_ref()
            .map(|target_root| prepare_refine_dir(target_root))
            .transpose()
    }

    pub fn load_state(&self) -> RefineResult<WorkflowAutomationState> {
        read_state(&self.state_path())
    }

    fn save_state(&self, state: &mut WorkflowAutomationState) -> RefineResult<()> {
        state.policy = self.policy()?;
        state.updated_at = Some(now_timestamp());
        write_state(&self.state_path(), state)
    }

    pub fn policy(&self) -> RefineResult<WorkflowPolicy> {
        let mut policy = WorkflowPolicy::default();
        if let Some(target_root) = &self.target_root {
            let refine_dir = prepare_refine_dir(target_root)?;
            let settings =
                FileSettingsService::with_active_root(&refine_dir, &self.runtime_root).load()?;
            policy.global_limit = setting_usize(&settings, "parallel_run_cap", policy.global_limit);
            policy.per_node_limit = setting_cap_with_default_values(
                &settings,
                "parallel_per_node_cap",
                policy.global_limit,
                &[1, 2],
            );
            policy.per_provider_limit = setting_cap_with_default_values(
                &settings,
                "parallel_per_provider_cap",
                policy.global_limit,
                &[2],
            );
            policy.per_target_app_limit = setting_cap_with_default_values(
                &settings,
                "parallel_per_target_app_cap",
                policy.global_limit,
                &[2],
            );
            policy.provider = setting_string(&settings, "agent_cli", &policy.provider);
            policy.target_app_id = target_root.display().to_string();
            policy.active_node_id =
                FileNodeRegistryService::with_active_root(&refine_dir, &self.runtime_root)
                    .active_node_id()?;
        }
        Ok(policy)
    }

    pub fn apply_runtime_settings(&self) -> RefineResult<usize> {
        let mut state = self.load_state()?;
        state.policy = self.policy()?;
        let runnable = match self.ensure_automation_running(&state) {
            Ok(()) => true,
            Err(RefineError::Conflict(_)) => false,
            Err(error) => return Err(error),
        };
        self.save_state(&mut state)?;
        if runnable { self.promote() } else { Ok(0) }
    }

    pub fn promote_backlog_to_todo(&self) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(0);
        };
        self.promote_backlog_to_todo_for_refine_dir(&refine_dir)
    }

    fn promote_backlog_to_todo_for_refine_dir(&self, refine_dir: &Path) -> RefineResult<usize> {
        BacklogPromotionService::new(refine_dir, &self.runtime_root).promote_backlog_to_todo()
    }

    pub fn set_workflow_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        let state = if paused {
            supervisor.set_agents_paused(true)?;
            let state = supervisor.set_background_processes_stopped(true)?;
            self.rollback_in_progress_goals_to_todo()?;
            self.pause(WorkflowPauseControl::AllAutomation)?;
            state
        } else {
            supervisor.set_background_processes_stopped(false)?;
            let state = supervisor.set_agents_paused(false)?;
            self.resume(WorkflowPauseControl::AllAutomation)?;
            self.resume(WorkflowPauseControl::Agents)?;
            self.resume(WorkflowPauseControl::TargetApp)?;
            state
        };
        Ok(state)
    }

    pub fn set_agent_workflow_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        self.set_workflow_paused(paused)
    }

    pub fn rollback_in_progress_goals_to_todo(&self) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(0);
        };
        let snapshot = self.projection_snapshot(&refine_dir)?;
        let active_node_id = FileNodeRegistryService::new(&refine_dir).active_node_id()?;
        let goal_ids = snapshot
            .goals
            .values()
            .filter(|projection| projection.goal.status == GoalStatus::InProgress)
            .filter(|projection| {
                projection.goal.node_id.as_deref().unwrap_or("default") == active_node_id
            })
            .map(|projection| projection.goal.id.clone())
            .collect::<Vec<_>>();
        if goal_ids.is_empty() {
            return Ok(0);
        }
        let work_items = FileWorkItemService::new(refine_dir);
        for goal_id in &goal_ids {
            work_items.rollback_in_progress_goal_to_todo(goal_id)?;
        }
        self.interrupt_active_claims(&goal_ids)?;
        Ok(goal_ids.len())
    }

    pub fn fail_interrupted_goals(&self, detail: &str) -> RefineResult<usize> {
        if let Some(target_root) = &self.target_root {
            return with_repository_git_lock(target_root, || {
                self.fail_interrupted_goals_locked(detail)
            });
        }
        self.fail_interrupted_goals_locked(detail)
    }

    fn fail_interrupted_goals_locked(&self, detail: &str) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(0);
        };
        let snapshot = self.projection_snapshot(&refine_dir)?;
        let active_node_id = FileNodeRegistryService::new(&refine_dir).active_node_id()?;
        let goal_ids = snapshot
            .goals
            .values()
            .filter(|projection| {
                matches!(
                    projection.goal.status,
                    GoalStatus::InProgress
                        | GoalStatus::ReadyMerge
                        | GoalStatus::Build
                        | GoalStatus::Qa
                )
            })
            .filter(|projection| {
                projection.goal.node_id.as_deref().unwrap_or("default") == active_node_id
            })
            .map(|projection| projection.goal.id.clone())
            .collect::<Vec<_>>();
        if goal_ids.is_empty() {
            return Ok(0);
        }

        let detail = detail.trim();
        let detail = if detail.is_empty() {
            "workflow runner stopped before the Goal completed"
        } else {
            detail
        };
        let work_items = FileWorkItemService::new(&refine_dir);
        let logs = FileLogService::new(&refine_dir);
        for goal_id in &goal_ids {
            work_items.advance_automated_goal_status(goal_id, GoalStatus::Failed)?;
            let round_idx = ensure_workflow_round(&work_items, goal_id)?;
            logs.append_round_log(
                goal_id,
                round_idx,
                LogEntry {
                    datetime: now_timestamp(),
                    severity: "error".to_string(),
                    category: "workflow".to_string(),
                    message: format!("Workflow interrupted: {detail}"),
                    details: Some(json_object(json!({"reason": detail}))),
                    actions: Vec::new(),
                    actor: Some("refine".to_string()),
                    goal_id: Some(goal_id.clone()),
                },
            )?;
        }
        self.interrupt_active_claims(&goal_ids)?;
        Ok(goal_ids.len())
    }

    fn signal_workflow_subprocesses(&self, execution_id: &str, signal: &str) -> RefineResult<()> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        for process in supervisor.list()? {
            let matches_execution = process
                .details
                .as_deref()
                .and_then(|details| serde_json::from_str::<Value>(details).ok())
                .and_then(|details| {
                    details
                        .get("execution_id")
                        .and_then(|value| value.as_str())
                        .map(|value| value == execution_id)
                })
                .unwrap_or(false);
            if matches_execution {
                supervisor.signal(&process.id, signal)?;
            }
        }
        Ok(())
    }

    fn ensure_automation_running(&self, state: &WorkflowAutomationState) -> RefineResult<()> {
        if state.paused.contains(&WorkflowPauseControl::AllAutomation)
            || state.paused.contains(&WorkflowPauseControl::Agents)
            || state.paused.contains(&WorkflowPauseControl::TargetApp)
        {
            return Err(RefineError::Conflict(
                "workflow automation is paused".to_string(),
            ));
        }
        let pause_state = FileProcessSupervisor::new(&self.runtime_root).pause_state()?;
        if pause_state.background_processes_stopped || pause_state.agents_paused {
            return Err(RefineError::Conflict(
                "workflow automation is paused".to_string(),
            ));
        }
        Ok(())
    }

    fn active_claim<'a>(
        state: &'a WorkflowAutomationState,
        goal_id: &str,
    ) -> Option<&'a WorkflowClaim> {
        state.claims.iter().find(|claim| {
            claim.goal_id == goal_id
                && matches!(
                    claim.state,
                    WorkflowClaimState::Claimed | WorkflowClaimState::Running
                )
        })
    }

    fn claim_load(state: &WorkflowAutomationState, policy: &WorkflowPolicy) -> ClaimLoad {
        Self::claim_load_excluding(state, policy, None)
    }

    fn claim_load_excluding(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
        excluded_index: Option<usize>,
    ) -> ClaimLoad {
        let mut load = ClaimLoad::default();
        for claim in state
            .claims
            .iter()
            .enumerate()
            .filter(|(index, claim)| {
                Some(*index) != excluded_index
                    && matches!(
                        claim.state,
                        WorkflowClaimState::Claimed | WorkflowClaimState::Running
                    )
            })
            .map(|(_, claim)| claim)
        {
            load.global += 1;
            *load.by_node.entry(claim.node_id.clone()).or_default() += 1;
            *load.by_provider.entry(claim.provider.clone()).or_default() += 1;
            *load
                .by_target_app
                .entry(claim.target_app_id.clone())
                .or_default() += 1;
        }
        load.ensure_policy_keys(policy);
        load
    }

    fn capacity_available(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
        node_id: &str,
        provider: &str,
        target_app_id: &str,
    ) -> bool {
        let load = Self::claim_load(state, policy);
        Self::capacity_available_for_load(&load, policy, node_id, provider, target_app_id)
    }

    fn capacity_available_for_load(
        load: &ClaimLoad,
        policy: &WorkflowPolicy,
        node_id: &str,
        provider: &str,
        target_app_id: &str,
    ) -> bool {
        load.global < policy.global_limit
            && load.by_node.get(node_id).copied().unwrap_or(0) < policy.per_node_limit
            && load.by_provider.get(provider).copied().unwrap_or(0) < policy.per_provider_limit
            && load.by_target_app.get(target_app_id).copied().unwrap_or(0)
                < policy.per_target_app_limit
    }

    fn capacity_available_excluding(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
        node_id: &str,
        provider: &str,
        target_app_id: &str,
        excluded_index: usize,
    ) -> bool {
        let load = Self::claim_load_excluding(state, policy, Some(excluded_index));
        Self::capacity_available_for_load(&load, policy, node_id, provider, target_app_id)
    }

    fn record_claim_load(load: &mut ClaimLoad, claim: &WorkflowClaim) {
        load.global += 1;
        *load.by_node.entry(claim.node_id.clone()).or_default() += 1;
        *load.by_provider.entry(claim.provider.clone()).or_default() += 1;
        *load
            .by_target_app
            .entry(claim.target_app_id.clone())
            .or_default() += 1;
    }

    fn running_claim_load(state: &WorkflowAutomationState, policy: &WorkflowPolicy) -> ClaimLoad {
        let mut load = ClaimLoad::default();
        for claim in state
            .claims
            .iter()
            .filter(|claim| claim.state == WorkflowClaimState::Running)
        {
            Self::record_claim_load(&mut load, claim);
        }
        load.ensure_policy_keys(policy);
        load
    }

    fn launchable_claim_ids(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
    ) -> Vec<String> {
        let mut load = Self::running_claim_load(state, policy);
        let mut claim_ids = Vec::new();
        for claim in state
            .claims
            .iter()
            .filter(|claim| claim.state == WorkflowClaimState::Claimed)
        {
            if Self::capacity_available_for_load(
                &load,
                policy,
                &claim.node_id,
                &claim.provider,
                &claim.target_app_id,
            ) {
                Self::record_claim_load(&mut load, claim);
                claim_ids.push(claim.claim_id.clone());
            }
        }
        claim_ids
    }

    fn claim_metadata(
        &self,
        goal: Option<&GoalSummaryProjection>,
        policy: &WorkflowPolicy,
    ) -> RefineResult<ClaimMetadata> {
        let node_id = goal
            .and_then(|goal| goal.goal.node_id.clone())
            .unwrap_or_else(default_node_id);
        if node_id != policy.active_node_id {
            let goal_id = goal
                .map(|goal| goal.goal.id.as_str())
                .unwrap_or("requested Goal");
            return Err(RefineError::Conflict(format!(
                "{goal_id} is owned by node {node_id}, not active node {}",
                policy.active_node_id
            )));
        }
        Ok(ClaimMetadata {
            node_id,
            provider: policy.provider.clone(),
            target_app_id: policy.target_app_id.clone(),
        })
    }

    fn projection_snapshot(&self, refine_dir: &Path) -> RefineResult<ProjectionSnapshot> {
        FileProjectStateStore::with_runtime_root(refine_dir, &self.runtime_root)
            .load_or_refresh_projection(&self.runtime_root.join("cache"))
    }

    fn feature_claim_eligible(snapshot: &ProjectionSnapshot, goal: &GoalSummaryProjection) -> bool {
        let Some(feature_id) = goal.goal.feature_id.as_deref() else {
            return true;
        };
        let Some(feature_order) = goal.goal.feature_order else {
            return true;
        };
        let node_id = goal.goal.node_id.as_deref().unwrap_or("default");
        !snapshot.goals.values().any(|other| {
            other.goal.feature_id.as_deref() == Some(feature_id)
                && other.goal.node_id.as_deref().unwrap_or("default") == node_id
                && other
                    .goal
                    .feature_order
                    .is_some_and(|order| order < feature_order)
                && !matches!(
                    other.goal.status,
                    GoalStatus::Review | GoalStatus::Done | GoalStatus::Cancelled
                )
        }) && !snapshot.goals.values().any(|other| {
            other.goal.id != goal.goal.id
                && other.goal.feature_id.as_deref() == Some(feature_id)
                && other.goal.node_id.as_deref().unwrap_or("default") == node_id
                && is_ordered_feature_goal(goal.goal.feature_order)
                && is_ordered_feature_goal(other.goal.feature_order)
                && matches!(
                    other.goal.status,
                    GoalStatus::InProgress
                        | GoalStatus::ReadyMerge
                        | GoalStatus::Build
                        | GoalStatus::Qa
                )
        })
    }

    fn priority_claim_eligible(
        snapshot: &ProjectionSnapshot,
        goal: &GoalSummaryProjection,
    ) -> bool {
        let node_id = goal.goal.node_id.as_deref().unwrap_or("default");
        !snapshot.goals.values().any(|other| {
            other.goal.id != goal.goal.id
                && other.goal.status == GoalStatus::Todo
                && other.goal.node_id.as_deref().unwrap_or("default") == node_id
                && priority_rank(&other.goal.priority) > priority_rank(&goal.goal.priority)
                && Self::feature_claim_eligible(snapshot, other)
        })
    }

    pub fn evaluate_workflow(&self) -> RefineResult<WorkflowPassResult> {
        self.evaluate_workflow_locked()
    }

    fn evaluate_workflow_locked(&self) -> RefineResult<WorkflowPassResult> {
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
        let policy = self.policy()?;
        let claim_ids = Self::launchable_claim_ids(&state, &policy);
        let mut prepared = Vec::with_capacity(claim_ids.len());
        let mut first_error = None;
        for claim_id in claim_ids {
            let preparation = match self.start_claim(&claim_id) {
                Ok(execution_id) => self.prepare_started_claim(&claim_id, &execution_id),
                Err(RefineError::Conflict(message))
                    if message == AUTOMATION_CONCURRENCY_LIMIT_REACHED =>
                {
                    continue;
                }
                Err(error) => Err(error),
            };
            match preparation {
                Ok(ctx) => prepared.push((claim_id, ctx)),
                Err(error) => {
                    let _ = self.mark_claim_state(&claim_id, WorkflowClaimState::Failed);
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        let mut results = Vec::new();
        std::thread::scope(|scope| {
            let handles = prepared
                .into_iter()
                .map(|(claim_id, ctx)| {
                    (
                        claim_id,
                        scope.spawn(move || self.execute_prepared_claim(ctx)),
                    )
                })
                .collect::<Vec<_>>();
            for (claim_id, handle) in handles {
                let outcome = handle.join().unwrap_or_else(|_| {
                    Err(RefineError::Conflict(format!(
                        "workflow worker panicked for claim {claim_id}"
                    )))
                });
                match outcome {
                    Ok(result) => {
                        if let Err(error) =
                            self.mark_claim_state(&claim_id, WorkflowClaimState::Completed)
                        {
                            if first_error.is_none() {
                                first_error = Some(error);
                            }
                        } else {
                            results.push(result);
                        }
                    }
                    Err(error) => {
                        let _ = self.mark_claim_state(&claim_id, WorkflowClaimState::Failed);
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        });
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(results)
    }

    fn prepare_started_claim<'a>(
        &'a self,
        claim_id: &str,
        execution_id: &str,
    ) -> RefineResult<WorkflowContext<'a>> {
        let claim = self.claim_by_id(claim_id)?;
        let target_root = self.target_root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput(
                "target root is required to execute claimed workflow work".to_string(),
            )
        })?;
        let refine_dir = prepare_refine_dir(target_root)?;
        let work_items = FileWorkItemService::with_projection_cache(
            &refine_dir,
            self.runtime_root
                .join("cache/workflow")
                .join(&claim.claim_id),
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
        match WorkflowTodo.advance(&mut ctx)? {
            WorkflowAdvanceOutcome::Transition {
                to: GoalStatus::InProgress,
                ..
            } => Ok(ctx),
            WorkflowAdvanceOutcome::Noop { reason }
            | WorkflowAdvanceOutcome::Blocked { reason }
            | WorkflowAdvanceOutcome::Failed { reason }
            | WorkflowAdvanceOutcome::Completed { reason, .. }
            | WorkflowAdvanceOutcome::Transition { reason, .. } => {
                Err(RefineError::Conflict(reason))
            }
        }
    }

    fn execute_prepared_claim(
        &self,
        mut ctx: WorkflowContext<'_>,
    ) -> RefineResult<WorkflowStepResult> {
        self.advance_claim_behaviors(&mut ctx, GoalStatus::InProgress)?;
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

    fn advance_claim_behaviors(
        &self,
        ctx: &mut WorkflowContext<'_>,
        mut current: GoalStatus,
    ) -> RefineResult<()> {
        let implementation = WorkflowImplementation;
        let ready_merge = WorkflowReadyMerge;
        let build = WorkflowBuild;
        let qa = WorkflowQa;
        let review = WorkflowReview;
        let behaviors: [&dyn WorkflowBehavior; 5] =
            [&implementation, &ready_merge, &build, &qa, &review];
        loop {
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
            match behavior.advance(ctx)? {
                WorkflowAdvanceOutcome::Transition { to, .. } => {
                    current = to;
                }
                WorkflowAdvanceOutcome::Completed { .. } => return Ok(()),
                WorkflowAdvanceOutcome::Noop { reason }
                | WorkflowAdvanceOutcome::Blocked { reason }
                | WorkflowAdvanceOutcome::Failed { reason } => {
                    return Err(RefineError::Conflict(reason));
                }
            }
        }
    }

    fn claim_by_id(&self, claim_id: &str) -> RefineResult<WorkflowClaim> {
        self.load_state()?
            .claims
            .into_iter()
            .find(|claim| claim.claim_id == claim_id)
            .ok_or_else(|| RefineError::NotFound(format!("claim {claim_id} was not found")))
    }

    fn mark_claim_state(
        &self,
        claim_id: &str,
        claim_state: WorkflowClaimState,
    ) -> RefineResult<()> {
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
        claim.state = claim_state;
        claim.updated_at = now_timestamp();
        self.save_state(&mut state)
    }

    fn interrupt_active_claims(&self, goal_ids: &[String]) -> RefineResult<()> {
        let goal_ids = goal_ids.iter().collect::<BTreeSet<_>>();
        let mut state = self.load_state()?;
        let mut changed = false;
        let now = now_timestamp();
        for claim in &mut state.claims {
            if goal_ids.contains(&claim.goal_id)
                && matches!(
                    claim.state,
                    WorkflowClaimState::Claimed | WorkflowClaimState::Running
                )
            {
                claim.state = WorkflowClaimState::Interrupted;
                claim.updated_at = now.clone();
                changed = true;
            }
        }
        if changed {
            self.save_state(&mut state)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ClaimLoad {
    global: usize,
    by_node: BTreeMap<String, usize>,
    by_provider: BTreeMap<String, usize>,
    by_target_app: BTreeMap<String, usize>,
}

impl ClaimLoad {
    fn ensure_policy_keys(&mut self, policy: &WorkflowPolicy) {
        self.by_node
            .entry(policy.active_node_id.clone())
            .or_default();
        self.by_provider.entry(policy.provider.clone()).or_default();
        self.by_target_app
            .entry(policy.target_app_id.clone())
            .or_default();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClaimMetadata {
    node_id: String,
    provider: String,
    target_app_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct GovernanceEvaluation {
    failed: bool,
    message: Option<String>,
    details: JsonObject,
}

impl WorkflowAutomation for WorkflowEngine {
    fn promote(&self) -> RefineResult<usize> {
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(state
                .claims
                .iter()
                .filter(|claim| claim.state == WorkflowClaimState::Claimed)
                .count());
        };
        self.promote_backlog_to_todo_for_refine_dir(&refine_dir)?;
        let snapshot = self.projection_snapshot(&refine_dir)?;
        let mut eligible = snapshot
            .goals
            .values()
            .filter(|projection| projection.goal.status == GoalStatus::Todo)
            .filter(|projection| Self::feature_claim_eligible(&snapshot, projection))
            .filter(|projection| Self::priority_claim_eligible(&snapshot, projection))
            .cloned()
            .collect::<Vec<_>>();
        eligible.sort_by(|a, b| {
            priority_rank(&b.goal.priority)
                .cmp(&priority_rank(&a.goal.priority))
                .then_with(|| {
                    compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                })
                .then_with(|| a.goal.created.cmp(&b.goal.created))
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });

        let mut promoted = 0;
        for goal in eligible {
            if Self::active_claim(&state, &goal.goal.id).is_some() {
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
            let now = now_timestamp();
            state.claims.push(WorkflowClaim {
                claim_id: new_claim_id(),
                goal_id: goal.goal.id,
                node_id: metadata.node_id,
                provider: metadata.provider,
                target_app_id: metadata.target_app_id,
                execution_id: None,
                state: WorkflowClaimState::Claimed,
                created_at: now.clone(),
                updated_at: now,
            });
            promoted += 1;
        }
        if promoted > 0 {
            self.save_state(&mut state)?;
        }
        Ok(promoted)
    }

    fn claim(&self, goal_id: &str) -> RefineResult<String> {
        let goal_id = goal_id.trim();
        if goal_id.is_empty() {
            return Err(RefineError::InvalidInput("Goal id is required".to_string()));
        }
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        if let Some(existing) = Self::active_claim(&state, goal_id) {
            return Ok(existing.claim_id.clone());
        }
        let goal = if let Some(refine_dir) = self.refine_dir()? {
            let snapshot = self.projection_snapshot(&refine_dir)?;
            let goal = snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
                RefineError::NotFound(format!("Goal {goal_id} was not found in target state"))
            })?;
            if !Self::feature_claim_eligible(&snapshot, &goal) {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} is blocked by Feature order"
                )));
            }
            if !Self::priority_claim_eligible(&snapshot, &goal) {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} is blocked by higher priority work"
                )));
            }
            Some(goal)
        } else {
            None
        };
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
        let claim_id = claim_id.trim();
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
        let running_load = Self::running_claim_load(&state, &policy);
        if !Self::capacity_available_for_load(
            &running_load,
            &policy,
            &claim.node_id,
            &claim.provider,
            &claim.target_app_id,
        ) {
            return Err(RefineError::Conflict(
                AUTOMATION_CONCURRENCY_LIMIT_REACHED.to_string(),
            ));
        }
        if let Some(refine_dir) = self.refine_dir()? {
            let snapshot = self.projection_snapshot(&refine_dir)?;
            let goal = snapshot.goals.get(&claim.goal_id).ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Goal {} was not found in target state",
                    claim.goal_id
                ))
            })?;
            self.claim_metadata(Some(goal), &policy)?;
            if !Self::feature_claim_eligible(&snapshot, goal) {
                return Err(RefineError::Conflict(format!(
                    "Goal {} is blocked by Feature order",
                    claim.goal_id
                )));
            }
            if !Self::priority_claim_eligible(&snapshot, goal) {
                return Err(RefineError::Conflict(format!(
                    "Goal {} is blocked by higher priority work",
                    claim.goal_id
                )));
            }
        }
        let execution_id = new_execution_id();
        let claim = &mut state.claims[claim_index];
        claim.execution_id = Some(execution_id.clone());
        claim.state = WorkflowClaimState::Running;
        claim.updated_at = now_timestamp();
        self.save_state(&mut state)?;
        Ok(execution_id)
    }

    fn pause(&self, control: WorkflowPauseControl) -> RefineResult<()> {
        let mut state = self.load_state()?;
        state.paused.insert(control);
        self.save_state(&mut state)
    }

    fn resume(&self, control: WorkflowPauseControl) -> RefineResult<()> {
        let mut state = self.load_state()?;
        state.paused.remove(&control);
        self.save_state(&mut state)
    }

    fn cancel(&self, execution_id: &str) -> RefineResult<()> {
        let execution_id = execution_id.trim();
        self.signal_workflow_subprocesses(execution_id, "terminate")?;
        let mut state = self.load_state()?;
        if let Some(claim) = state
            .claims
            .iter_mut()
            .find(|claim| claim.execution_id.as_deref() == Some(execution_id))
        {
            claim.state = WorkflowClaimState::Cancelled;
            claim.updated_at = now_timestamp();
            self.save_state(&mut state)?;
        }
        Ok(())
    }

    fn retry(&self, execution_id: &str) -> RefineResult<String> {
        let execution_id = execution_id.trim();
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
        let node_id = state.claims[claim_index].node_id.clone();
        let provider = state.claims[claim_index].provider.clone();
        let target_app_id = state.claims[claim_index].target_app_id.clone();
        if !Self::capacity_available_excluding(
            &state,
            &policy,
            &node_id,
            &provider,
            &target_app_id,
            claim_index,
        ) {
            return Err(RefineError::Conflict(
                AUTOMATION_CONCURRENCY_LIMIT_REACHED.to_string(),
            ));
        }
        self.signal_workflow_subprocesses(execution_id, "terminate")?;
        let retried_execution_id = new_execution_id();
        let claim = &mut state.claims[claim_index];
        claim.execution_id = Some(retried_execution_id.clone());
        claim.state = WorkflowClaimState::Running;
        claim.updated_at = now_timestamp();
        self.save_state(&mut state)?;
        Ok(retried_execution_id)
    }
}

fn read_state(path: &Path) -> RefineResult<WorkflowAutomationState> {
    if !path.exists() {
        return Ok(WorkflowAutomationState::default());
    }
    let bytes = fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read automation state {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice::<WorkflowAutomationState>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse automation state {}: {error}",
            path.display()
        ))
    })
}

fn write_state(path: &Path, state: &WorkflowAutomationState) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create automation state directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
        RefineError::Serialization(format!("failed to encode automation state: {error}"))
    })?;
    fs::write(path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write automation state {}: {error}",
            path.display()
        ))
    })
}

fn setting_usize(settings: &JsonObject, key: &str, fallback: usize) -> usize {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn setting_cap_with_default_values(
    settings: &JsonObject,
    key: &str,
    fallback: usize,
    default_values: &[usize],
) -> usize {
    let Some(value) = settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
    else {
        return fallback;
    };
    if fallback > value && default_values.contains(&value) {
        fallback
    } else {
        value
    }
}

fn setting_string(settings: &JsonObject, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn ensure_workflow_round(work_items: &FileWorkItemService, goal_id: &str) -> RefineResult<usize> {
    let goal = work_items.show_goal_summary(goal_id)?;
    if let Some(idx) = goal.goal.round_count.checked_sub(1) {
        return Ok(idx);
    }
    let goal = work_items.append_goal_round_summary(
        goal_id,
        "Refine",
        "Implement and verify this Goal.",
    )?;
    goal.goal
        .round_count
        .checked_sub(1)
        .ok_or_else(|| RefineError::InvalidInput(format!("Goal {goal_id} has no rounds")))
}

fn implementation_branch_name(pattern: &str, goal_id: &str, round_idx: usize) -> String {
    let pattern = pattern.trim();
    let base = if pattern.is_empty() {
        "refine/{goal_id}"
    } else {
        pattern
    };
    let round = (round_idx + 1).to_string();
    let branch = base
        .replace("{goal_id}", goal_id)
        .replace("{goal}", goal_id)
        .replace("{round}", &round);
    if branch.contains(&format!("round-{round}")) || branch.contains(&format!("round/{round}")) {
        branch
    } else {
        format!("{branch}/round-{round}")
    }
}

fn agent_worktree_cwd(worktree_path: &str, agent_subpath: &str) -> RefineResult<PathBuf> {
    let root = PathBuf::from(worktree_path);
    let subpath = agent_subpath.trim();
    if subpath.is_empty() {
        return Ok(root);
    }
    let relative = Path::new(subpath);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(RefineError::InvalidInput(
            "agent_subpath must be a relative path inside the worktree".to_string(),
        ));
    }
    Ok(root.join(relative))
}

fn post_implementation_governance_prompt(
    governance: &Value,
    rules: &[Value],
    worktree_path: &str,
    provider_cwd: &Path,
    goal_id: &str,
    round_idx: usize,
) -> String {
    let product = governance
        .get("product")
        .and_then(Value::as_str)
        .unwrap_or("");
    let constitution = governance
        .get("constitution")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rules_json = serde_json::to_string_pretty(rules).unwrap_or_else(|_| "[]".to_string());
    format!(
        "Post-implementation governance review for Goal {goal_id}, round {}.\n\
         Inspect the current implementation worktree and determine whether the completed \
         implementation violates any Governance rule. The implementation has already been \
         committed on the current branch; inspect the repository and compare the branch changes \
         when needed. Do not edit files.\n\n\
         Worktree root: {worktree_path}\n\
         Provider cwd: {}\n\n\
         Return only JSON with this shape:\n\
         {{\"status\":\"passed|failed\",\"message\":\"short human-readable result\",\
         \"violations\":[{{\"rule_id\":\"...\",\"rule\":\"...\",\"message\":\"...\"}}]}}\n\n\
         Product:\n{product}\n\n\
         Constitution:\n{constitution}\n\n\
         Governance rules:\n{rules_json}",
        round_idx + 1,
        provider_cwd.display()
    )
}

fn parse_governance_provider_output(output: &str, rules_checked: usize) -> GovernanceEvaluation {
    let trimmed = output.trim();
    if let Some(value) = parse_json_value(trimmed) {
        return governance_evaluation_from_json(value, trimmed, rules_checked);
    }

    let normalized = trimmed.to_ascii_lowercase();
    let failed = !normalized.contains("no violation")
        && !normalized.contains("no governance violation")
        && (normalized.contains("rule violation")
            || normalized.contains("violates governance")
            || normalized.contains("governance failed")
            || normalized.contains("status: failed"));
    let message = failed.then(|| {
        if trimmed.is_empty() {
            "Governance rule violation detected.".to_string()
        } else {
            governance_violation_message(trimmed)
        }
    });
    GovernanceEvaluation {
        failed,
        message,
        details: json_object(json!({
            "phase": "post_implementation",
            "configured": true,
            "rules_checked": rules_checked,
            "failed_actions": if failed {
                json!([{"action": "fail", "message": trimmed}])
            } else {
                json!([])
            },
            "raw_output": trimmed
        })),
    }
}

fn governance_evaluation_from_json(
    value: Value,
    raw_output: &str,
    rules_checked: usize,
) -> GovernanceEvaluation {
    let violations = value
        .get("violations")
        .or_else(|| value.get("rule_violations"))
        .or_else(|| value.get("failed_actions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let status = value
        .get("status")
        .or_else(|| value.get("verdict"))
        .or_else(|| value.get("result"))
        .and_then(Value::as_str)
        .map(|status| status.trim().to_ascii_lowercase());
    let ok = value.get("ok").and_then(Value::as_bool);
    let explicit_failed = value
        .get("failed")
        .or_else(|| value.get("violates"))
        .or_else(|| value.get("violation"))
        .and_then(Value::as_bool);
    let failed = explicit_failed
        .or_else(|| ok.map(|ok| !ok))
        .or_else(|| {
            status.as_ref().map(|status| {
                matches!(
                    status.as_str(),
                    "failed" | "fail" | "blocked" | "violated" | "violation"
                )
            })
        })
        .unwrap_or(!violations.is_empty());
    let provider_message = value
        .get("message")
        .or_else(|| value.get("reason"))
        .or_else(|| value.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToString::to_string);
    let message = if failed {
        Some(provider_message.unwrap_or_else(|| violation_message_from_actions(&violations)))
    } else {
        provider_message
    };
    GovernanceEvaluation {
        failed,
        message,
        details: json_object(json!({
            "phase": "post_implementation",
            "configured": true,
            "rules_checked": rules_checked,
            "failed_actions": violations,
            "raw_output": raw_output,
            "verdict": value
        })),
    }
}

fn parse_json_value(raw: &str) -> Option<Value> {
    serde_json::from_str::<Value>(raw)
        .ok()
        .or_else(|| extract_json_object(raw).and_then(|json| serde_json::from_str(&json).ok()))
}

fn extract_json_object(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in raw[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(raw[start..=start + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn violation_message_from_actions(actions: &[Value]) -> String {
    actions
        .iter()
        .find_map(|action| {
            action
                .get("message")
                .or_else(|| action.get("reason"))
                .or_else(|| action.get("text"))
                .or_else(|| action.get("rule"))
                .and_then(Value::as_str)
                .map(governance_violation_message)
        })
        .unwrap_or_else(|| "Governance rule violation detected.".to_string())
}

fn governance_violation_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "Governance rule violation detected.".to_string()
    } else if message
        .to_ascii_lowercase()
        .contains("governance rule violation")
    {
        message.to_string()
    } else {
        format!("Governance rule violation: {message}")
    }
}

fn goal_agent_prompt(goal_id: &str) -> String {
    format!(
        "Run the goal agent for ready Goal {goal_id}. Work on Goal {goal_id}, report deterministic command outcomes, and leave the Goal ready for review. End with a short after-action report in simple terms covering what changed, why it changed, and the exact verification outcomes."
    )
}

fn json_object(value: serde_json::Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn default_node_id() -> String {
    "default".to_string()
}

fn default_provider() -> String {
    "claude".to_string()
}

fn default_target_app_id() -> String {
    "default".to_string()
}

fn priority_rank(priority: &GoalPriority) -> u8 {
    match priority {
        GoalPriority::Low => 0,
        GoalPriority::Medium => 1,
        GoalPriority::High => 2,
    }
}

fn new_claim_id() -> String {
    format!("res-{}", Uuid::new_v4())
}

fn new_execution_id() -> String {
    format!("exec-{}", Uuid::new_v4())
}

fn missing_workflow_artifact(name: &str, goal_id: &str) -> RefineError {
    RefineError::Conflict(format!(
        "workflow artifact {name} is missing for Goal {goal_id}"
    ))
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::supervisor::config::{FileGovernanceService, FileSettingsService};
    use crate::tools::host::agent_providers::smoke_ai_env_lock;
    use crate::tools::product::nodes::FileNodeRegistryService;
    use crate::tools::product::work_items::{BulkGoalSelection, FileWorkItemService};
    use std::process::Command;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn file_automation_promotes_todo_goals_and_starts_executions() {
        let temp_root = unique_temp_dir("automation");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Queued", Some("GOAL1"))
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .create_goal_summary("Backlog", Some("GOAL2"))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 1);
        assert_eq!(automation.promote().unwrap(), 0);
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].goal_id, "GOAL1");

        let execution_id = automation.start_claim(&state.claims[0].claim_id).unwrap();
        assert!(execution_id.starts_with("exec-"));
        let state = automation.load_state().unwrap();
        assert_eq!(
            state.claims[0].execution_id.as_deref(),
            Some(execution_id.as_str())
        );
        assert_eq!(state.claims[0].state, WorkflowClaimState::Running);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_auto_promotes_backlog_goals_when_configured() {
        let temp_root = unique_temp_dir("automation-backlog-promote");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Instant Backlog", Some("GOAL1"))
            .unwrap();
        work_items
            .create_goal_summary("Never Backlog", Some("GOAL2"))
            .unwrap();
        let settings = FileSettingsService::new(&refine_dir);
        settings
            .update(&json!({"backlog_promote_after_seconds": "-1"}))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 0);
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Backlog
        );

        settings
            .update(&json!({"backlog_promote_after_seconds": "0"}))
            .unwrap();
        assert_eq!(automation.promote().unwrap(), 2);
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Todo
        );
        assert_eq!(
            work_items.show_goal_summary("GOAL2").unwrap().goal.status,
            GoalStatus::Todo
        );
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 2);
        let mut claimed_goal_ids = state
            .claims
            .iter()
            .map(|claim| claim.goal_id.as_str())
            .collect::<Vec<_>>();
        claimed_goal_ids.sort_unstable();
        assert_eq!(claimed_goal_ids, vec!["GOAL1", "GOAL2"]);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_promotes_all_ordered_feature_backlog_goals() {
        let temp_root = unique_temp_dir("automation-feature-backlog-promote");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_feature_summary("Imported Feature", Some("FEA1"), None, None, None)
            .unwrap();
        for id in ["GOAL1", "GOAL2", "GOAL3"] {
            work_items.create_goal_summary(id, Some(id)).unwrap();
            work_items.assign_goal_to_feature("FEA1", id).unwrap();
            work_items.order_goal_in_feature("FEA1", id).unwrap();
        }
        FileSettingsService::new(&refine_dir)
            .update(&json!({"backlog_promote_after_seconds": "0"}))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote_backlog_to_todo().unwrap(), 3);
        for id in ["GOAL1", "GOAL2", "GOAL3"] {
            assert_eq!(
                work_items.show_goal_summary(id).unwrap().goal.status,
                GoalStatus::Todo
            );
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_uses_global_cap_for_single_node_defaults() {
        let temp_root = unique_temp_dir("automation-global-cap");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&json!({"parallel_run_cap": 3}))
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        for id in ["GOAL1", "GOAL2", "GOAL3", "GOAL4"] {
            work_items.create_goal_summary(id, Some(id)).unwrap();
            work_items
                .update_goal_metadata_summary(id, None, Some("high"), None, None)
                .unwrap();
            work_items
                .transition_goal_status(id, GoalStatus::Todo)
                .unwrap();
        }

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 3);
        let state = automation.load_state().unwrap();
        assert_eq!(state.policy.global_limit, 3);
        assert_eq!(state.policy.per_node_limit, 3);
        assert_eq!(state.policy.per_provider_limit, 3);
        assert_eq!(state.policy.per_target_app_limit, 3);
        assert_eq!(state.claims.len(), 3);
        assert_eq!(
            state
                .claims
                .iter()
                .map(|claim| claim.goal_id.as_str())
                .collect::<Vec<_>>(),
            vec!["GOAL1", "GOAL2", "GOAL3"]
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_blocks_lower_priority_work_behind_higher_priority_goals() {
        let temp_root = unique_temp_dir("automation-priority-band");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&json!({"parallel_run_cap": 3}))
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        for (id, priority) in [("LOW", "low"), ("MEDIUM", "medium"), ("HIGH", "high")] {
            work_items.create_goal_summary(id, Some(id)).unwrap();
            work_items
                .update_goal_metadata_summary(id, None, Some(priority), None, None)
                .unwrap();
            work_items
                .transition_goal_status(id, GoalStatus::Todo)
                .unwrap();
        }

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert!(automation.claim("MEDIUM").is_err());
        assert!(automation.claim("LOW").is_err());
        assert_eq!(automation.promote().unwrap(), 1);
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].goal_id, "HIGH");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_applies_runtime_settings_without_waiting_for_automation() {
        let temp_root = unique_temp_dir("automation-apply-runtime-settings");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Instant Backlog", Some("GOAL1"))
            .unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "parallel_run_cap": 7,
                "parallel_per_node_cap": 7,
                "backlog_promote_after_seconds": "0",
                "agent_cli": "smoke-ai"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.apply_runtime_settings().unwrap(), 1);
        let state = automation.load_state().unwrap();
        assert_eq!(state.policy.global_limit, 7);
        assert_eq!(state.policy.per_node_limit, 7);
        assert_eq!(state.policy.provider, "smoke-ai");
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].goal_id, "GOAL1");
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Todo
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_applies_runtime_settings_with_legacy_gap_claims() {
        let temp_root = unique_temp_dir("automation-legacy-gap-claims");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&runtime_root).unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({"agent_cli": "smoke-ai"}))
            .unwrap();
        fs::write(
            runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE),
            serde_json::to_vec_pretty(&json!({
                "paused": [],
                "claims": [{
                    "claim_id": "res-legacy",
                    "gap_id": "GOAL1",
                    "state": "completed",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z"
                }],
                "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.apply_runtime_settings().unwrap(), 0);
        let state = automation.load_state().unwrap();
        assert_eq!(state.policy.provider, "smoke-ai");
        assert_eq!(state.claims[0].goal_id, "GOAL1");
        let persisted: Value = serde_json::from_slice(
            &fs::read(runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted["claims"][0]["goal_id"], "GOAL1");
        assert!(persisted["claims"][0].get("gap_id").is_none());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_runtime_settings_skip_off_node_backlog_promotions() {
        let temp_root = unique_temp_dir("automation-runtime-settings-off-node");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&json!({"backlog_promote_after_seconds": "0"}))
            .unwrap();
        FileNodeRegistryService::new(&refine_dir)
            .create("remote-node")
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Local backlog", Some("LOCAL"))
            .unwrap();
        work_items
            .create_goal_summary("Remote backlog", Some("REMOTE"))
            .unwrap();
        work_items
            .bulk_transfer_goals_to_node(
                "remote-node",
                BulkGoalSelection {
                    selected_ids: Some(vec!["REMOTE".to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.apply_runtime_settings().unwrap(), 1);
        assert_eq!(
            work_items.show_goal_summary("LOCAL").unwrap().goal.status,
            GoalStatus::Todo
        );
        assert_eq!(
            work_items.show_goal_summary("REMOTE").unwrap().goal.status,
            GoalStatus::Backlog
        );
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].goal_id, "LOCAL");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_enforces_configured_concurrency_limits() {
        let temp_root = unique_temp_dir("automation-limits");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "parallel_run_cap": 2,
                "parallel_per_node_cap": 2,
                "parallel_per_provider_cap": 1,
                "parallel_per_target_app_cap": 2,
                "agent_cli": "smoke-ai"
            }))
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        for id in ["GOAL1", "GOAL2", "GOAL3"] {
            work_items.create_goal_summary(id, Some(id)).unwrap();
            work_items
                .transition_goal_status(id, GoalStatus::Todo)
                .unwrap();
        }

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 1);
        assert_eq!(automation.promote().unwrap(), 0);
        let state = automation.load_state().unwrap();
        assert_eq!(state.policy.provider, "smoke-ai");
        assert_eq!(state.policy.per_provider_limit, 1);
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].provider, "smoke-ai");
        assert_eq!(state.claims[0].node_id, "default");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_enforces_active_node_ownership() {
        let temp_root = unique_temp_dir("automation-node-ownership");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Local", Some("LOCAL"))
            .unwrap();
        work_items
            .transition_goal_status("LOCAL", GoalStatus::Todo)
            .unwrap();
        work_items
            .create_goal_summary("Remote", Some("REMOTE"))
            .unwrap();
        work_items
            .transition_goal_status("REMOTE", GoalStatus::Todo)
            .unwrap();
        FileNodeRegistryService::new(&refine_dir)
            .create("remote-node")
            .unwrap();
        work_items
            .bulk_transfer_goals_to_node(
                "remote-node",
                BulkGoalSelection {
                    selected_ids: Some(vec!["REMOTE".to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 1);
        assert!(automation.claim("REMOTE").is_err());

        FileNodeRegistryService::with_active_root(&refine_dir, &runtime_root)
            .activate("remote-node")
            .unwrap();
        let remote_automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let remote_claim = remote_automation.claim("REMOTE").unwrap();
        let state = remote_automation.load_state().unwrap();
        assert!(
            state
                .claims
                .iter()
                .any(|claim| { claim.claim_id == remote_claim && claim.node_id == "remote-node" })
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_respects_feature_order_on_promote_claim_and_start() {
        let temp_root = unique_temp_dir("automation-feature-order");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let claim_runtime_root = temp_root.join("run/8081");
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "parallel_run_cap": 2,
                "parallel_per_node_cap": 2
            }))
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_feature_summary("Feature", Some("FEAT1"), None, None, None)
            .unwrap();
        for id in ["FIRST", "SECOND", "UNORDERED"] {
            work_items.create_goal_summary(id, Some(id)).unwrap();
            work_items
                .transition_goal_status(id, GoalStatus::Todo)
                .unwrap();
            work_items.assign_goal_to_feature("FEAT1", id).unwrap();
        }
        for id in ["FIRST", "SECOND"] {
            work_items.order_goal_in_feature("FEAT1", id).unwrap();
        }

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert!(automation.claim("SECOND").is_err());
        assert_eq!(automation.promote().unwrap(), 2);
        let state = automation.load_state().unwrap();
        let claimed_goal_ids = state
            .claims
            .iter()
            .map(|claim| claim.goal_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(claimed_goal_ids, vec!["FIRST", "UNORDERED"]);

        work_items
            .bulk_update_goals(
                BulkGoalSelection {
                    selected_ids: Some(vec!["FIRST".to_string()]),
                    ..Default::default()
                },
                crate::tools::product::work_items::BulkGoalUpdate::Status("review".to_string()),
            )
            .unwrap();
        let claim_automation = WorkflowEngine::with_target_root(&claim_runtime_root, &target_root);
        assert_eq!(claim_automation.promote().unwrap(), 2);
        let state = claim_automation.load_state().unwrap();
        let second_claim = state
            .claims
            .iter()
            .find(|claim| claim.goal_id == "SECOND")
            .map(|claim| claim.claim_id.clone())
            .unwrap();
        work_items
            .bulk_update_goals(
                BulkGoalSelection {
                    selected_ids: Some(vec!["FIRST".to_string()]),
                    ..Default::default()
                },
                crate::tools::product::work_items::BulkGoalUpdate::Status("todo".to_string()),
            )
            .unwrap();
        assert!(claim_automation.start_claim(&second_claim).is_err());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_reapplies_lowered_concurrency_limits_before_launch() {
        let temp_root = unique_temp_dir("automation-lowered-launch-limits");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
        fs::write(
            target_root.join("app.py"),
            "def health():\n    return 'ok'\n",
        )
        .unwrap();
        git(
            &target_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
        git(&target_root, &["add", "app.py"]).unwrap();
        git(&target_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             printf '%s\\n' 'lowered-cap agent completed' > agent.txt\n\
             printf '%s\\n' 'smoke-ai lowered-cap goal-agent response'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        for goal_id in ["GOAL1", "GOAL2"] {
            work_items
                .create_goal_summary(goal_id, Some(goal_id))
                .unwrap();
            work_items
                .append_goal_round_summary(goal_id, "Reporter", "Prompt")
                .unwrap();
            work_items
                .transition_goal_status(goal_id, GoalStatus::Todo)
                .unwrap();
        }
        let settings = FileSettingsService::new(&refine_dir);
        settings
            .update(&json!({
                "parallel_run_cap": 2,
                "parallel_per_node_cap": 2,
                "parallel_per_provider_cap": 2,
                "parallel_per_target_app_cap": 2,
                "agent_cli": "smoke-ai",
                "quality_enabled": "0"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 2);
        settings
            .update(&json!({
                "parallel_run_cap": 1,
                "parallel_per_node_cap": 1,
                "parallel_per_provider_cap": 1,
                "parallel_per_target_app_cap": 1
            }))
            .unwrap();

        let steps = automation.execute_claimed_work().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].goal_id, "GOAL1");
        let state = automation.load_state().unwrap();
        assert_eq!(state.policy.global_limit, 1);
        assert_eq!(state.claims[0].state, WorkflowClaimState::Completed);
        assert_eq!(state.claims[1].state, WorkflowClaimState::Claimed);
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Review
        );
        assert_eq!(
            work_items.show_goal_summary("GOAL2").unwrap().goal.status,
            GoalStatus::Todo
        );

        unsafe {
            if let Some(previous) = previous_smoke_ai {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_executes_eligible_claims_in_parallel() {
        let temp_root = unique_temp_dir("automation-parallel-execution");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let marker_root = temp_root.join("parallel-markers");
        let smoke_ai = temp_root.join("smoke-ai");
        fs::create_dir_all(&marker_root).unwrap();
        fs::write(
            target_root.join("app.py"),
            "def health():\n    return 'ok'\n",
        )
        .unwrap();
        git(
            &target_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
        git(&target_root, &["add", "app.py"]).unwrap();
        git(&target_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
        fs::write(
            &smoke_ai,
            format!(
                "#!/bin/sh\n\
                 marker_root='{}'\n\
                 touch \"$marker_root/$(basename \"$PWD\")\"\n\
                 attempt=0\n\
                 while [ \"$(find \"$marker_root\" -type f | wc -l)\" -lt 2 ]; do\n\
                   attempt=$((attempt + 1))\n\
                   [ \"$attempt\" -ge 500 ] && exit 42\n\
                   sleep 0.01\n\
                 done\n\
                 printf '%s\\n' 'parallel agent completed' > agent.txt\n\
                 printf '%s\\n' 'smoke-ai parallel goal-agent response'\n",
                marker_root.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        for goal_id in ["GOAL1", "GOAL2"] {
            work_items
                .create_goal_summary(goal_id, Some(goal_id))
                .unwrap();
            work_items
                .append_goal_round_summary(goal_id, "Reporter", "Prompt")
                .unwrap();
            work_items
                .transition_goal_status(goal_id, GoalStatus::Todo)
                .unwrap();
        }
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "parallel_run_cap": 2,
                "parallel_per_node_cap": 2,
                "parallel_per_provider_cap": 2,
                "parallel_per_target_app_cap": 2,
                "agent_cli": "smoke-ai",
                "quality_enabled": "0"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let result = automation.evaluate_workflow().unwrap();
        assert_eq!(
            result
                .steps
                .iter()
                .map(|step| step.goal_id.as_str())
                .collect::<Vec<_>>(),
            vec!["GOAL1", "GOAL2"]
        );
        assert_eq!(fs::read_dir(&marker_root).unwrap().count(), 2);
        for goal_id in ["GOAL1", "GOAL2"] {
            assert_eq!(
                work_items.show_goal_summary(goal_id).unwrap().goal.status,
                GoalStatus::Review
            );
        }
        assert!(
            automation
                .load_state()
                .unwrap()
                .claims
                .iter()
                .all(|claim| claim.state == WorkflowClaimState::Completed)
        );

        unsafe {
            if let Some(previous) = previous_smoke_ai {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_fails_in_progress_goal_on_post_implementation_governance_violation() {
        let temp_root = unique_temp_dir("automation-governance");
        let target_root = temp_root.clone();
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
        fs::create_dir_all(&temp_root).unwrap();
        fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
        git(&temp_root, &["init", "-q"]).unwrap();
        git(
            &temp_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
        git(&temp_root, &["add", "app.py"]).unwrap();
        git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"Post-implementation governance review\"*)\n\
               printf '%s\\n' '{\"status\":\"failed\",\"message\":\"Do not append smoke markers.\",\"violations\":[{\"rule_id\":\"rule-1\",\"rule\":\"Do not append smoke markers.\",\"message\":\"app.py contains a smoke marker\"}]}'\n\
               ;;\n\
             *)\n\
               printf '\\n# automated by smoke-ai governance violation\\n' >> app.py\n\
               printf '%s\\n' 'smoke-ai goal-agent response'\n\
               ;;\n\
             esac\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Governed implementation", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({"agent_cli": "smoke-ai"}))
            .unwrap();
        FileGovernanceService::new(&refine_dir)
            .save(&json!({
                "product": "A small app.",
                "constitution": "Keep generated markers out of app.py.",
                "rules": [{"id": "rule-1", "text": "Do not append smoke markers.", "source": "manual"}]
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let error = automation.evaluate_workflow().unwrap_err();
        assert!(error.to_string().contains("Do not append smoke markers."));
        let goal = work_items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(goal["status"], "failed");
        let latest = &goal["rounds"][0];
        assert_eq!(latest["rule_state"], "failed");
        assert_eq!(latest["quality_state"], "unclassified");
        assert!(
            latest["governance_message"]
                .as_str()
                .unwrap_or("")
                .contains("Do not append smoke markers.")
        );
        assert_eq!(latest["governance_details"]["phase"], "post_implementation");
        assert_eq!(latest["governance_rule_actions"][0]["rule_id"], "rule-1");
        unsafe {
            if let Some(previous) = previous_smoke_ai {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_accepts_agent_precommitted_implementation_branch() {
        let temp_root = unique_temp_dir("automation-agent-precommit");
        let target_root = temp_root.clone();
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
        fs::create_dir_all(&temp_root).unwrap();
        fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
        git(&temp_root, &["init", "-q"]).unwrap();
        git(
            &temp_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
        git(&temp_root, &["add", "app.py"]).unwrap();
        git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             printf '%s\\n' 'agent precommitted implementation' > agent.txt\n\
             git add agent.txt\n\
             git commit -q -m 'agent precommit'\n\
             printf '%s\\n' 'smoke-ai committed before Refine commit step'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Precommitted implementation", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "agent_cli": "smoke-ai",
                "quality_enabled": "0"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let result = automation.evaluate_workflow().unwrap();
        let worktree_path = target_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].commit.len(), 40);
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Review
        );
        assert_eq!(
            fs::read_to_string(worktree_path.join("agent.txt")).unwrap(),
            "agent precommitted implementation\n"
        );
        assert!(!target_root.join("agent.txt").exists());
        assert_eq!(
            git_stdout(&worktree_path, &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            result.steps[0].commit
        );
        assert_eq!(
            git_stdout(&worktree_path, &["log", "--pretty=%s", "-1"])
                .unwrap()
                .trim(),
            "agent precommit"
        );

        unsafe {
            if let Some(previous) = previous_smoke_ai {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }

        fs::remove_dir_all(worktree_path).ok();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_treats_clean_noop_implementation_as_reviewable() {
        let temp_root = unique_temp_dir("automation-agent-noop");
        let target_root = temp_root.clone();
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
        fs::create_dir_all(&temp_root).unwrap();
        fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
        git(&temp_root, &["init", "-q"]).unwrap();
        git(
            &temp_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
        git(&temp_root, &["add", "app.py"]).unwrap();
        git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
        let initial_head = git_stdout(&target_root, &["rev-parse", "HEAD"]).unwrap();
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             printf '%s\\n' 'smoke-ai verified clean no-op implementation'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("No-op implementation", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "agent_cli": "smoke-ai",
                "quality_enabled": "0"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let result = automation.evaluate_workflow().unwrap();
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].commit, initial_head.trim());
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Review
        );
        assert_eq!(
            git_stdout(&target_root, &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            initial_head.trim()
        );
        let goal = work_items.show_goal_detail("GOAL1").unwrap();
        let round_logs = goal["rounds"][0]["logs"].as_array().unwrap();
        assert!(
            round_logs.iter().any(|log| {
                log["message"].as_str() == Some("No implementation changes to commit")
            })
        );
        assert!(!round_logs.iter().any(|log| {
            log["message"]
                .as_str()
                .unwrap_or("")
                .starts_with("Workflow failed")
        }));

        unsafe {
            if let Some(previous) = previous_smoke_ai {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_reuses_existing_round_worktree_on_retry() {
        let temp_root = unique_temp_dir("automation-existing-worktree-retry");
        let target_root = temp_root.clone();
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
        fs::create_dir_all(&temp_root).unwrap();
        fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
        git(&temp_root, &["init", "-q"]).unwrap();
        git(
            &temp_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
        git(&temp_root, &["add", "app.py"]).unwrap();
        git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();

        let branch = "refine/GOAL1/round-1";
        let worktree_path = temp_root
            .join(".git/refine-worktrees")
            .join(branch.replace('/', "-"));
        fs::create_dir_all(worktree_path.parent().unwrap()).unwrap();
        git(
            &temp_root,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::write(
            worktree_path.join("agent.txt"),
            "existing retry implementation\n",
        )
        .unwrap();
        git(&worktree_path, &["add", "agent.txt"]).unwrap();
        git(&worktree_path, &["commit", "-q", "-m", "agent precommit"]).unwrap();
        let precommitted = git_stdout(&worktree_path, &["rev-parse", "HEAD"]).unwrap();
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             printf '%s\\n' 'smoke-ai reused existing worktree'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Retry existing worktree", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
            .unwrap();
        work_items
            .update_goal_branch_name("GOAL1", Some(branch))
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "agent_cli": "smoke-ai",
                "quality_enabled": "0"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let result = automation.evaluate_workflow().unwrap();
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].commit, precommitted.trim());
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Review
        );
        assert_eq!(
            fs::read_to_string(worktree_path.join("agent.txt")).unwrap(),
            "existing retry implementation\n"
        );
        assert!(!target_root.join("agent.txt").exists());

        unsafe {
            if let Some(previous) = previous_smoke_ai {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }

        fs::remove_dir_all(&worktree_path).ok();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_fails_goal_and_preserves_candidate_on_qa_failure() {
        let temp_root = unique_temp_dir("automation-qa-candidate");
        let target_root = temp_root.clone();
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
        fs::create_dir_all(&temp_root).unwrap();
        fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
        git(&temp_root, &["init", "-q"]).unwrap();
        git(
            &temp_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
        git(&temp_root, &["add", "app.py"]).unwrap();
        git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             printf 'qa should fail\\n' > fail-qa\n\
             printf '%s\\n' 'smoke-ai goal-agent response'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let branch = "refine/GOAL1/round-1";
        let worktree_path = target_root
            .join(".git/refine-worktrees")
            .join(branch.replace('/', "-"));
        let initial_head = git_stdout(&target_root, &["rev-parse", "HEAD"]).unwrap();

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Implementation with failing QA", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "agent_cli": "smoke-ai",
                "quality_enabled": "1",
                "target_app_build_command": "printf build-ok",
                "target_app_test_command": "test ! -f fail-qa",
                "allowed_commands": "printf, test"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let error = automation.evaluate_workflow().unwrap_err();
        assert!(error.to_string().contains("quality checks failed"));
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Failed
        );
        assert!(!target_root.join("fail-qa").exists());
        assert!(worktree_path.exists());
        assert!(worktree_path.join("fail-qa").exists());
        assert_eq!(
            git_stdout(&target_root, &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            initial_head.trim()
        );
        let worktrees = git_stdout(&target_root, &["worktree", "list", "--porcelain"]).unwrap();
        assert!(worktrees.contains(&format!("branch refs/heads/{branch}")));

        unsafe {
            if let Some(previous) = previous_smoke_ai {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }

        fs::remove_dir_all(&worktree_path).ok();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_pauses_cancels_and_retries_executions() {
        let temp_root = unique_temp_dir("automation-controls");
        let automation = WorkflowEngine::new(temp_root.join("run/8080"));

        automation
            .pause(WorkflowPauseControl::AllAutomation)
            .unwrap();
        assert!(automation.claim("GOAL1").is_err());
        automation
            .resume(WorkflowPauseControl::AllAutomation)
            .unwrap();
        FileProcessSupervisor::new(temp_root.join("run/8080"))
            .set_agents_paused(true)
            .unwrap();
        assert!(automation.claim("GOAL1").is_err());
        FileProcessSupervisor::new(temp_root.join("run/8080"))
            .set_agents_paused(false)
            .unwrap();

        let claim_id = automation.claim("GOAL1").unwrap();
        assert_eq!(automation.claim("GOAL1").unwrap(), claim_id);
        let execution_id = automation.start_claim(&claim_id).unwrap();
        automation.cancel(&execution_id).unwrap();
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims[0].state, WorkflowClaimState::Cancelled);

        let retried_execution_id = automation.retry(&execution_id).unwrap();
        assert_ne!(retried_execution_id, execution_id);
        assert!(retried_execution_id.starts_with("exec-"));
        let state = automation.load_state().unwrap();
        assert_eq!(
            state.claims[0].execution_id.as_deref(),
            Some(retried_execution_id.as_str())
        );
        assert_eq!(state.claims[0].state, WorkflowClaimState::Running);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_pause_moves_in_progress_goals_back_to_todo() {
        let temp_root = unique_temp_dir("automation-pause-rollback");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Running work", Some("GOAL1"))
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let claim_id = automation.claim("GOAL1").unwrap();
        automation.start_claim(&claim_id).unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
            .unwrap();

        let pause_state = automation.set_agent_workflow_paused(true).unwrap();
        assert!(pause_state.agents_paused);
        assert!(pause_state.background_processes_stopped);
        assert!(
            automation
                .load_state()
                .unwrap()
                .paused
                .contains(&WorkflowPauseControl::AllAutomation)
        );
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Todo
        );
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims[0].state, WorkflowClaimState::Interrupted);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_recovery_fails_interrupted_goals_for_restart() {
        let temp_root = unique_temp_dir("automation-interrupted-recovery");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Interrupted work", Some("GOAL1"))
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let claim_id = automation.claim("GOAL1").unwrap();
        automation.start_claim(&claim_id).unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
            .unwrap();

        assert_eq!(
            automation
                .fail_interrupted_goals("runner terminated")
                .unwrap(),
            1
        );
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Failed
        );
        assert_eq!(
            automation.load_state().unwrap().claims[0].state,
            WorkflowClaimState::Interrupted
        );
        let logs = FileLogService::new(&refine_dir)
            .all_round_logs("GOAL1")
            .unwrap();
        assert!(logs.iter().any(|entry| {
            entry.entry.severity == "error" && entry.entry.message.contains("runner terminated")
        }));

        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        assert_eq!(
            work_items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Todo
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn workflow_evaluation_does_not_hold_the_repository_lock_between_git_steps() {
        let temp_root = unique_temp_dir("automation-narrow-git-lock");
        let target_root = temp_root.join("target");
        let _refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");

        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let lock_root = target_root.clone();
        let lock_thread = std::thread::spawn(move || {
            with_repository_git_lock(&lock_root, || {
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        locked_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let evaluation_thread = std::thread::spawn(move || {
            finished_tx.send(automation.evaluate_workflow()).unwrap();
        });
        let evaluation = finished_rx.recv_timeout(Duration::from_millis(250));

        release_tx.send(()).unwrap();
        lock_thread.join().unwrap();
        evaluation_thread.join().unwrap();
        evaluation
            .expect("workflow evaluation waited on the repository lock outside a Git step")
            .unwrap();

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn test_refine_dir(target_root: &Path) -> PathBuf {
        fs::create_dir_all(target_root).unwrap();
        if !target_root.join(".git").exists() {
            git(target_root, &["init", "-q"]).unwrap();
        }
        crate::tools::host::project_layout::refine_dir_for_target_root(target_root).unwrap()
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if output.status.success() {
            return Ok(());
        }
        Err(RefineError::Io(format!(
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> RefineResult<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        Err(RefineError::Io(format!(
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}
