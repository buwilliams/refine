use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::model::Timestamp;
use crate::model::goal::{QUALITY_PROOF_SCHEMA_VERSION, QualityProof};
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
    write_json_atomically,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::process::supervisor::security::{FileSecurityService, SecurityService};
use crate::prompts::{PromptEngine, PromptTemplate, render};
use crate::structured_output::Contract;
use crate::tools::host::agent_providers::{HostAgentProviderService, ProviderInvocation};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::work_items::{FileWorkItemService, WorkflowAttemptAuthority};
use crate::workflow::WorkflowEngine;

use super::types::*;
use super::{
    ISOLATED_CANDIDATE, QualityIdentityCommitment, is_quality_candidate_infrastructure,
    validate_quality_identity,
};

mod cancellation;
mod execution;
mod provider_output;
mod runner;
mod settings;
mod settlement;
mod summary;
mod wire;

use cancellation::*;
use wire::*;
pub(crate) use provider_output::parse_quality_provider_output;
use provider_output::*;
pub(crate) use provider_output::{is_quality_harness_fault, is_quality_output_contract_fault};
pub use runner::QualityOperationRunner;
pub(crate) use summary::{quality_error_summary, quality_failure_summary};

pub(super) const SETTINGS_MIGRATION_VERSION: u32 = 3;

fn default_quality_instructions() -> &'static str {
    PromptEngine::load(PromptTemplate::QualityDefaultInstructions)
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_commitment: Option<QualityIdentityCommitment>,
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
    /// One timestamp shared by the durable Goal proof and terminal operation result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_attempts: Vec<QualityProviderAttempt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QualityProviderAttempt {
    pub attempt: usize,
    pub process_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    pub raw_output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<String>,
    pub accepted: bool,
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
