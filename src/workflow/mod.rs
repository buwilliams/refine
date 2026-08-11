use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;

pub mod behavior;
pub mod behaviors;
pub mod capacity;
pub mod context;
pub mod promotion;

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::JsonObject;
use crate::model::goal::GoalPriority;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::MergeResult;

pub const WORKFLOW_AUTOMATION_STATE_FILE: &str = "workflow-automation-state.json";
const WORKFLOW_AUTOMATION_STATE_LOCK_FILE: &str = ".workflow-automation-state.lock";
const AUTOMATION_CONCURRENCY_LIMIT_REACHED: &str = "automation concurrency limit reached";
const ACTIVE_WORK_REPLENISH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_idx: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub decision_version: u64,
    /// Number of semantically equivalent terminal attempts represented by this
    /// record after claim-history compaction.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub occurrences: u32,
    pub state: WorkflowClaimState,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowGoalClaimSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_claim: Option<WorkflowClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_preparation_failure: Option<WorkflowClaim>,
    #[serde(default)]
    pub consecutive_execution_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_not_before: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowRetryDelay {
    pub goal_id: String,
    pub node_id: String,
    pub claim_id: String,
    pub retry_not_before: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
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
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub policy: WorkflowPolicy,
    #[serde(default)]
    pub claim_history_version: u32,
    /// Compact per-Goal authority and retry projection. This remains bounded by
    /// Goals rather than attempts and lets hot paths avoid historical scans.
    #[serde(default)]
    pub claim_summaries: BTreeMap<String, WorkflowGoalClaimSummary>,
    pub claims: Vec<WorkflowClaim>,
    pub updated_at: Option<String>,
}

impl WorkflowAutomationState {
    pub(crate) fn active_claim(&self, goal_id: &str) -> Option<&WorkflowClaim> {
        self.claim_summaries
            .get(goal_id)
            .and_then(|summary| summary.latest_claim.as_ref())
            .filter(|claim| claim.is_active())
            .or_else(|| {
                // Compatibility for callers that deserialize legacy state
                // directly rather than through WorkflowEngine::load_state.
                self.claims
                    .iter()
                    .rev()
                    .find(|claim| claim.goal_id == goal_id && claim.is_active())
            })
    }
}

impl WorkflowClaim {
    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.state,
            WorkflowClaimState::Claimed | WorkflowClaimState::Running
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowExecutionFence {
    pub claim_id: String,
    pub execution_id: String,
    pub goal_id: String,
    pub node_id: String,
    pub round_idx: usize,
    pub goal_revision: u64,
    pub decision_version: u64,
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
    fn cancel(&self, execution_id: &str) -> RefineResult<()>;
    fn retry(&self, execution_id: &str) -> RefineResult<String>;
}

#[cfg(test)]
type BeforeWorkerPrepareHook = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Clone)]
pub struct WorkflowEngine {
    pub runtime_root: PathBuf,
    pub target_root: Option<PathBuf>,
    #[cfg(test)]
    before_worker_prepare_hook: Option<BeforeWorkerPrepareHook>,
}

impl std::fmt::Debug for WorkflowEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkflowEngine")
            .field("runtime_root", &self.runtime_root)
            .field("target_root", &self.target_root)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct WorkflowStateMutationLock {
    file: File,
}

impl Drop for WorkflowStateMutationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

impl WorkflowEngine {}

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

fn one() -> u32 {
    1
}

fn is_one(value: &u32) -> bool {
    *value == 1
}

mod automation;
mod claim_history;
mod execution;
mod execution_context;
mod failure_settlement;
mod goal_agent_context;
mod goal_agent_spec;
mod governance;
mod implementation_planning;
mod policy;
mod ready_merge;
mod reconciliation;
mod settings;
mod state;
mod state_persistence;
#[cfg(test)]
mod tests;

use execution_context::{
    agent_worktree_cwd, ensure_workflow_round, hydrate_retry_context, implementation_branch_name,
};
use goal_agent_context::{round_agent_context, selected_agent_context};
use goal_agent_spec::goal_agent_prompt;
#[cfg(test)]
use governance::GOVERNANCE_VERDICT_UNPARSABLE;
use governance::{
    GovernanceEvaluation, parse_governance_provider_output, post_implementation_governance_prompt,
};
#[cfg(test)]
use implementation_planning::persist_plan;
use implementation_planning::{
    active_implementation_operation_id, complete_implementation_planning,
    fail_implementation_phase, governed_implementation_prompt, record_current_process_settlement,
    run_governed_implementation_planning,
};
use settings::{setting_cap_with_default_values, setting_string, setting_usize};
use state_persistence::{read_state, write_state};
