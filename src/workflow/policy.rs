use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::model::goal::GoalIndexProjection;
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{FileProcessSupervisor, ProcessOwner, ProcessPauseState};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::host_resources::{HostResources, observed_agent_memory_bytes};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_projection::ActiveGoalIndex;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::promotion::BacklogPromotionService;

use super::{
    WorkflowEngine, WorkflowPolicy, json_object, now_timestamp, priority_rank,
    setting_cap_with_default_values, setting_string, setting_usize,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ExecutionLoad {
    pub global: usize,
    pub by_node: BTreeMap<String, usize>,
    pub by_provider: BTreeMap<String, usize>,
    pub by_target_app: BTreeMap<String, usize>,
}

impl ExecutionLoad {
    pub(super) fn record(&mut self, node: &str, provider: &str, target_app: &str) {
        self.global += 1;
        *self.by_node.entry(node.to_string()).or_default() += 1;
        *self.by_provider.entry(provider.to_string()).or_default() += 1;
        *self
            .by_target_app
            .entry(target_app.to_string())
            .or_default() += 1;
    }

    pub(super) fn available(
        &self,
        policy: &WorkflowPolicy,
        node: &str,
        provider: &str,
        target_app: &str,
    ) -> bool {
        self.global < policy.global_limit
            && self.by_node.get(node).copied().unwrap_or(0) < policy.per_node_limit
            && self.by_provider.get(provider).copied().unwrap_or(0) < policy.per_provider_limit
            && self.by_target_app.get(target_app).copied().unwrap_or(0)
                < policy.per_target_app_limit
    }
}

impl WorkflowEngine {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            target_root: None,
        }
    }

    pub fn with_target_root(
        runtime_root: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            target_root: Some(target_root.into()),
        }
    }

    pub(super) fn refine_dir(&self) -> RefineResult<Option<PathBuf>> {
        self.target_root
            .as_ref()
            .map(|target_root| prepare_refine_dir(target_root))
            .transpose()
    }

    pub fn policy(&self) -> RefineResult<WorkflowPolicy> {
        let Some(target_root) = &self.target_root else {
            return Ok(WorkflowPolicy::default());
        };
        let refine_dir = prepare_refine_dir(target_root)?;
        self.policy_for_refine_dir(&refine_dir)
    }

    pub fn policy_for_refine_dir(&self, refine_dir: &Path) -> RefineResult<WorkflowPolicy> {
        let active_node_id =
            FileNodeRegistryService::with_active_root(refine_dir, &self.runtime_root)
                .active_node_id()?;
        self.policy_for_refine_dir_and_node(refine_dir, &active_node_id)
    }

    pub fn policy_for_refine_dir_and_node(
        &self,
        refine_dir: &Path,
        node_id: &str,
    ) -> RefineResult<WorkflowPolicy> {
        let mut policy = WorkflowPolicy::default();
        if let Some(target_root) = &self.target_root {
            let settings = FileSettingsService::for_node(refine_dir, node_id).load()?;
            let governed_limit = HostResources::current(&self.runtime_root)
                .recommended_agent_concurrency(observed_agent_memory_bytes(&self.runtime_root));
            policy.global_limit = setting_usize(&settings, "parallel_run_cap", governed_limit);
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
            policy.active_node_id = node_id.to_string();
        }
        Ok(policy)
    }

    pub fn apply_runtime_settings(&self) -> RefineResult<usize> {
        if self.workflow_paused()? {
            return Ok(0);
        }
        self.promote_backlog_to_todo()
    }

    pub fn promote_backlog_to_todo(&self) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(0);
        };
        self.promote_backlog_to_todo_for_refine_dir(&refine_dir)
    }

    pub(super) fn promote_backlog_to_todo_for_refine_dir(
        &self,
        refine_dir: &Path,
    ) -> RefineResult<usize> {
        BacklogPromotionService::new(refine_dir, &self.runtime_root).promote_backlog_to_todo()
    }

    pub fn set_workflow_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        FileProcessSupervisor::new(&self.runtime_root).set_workflow_paused(paused)
    }

    pub(super) fn workflow_paused(&self) -> RefineResult<bool> {
        Ok(FileProcessSupervisor::new(&self.runtime_root)
            .pause_state()?
            .workflow_paused)
    }

    pub(super) fn ensure_automation_running(&self) -> RefineResult<()> {
        if self.workflow_paused()? {
            return Err(RefineError::Conflict(
                "workflow automation is paused".to_string(),
            ));
        }
        Ok(())
    }

    /// Reconciles node-local execution after a runner restart without changing synchronized
    /// workflow authority. Old workers are stopped; durable stages remain where they were so the
    /// next scheduler pass can repeat the incomplete idempotent work.
    pub fn recover_interrupted_goals(&self, detail: &str) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(0);
        };
        self.terminate_stale_workflow_processes()?;
        self.cleanup_legacy_execution_state()?;
        let active = ActiveGoalIndex::load_or_rebuild(&refine_dir)?;
        let active_node_id = FileNodeRegistryService::new(&refine_dir).active_node_id()?;
        let active_goals = active
            .goals()
            .filter(|goal| {
                matches!(
                    goal.status,
                    GoalStatus::InProgress
                        | GoalStatus::ReadyMerge
                        | GoalStatus::Build
                        | GoalStatus::Qa
                )
            })
            .filter(|goal| goal.node_id.as_deref().unwrap_or("default") == active_node_id)
            .cloned()
            .collect::<Vec<_>>();
        let work_items = FileWorkItemService::new(&refine_dir);
        let logs = FileLogService::new(&refine_dir);
        let detail = if detail.trim().is_empty() {
            "workflow runner restarted before the Goal completed"
        } else {
            detail.trim()
        };
        let mut recovered = 0;
        for goal in &active_goals {
            let Some(round_idx) = goal.round_count.checked_sub(1) else {
                eprintln!(
                    "refine workflow recovery: Goal {} is {} with no authored Round; preserving the Goal and creating no worker or worktree",
                    goal.id,
                    goal.status.as_str()
                );
                continue;
            };
            super::implementation_planning::recover_interrupted_plan(
                &work_items,
                &goal.id,
                round_idx,
            )?;
            logs.append_round_log(
                &goal.id,
                round_idx,
                LogEntry {
                    datetime: now_timestamp(),
                    severity: "warning".to_string(),
                    category: "workflow".to_string(),
                    message: format!(
                        "Workflow execution was restarted from durable {} state: {detail}",
                        goal.status.as_str()
                    ),
                    details: Some(json_object(json!({
                        "reason": detail,
                        "checkpoint": goal.status.as_str(),
                        "automatic_restart": true
                    }))),
                    actions: Vec::new(),
                    actor: Some("refine".to_string()),
                    goal_id: Some(goal.id.clone()),
                },
            )?;
            recovered += 1;
        }
        Ok(recovered)
    }

    fn terminate_stale_workflow_processes(&self) -> RefineResult<()> {
        for root in [self.runtime_root.join("agents"), self.runtime_root.clone()] {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list()? {
                let metadata = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok());
                let workflow_owned = metadata
                    .as_ref()
                    .and_then(|details| details.get("kind"))
                    .and_then(Value::as_str)
                    == Some("workflow");
                if workflow_owned && FileProcessSupervisor::process_is_alive(&process)? {
                    supervisor.terminate_and_confirm_exit(&process, Duration::from_secs(10))?;
                }
            }
        }
        Ok(())
    }

    fn cleanup_legacy_execution_state(&self) -> RefineResult<()> {
        for name in [
            "workflow-automation-state.json",
            ".workflow-automation-state.lock",
            "agent-capacity-state.json",
            ".agent-capacity.lock",
            ".workflow-process-registration.lock",
        ] {
            let path = self.runtime_root.join(name);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove retired execution state {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        let tombstones = self
            .runtime_root
            .join("operations")
            .join(".workflow-cancellations");
        match std::fs::remove_dir_all(&tombstones) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove retired workflow cancellation state {}: {error}",
                    tombstones.display()
                )));
            }
        }
        let outcomes = self.runtime_root.join("process-stop-outcomes");
        let entries = match std::fs::read_dir(&outcomes) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to inspect retired cancellation journals {}: {error}",
                    outcomes.display()
                )));
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|error| RefineError::Io(error.to_string()))?
                .path();
            let legacy = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("workflow-cancellation-") && name.ends_with(".json")
                });
            if legacy {
                std::fs::remove_file(&path).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to remove retired cancellation journal {}: {error}",
                        path.display()
                    ))
                })?;
            }
        }
        Ok(())
    }

    pub(super) fn observed_execution_load(&self) -> RefineResult<ExecutionLoad> {
        let mut load = ExecutionLoad::default();
        for root in [self.runtime_root.clone(), self.runtime_root.join("agents")] {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list()? {
                if !matches!(process.owner, ProcessOwner::Agent | ProcessOwner::Quality)
                    || !FileProcessSupervisor::process_is_alive(&process)?
                {
                    continue;
                }
                let details = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                    .unwrap_or_else(|| json!({}));
                let node = details
                    .get("node_id")
                    .and_then(Value::as_str)
                    .unwrap_or("default");
                let provider = details
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let target = details
                    .get("target_app_id")
                    .or_else(|| details.get("cwd"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                load.record(node, provider, target);
            }
        }
        Ok(load)
    }

    pub fn soft_capacity_available(
        &self,
        policy: &WorkflowPolicy,
        node: &str,
        provider: &str,
        target_app: &str,
    ) -> RefineResult<bool> {
        Ok(self
            .observed_execution_load()?
            .available(policy, node, provider, target_app))
    }
}

fn releases_feature_order(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::Review | GoalStatus::Done | GoalStatus::Cancelled
    )
}

