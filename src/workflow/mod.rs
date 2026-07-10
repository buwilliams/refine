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
use crate::model::feature::{compare_feature_gap_order, is_ordered_feature_gap};
use crate::model::gap::GapPriority;
use crate::model::workflow::GapStatus;
use crate::process::subprocess::{FileProcessSupervisor, ProcessPauseState, ProcessSupervisor};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::MergeResult;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{
    FileProjectStateStore, GapSummaryProjection, ProjectionSnapshot,
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
    pub gap_id: String,
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
    pub gap_id: String,
    pub execution_id: String,
    pub provider: String,
    pub branch: String,
    pub commit: String,
    pub merge: MergeResult,
    pub final_status: String,
    pub provider_output: String,
}

pub trait WorkflowAutomation {
    fn promote(&self) -> RefineResult<usize>;
    fn claim(&self, gap_id: &str) -> RefineResult<String>;
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

    fn refine_dir(&self) -> Option<PathBuf> {
        self.target_root
            .as_ref()
            .map(|target_root| target_root.join(".refine"))
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
            let refine_dir = target_root.join(".refine");
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
        let Some(refine_dir) = self.refine_dir() else {
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
            self.rollback_in_progress_gaps_to_todo()?;
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

    pub fn rollback_in_progress_gaps_to_todo(&self) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir() else {
            return Ok(0);
        };
        let snapshot = self.projection_snapshot(&refine_dir)?;
        let active_node_id = FileNodeRegistryService::new(&refine_dir).active_node_id()?;
        let gap_ids = snapshot
            .gaps
            .values()
            .filter(|projection| projection.gap.status == GapStatus::InProgress)
            .filter(|projection| {
                projection.gap.node_id.as_deref().unwrap_or("default") == active_node_id
            })
            .map(|projection| projection.gap.id.clone())
            .collect::<Vec<_>>();
        if gap_ids.is_empty() {
            return Ok(0);
        }
        let work_items = FileWorkItemService::new(refine_dir);
        for gap_id in &gap_ids {
            work_items.rollback_in_progress_gap_to_todo(gap_id)?;
        }
        self.interrupt_active_claims(&gap_ids)?;
        Ok(gap_ids.len())
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
        gap_id: &str,
    ) -> Option<&'a WorkflowClaim> {
        state.claims.iter().find(|claim| {
            claim.gap_id == gap_id
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
        load.global < policy.global_limit
            && load.by_node.get(node_id).copied().unwrap_or(0) < policy.per_node_limit
            && load.by_provider.get(provider).copied().unwrap_or(0) < policy.per_provider_limit
            && load.by_target_app.get(target_app_id).copied().unwrap_or(0)
                < policy.per_target_app_limit
    }

    fn claim_metadata(
        &self,
        gap: Option<&GapSummaryProjection>,
        policy: &WorkflowPolicy,
    ) -> RefineResult<ClaimMetadata> {
        let node_id = gap
            .and_then(|gap| gap.gap.node_id.clone())
            .unwrap_or_else(|| default_node_id());
        if node_id != policy.active_node_id {
            let gap_id = gap
                .map(|gap| gap.gap.id.as_str())
                .unwrap_or("requested Gap");
            return Err(RefineError::Conflict(format!(
                "{gap_id} is owned by node {node_id}, not active node {}",
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

    fn feature_claim_eligible(snapshot: &ProjectionSnapshot, gap: &GapSummaryProjection) -> bool {
        let Some(feature_id) = gap.gap.feature_id.as_deref() else {
            return true;
        };
        let Some(feature_order) = gap.gap.feature_order else {
            return true;
        };
        let node_id = gap.gap.node_id.as_deref().unwrap_or("default");
        !snapshot.gaps.values().any(|other| {
            other.gap.feature_id.as_deref() == Some(feature_id)
                && other.gap.node_id.as_deref().unwrap_or("default") == node_id
                && other
                    .gap
                    .feature_order
                    .is_some_and(|order| order < feature_order)
                && !matches!(
                    other.gap.status,
                    GapStatus::Review | GapStatus::Done | GapStatus::Cancelled
                )
        }) && !snapshot.gaps.values().any(|other| {
            other.gap.id != gap.gap.id
                && other.gap.feature_id.as_deref() == Some(feature_id)
                && other.gap.node_id.as_deref().unwrap_or("default") == node_id
                && is_ordered_feature_gap(gap.gap.feature_order)
                && is_ordered_feature_gap(other.gap.feature_order)
                && matches!(
                    other.gap.status,
                    GapStatus::InProgress
                        | GapStatus::ReadyMerge
                        | GapStatus::Build
                        | GapStatus::Qa
                )
        })
    }

    fn priority_claim_eligible(snapshot: &ProjectionSnapshot, gap: &GapSummaryProjection) -> bool {
        let node_id = gap.gap.node_id.as_deref().unwrap_or("default");
        !snapshot.gaps.values().any(|other| {
            other.gap.id != gap.gap.id
                && other.gap.status == GapStatus::Todo
                && other.gap.node_id.as_deref().unwrap_or("default") == node_id
                && priority_rank(&other.gap.priority) > priority_rank(&gap.gap.priority)
                && Self::feature_claim_eligible(snapshot, other)
        })
    }

    pub fn evaluate_workflow(&self) -> RefineResult<WorkflowPassResult> {
        if let Some(target_root) = &self.target_root {
            return with_repository_git_lock(target_root, || self.evaluate_workflow_locked());
        }
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
        let claim_ids = state
            .claims
            .iter()
            .filter(|claim| claim.state == WorkflowClaimState::Claimed)
            .map(|claim| claim.claim_id.clone())
            .collect::<Vec<_>>();
        let mut results = Vec::new();
        for claim_id in claim_ids {
            let execution_id = self.start_claim(&claim_id)?;
            match self.execute_started_claim(&claim_id, &execution_id) {
                Ok(result) => results.push(result),
                Err(error) => {
                    let _ = self.mark_claim_state(&claim_id, WorkflowClaimState::Failed);
                    return Err(error);
                }
            }
        }
        Ok(results)
    }

    fn execute_started_claim(
        &self,
        claim_id: &str,
        execution_id: &str,
    ) -> RefineResult<WorkflowStepResult> {
        let claim = self.claim_by_id(claim_id)?;
        let target_root = self.target_root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput(
                "target root is required to execute claimed workflow work".to_string(),
            )
        })?;
        let refine_dir = target_root.join(".refine");
        let work_items = FileWorkItemService::with_projection_cache(
            &refine_dir,
            self.runtime_root.join("cache"),
        );
        let round_idx = ensure_workflow_round(&work_items, &claim.gap_id)?;
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
        self.advance_claim_behaviors(&mut ctx)?;
        let branch = ctx
            .branch
            .clone()
            .ok_or_else(|| missing_workflow_artifact("branch", &ctx.gap_id))?;
        let commit = ctx
            .commit
            .clone()
            .ok_or_else(|| missing_workflow_artifact("commit", &ctx.gap_id))?;
        let merge = ctx
            .merge
            .clone()
            .ok_or_else(|| missing_workflow_artifact("merge", &ctx.gap_id))?;
        let provider_output = ctx
            .provider_output
            .clone()
            .ok_or_else(|| missing_workflow_artifact("provider output", &ctx.gap_id))?;
        let final_status = ctx
            .final_status
            .clone()
            .unwrap_or(GapStatus::Review)
            .as_str()
            .to_string();

        self.mark_claim_state(&ctx.claim_id, WorkflowClaimState::Completed)?;
        Ok(WorkflowStepResult {
            claim_id: ctx.claim_id,
            gap_id: ctx.gap_id,
            execution_id: execution_id.to_string(),
            provider: ctx.provider,
            branch,
            commit,
            merge,
            final_status,
            provider_output,
        })
    }

    fn advance_claim_behaviors(&self, ctx: &mut WorkflowContext<'_>) -> RefineResult<()> {
        let todo = WorkflowTodo;
        let implementation = WorkflowImplementation;
        let ready_merge = WorkflowReadyMerge;
        let build = WorkflowBuild;
        let qa = WorkflowQa;
        let review = WorkflowReview;
        let behaviors: [&dyn WorkflowBehavior; 6] =
            [&todo, &implementation, &ready_merge, &build, &qa, &review];
        let mut current = GapStatus::Todo;
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

    fn interrupt_active_claims(&self, gap_ids: &[String]) -> RefineResult<()> {
        let gap_ids = gap_ids.iter().collect::<BTreeSet<_>>();
        let mut state = self.load_state()?;
        let mut changed = false;
        let now = now_timestamp();
        for claim in &mut state.claims {
            if gap_ids.contains(&claim.gap_id)
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
        let Some(refine_dir) = self.refine_dir() else {
            return Ok(state
                .claims
                .iter()
                .filter(|claim| claim.state == WorkflowClaimState::Claimed)
                .count());
        };
        self.promote_backlog_to_todo_for_refine_dir(&refine_dir)?;
        let snapshot = self.projection_snapshot(&refine_dir)?;
        let mut eligible = snapshot
            .gaps
            .values()
            .filter(|projection| projection.gap.status == GapStatus::Todo)
            .filter(|projection| Self::feature_claim_eligible(&snapshot, projection))
            .filter(|projection| Self::priority_claim_eligible(&snapshot, projection))
            .cloned()
            .collect::<Vec<_>>();
        eligible.sort_by(|a, b| {
            priority_rank(&b.gap.priority)
                .cmp(&priority_rank(&a.gap.priority))
                .then_with(|| compare_feature_gap_order(a.gap.feature_order, b.gap.feature_order))
                .then_with(|| a.gap.created.cmp(&b.gap.created))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });

        let mut promoted = 0;
        for gap in eligible {
            if Self::active_claim(&state, &gap.gap.id).is_some() {
                continue;
            }
            let metadata = match self.claim_metadata(Some(&gap), &policy) {
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
                gap_id: gap.gap.id,
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

    fn claim(&self, gap_id: &str) -> RefineResult<String> {
        let gap_id = gap_id.trim();
        if gap_id.is_empty() {
            return Err(RefineError::InvalidInput("Gap id is required".to_string()));
        }
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        if let Some(existing) = Self::active_claim(&state, gap_id) {
            return Ok(existing.claim_id.clone());
        }
        let gap = if let Some(refine_dir) = self.refine_dir() {
            let snapshot = self.projection_snapshot(&refine_dir)?;
            let gap = snapshot.gaps.get(gap_id).cloned().ok_or_else(|| {
                RefineError::NotFound(format!("Gap {gap_id} was not found in target state"))
            })?;
            if !Self::feature_claim_eligible(&snapshot, &gap) {
                return Err(RefineError::Conflict(format!(
                    "Gap {gap_id} is blocked by Feature order"
                )));
            }
            if !Self::priority_claim_eligible(&snapshot, &gap) {
                return Err(RefineError::Conflict(format!(
                    "Gap {gap_id} is blocked by higher priority work"
                )));
            }
            Some(gap)
        } else {
            None
        };
        let metadata = self.claim_metadata(gap.as_ref(), &policy)?;
        if !Self::capacity_available(
            &state,
            &policy,
            &metadata.node_id,
            &metadata.provider,
            &metadata.target_app_id,
        ) {
            return Err(RefineError::Conflict(
                "automation concurrency limit reached".to_string(),
            ));
        }
        let now = now_timestamp();
        let claim = WorkflowClaim {
            claim_id: new_claim_id(),
            gap_id: gap_id.to_string(),
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
        self.ensure_automation_running(&state)?;
        let Some(claim) = state
            .claims
            .iter_mut()
            .find(|claim| claim.claim_id == claim_id)
        else {
            return Err(RefineError::NotFound(format!(
                "claim {claim_id} was not found"
            )));
        };
        if claim.state != WorkflowClaimState::Claimed {
            return Err(RefineError::Conflict(format!(
                "claim {claim_id} is not claimed"
            )));
        }
        if let Some(refine_dir) = self.refine_dir() {
            let policy = self.policy()?;
            let snapshot = self.projection_snapshot(&refine_dir)?;
            let gap = snapshot.gaps.get(&claim.gap_id).ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Gap {} was not found in target state",
                    claim.gap_id
                ))
            })?;
            self.claim_metadata(Some(gap), &policy)?;
            if !Self::feature_claim_eligible(&snapshot, gap) {
                return Err(RefineError::Conflict(format!(
                    "Gap {} is blocked by Feature order",
                    claim.gap_id
                )));
            }
            if !Self::priority_claim_eligible(&snapshot, gap) {
                return Err(RefineError::Conflict(format!(
                    "Gap {} is blocked by higher priority work",
                    claim.gap_id
                )));
            }
        }
        let execution_id = new_execution_id();
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
                "automation concurrency limit reached".to_string(),
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

fn ensure_workflow_round(work_items: &FileWorkItemService, gap_id: &str) -> RefineResult<usize> {
    let gap = work_items.show_gap_summary(gap_id)?;
    if let Some(idx) = gap.gap.round_count.checked_sub(1) {
        return Ok(idx);
    }
    let gap = work_items.append_gap_round_summary(
        gap_id,
        "Refine",
        "Automated workflow requested",
        "Implement and verify this Gap",
    )?;
    gap.gap
        .round_count
        .checked_sub(1)
        .ok_or_else(|| RefineError::InvalidInput(format!("Gap {gap_id} has no rounds")))
}

fn implementation_branch_name(pattern: &str, gap_id: &str, round_idx: usize) -> String {
    let pattern = pattern.trim();
    let base = if pattern.is_empty() {
        "refine/{gap_id}"
    } else {
        pattern
    };
    let round = (round_idx + 1).to_string();
    let branch = base
        .replace("{gap_id}", gap_id)
        .replace("{gap}", gap_id)
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
    gap_id: &str,
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
        "Post-implementation governance review for Gap {gap_id}, round {}.\n\
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
        .unwrap_or_else(|| !violations.is_empty());
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

fn gap_agent_prompt(gap_id: &str) -> String {
    format!(
        "Run the gap agent for ready Gap {gap_id}. Work on Gap {gap_id}, report deterministic command outcomes, and leave the Gap ready for review."
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

fn priority_rank(priority: &GapPriority) -> u8 {
    match priority {
        GapPriority::Low => 0,
        GapPriority::Medium => 1,
        GapPriority::High => 2,
    }
}

fn new_claim_id() -> String {
    format!("res-{}", Uuid::new_v4())
}

fn new_execution_id() -> String {
    format!("exec-{}", Uuid::new_v4())
}

fn missing_workflow_artifact(name: &str, gap_id: &str) -> RefineError {
    RefineError::Conflict(format!(
        "workflow artifact {name} is missing for Gap {gap_id}"
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
    use crate::tools::product::work_items::{BulkGapSelection, FileWorkItemService};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_automation_promotes_todo_gaps_and_starts_executions() {
        let temp_root = unique_temp_dir("automation");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_gap_summary("Queued", Some("GAP1"))
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
            .unwrap();
        work_items
            .create_gap_summary("Backlog", Some("GAP2"))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 1);
        assert_eq!(automation.promote().unwrap(), 0);
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].gap_id, "GAP1");

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
    fn file_automation_auto_promotes_backlog_gaps_when_configured() {
        let temp_root = unique_temp_dir("automation-backlog-promote");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_gap_summary("Instant Backlog", Some("GAP1"))
            .unwrap();
        work_items
            .create_gap_summary("Never Backlog", Some("GAP2"))
            .unwrap();
        let settings = FileSettingsService::new(&refine_dir);
        settings
            .update(&json!({"backlog_promote_after_seconds": "-1"}))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote().unwrap(), 0);
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Backlog
        );

        settings
            .update(&json!({"backlog_promote_after_seconds": "0"}))
            .unwrap();
        assert_eq!(automation.promote().unwrap(), 2);
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Todo
        );
        assert_eq!(
            work_items.show_gap_summary("GAP2").unwrap().gap.status,
            GapStatus::Todo
        );
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 2);
        let mut claimed_gap_ids = state
            .claims
            .iter()
            .map(|claim| claim.gap_id.as_str())
            .collect::<Vec<_>>();
        claimed_gap_ids.sort_unstable();
        assert_eq!(claimed_gap_ids, vec!["GAP1", "GAP2"]);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_promotes_all_ordered_feature_backlog_gaps() {
        let temp_root = unique_temp_dir("automation-feature-backlog-promote");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_feature_summary("Imported Feature", Some("FEA1"), None, None, None)
            .unwrap();
        for id in ["GAP1", "GAP2", "GAP3"] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items.assign_gap_to_feature("FEA1", id).unwrap();
            work_items.order_gap_in_feature("FEA1", id).unwrap();
        }
        FileSettingsService::new(&refine_dir)
            .update(&json!({"backlog_promote_after_seconds": "0"}))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.promote_backlog_to_todo().unwrap(), 3);
        for id in ["GAP1", "GAP2", "GAP3"] {
            assert_eq!(
                work_items.show_gap_summary(id).unwrap().gap.status,
                GapStatus::Todo
            );
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_uses_global_cap_for_single_node_defaults() {
        let temp_root = unique_temp_dir("automation-global-cap");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&json!({"parallel_run_cap": 3}))
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        for id in ["GAP1", "GAP2", "GAP3", "GAP4"] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .update_gap_metadata_summary(id, None, Some("high"), None, None)
                .unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
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
                .map(|claim| claim.gap_id.as_str())
                .collect::<Vec<_>>(),
            vec!["GAP1", "GAP2", "GAP3"]
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_blocks_lower_priority_work_behind_higher_priority_gaps() {
        let temp_root = unique_temp_dir("automation-priority-band");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&json!({"parallel_run_cap": 3}))
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        for (id, priority) in [("LOW", "low"), ("MEDIUM", "medium"), ("HIGH", "high")] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .update_gap_metadata_summary(id, None, Some(priority), None, None)
                .unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
                .unwrap();
        }

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert!(automation.claim("MEDIUM").is_err());
        assert!(automation.claim("LOW").is_err());
        assert_eq!(automation.promote().unwrap(), 1);
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].gap_id, "HIGH");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_applies_runtime_settings_without_waiting_for_automation() {
        let temp_root = unique_temp_dir("automation-apply-runtime-settings");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_gap_summary("Instant Backlog", Some("GAP1"))
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
        assert_eq!(state.claims[0].gap_id, "GAP1");
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Todo
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_runtime_settings_skip_off_node_backlog_promotions() {
        let temp_root = unique_temp_dir("automation-runtime-settings-off-node");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&json!({"backlog_promote_after_seconds": "0"}))
            .unwrap();
        FileNodeRegistryService::new(&refine_dir)
            .create("remote-node")
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_gap_summary("Local backlog", Some("LOCAL"))
            .unwrap();
        work_items
            .create_gap_summary("Remote backlog", Some("REMOTE"))
            .unwrap();
        work_items
            .bulk_transfer_gaps_to_node(
                "remote-node",
                BulkGapSelection {
                    selected_ids: Some(vec!["REMOTE".to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(automation.apply_runtime_settings().unwrap(), 1);
        assert_eq!(
            work_items.show_gap_summary("LOCAL").unwrap().gap.status,
            GapStatus::Todo
        );
        assert_eq!(
            work_items.show_gap_summary("REMOTE").unwrap().gap.status,
            GapStatus::Backlog
        );
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[0].gap_id, "LOCAL");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_enforces_configured_concurrency_limits() {
        let temp_root = unique_temp_dir("automation-limits");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
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
        for id in ["GAP1", "GAP2", "GAP3"] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
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
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_gap_summary("Local", Some("LOCAL"))
            .unwrap();
        work_items
            .transition_gap_status("LOCAL", GapStatus::Todo)
            .unwrap();
        work_items
            .create_gap_summary("Remote", Some("REMOTE"))
            .unwrap();
        work_items
            .transition_gap_status("REMOTE", GapStatus::Todo)
            .unwrap();
        FileNodeRegistryService::new(&refine_dir)
            .create("remote-node")
            .unwrap();
        work_items
            .bulk_transfer_gaps_to_node(
                "remote-node",
                BulkGapSelection {
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
        let refine_dir = target_root.join(".refine");
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
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
                .unwrap();
            work_items.assign_gap_to_feature("FEAT1", id).unwrap();
        }
        for id in ["FIRST", "SECOND"] {
            work_items.order_gap_in_feature("FEAT1", id).unwrap();
        }

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert!(automation.claim("SECOND").is_err());
        assert_eq!(automation.promote().unwrap(), 2);
        let state = automation.load_state().unwrap();
        let claimed_gap_ids = state
            .claims
            .iter()
            .map(|claim| claim.gap_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(claimed_gap_ids, vec!["FIRST", "UNORDERED"]);

        work_items
            .bulk_update_gaps(
                BulkGapSelection {
                    selected_ids: Some(vec!["FIRST".to_string()]),
                    ..Default::default()
                },
                crate::tools::product::work_items::BulkGapUpdate::Status("review".to_string()),
            )
            .unwrap();
        let claim_automation = WorkflowEngine::with_target_root(&claim_runtime_root, &target_root);
        assert_eq!(claim_automation.promote().unwrap(), 2);
        let state = claim_automation.load_state().unwrap();
        let second_claim = state
            .claims
            .iter()
            .find(|claim| claim.gap_id == "SECOND")
            .map(|claim| claim.claim_id.clone())
            .unwrap();
        work_items
            .bulk_update_gaps(
                BulkGapSelection {
                    selected_ids: Some(vec!["FIRST".to_string()]),
                    ..Default::default()
                },
                crate::tools::product::work_items::BulkGapUpdate::Status("todo".to_string()),
            )
            .unwrap();
        assert!(claim_automation.start_claim(&second_claim).is_err());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_fails_in_progress_gap_on_post_implementation_governance_violation() {
        let temp_root = unique_temp_dir("automation-governance");
        let target_root = temp_root.clone();
        let refine_dir = target_root.join(".refine");
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
               printf '%s\\n' 'smoke-ai gap-agent response'\n\
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
            .create_gap_summary("Governed implementation", Some("GAP1"))
            .unwrap();
        work_items
            .append_gap_round_summary("GAP1", "Reporter", "Actual", "Target")
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
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
        let gap = work_items.show_gap_detail("GAP1").unwrap();
        assert_eq!(gap["status"], "failed");
        let latest = &gap["rounds"][0];
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
        let refine_dir = target_root.join(".refine");
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
            .create_gap_summary("Precommitted implementation", Some("GAP1"))
            .unwrap();
        work_items
            .append_gap_round_summary("GAP1", "Reporter", "Actual", "Target")
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
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
        assert_eq!(result.steps[0].commit.len(), 40);
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Review
        );
        assert_eq!(
            fs::read_to_string(target_root.join("agent.txt")).unwrap(),
            "agent precommitted implementation\n"
        );
        assert_eq!(
            git_stdout(&target_root, &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            result.steps[0].commit
        );
        assert_eq!(
            git_stdout(&target_root, &["log", "--pretty=%s", "-1"])
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

        fs::remove_dir_all(temp_root.parent().unwrap().join(format!(
            "{}-refine-GAP1-round-1",
            temp_root.file_name().unwrap().to_string_lossy()
        )))
        .ok();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_treats_clean_noop_implementation_as_reviewable() {
        let temp_root = unique_temp_dir("automation-agent-noop");
        let target_root = temp_root.clone();
        let refine_dir = target_root.join(".refine");
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
            .create_gap_summary("No-op implementation", Some("GAP1"))
            .unwrap();
        work_items
            .append_gap_round_summary("GAP1", "Reporter", "Actual", "Target")
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
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
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Review
        );
        assert_eq!(
            git_stdout(&target_root, &["rev-parse", "HEAD"])
                .unwrap()
                .trim(),
            initial_head.trim()
        );
        let gap = work_items.show_gap_detail("GAP1").unwrap();
        let round_logs = gap["rounds"][0]["logs"].as_array().unwrap();
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

        fs::remove_dir_all(temp_root.parent().unwrap().join(format!(
            "{}-refine-GAP1-round-1",
            temp_root.file_name().unwrap().to_string_lossy()
        )))
        .ok();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_automation_reuses_existing_round_worktree_on_retry() {
        let temp_root = unique_temp_dir("automation-existing-worktree-retry");
        let target_root = temp_root.clone();
        let refine_dir = target_root.join(".refine");
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

        let branch = "refine/GAP1/round-1";
        let worktree_path = temp_root.parent().unwrap().join(format!(
            "{}-{}",
            temp_root.file_name().unwrap().to_string_lossy(),
            branch.replace('/', "-")
        ));
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
            .create_gap_summary("Retry existing worktree", Some("GAP1"))
            .unwrap();
        work_items
            .append_gap_round_summary("GAP1", "Reporter", "Actual", "Target")
            .unwrap();
        work_items
            .update_gap_branch_name("GAP1", Some(branch))
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
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
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Review
        );
        assert_eq!(
            fs::read_to_string(target_root.join("agent.txt")).unwrap(),
            "existing retry implementation\n"
        );

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
    fn file_automation_fails_gap_reverts_merge_and_recreates_worktree_on_qa_failure() {
        let temp_root = unique_temp_dir("automation-qa-revert");
        let target_root = temp_root.clone();
        let refine_dir = target_root.join(".refine");
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
             printf '%s\\n' 'smoke-ai gap-agent response'\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let branch = "refine/GAP1/round-1";
        let worktree_path = temp_root.parent().unwrap().join(format!(
            "{}-{}",
            temp_root.file_name().unwrap().to_string_lossy(),
            branch.replace('/', "-")
        ));
        let build_command = format!("rm -rf {} && printf build-ok", shell_word(&worktree_path));

        let _smoke_ai_env_guard = smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_gap_summary("Implementation with failing QA", Some("GAP1"))
            .unwrap();
        work_items
            .append_gap_round_summary("GAP1", "Reporter", "Actual", "Target")
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
            .unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "agent_cli": "smoke-ai",
                "quality_enabled": "1",
                "target_app_build_command": build_command,
                "target_app_test_command": "test ! -f fail-qa",
                "allowed_commands": "rm, printf, test"
            }))
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let error = automation.evaluate_workflow().unwrap_err();
        assert!(error.to_string().contains("quality checks failed"));
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Failed
        );
        assert!(!target_root.join("fail-qa").exists());
        assert!(worktree_path.exists());
        let subject = git_stdout(&target_root, &["log", "--pretty=%s", "-1"]).unwrap();
        assert!(subject.trim().starts_with("Revert "));
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
        assert!(automation.claim("GAP1").is_err());
        automation
            .resume(WorkflowPauseControl::AllAutomation)
            .unwrap();
        FileProcessSupervisor::new(temp_root.join("run/8080"))
            .set_agents_paused(true)
            .unwrap();
        assert!(automation.claim("GAP1").is_err());
        FileProcessSupervisor::new(temp_root.join("run/8080"))
            .set_agents_paused(false)
            .unwrap();

        let claim_id = automation.claim("GAP1").unwrap();
        assert_eq!(automation.claim("GAP1").unwrap(), claim_id);
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
    fn file_automation_pause_moves_in_progress_gaps_back_to_todo() {
        let temp_root = unique_temp_dir("automation-pause-rollback");
        let target_root = temp_root.join("target");
        let refine_dir = target_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_gap_summary("Running work", Some("GAP1"))
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
            .unwrap();

        let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        let claim_id = automation.claim("GAP1").unwrap();
        automation.start_claim(&claim_id).unwrap();
        work_items
            .advance_automated_gap_status("GAP1", GapStatus::InProgress)
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
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Todo
        );
        let state = automation.load_state().unwrap();
        assert_eq!(state.claims[0].state, WorkflowClaimState::Interrupted);

        fs::remove_dir_all(temp_root).unwrap();
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

    fn shell_word(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }
}
