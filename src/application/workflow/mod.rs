use std::path::PathBuf;

mod agents;
pub mod engine;
pub mod governance;
pub mod phases;
pub mod recovery;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::error::RefineError;
use crate::infrastructure::git::worktrees::MergeResult;
use crate::model::JsonObject;
use crate::model::goal::GoalPriority;

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

#[cfg(test)]
mod tests;

use agents::{goal_agent_prompt, round_agent_context, selected_agent_context};
use engine::{
    agent_idle_timeout, agent_worktree_cwd, implementation_branch_name, setting_string,
    setting_usize,
};
#[cfg(test)]
use governance::GOVERNANCE_VERDICT_UNPARSABLE;
use governance::{
    GovernanceEvaluation, parse_governance_provider_output, plan_governance_precheck_prompt,
    post_implementation_governance_prompt,
};
use phases::{
    complete_implementation_planning, fail_implementation_phase, governed_implementation_prompt,
    implementation_resume_session, run_governed_implementation_planning,
};
#[cfg(test)]
use recovery::workflow_conflict_resolution_enabled;
use recovery::{
    CandidateRefreshOutcome, refresh_candidate_for_target_advancement,
    refresh_candidate_with_resolver, workflow_conflict_resolver,
};
use recovery::{
    QualityRecoveryInvestigation, parse_quality_recovery_provider_output, quality_recovery_prompt,
};
