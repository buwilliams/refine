use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcess};
use crate::process::supervisor::coordination::replace_file_durably;
use crate::process::supervisor::errors::{RefineError, RefineResult};

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