fn occupies_feature_slot(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::InProgress | GoalStatus::ReadyMerge | GoalStatus::Build | GoalStatus::Qa
    )
}

pub(super) struct SchedulingEligibility {
    feature_eligible: BTreeSet<String>,
    highest_todo_rank: BTreeMap<String, u8>,
}

impl SchedulingEligibility {
    pub(super) fn new<'a>(
        goals: impl IntoIterator<Item = &'a GoalIndexProjection> + Clone,
        excluded_goal_ids: &BTreeSet<String>,
    ) -> Self {
        let mut lowest_holding_order: BTreeMap<(&str, &str), i64> = BTreeMap::new();
        let mut occupying_count: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for goal in goals.clone() {
            if goal.round_count == 0 {
                continue;
            }
            let (Some(feature_id), Some(order)) = (goal.feature_id.as_deref(), goal.feature_order)
            else {
                continue;
            };
            let key = (goal.node_id.as_deref().unwrap_or("default"), feature_id);
            if !releases_feature_order(&goal.status) {
                lowest_holding_order
                    .entry(key)
                    .and_modify(|lowest| *lowest = (*lowest).min(order))
                    .or_insert(order);
            }
            if occupies_feature_slot(&goal.status) {
                *occupying_count.entry(key).or_default() += 1;
            }
        }
        let feature_eligible = goals
            .clone()
            .into_iter()
            .filter(|goal| {
                if goal.round_count == 0 {
                    return false;
                }
                let (Some(feature_id), Some(order)) =
                    (goal.feature_id.as_deref(), goal.feature_order)
                else {
                    return true;
                };
                let key = (goal.node_id.as_deref().unwrap_or("default"), feature_id);
                lowest_holding_order.get(&key).copied() == Some(order)
                    && (goal.status != GoalStatus::Todo
                        || occupying_count.get(&key).copied().unwrap_or(0) == 0)
            })
            .map(|goal| goal.id.clone())
            .collect::<BTreeSet<_>>();
        let mut highest_todo_rank = BTreeMap::new();
        for goal in goals {
            if goal.status != GoalStatus::Todo
                || goal.round_count == 0
                || excluded_goal_ids.contains(&goal.id)
                || !feature_eligible.contains(&goal.id)
            {
                continue;
            }
            let rank = priority_rank(&goal.priority);
            highest_todo_rank
                .entry(goal.node_id.as_deref().unwrap_or("default").to_string())
                .and_modify(|highest: &mut u8| *highest = (*highest).max(rank))
                .or_insert(rank);
        }
        Self {
            feature_eligible,
            highest_todo_rank,
        }
    }

    pub(super) fn feature_eligible(&self, goal_id: &str) -> bool {
        self.feature_eligible.contains(goal_id)
    }

    pub(super) fn priority_eligible(&self, goal: &GoalIndexProjection) -> bool {
        goal.status != GoalStatus::Todo
            || self
                .highest_todo_rank
                .get(goal.node_id.as_deref().unwrap_or("default"))
                .is_none_or(|highest| priority_rank(&goal.priority) >= *highest)
    }
}
