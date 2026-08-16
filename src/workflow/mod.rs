use std::path::PathBuf;

pub mod behavior;
pub mod behaviors;
mod candidate_handoff;
mod candidate_refresh;
pub mod context;
pub mod promotion;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::model::JsonObject;
use crate::model::goal::GoalPriority;
use crate::process::supervisor::errors::RefineError;
use crate::tools::host::git_worktrees::MergeResult;

const ACTIVE_WORK_REPLENISH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowPassResult {
    pub promoted: usize,
    pub steps: Vec<WorkflowStepResult>,
}

impl WorkflowPassResult {
    pub fn changed_projection(&self) -> bool {
        self.promoted != 0 || !self.steps.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowStepResult {
    pub goal_id: String,
    pub provider: String,
    pub branch: String,
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeResult>,
    pub final_status: String,
    pub provider_output: String,
}

#[derive(Clone)]
pub struct WorkflowEngine {
    pub runtime_root: PathBuf,
    pub target_root: Option<PathBuf>,
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

fn missing_workflow_artifact(name: &str, goal_id: &str) -> RefineError {
    RefineError::Conflict(format!(
        "workflow artifact {name} is missing for Goal {goal_id}"
    ))
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

mod execution;
mod execution_context;
mod failure_settlement;
mod goal_agent_context;
mod goal_agent_spec;
mod governance;
mod implementation_planning;
mod policy;
mod quality_recovery;
mod reconciliation;
mod settings;
#[cfg(test)]
mod tests;

use candidate_refresh::{CandidateRefreshOutcome, refresh_candidate_for_target_advancement};
use execution_context::{
    agent_worktree_cwd, authored_workflow_commitment, hydrate_plan_or_implement_context,
    hydrate_retry_context, implementation_branch_name,
};
use goal_agent_context::{round_agent_context, selected_agent_context};
use goal_agent_spec::goal_agent_prompt;
#[cfg(test)]
use governance::GOVERNANCE_VERDICT_UNPARSABLE;
use governance::{
    GovernanceEvaluation, parse_governance_provider_output, post_implementation_governance_prompt,
};
use implementation_planning::{
    complete_implementation_planning, fail_implementation_phase, governed_implementation_prompt,
    run_governed_implementation_planning,
};
use quality_recovery::{
    QualityRecoveryInvestigation, parse_quality_recovery_provider_output, quality_recovery_prompt,
};
use settings::{
    agent_idle_timeout, setting_cap_with_default_values, setting_string, setting_usize,
};
