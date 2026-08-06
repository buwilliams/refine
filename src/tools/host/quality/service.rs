use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::log::LogEntry;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
    write_json_atomically,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::process::supervisor::security::{FileSecurityService, SecurityService};
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::WorkflowEngine;
use crate::workflow::capacity::{AgentCapacityRequest, AgentCapacityService};

use super::types::*;

mod cancellation;
mod execution;
mod provider_output;
mod runner;
mod settings;
mod settlement;
mod summary;

use cancellation::*;
pub(crate) use provider_output::is_quality_harness_fault;
pub(crate) use provider_output::parse_quality_provider_output;
use provider_output::*;
pub use runner::QualityOperationRunner;
pub(crate) use summary::{quality_error_summary, quality_failure_summary};

pub(super) const SETTINGS_MIGRATION_VERSION: u32 = 2;

fn default_evaluation_scope() -> String {
    "isolated_candidate".to_string()
}

#[derive(Clone, Debug)]
pub struct FileQualityService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
    #[cfg(test)]
    pub migration_failure_after_stage: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckRequest {
    pub owner_id: String,
    pub round_idx: usize,
    pub node_id: String,
    pub provider: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_candidate_commit: Option<String>,
    #[serde(default = "default_evaluation_scope")]
    pub evaluation_scope: String,
    pub candidate_commit: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub process_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityTestResult {
    pub test: String,
    pub status: String,
    pub evidence: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityCheckResult {
    pub owner_id: String,
    pub ok: bool,
    pub summary: String,
    pub results: Vec<QualityTestResult>,
    pub diagnostics: Vec<String>,
    pub candidate_commit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityOperationResult {
    pub operation: OperationHandle,
    pub result: QualityCheckResult,
}

pub trait QualityService {
    fn run_checks(&self, request: QualityCheckRequest) -> RefineResult<QualityCheckResult>;
    fn screenshots(&self, owner_id: &str) -> RefineResult<Vec<String>>;
    fn compare(&self, baseline: &str, candidate: &str) -> RefineResult<QualityCheckResult>;
    fn gate(&self, owner_id: &str) -> RefineResult<QualityCheckResult>;
}
