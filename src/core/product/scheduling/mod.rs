use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::core::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService, MergeResult};
use crate::core::host::process_supervision::{FileProcessSupervisor, ProcessPauseState};
use crate::core::host::quality::{
    FileQualityService, QualityCheckRequest, QualityCheckResult, QualityService,
};
use crate::core::observability::logs::FileLogService;
use crate::core::product::merging::{FileMergerService, MergerGapResult};
use crate::core::product::nodes::FileNodeRegistryService;
use crate::core::product::project_state::{
    FileProjectStateStore, GapSummaryProjection, ProjectionSnapshot,
};
use crate::core::product::work_items::FileWorkItemService;
use crate::core::supervisor::config::{ConfigService, FileGovernanceService, FileSettingsService};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::jobs::{FileJobRegistry, JobRegistry, JobState};
use crate::model::JsonObject;
use crate::model::log::LogEntry;
use crate::model::workflow::GapStatus;

pub const SCHEDULER_STATE_FILE: &str = "scheduler-state.json";

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerControl {
    Agents,
    TargetApp,
    Job(String),
    AllAutomation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationState {
    Reserved,
    Dispatched,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduleReservation {
    pub reservation_id: String,
    pub gap_id: String,
    #[serde(default = "default_node_id")]
    pub node_id: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_target_app_id")]
    pub target_app_id: String,
    pub job_id: Option<String>,
    pub state: ReservationState,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchedulerPolicy {
    pub global_limit: usize,
    pub per_node_limit: usize,
    pub per_provider_limit: usize,
    pub per_target_app_limit: usize,
    pub active_node_id: String,
    pub provider: String,
    pub target_app_id: String,
}

impl Default for SchedulerPolicy {
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
pub struct SchedulerState {
    pub paused: BTreeSet<SchedulerControl>,
    #[serde(default)]
    pub policy: SchedulerPolicy,
    pub reservations: Vec<ScheduleReservation>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowScheduleRun {
    pub promoted: usize,
    pub reservations: Vec<ScheduleReservation>,
    pub dispatched: Vec<ScheduleDispatchResult>,
    pub merged: Option<MergerGapResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduleDispatchResult {
    pub reservation_id: String,
    pub gap_id: String,
    pub job_id: String,
    pub provider: String,
    pub branch: String,
    pub commit: String,
    pub merge: MergeResult,
    pub final_status: String,
    pub provider_output: String,
}

pub trait SchedulingService {
    fn promote(&self) -> RefineResult<usize>;
    fn reserve(&self, gap_id: &str) -> RefineResult<String>;
    fn dispatch(&self, reservation_id: &str) -> RefineResult<String>;
    fn pause(&self, control: SchedulerControl) -> RefineResult<()>;
    fn resume(&self, control: SchedulerControl) -> RefineResult<()>;
    fn cancel(&self, job_id: &str) -> RefineResult<()>;
    fn retry(&self, job_id: &str) -> RefineResult<String>;
}

#[derive(Clone, Debug)]
pub struct FileSchedulingService {
    pub runtime_root: PathBuf,
    pub durable_root: Option<PathBuf>,
    pub job_registry: FileJobRegistry,
}

impl FileSchedulingService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        let runtime_root = runtime_root.into();
        Self {
            job_registry: FileJobRegistry::new(&runtime_root),
            runtime_root,
            durable_root: None,
        }
    }

    pub fn with_durable_root(
        runtime_root: impl Into<PathBuf>,
        durable_root: impl Into<PathBuf>,
    ) -> Self {
        let runtime_root = runtime_root.into();
        Self {
            job_registry: FileJobRegistry::new(&runtime_root),
            runtime_root,
            durable_root: Some(durable_root.into()),
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.runtime_root.join(SCHEDULER_STATE_FILE)
    }

    pub fn load_state(&self) -> RefineResult<SchedulerState> {
        read_state(&self.state_path())
    }

    fn save_state(&self, state: &mut SchedulerState) -> RefineResult<()> {
        state.policy = self.policy()?;
        state.updated_at = Some(now_timestamp());
        write_state(&self.state_path(), state)
    }

    pub fn policy(&self) -> RefineResult<SchedulerPolicy> {
        let mut policy = SchedulerPolicy::default();
        if let Some(durable_root) = &self.durable_root {
            let settings = FileSettingsService::new(durable_root).load()?;
            policy.global_limit = setting_usize(&settings, "parallel_run_cap", policy.global_limit);
            policy.per_node_limit =
                setting_usize(&settings, "parallel_per_node_cap", policy.per_node_limit);
            policy.per_provider_limit = setting_usize(
                &settings,
                "parallel_per_provider_cap",
                policy.per_provider_limit,
            );
            policy.per_target_app_limit = setting_usize(
                &settings,
                "parallel_per_target_app_cap",
                policy.per_target_app_limit,
            );
            policy.provider = setting_string(&settings, "agent_cli", &policy.provider);
            policy.target_app_id = durable_root
                .parent()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| policy.target_app_id.clone());
            policy.active_node_id = FileNodeRegistryService::new(durable_root).active_node_id()?;
        }
        Ok(policy)
    }

    pub fn apply_runtime_settings(&self) -> RefineResult<usize> {
        let mut state = self.load_state()?;
        state.policy = self.policy()?;
        let promoted = match &self.durable_root {
            Some(durable_root) => match self.ensure_automation_running(&state) {
                Ok(()) => self.promote_backlog_to_todo(durable_root)?,
                Err(RefineError::Conflict(_)) => 0,
                Err(error) => return Err(error),
            },
            None => 0,
        };
        self.save_state(&mut state)?;
        Ok(promoted)
    }

    pub fn set_agent_workflow_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        let state = if paused {
            supervisor.set_agents_paused(true)?;
            let state = supervisor.set_background_processes_stopped(true)?;
            self.pause(SchedulerControl::Agents)?;
            state
        } else {
            supervisor.set_background_processes_stopped(false)?;
            let state = supervisor.set_agents_paused(false)?;
            self.resume(SchedulerControl::Agents)?;
            state
        };
        Ok(state)
    }

    fn ensure_automation_running(&self, state: &SchedulerState) -> RefineResult<()> {
        if state.paused.contains(&SchedulerControl::AllAutomation)
            || state.paused.contains(&SchedulerControl::Agents)
        {
            return Err(RefineError::Conflict(
                "automation is paused for agents".to_string(),
            ));
        }
        let pause_state = FileProcessSupervisor::new(&self.runtime_root).pause_state()?;
        if pause_state.background_processes_stopped || pause_state.agents_paused {
            return Err(RefineError::Conflict(
                "automation is paused for agents".to_string(),
            ));
        }
        Ok(())
    }

    fn active_reservation<'a>(
        state: &'a SchedulerState,
        gap_id: &str,
    ) -> Option<&'a ScheduleReservation> {
        state.reservations.iter().find(|reservation| {
            reservation.gap_id == gap_id
                && matches!(
                    reservation.state,
                    ReservationState::Reserved | ReservationState::Dispatched
                )
        })
    }

    fn reservation_load(state: &SchedulerState, policy: &SchedulerPolicy) -> ReservationLoad {
        Self::reservation_load_excluding(state, policy, None)
    }

    fn reservation_load_excluding(
        state: &SchedulerState,
        policy: &SchedulerPolicy,
        excluded_index: Option<usize>,
    ) -> ReservationLoad {
        let mut load = ReservationLoad::default();
        for reservation in state
            .reservations
            .iter()
            .enumerate()
            .filter(|(index, reservation)| {
                Some(*index) != excluded_index
                    && matches!(
                        reservation.state,
                        ReservationState::Reserved | ReservationState::Dispatched
                    )
            })
            .map(|(_, reservation)| reservation)
        {
            load.global += 1;
            *load.by_node.entry(reservation.node_id.clone()).or_default() += 1;
            *load
                .by_provider
                .entry(reservation.provider.clone())
                .or_default() += 1;
            *load
                .by_target_app
                .entry(reservation.target_app_id.clone())
                .or_default() += 1;
        }
        load.ensure_policy_keys(policy);
        load
    }

    fn capacity_available(
        state: &SchedulerState,
        policy: &SchedulerPolicy,
        node_id: &str,
        provider: &str,
        target_app_id: &str,
    ) -> bool {
        let load = Self::reservation_load(state, policy);
        load.global < policy.global_limit
            && load.by_node.get(node_id).copied().unwrap_or(0) < policy.per_node_limit
            && load.by_provider.get(provider).copied().unwrap_or(0) < policy.per_provider_limit
            && load.by_target_app.get(target_app_id).copied().unwrap_or(0)
                < policy.per_target_app_limit
    }

    fn capacity_available_excluding(
        state: &SchedulerState,
        policy: &SchedulerPolicy,
        node_id: &str,
        provider: &str,
        target_app_id: &str,
        excluded_index: usize,
    ) -> bool {
        let load = Self::reservation_load_excluding(state, policy, Some(excluded_index));
        load.global < policy.global_limit
            && load.by_node.get(node_id).copied().unwrap_or(0) < policy.per_node_limit
            && load.by_provider.get(provider).copied().unwrap_or(0) < policy.per_provider_limit
            && load.by_target_app.get(target_app_id).copied().unwrap_or(0)
                < policy.per_target_app_limit
    }

    fn reservation_metadata(
        &self,
        gap: Option<&GapSummaryProjection>,
        policy: &SchedulerPolicy,
    ) -> RefineResult<ReservationMetadata> {
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
        Ok(ReservationMetadata {
            node_id,
            provider: policy.provider.clone(),
            target_app_id: policy.target_app_id.clone(),
        })
    }

    fn projection_snapshot(&self, durable_root: &Path) -> RefineResult<ProjectionSnapshot> {
        FileProjectStateStore::new(durable_root)
            .load_or_refresh_projection(&self.runtime_root.join("cache"))
    }

    fn feature_dispatch_eligible(
        snapshot: &ProjectionSnapshot,
        gap: &GapSummaryProjection,
    ) -> bool {
        let Some(feature_id) = gap.gap.feature_id.as_deref() else {
            return true;
        };
        let Some(feature_order) = gap.gap.feature_order else {
            return false;
        };
        let node_id = gap.gap.node_id.as_deref().unwrap_or("default");
        !snapshot.gaps.values().any(|other| {
            other.gap.feature_id.as_deref() == Some(feature_id)
                && other.gap.node_id.as_deref().unwrap_or("default") == node_id
                && other
                    .gap
                    .feature_order
                    .is_some_and(|order| order < feature_order)
                && !matches!(other.gap.status, GapStatus::Done | GapStatus::Cancelled)
        }) && !snapshot.gaps.values().any(|other| {
            other.gap.id != gap.gap.id
                && other.gap.feature_id.as_deref() == Some(feature_id)
                && other.gap.node_id.as_deref().unwrap_or("default") == node_id
                && matches!(other.gap.status, GapStatus::InProgress | GapStatus::Qa)
        })
    }

    fn promote_backlog_to_todo(&self, durable_root: &Path) -> RefineResult<usize> {
        let settings = FileSettingsService::new(durable_root).load()?;
        let threshold = setting_i64(&settings, "backlog_promote_after_seconds", 3600);
        if threshold < 0 {
            return Ok(0);
        }
        let snapshot = self.projection_snapshot(durable_root)?;
        let service = FileWorkItemService::new(durable_root);
        let now = Utc::now();
        let mut promoted = 0;
        let mut candidates = snapshot
            .gaps
            .values()
            .filter(|projection| projection.gap.status == GapStatus::Backlog)
            .filter(|projection| Self::feature_dispatch_eligible(&snapshot, projection))
            .filter(|projection| backlog_gap_age_seconds(projection, now) >= Some(threshold))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| {
            a.gap
                .feature_order
                .unwrap_or(i64::MAX)
                .cmp(&b.gap.feature_order.unwrap_or(i64::MAX))
                .then_with(|| a.gap.updated.cmp(&b.gap.updated))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });
        for gap in candidates {
            service.transition_gap_status(&gap.gap.id, GapStatus::Todo)?;
            promoted += 1;
        }
        Ok(promoted)
    }

    pub fn schedule_and_dispatch(&self) -> RefineResult<WorkflowScheduleRun> {
        let promoted = self.promote()?;
        let dispatched = self.dispatch_reserved()?;
        let merged = match &self.durable_root {
            Some(durable_root) => {
                FileMergerService::new(&self.runtime_root, durable_root)
                    .tick()?
                    .processed
            }
            None => None,
        };
        let state = self.load_state()?;
        Ok(WorkflowScheduleRun {
            promoted,
            reservations: state.reservations,
            dispatched,
            merged,
        })
    }

    pub fn dispatch_reserved(&self) -> RefineResult<Vec<ScheduleDispatchResult>> {
        let state = self.load_state()?;
        self.ensure_automation_running(&state)?;
        let reservation_ids = state
            .reservations
            .iter()
            .filter(|reservation| reservation.state == ReservationState::Reserved)
            .map(|reservation| reservation.reservation_id.clone())
            .collect::<Vec<_>>();
        let mut results = Vec::new();
        for reservation_id in reservation_ids {
            let job_id = self.dispatch(&reservation_id)?;
            match self.execute_dispatched_reservation(&reservation_id, &job_id) {
                Ok(result) => results.push(result),
                Err(error) => {
                    let _ = self.mark_reservation_state(&reservation_id, ReservationState::Failed);
                    let _ = self.job_registry.fail_with_error(
                        &job_id,
                        json!({
                            "reservation_id": reservation_id,
                            "error": error.to_string()
                        }),
                    );
                    return Err(error);
                }
            }
        }
        Ok(results)
    }

    fn execute_dispatched_reservation(
        &self,
        reservation_id: &str,
        job_id: &str,
    ) -> RefineResult<ScheduleDispatchResult> {
        let reservation = self.reservation(reservation_id)?;
        let durable_root = self.durable_root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput(
                "durable root is required to dispatch scheduled work".to_string(),
            )
        })?;
        let work_items = FileWorkItemService::with_projection_cache(
            durable_root,
            self.runtime_root.join("cache"),
        );
        let round_idx = ensure_dispatch_round(&work_items, &reservation.gap_id)?;
        let settings = FileSettingsService::new(durable_root).load()?;
        let app_root = durable_root.parent().ok_or_else(|| {
            RefineError::InvalidInput(
                "durable root must be inside an attached target app".to_string(),
            )
        })?;
        let branch = implementation_branch_name(
            setting_string(&settings, "branch_name_pattern", "refine/{gap_id}").as_str(),
            &reservation.gap_id,
            round_idx,
        );
        let app_git = FileGitWorktreeService::with_runtime_root(app_root, &self.runtime_root);
        let workflow = WorkflowExecution {
            job_id,
            gap_id: &reservation.gap_id,
            round_idx,
            work_items: &work_items,
            durable_root,
        };

        work_items.advance_automated_gap_status(&reservation.gap_id, GapStatus::InProgress)?;
        workflow.log(
            self,
            "state",
            "Workflow status changed: todo -> in-progress",
            None,
        )?;

        let worktree_path = match app_git.worktree(&branch) {
            Ok(path) => path,
            Err(error) => {
                self.fail_workflow(&workflow, "branch", &error)?;
                return Err(error);
            }
        };
        workflow.log(
            self,
            "git",
            &format!("Created implementation worktree for {branch}"),
            Some(json_object(json!({
                "branch": branch,
                "worktree": worktree_path
            }))),
        )?;
        if let Err(error) = work_items.update_gap_branch_name(&reservation.gap_id, Some(&branch)) {
            self.fail_workflow(&workflow, "branch", &error)?;
            return Err(error);
        }

        let prompt = gap_agent_prompt(&reservation.gap_id);
        let agent_cwd = agent_worktree_cwd(
            &worktree_path,
            setting_string(&settings, "agent_subpath", "").as_str(),
        )?;
        let provider =
            HostAgentProviderService::with_runtime_root(self.runtime_root.join("agents"));
        let provider_output = match provider.invoke(ProviderInvocation {
            provider: reservation.provider.clone(),
            prompt,
            session_id: None,
            cwd: Some(agent_cwd.display().to_string()),
        }) {
            Ok(output) => output,
            Err(error) => {
                self.fail_workflow(&workflow, "agent", &error)?;
                return Err(error);
            }
        };
        workflow.log(
            self,
            "agent",
            "Gap agent completed",
            Some(json_object(json!({
                "provider": reservation.provider,
                "output": provider_output,
                "branch": branch,
                "worktree": worktree_path
            }))),
        )?;

        let worktree_git =
            FileGitWorktreeService::with_runtime_root(&worktree_path, &self.runtime_root);
        let commit = match worktree_git.commit(
            &format!("Implement {} round {}", reservation.gap_id, round_idx + 1),
            &[],
        ) {
            Ok(commit) => commit,
            Err(error) => {
                self.fail_workflow(&workflow, "commit", &error)?;
                return Err(error);
            }
        };
        workflow.log(
            self,
            "git",
            &format!("Committed implementation branch {branch}"),
            Some(json_object(json!({
                "branch": branch,
                "commit": commit,
                "worktree": worktree_path
            }))),
        )?;

        if let Err(error) = workflow.advance(self, GapStatus::Qa, "in-progress") {
            self.fail_workflow(&workflow, "state", &error)?;
            return Err(error);
        }
        let quality = match self.run_workflow_quality(durable_root, &settings, &reservation.gap_id)
        {
            Ok(result) => result,
            Err(error) => {
                self.record_quality_error(&workflow, &error)?;
                self.fail_workflow(&workflow, "quality", &error)?;
                return Err(error);
            }
        };
        self.record_quality(&workflow, &quality)?;
        if !quality.ok {
            let error = RefineError::Conflict("quality checks failed".to_string());
            self.fail_workflow(&workflow, "quality", &error)?;
            return Err(error);
        }

        let governance = self.evaluate_workflow_governance(durable_root, &settings)?;
        self.record_governance(&workflow, &governance)?;
        if governance.failed {
            let error = RefineError::Conflict(
                governance
                    .message
                    .clone()
                    .unwrap_or_else(|| "governance checks failed".to_string()),
            );
            self.fail_workflow(&workflow, "governance", &error)?;
            return Err(error);
        }

        if let Err(error) = workflow.advance(self, GapStatus::ReadyMerge, "qa") {
            self.fail_workflow(&workflow, "state", &error)?;
            return Err(error);
        }
        let merge = match merge_with_transient_lock_retry(&app_git, &branch) {
            Ok(result) if result.ok => result,
            Ok(result) => {
                let error = RefineError::Conflict(
                    result
                        .message
                        .clone()
                        .unwrap_or_else(|| "implementation merge failed".to_string()),
                );
                workflow.log(
                    self,
                    "merge",
                    "Implementation merge failed",
                    Some(json_object(json!({"branch": branch, "merge": &result}))),
                )?;
                self.fail_workflow(&workflow, "merge", &error)?;
                return Err(error);
            }
            Err(error) => {
                self.fail_workflow(&workflow, "merge", &error)?;
                return Err(error);
            }
        };
        workflow.log(
            self,
            "merge",
            &format!("Merged implementation branch {branch}"),
            Some(json_object(json!({
                "branch": branch,
                "commit": commit,
                "merge": &merge
            }))),
        )?;

        for (from, to) in [
            ("ready-merge", GapStatus::AwaitingRebuild),
            ("awaiting-rebuild", GapStatus::Review),
        ] {
            if let Err(error) = workflow.advance(self, to, from) {
                self.fail_workflow(&workflow, "state", &error)?;
                return Err(error);
            }
        }

        self.job_registry.finish_with_result(
            job_id,
            JobState::Succeeded,
            json!({
                "gap_id": reservation.gap_id,
                "provider": reservation.provider,
                "branch": branch,
                "commit": commit,
                "merge": merge,
                "quality": quality,
                "governance": governance.details,
                "provider_output": provider_output,
                "final_status": "review"
            }),
        )?;
        self.mark_reservation_state(reservation_id, ReservationState::Completed)?;
        Ok(ScheduleDispatchResult {
            reservation_id: reservation_id.to_string(),
            gap_id: reservation.gap_id,
            job_id: job_id.to_string(),
            provider: reservation.provider,
            branch,
            commit,
            merge,
            final_status: "review".to_string(),
            provider_output,
        })
    }

    fn fail_workflow(
        &self,
        workflow: &WorkflowExecution<'_>,
        category: &str,
        error: &RefineError,
    ) -> RefineResult<()> {
        let _ = workflow
            .work_items
            .advance_automated_gap_status(workflow.gap_id, GapStatus::Failed);
        workflow.log(
            self,
            category,
            &format!("Workflow failed: {error}"),
            Some(json_object(json!({"error": error.to_string()}))),
        )
    }

    fn run_workflow_quality(
        &self,
        durable_root: &Path,
        settings: &JsonObject,
        gap_id: &str,
    ) -> RefineResult<QualityCheckResult> {
        if setting_string(settings, "quality_enabled", "0") != "1" {
            return Ok(QualityCheckResult {
                owner_id: gap_id.to_string(),
                ok: true,
                diagnostics: vec!["Quality checks disabled.".to_string()],
            });
        }
        let service = FileQualityService::with_runtime_root(durable_root, &self.runtime_root);
        let browser_required = setting_string(settings, "quality_regressions_enabled", "0") == "1";
        service.run_checks(QualityCheckRequest {
            owner_id: gap_id.to_string(),
            command: String::new(),
            browser_required,
        })
    }

    fn record_quality(
        &self,
        workflow: &WorkflowExecution<'_>,
        result: &QualityCheckResult,
    ) -> RefineResult<()> {
        let message = if result.ok {
            "Quality checks passed"
        } else {
            "Quality checks failed"
        };
        workflow
            .work_items
            .update_latest_gap_round_evaluation_summary(
                workflow.gap_id,
                &json!({
                    "quality_state": if result.ok { "passed" } else { "failed" },
                    "quality_message": message,
                    "quality_details": {"diagnostics": result.diagnostics},
                    "quality_checked_at": now_timestamp()
                }),
            )?;
        workflow.log(
            self,
            "quality",
            message,
            Some(json_object(json!({
                "ok": result.ok,
                "diagnostics": result.diagnostics
            }))),
        )
    }

    fn record_quality_error(
        &self,
        workflow: &WorkflowExecution<'_>,
        error: &RefineError,
    ) -> RefineResult<()> {
        workflow
            .work_items
            .update_latest_gap_round_evaluation_summary(
                workflow.gap_id,
                &json!({
                    "quality_state": "failed",
                    "quality_message": "Quality checks failed.",
                    "quality_details": {"error": error.to_string()},
                    "quality_checked_at": now_timestamp()
                }),
            )?;
        Ok(())
    }

    fn evaluate_workflow_governance(
        &self,
        durable_root: &Path,
        _settings: &JsonObject,
    ) -> RefineResult<GovernanceEvaluation> {
        let governance = FileGovernanceService::new(durable_root).load()?;
        let rules = governance
            .get("rules")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let failed_actions = rules
            .iter()
            .filter_map(|rule| {
                let action = rule
                    .get("action")
                    .and_then(|value| value.as_str())
                    .unwrap_or("allow");
                matches!(action, "block" | "fail" | "failed").then(|| rule.clone())
            })
            .collect::<Vec<_>>();
        let failed = !failed_actions.is_empty();
        Ok(GovernanceEvaluation {
            failed,
            message: failed.then(|| "Governance rules blocked implementation.".to_string()),
            details: json_object(json!({
                "configured": !rules.is_empty(),
                "rules_checked": rules.len(),
                "failed_actions": failed_actions
            })),
        })
    }

    fn record_governance(
        &self,
        workflow: &WorkflowExecution<'_>,
        evaluation: &GovernanceEvaluation,
    ) -> RefineResult<()> {
        let message = evaluation.message.clone().unwrap_or_else(|| {
            if evaluation.details["configured"].as_bool() == Some(true) {
                "Governance checks passed.".to_string()
            } else {
                "No governance rules configured.".to_string()
            }
        });
        workflow
            .work_items
            .update_latest_gap_round_evaluation_summary(
                workflow.gap_id,
                &json!({
                    "rule_state": if evaluation.failed { "failed" } else { "passed" },
                    "meta_rule_state": "passed",
                    "product_state": "passed",
                    "constitution_state": "passed",
                    "governance_message": message,
                    "governance_details": evaluation.details,
                    "governance_checked_at": now_timestamp(),
                    "governance_rule_actions": evaluation.details
                        .get("failed_actions")
                        .cloned()
                        .unwrap_or_else(|| json!([]))
                }),
            )?;
        workflow.log(
            self,
            "governance",
            if evaluation.failed {
                "Governance checks failed"
            } else {
                "Governance checks passed"
            },
            Some(evaluation.details.clone()),
        )
    }

    fn reservation(&self, reservation_id: &str) -> RefineResult<ScheduleReservation> {
        self.load_state()?
            .reservations
            .into_iter()
            .find(|reservation| reservation.reservation_id == reservation_id)
            .ok_or_else(|| {
                RefineError::NotFound(format!("reservation {reservation_id} was not found"))
            })
    }

    fn mark_reservation_state(
        &self,
        reservation_id: &str,
        reservation_state: ReservationState,
    ) -> RefineResult<()> {
        let mut state = self.load_state()?;
        let Some(reservation) = state
            .reservations
            .iter_mut()
            .find(|reservation| reservation.reservation_id == reservation_id)
        else {
            return Err(RefineError::NotFound(format!(
                "reservation {reservation_id} was not found"
            )));
        };
        reservation.state = reservation_state;
        reservation.updated_at = now_timestamp();
        self.save_state(&mut state)
    }

    fn append_job_log(
        &self,
        job_id: &str,
        gap_id: &str,
        category: &str,
        message: &str,
        details: Option<JsonObject>,
    ) -> RefineResult<()> {
        self.job_registry.append_log(
            job_id,
            LogEntry {
                datetime: now_timestamp(),
                severity: "info".to_string(),
                category: category.to_string(),
                message: message.to_string(),
                details,
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                gap_id: Some(gap_id.to_string()),
            },
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ReservationLoad {
    global: usize,
    by_node: BTreeMap<String, usize>,
    by_provider: BTreeMap<String, usize>,
    by_target_app: BTreeMap<String, usize>,
}

impl ReservationLoad {
    fn ensure_policy_keys(&mut self, policy: &SchedulerPolicy) {
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
struct ReservationMetadata {
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

struct WorkflowExecution<'a> {
    job_id: &'a str,
    gap_id: &'a str,
    round_idx: usize,
    work_items: &'a FileWorkItemService,
    durable_root: &'a Path,
}

impl WorkflowExecution<'_> {
    fn advance(
        &self,
        scheduler: &FileSchedulingService,
        to: GapStatus,
        from: &str,
    ) -> RefineResult<()> {
        self.work_items
            .advance_automated_gap_status(self.gap_id, to.clone())?;
        self.log(
            scheduler,
            "state",
            &format!("Workflow status changed: {from} -> {}", to.as_str()),
            None,
        )
    }

    fn log(
        &self,
        scheduler: &FileSchedulingService,
        category: &str,
        message: &str,
        details: Option<JsonObject>,
    ) -> RefineResult<()> {
        scheduler.append_job_log(self.job_id, self.gap_id, category, message, details.clone())?;
        FileLogService::new(self.durable_root).append_round_log(
            self.gap_id,
            self.round_idx,
            LogEntry {
                datetime: now_timestamp(),
                severity: "info".to_string(),
                category: category.to_string(),
                message: message.to_string(),
                details,
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                gap_id: Some(self.gap_id.to_string()),
            },
        )?;
        Ok(())
    }
}

impl SchedulingService for FileSchedulingService {
    fn promote(&self) -> RefineResult<usize> {
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        let Some(durable_root) = &self.durable_root else {
            return Ok(state
                .reservations
                .iter()
                .filter(|reservation| reservation.state == ReservationState::Reserved)
                .count());
        };
        self.promote_backlog_to_todo(durable_root)?;
        let snapshot = self.projection_snapshot(durable_root)?;
        let mut eligible = snapshot
            .gaps
            .values()
            .filter(|projection| projection.gap.status == GapStatus::Todo)
            .filter(|projection| Self::feature_dispatch_eligible(&snapshot, projection))
            .cloned()
            .collect::<Vec<_>>();
        eligible.sort_by(|a, b| {
            a.gap
                .feature_order
                .unwrap_or(i64::MAX)
                .cmp(&b.gap.feature_order.unwrap_or(i64::MAX))
                .then_with(|| a.gap.created.cmp(&b.gap.created))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });

        let mut promoted = 0;
        for gap in eligible {
            if Self::active_reservation(&state, &gap.gap.id).is_some() {
                continue;
            }
            let metadata = match self.reservation_metadata(Some(&gap), &policy) {
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
            state.reservations.push(ScheduleReservation {
                reservation_id: new_reservation_id(),
                gap_id: gap.gap.id,
                node_id: metadata.node_id,
                provider: metadata.provider,
                target_app_id: metadata.target_app_id,
                job_id: None,
                state: ReservationState::Reserved,
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

    fn reserve(&self, gap_id: &str) -> RefineResult<String> {
        let gap_id = gap_id.trim();
        if gap_id.is_empty() {
            return Err(RefineError::InvalidInput("Gap id is required".to_string()));
        }
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        if let Some(existing) = Self::active_reservation(&state, gap_id) {
            return Ok(existing.reservation_id.clone());
        }
        let gap = if let Some(durable_root) = &self.durable_root {
            let snapshot = self.projection_snapshot(durable_root)?;
            let gap = snapshot.gaps.get(gap_id).cloned().ok_or_else(|| {
                RefineError::NotFound(format!("Gap {gap_id} was not found in durable state"))
            })?;
            if !Self::feature_dispatch_eligible(&snapshot, &gap) {
                return Err(RefineError::Conflict(format!(
                    "Gap {gap_id} is blocked by Feature order"
                )));
            }
            Some(gap)
        } else {
            None
        };
        let metadata = self.reservation_metadata(gap.as_ref(), &policy)?;
        if !Self::capacity_available(
            &state,
            &policy,
            &metadata.node_id,
            &metadata.provider,
            &metadata.target_app_id,
        ) {
            return Err(RefineError::Conflict(
                "scheduler concurrency limit reached".to_string(),
            ));
        }
        let now = now_timestamp();
        let reservation = ScheduleReservation {
            reservation_id: new_reservation_id(),
            gap_id: gap_id.to_string(),
            node_id: metadata.node_id,
            provider: metadata.provider,
            target_app_id: metadata.target_app_id,
            job_id: None,
            state: ReservationState::Reserved,
            created_at: now.clone(),
            updated_at: now,
        };
        let id = reservation.reservation_id.clone();
        state.reservations.push(reservation);
        self.save_state(&mut state)?;
        Ok(id)
    }

    fn dispatch(&self, reservation_id: &str) -> RefineResult<String> {
        let reservation_id = reservation_id.trim();
        let mut state = self.load_state()?;
        self.ensure_automation_running(&state)?;
        let Some(reservation) = state
            .reservations
            .iter_mut()
            .find(|reservation| reservation.reservation_id == reservation_id)
        else {
            return Err(RefineError::NotFound(format!(
                "reservation {reservation_id} was not found"
            )));
        };
        if reservation.state != ReservationState::Reserved {
            return Err(RefineError::Conflict(format!(
                "reservation {reservation_id} is not reserved"
            )));
        }
        if let Some(durable_root) = &self.durable_root {
            let policy = self.policy()?;
            let snapshot = self.projection_snapshot(durable_root)?;
            let gap = snapshot.gaps.get(&reservation.gap_id).ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Gap {} was not found in durable state",
                    reservation.gap_id
                ))
            })?;
            self.reservation_metadata(Some(gap), &policy)?;
            if !Self::feature_dispatch_eligible(&snapshot, gap) {
                return Err(RefineError::Conflict(format!(
                    "Gap {} is blocked by Feature order",
                    reservation.gap_id
                )));
            }
        }
        let job = self
            .job_registry
            .register(&format!("gap:{}", reservation.gap_id))?;
        reservation.job_id = Some(job.id.clone());
        reservation.state = ReservationState::Dispatched;
        reservation.updated_at = now_timestamp();
        self.save_state(&mut state)?;
        Ok(job.id)
    }

    fn pause(&self, control: SchedulerControl) -> RefineResult<()> {
        let mut state = self.load_state()?;
        state.paused.insert(control);
        self.save_state(&mut state)
    }

    fn resume(&self, control: SchedulerControl) -> RefineResult<()> {
        let mut state = self.load_state()?;
        state.paused.remove(&control);
        self.save_state(&mut state)
    }

    fn cancel(&self, job_id: &str) -> RefineResult<()> {
        let job_id = job_id.trim();
        self.job_registry.cancel(job_id)?;
        let mut state = self.load_state()?;
        if let Some(reservation) = state
            .reservations
            .iter_mut()
            .find(|reservation| reservation.job_id.as_deref() == Some(job_id))
        {
            reservation.state = ReservationState::Cancelled;
            reservation.updated_at = now_timestamp();
            self.save_state(&mut state)?;
        }
        Ok(())
    }

    fn retry(&self, job_id: &str) -> RefineResult<String> {
        let job_id = job_id.trim();
        let mut state = self.load_state()?;
        let policy = self.policy()?;
        state.policy = policy.clone();
        self.ensure_automation_running(&state)?;
        let Some(reservation_index) = state
            .reservations
            .iter()
            .position(|reservation| reservation.job_id.as_deref() == Some(job_id))
        else {
            return Err(RefineError::NotFound(format!(
                "reservation for job {job_id} was not found"
            )));
        };
        let gap_id = state.reservations[reservation_index].gap_id.clone();
        let node_id = state.reservations[reservation_index].node_id.clone();
        let provider = state.reservations[reservation_index].provider.clone();
        let target_app_id = state.reservations[reservation_index].target_app_id.clone();
        if !Self::capacity_available_excluding(
            &state,
            &policy,
            &node_id,
            &provider,
            &target_app_id,
            reservation_index,
        ) {
            return Err(RefineError::Conflict(
                "scheduler concurrency limit reached".to_string(),
            ));
        }
        let job = self.job_registry.register(&format!("gap:{gap_id}"))?;
        let mut details = JsonObject::new();
        details.insert("retried_job_id".to_string(), json!(job.id));
        self.job_registry.append_log(
            job_id,
            LogEntry {
                datetime: now_timestamp(),
                severity: "info".to_string(),
                category: "job".to_string(),
                message: format!("Job retried as {}", job.id),
                details: Some(details),
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                gap_id: Some(gap_id),
            },
        )?;
        let reservation = &mut state.reservations[reservation_index];
        reservation.job_id = Some(job.id.clone());
        reservation.state = ReservationState::Dispatched;
        reservation.updated_at = now_timestamp();
        self.save_state(&mut state)?;
        Ok(job.id)
    }
}

fn read_state(path: &Path) -> RefineResult<SchedulerState> {
    if !path.exists() {
        return Ok(SchedulerState::default());
    }
    let bytes = fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read scheduler state {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice::<SchedulerState>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse scheduler state {}: {error}",
            path.display()
        ))
    })
}

fn write_state(path: &Path, state: &SchedulerState) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create scheduler state directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
        RefineError::Serialization(format!("failed to encode scheduler state: {error}"))
    })?;
    fs::write(path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write scheduler state {}: {error}",
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

fn setting_i64(settings: &JsonObject, key: &str, fallback: i64) -> i64 {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback)
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

fn ensure_dispatch_round(work_items: &FileWorkItemService, gap_id: &str) -> RefineResult<usize> {
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

fn merge_with_transient_lock_retry(
    service: &FileGitWorktreeService,
    branch: &str,
) -> RefineResult<MergeResult> {
    let mut result = service.merge(branch)?;
    for _ in 0..5 {
        if result.ok || !merge_message_has_index_lock(&result) {
            return Ok(result);
        }
        thread::sleep(Duration::from_millis(50));
        result = service.merge(branch)?;
    }
    Ok(result)
}

fn merge_message_has_index_lock(result: &MergeResult) -> bool {
    result
        .message
        .as_deref()
        .is_some_and(|message| message.contains("index.lock"))
}

fn gap_agent_prompt(gap_id: &str) -> String {
    format!(
        "Run the gap agent for ready Gap {gap_id}. Work on Gap {gap_id}, report deterministic command outcomes, and leave the Gap ready for review."
    )
}

fn json_object(value: serde_json::Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn backlog_gap_age_seconds(gap: &GapSummaryProjection, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(&gap.gap.updated)
        .ok()
        .map(|timestamp| {
            now.signed_duration_since(timestamp.with_timezone(&Utc))
                .num_seconds()
        })
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

fn new_reservation_id() -> String {
    format!("res-{}", Uuid::new_v4())
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product::nodes::FileNodeRegistryService;
    use crate::core::product::work_items::{BulkGapSelection, FileWorkItemService};
    use crate::core::supervisor::config::FileSettingsService;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_scheduler_promotes_todo_gaps_and_dispatches_jobs() {
        let temp_root = unique_temp_dir("scheduler");
        let durable_root = temp_root.join("durable");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&durable_root);
        work_items
            .create_gap_summary("Queued", Some("GAP1"))
            .unwrap();
        work_items
            .transition_gap_status("GAP1", GapStatus::Todo)
            .unwrap();
        work_items
            .create_gap_summary("Backlog", Some("GAP2"))
            .unwrap();

        let scheduler = FileSchedulingService::with_durable_root(&runtime_root, &durable_root);
        assert_eq!(scheduler.promote().unwrap(), 1);
        assert_eq!(scheduler.promote().unwrap(), 0);
        let state = scheduler.load_state().unwrap();
        assert_eq!(state.reservations.len(), 1);
        assert_eq!(state.reservations[0].gap_id, "GAP1");

        let job_id = scheduler
            .dispatch(&state.reservations[0].reservation_id)
            .unwrap();
        assert_eq!(
            scheduler.job_registry.status(&job_id).unwrap().owner,
            "gap:GAP1"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_scheduler_auto_promotes_backlog_gaps_when_configured() {
        let temp_root = unique_temp_dir("scheduler-backlog-promote");
        let durable_root = temp_root.join("durable");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&durable_root);
        work_items
            .create_gap_summary("Instant Backlog", Some("GAP1"))
            .unwrap();
        work_items
            .create_gap_summary("Never Backlog", Some("GAP2"))
            .unwrap();
        let settings = FileSettingsService::new(&durable_root);
        settings
            .update(&json!({"backlog_promote_after_seconds": "-1"}))
            .unwrap();

        let scheduler = FileSchedulingService::with_durable_root(&runtime_root, &durable_root);
        assert_eq!(scheduler.promote().unwrap(), 0);
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Backlog
        );

        settings
            .update(&json!({"backlog_promote_after_seconds": "0"}))
            .unwrap();
        assert_eq!(scheduler.promote().unwrap(), 1);
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Todo
        );
        assert_eq!(
            work_items.show_gap_summary("GAP2").unwrap().gap.status,
            GapStatus::Todo
        );
        let state = scheduler.load_state().unwrap();
        assert_eq!(state.reservations.len(), 1);
        assert_eq!(state.reservations[0].gap_id, "GAP1");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_scheduler_applies_runtime_settings_without_waiting_for_schedule() {
        let temp_root = unique_temp_dir("scheduler-apply-runtime-settings");
        let durable_root = temp_root.join("durable");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&durable_root);
        work_items
            .create_gap_summary("Instant Backlog", Some("GAP1"))
            .unwrap();
        FileSettingsService::new(&durable_root)
            .update(&json!({
                "parallel_run_cap": 7,
                "parallel_per_node_cap": 7,
                "backlog_promote_after_seconds": "0",
                "agent_cli": "smoke-ai"
            }))
            .unwrap();

        let scheduler = FileSchedulingService::with_durable_root(&runtime_root, &durable_root);
        assert_eq!(scheduler.apply_runtime_settings().unwrap(), 1);
        let state = scheduler.load_state().unwrap();
        assert_eq!(state.policy.global_limit, 7);
        assert_eq!(state.policy.per_node_limit, 7);
        assert_eq!(state.policy.provider, "smoke-ai");
        assert!(state.reservations.is_empty());
        assert_eq!(
            work_items.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Todo
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_scheduler_enforces_configured_concurrency_limits() {
        let temp_root = unique_temp_dir("scheduler-limits");
        let durable_root = temp_root.join("durable");
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&durable_root)
            .update(&json!({
                "parallel_run_cap": 2,
                "parallel_per_node_cap": 2,
                "parallel_per_provider_cap": 1,
                "parallel_per_target_app_cap": 2,
                "agent_cli": "smoke-ai"
            }))
            .unwrap();
        let work_items = FileWorkItemService::new(&durable_root);
        for id in ["GAP1", "GAP2", "GAP3"] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
                .unwrap();
        }

        let scheduler = FileSchedulingService::with_durable_root(&runtime_root, &durable_root);
        assert_eq!(scheduler.promote().unwrap(), 1);
        assert_eq!(scheduler.promote().unwrap(), 0);
        let state = scheduler.load_state().unwrap();
        assert_eq!(state.policy.provider, "smoke-ai");
        assert_eq!(state.policy.per_provider_limit, 1);
        assert_eq!(state.reservations.len(), 1);
        assert_eq!(state.reservations[0].provider, "smoke-ai");
        assert_eq!(state.reservations[0].node_id, "default");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_scheduler_enforces_active_node_ownership() {
        let temp_root = unique_temp_dir("scheduler-node-ownership");
        let durable_root = temp_root.join("durable");
        let runtime_root = temp_root.join("run/8080");
        let work_items = FileWorkItemService::new(&durable_root);
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
        work_items
            .bulk_transfer_gaps_to_node(
                "remote-node",
                BulkGapSelection {
                    selected_ids: Some(vec!["REMOTE".to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();
        FileNodeRegistryService::new(&durable_root)
            .create("remote-node")
            .unwrap();

        let scheduler = FileSchedulingService::with_durable_root(&runtime_root, &durable_root);
        assert_eq!(scheduler.promote().unwrap(), 1);
        assert!(scheduler.reserve("REMOTE").is_err());

        FileNodeRegistryService::new(&durable_root)
            .activate("remote-node")
            .unwrap();
        let remote_scheduler =
            FileSchedulingService::with_durable_root(&runtime_root, &durable_root);
        let remote_reservation = remote_scheduler.reserve("REMOTE").unwrap();
        let state = remote_scheduler.load_state().unwrap();
        assert!(state.reservations.iter().any(|reservation| {
            reservation.reservation_id == remote_reservation && reservation.node_id == "remote-node"
        }));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_scheduler_respects_feature_order_on_promote_reserve_and_dispatch() {
        let temp_root = unique_temp_dir("scheduler-feature-order");
        let durable_root = temp_root.join("durable");
        let runtime_root = temp_root.join("run/8080");
        let dispatch_runtime_root = temp_root.join("run/8081");
        FileSettingsService::new(&durable_root)
            .update(&json!({
                "parallel_run_cap": 2,
                "parallel_per_node_cap": 2
            }))
            .unwrap();
        let work_items = FileWorkItemService::new(&durable_root);
        work_items
            .create_feature_summary("Feature", Some("FEAT1"), None, None)
            .unwrap();
        for id in ["FIRST", "SECOND"] {
            work_items.create_gap_summary(id, Some(id)).unwrap();
            work_items
                .transition_gap_status(id, GapStatus::Todo)
                .unwrap();
            work_items.assign_gap_to_feature("FEAT1", id).unwrap();
        }

        let scheduler = FileSchedulingService::with_durable_root(&runtime_root, &durable_root);
        assert!(scheduler.reserve("SECOND").is_err());
        assert_eq!(scheduler.promote().unwrap(), 1);
        let state = scheduler.load_state().unwrap();
        assert_eq!(state.reservations.len(), 1);
        assert_eq!(state.reservations[0].gap_id, "FIRST");

        work_items
            .bulk_update_gaps(
                BulkGapSelection {
                    selected_ids: Some(vec!["FIRST".to_string()]),
                    ..Default::default()
                },
                crate::core::product::work_items::BulkGapUpdate::Status("done".to_string()),
            )
            .unwrap();
        let dispatch_scheduler =
            FileSchedulingService::with_durable_root(&dispatch_runtime_root, &durable_root);
        let second_reservation = dispatch_scheduler.reserve("SECOND").unwrap();
        work_items
            .bulk_update_gaps(
                BulkGapSelection {
                    selected_ids: Some(vec!["FIRST".to_string()]),
                    ..Default::default()
                },
                crate::core::product::work_items::BulkGapUpdate::Status("todo".to_string()),
            )
            .unwrap();
        assert!(dispatch_scheduler.dispatch(&second_reservation).is_err());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_scheduler_pauses_cancels_and_retries_jobs() {
        let temp_root = unique_temp_dir("scheduler-controls");
        let scheduler = FileSchedulingService::new(temp_root.join("run/8080"));

        scheduler.pause(SchedulerControl::Agents).unwrap();
        assert!(scheduler.reserve("GAP1").is_err());
        scheduler.resume(SchedulerControl::Agents).unwrap();
        FileProcessSupervisor::new(temp_root.join("run/8080"))
            .set_agents_paused(true)
            .unwrap();
        assert!(scheduler.reserve("GAP1").is_err());
        FileProcessSupervisor::new(temp_root.join("run/8080"))
            .set_agents_paused(false)
            .unwrap();

        let reservation_id = scheduler.reserve("GAP1").unwrap();
        assert_eq!(scheduler.reserve("GAP1").unwrap(), reservation_id);
        let job_id = scheduler.dispatch(&reservation_id).unwrap();
        scheduler.cancel(&job_id).unwrap();
        let state = scheduler.load_state().unwrap();
        assert_eq!(state.reservations[0].state, ReservationState::Cancelled);

        let retried_job_id = scheduler.retry(&job_id).unwrap();
        assert_ne!(retried_job_id, job_id);
        assert_eq!(
            scheduler
                .job_registry
                .status(&retried_job_id)
                .unwrap()
                .owner,
            "gap:GAP1"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
