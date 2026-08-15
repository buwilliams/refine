use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::process::subprocess::{FileProcessSupervisor, ProcessOwner, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::http_probe;
use crate::process::supervisor::operations::{
    ExternalAttemptState, ExternalOperationAttempt, FileOperationRegistry, OperationHandle,
    OperationRegistry, OperationState,
};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::tools::host::daemon_lifecycle::{
    HandoffLaunchReceipt, HandoffObservation, HostRestartSafeHandoffLauncher, RestartSafeHandoff,
    RestartSafeHandoffLauncher, handoff_argument_fingerprint, handoff_mechanism,
    handoff_mechanism_identity, live_daemon_executable,
};
use crate::tools::host::installation::{
    FileInstallationService, InstalledServiceAction, ServiceRegistrationUpdate,
};

pub const SOURCE_PROMOTION_STATE_FILE: &str = "source-promotion.json";
pub const SOURCE_UPDATE_CHECK_STATE_FILE: &str = "source-update-check.json";
pub const SOURCE_UPDATE_CHECK_INTERVAL_SECONDS: i64 = 60 * 60;

mod agent;
mod agent_plan;
mod cancellation;
mod execution;
mod git_support;
mod handoff_protocol;
mod host;
mod recovery;
mod service;
mod update_check;
mod validation;

#[cfg(test)]
pub(crate) use agent::AgentLaunchFailpoint;
pub use execution::run_source_promotion;
pub use service::FileSourcePromotionService;
pub use update_check::{CachedSourcePromotionStatus, SourceUpdateCheckState};
pub use validation::{validate_promotion, validate_update_intent};

use git_support::*;
use host::*;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionSnapshot {
    pub checkout_path: String,
    pub current_commit: String,
    pub remote: String,
    pub local_branch: String,
    pub branch: String,
    pub available_commit: String,
    pub clean: bool,
    pub fast_forward: bool,
    pub update_available: bool,
    #[serde(default)]
    pub active_work: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<SourcePromotionOperation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionAffordance {
    pub visible: bool,
    pub enabled: bool,
    pub state: String,
    pub update_available: bool,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionOperation {
    pub id: String,
    pub status: String,
    pub stage: String,
    pub message: String,
    pub checkout_path: String,
    pub from_commit: String,
    pub to_commit: String,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub rollback_attempted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_succeeded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_manager: Option<String>,
    #[serde(default)]
    pub registration_updated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_rollback_succeeded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_evidence: Option<SourcePromotionRollbackEvidence>,
    /// Stash commit that preserves uncommitted work found at queue time. The
    /// stash is reported to the operator and never reapplied automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stashed_changes: Option<String>,
    /// The one dedicated managed maintenance Agent launched for this operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_process_id: Option<String>,
    /// The restart-independent maintenance worker that launches and observes
    /// the installed provider. It is not itself represented as an Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_worker_process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_context_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_output: Option<String>,
    /// Admission intent captured before the Agent pauses new workflow claims.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_upgrade_workflow_paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_pause_restored: Option<bool>,
    /// Independent primary and restoration outcomes prevent restoration
    /// failures from being hidden behind a successful promotion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restoration_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation_evidence: Option<Value>,
    /// Ordered typed-capability decisions made by the installed Agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_decisions: Vec<Value>,
    /// Redacted projection of the authoritative operation-registry handoff attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_attempt: Option<ExternalOperationAttempt>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionRollbackEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_restored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_restored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_executable: Option<String>,
    #[serde(default)]
    pub replacement_attempted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_succeeded: Option<bool>,
    #[serde(default)]
    pub reachability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_matches: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionDaemonObservation {
    pub reachability: String,
    pub expected_executable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_matches: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SourcePromotionDaemonObservation {
    fn verified(&self) -> bool {
        self.reachability == "reachable" && self.identity_matches == Some(true)
    }
}

impl SourcePromotionOperation {
    fn queued(snapshot: &SourcePromotionSnapshot) -> Self {
        let now = now_timestamp();
        Self {
            id: format!("source-{}", Uuid::new_v4()),
            status: "queued".to_string(),
            stage: "queued".to_string(),
            message: "Source promotion queued".to_string(),
            checkout_path: snapshot.checkout_path.clone(),
            from_commit: snapshot.current_commit.clone(),
            to_commit: snapshot.available_commit.clone(),
            started_at: now.clone(),
            updated_at: now,
            error: None,
            rollback_attempted: false,
            rollback_succeeded: None,
            recovery: None,
            candidate_executable: None,
            service_manager: None,
            registration_updated: false,
            registration_rollback_succeeded: None,
            observed_executable: None,
            rollback_evidence: None,
            stashed_changes: None,
            agent_process_id: None,
            agent_worker_process_id: None,
            agent_provider: None,
            agent_context_path: None,
            agent_output: None,
            pre_upgrade_workflow_paused: None,
            workflow_pause_restored: None,
            primary_outcome: None,
            restoration_error: None,
            reconciliation_evidence: None,
            agent_decisions: Vec::new(),
            handoff_attempt: None,
        }
    }
}

pub fn source_promotion_affordance(
    checkout_discoverable: bool,
    source: &SourcePromotionSnapshot,
) -> SourcePromotionAffordance {
    if !checkout_discoverable {
        return SourcePromotionAffordance {
            visible: false,
            enabled: false,
            state: "hidden".to_string(),
            update_available: source.update_available,
            title: "Running Refine source checkout is not discoverable".to_string(),
        };
    }

    if let Some(operation) = source
        .operation
        .as_ref()
        .filter(|operation| matches!(operation.status.as_str(), "queued" | "running"))
    {
        return SourcePromotionAffordance {
            visible: true,
            enabled: false,
            state: "updating".to_string(),
            update_available: source.update_available,
            title: if operation.message.is_empty() {
                format!("Refine source promotion is {}", operation.status)
            } else {
                operation.message.clone()
            },
        };
    }

    if !source.update_available {
        return SourcePromotionAffordance {
            visible: true,
            enabled: true,
            state: "current".to_string(),
            update_available: false,
            title: format!(
                "Running Refine source is current at {}",
                short_commit(&source.current_commit)
            ),
        };
    }

    // A dirty checkout is not a blocker: queueing an update stashes and
    // reports uncommitted work automatically.
    let mut blockers = Vec::new();
    if !source.fast_forward {
        blockers.push("upstream is not a fast-forward".to_string());
    }
    if !blockers.is_empty() {
        return SourcePromotionAffordance {
            visible: true,
            enabled: false,
            state: "blocked".to_string(),
            update_available: true,
            title: format!("Refine source update unavailable: {}", blockers.join("; ")),
        };
    }

    SourcePromotionAffordance {
        visible: true,
        enabled: true,
        state: "available".to_string(),
        update_available: true,
        title: format!(
            "Update running Refine to {}",
            short_commit(&source.available_commit)
        ),
    }
}

fn short_commit(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}

fn require_checkout_binary(executable: &Path, purpose: &str) -> RefineResult<()> {
    if executable.is_file() {
        Ok(())
    } else {
        Err(RefineError::NotFound(format!(
            "checkout-local binary {} is required for {purpose}; build and install bin/refine before continuing",
            executable.display()
        )))
    }
}

pub trait SourcePromotionHost {
    fn build_candidate(&mut self, commit: &str) -> RefineResult<PathBuf>;
    fn verify_preconditions(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()>;
    fn prepare_restart(
        &mut self,
        executable: &Path,
    ) -> RefineResult<Option<ServiceRegistrationUpdate>>;
    fn stop_daemon(&mut self) -> RefineResult<()>;
    fn activate_candidate_binary(&mut self, candidate: &Path) -> RefineResult<PathBuf>;
    fn activate(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()>;
    fn restart_daemon(&mut self, executable: &Path) -> RefineResult<()>;
    fn verify_daemon(
        &mut self,
        expected_commit: &str,
        expected_executable: &Path,
    ) -> RefineResult<PathBuf>;
    fn complete_restart_registration(&mut self) -> RefineResult<()>;
    fn restore_restart_registration(&mut self) -> RefineResult<bool>;
    fn restore_previous_binary(&mut self) -> RefineResult<bool>;
    fn verify_previous_registration(&mut self) -> RefineResult<PathBuf>;
    fn verify_previous_source(&mut self, expected_commit: &str) -> RefineResult<()>;
    fn rollback(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()>;
    fn restart_previous_daemon(&mut self) -> RefineResult<()>;
    fn observe_previous_daemon(&mut self) -> SourcePromotionDaemonObservation;
}

#[cfg(test)]
mod tests;

#[cfg(all(test, target_os = "linux"))]
mod systemd_tests;
