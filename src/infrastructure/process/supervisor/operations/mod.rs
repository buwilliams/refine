use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::subprocess::{FileProcessSupervisor, ManagedProcess};
use crate::model::log::LogEntry;

const RECOVERY_PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

mod cancellation;
mod completion;
mod helpers;
mod recovery;
mod registration;
mod registry;
mod storage;

use helpers::*;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OperationState {
    Pending,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

fn operation_schema_version() -> u32 {
    1
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalAttemptState {
    Reserved,
    Submitted,
    Received,
    Claimed,
    Active,
    Cancelling,
    Interrupted,
    Failed,
    Cancelled,
    Completed,
}

impl ExternalAttemptState {
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Interrupted | Self::Failed | Self::Cancelled | Self::Completed
        )
    }
}

/// Durable, non-secret evidence for one bounded processless handoff attempt.
///
/// The raw claim nonce is intentionally never persisted. `nonce_verifier` is a
/// SHA-256 verifier used only to fence stale or duplicate helpers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExternalOperationAttempt {
    pub attempt_id: String,
    pub state: ExternalAttemptState,
    pub nonce_verifier: String,
    pub mechanism: String,
    pub mechanism_identity: String,
    pub executable: String,
    pub argument_fingerprint: String,
    pub reserved_at: String,
    pub claim_deadline_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimant_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimant_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_evidence: Option<Value>,
}

impl OperationState {
    pub fn as_api_status(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationHandle {
    #[serde(default = "operation_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub revision: u64,
    pub id: String,
    pub owner: String,
    pub state: OperationState,
    #[serde(default = "empty_object")]
    pub request: Value,
    #[serde(default = "empty_object")]
    pub progress: Value,
    #[serde(default = "empty_object")]
    pub result: Value,
    #[serde(default)]
    pub error: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_attempt: Option<ExternalOperationAttempt>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplacementOperationRegistration {
    pub operation: OperationHandle,
    pub created: bool,
}

pub trait OperationRegistry {
    fn register(&self, owner: &str) -> RefineResult<OperationHandle>;
    fn status(&self, operation_id: &str) -> RefineResult<OperationHandle>;
    fn cancel(&self, operation_id: &str) -> RefineResult<OperationHandle>;
    fn recover(&self) -> RefineResult<Vec<OperationHandle>>;
}

pub trait OperationProjectionRefresher {
    fn refresh_operation_projection(&self) -> RefineResult<()>;
}

impl<F> OperationProjectionRefresher for F
where
    F: Fn() -> RefineResult<()>,
{
    fn refresh_operation_projection(&self) -> RefineResult<()> {
        self()
    }
}

#[derive(Clone, Debug)]
pub struct FileOperationRegistry {
    pub runtime_root: PathBuf,
}

/// Holds the same mutation lock used by cancellation until a supervised process has been
/// durably registered. This closes the gap where cancellation could observe no process and a
/// worker could launch one immediately afterward.
pub struct OperationLaunchGuard {
    _lock: fs::File,
}

#[cfg(test)]
mod tests;
