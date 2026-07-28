use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationLaunchGuard};
use crate::process::supervisor::security::FileSecurityService;

const PROCESS_IDENTITIES_DIR: &str = "process-identities";
const PROCESS_HISTORY_DIR: &str = "process-history";
const WORKFLOW_PROCESS_REGISTRATION_LOCK_FILE: &str = ".workflow-process-registration.lock";
const WORKFLOW_AUTOMATION_STATE_FILE: &str = "workflow-automation-state.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessOwner {
    Daemon,
    Runner,
    TargetApp,
    Agent,
    Quality,
    Import,
    Maintenance,
    UserHelper,
}

impl ProcessOwner {
    pub fn as_kind(&self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Runner => "runner",
            Self::TargetApp => "target_app",
            Self::Agent => "agent",
            Self::Quality => "quality",
            Self::Import => "import",
            Self::Maintenance => "maintenance",
            Self::UserHelper => "user_helper",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedProcessSpec {
    pub owner: ProcessOwner,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ProcessResourceLimits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_command: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessResourceLimits {
    pub max_memory_bytes: Option<u64>,
    pub cpu_priority: Option<String>,
    pub kill_on_parent_exit: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedProcess {
    pub id: String,
    pub owner: ProcessOwner,
    pub pid: Option<u32>,
    pub state: String,
    pub label: Option<String>,
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ProcessResourceLimits>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedProcessOutput {
    pub process: ManagedProcess,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
// Observations are passed briefly between the supervisor and adapters. Keeping
// the process value inline avoids heap allocation on every live output poll.
#[allow(clippy::large_enum_variant)]
pub enum ProcessOutputObservation {
    Observed {
        process: ManagedProcess,
        output: String,
    },
    Terminal {
        process_id: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedProcessOutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ManagedProcessIdentity {
    process_id: String,
    owner: ProcessOwner,
    pid: Option<u32>,
    os_identity: Option<String>,
    registered_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfirmedProcessExit {
    pub process_id: String,
    pub pid: Option<u32>,
    pub signal: String,
    pub os_identity: Option<String>,
    pub confirmed_exit: bool,
    pub registry_retained_until_exit: bool,
    pub registry_cleanup_completed: bool,
    pub identity_cleanup_completed: bool,
    pub waited_ms: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessCleanupStage {
    Registry,
    Identity,
}

#[derive(Debug)]
pub(crate) struct ConfirmedProcessCleanupFailure {
    pub outcome: ConfirmedProcessExit,
    pub error: RefineError,
}

impl ManagedProcessOutput {
    pub fn success(&self) -> bool {
        self.process.exit_code == Some(0)
    }
}

impl ManagedProcess {
    pub fn api_json(&self) -> serde_json::Value {
        let mut value = json!({
            "id": self.id,
            "kind": self.owner.as_kind(),
            "label": self.label.as_deref().unwrap_or(self.owner.as_kind()),
            "status": self.state,
            "pid": self.pid,
            "details": self.details.as_deref().unwrap_or(""),
            "output_available": self.stdout_path.is_some() || self.stderr_path.is_some(),
            "cpu_priority": {"label": self.limits.as_ref().and_then(|limits| limits.cpu_priority.as_deref()).unwrap_or("-")},
            "max_memory": {"label": self.limits.as_ref().and_then(|limits| limits.max_memory_bytes.map(|bytes| bytes.to_string())).unwrap_or_else(|| "-".to_string())},
            "isolation": process_isolation_label(self.limits.as_ref()),
            "actions": process_actions(&self.state)
        });
        if let Some(object) = value.as_object_mut()
            && let Some(details) = self
                .details
                .as_deref()
                .and_then(|details| serde_json::from_str::<serde_json::Value>(details).ok())
                .and_then(|details| details.as_object().cloned())
        {
            for key in [
                "goal_id",
                "feature_id",
                "session_id",
                "claim_id",
                "execution_id",
                "mode",
                "profile",
                "role",
                "provider",
                "worktree",
                "round_idx",
                "attention_state",
                "attention_message",
                "worker_kind",
                "operation_id",
            ] {
                if let Some(field) = details.get(key) {
                    object.insert(key.to_string(), field.clone());
                }
            }
            if let Some(kind) = details.get("kind").and_then(|kind| kind.as_str())
                && matches!(kind, "ui" | "runner")
            {
                object.insert("kind".to_string(), json!(kind));
            }
            if details.get("kind").and_then(Value::as_str) == Some("interactive_session") {
                object.insert("kind".to_string(), json!("interactive_session"));
            } else if details.get("session_id").is_some() {
                object.insert("kind".to_string(), json!("chat"));
            }
        }
        value
    }
}

pub fn workflow_subprocess_metadata(
    execution_id: &str,
    goal_id: &str,
    workflow_state: &str,
    behavior: &str,
    round_idx: Option<usize>,
) -> Map<String, Value> {
    let mut metadata = Map::new();
    metadata.insert("kind".to_string(), json!("workflow"));
    metadata.insert("execution_id".to_string(), json!(execution_id));
    metadata.insert("goal_id".to_string(), json!(goal_id));
    metadata.insert("workflow_state".to_string(), json!(workflow_state));
    metadata.insert("behavior".to_string(), json!(behavior));
    if let Some(round_idx) = round_idx {
        metadata.insert("round_idx".to_string(), json!(round_idx));
    }
    metadata
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ProcessPauseState {
    pub workflow_paused: bool,
}

impl<'de> Deserialize<'de> for ProcessPauseState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireState {
            #[serde(default)]
            workflow_paused: Option<bool>,
            #[serde(default)]
            paused: Option<bool>,
            #[serde(default)]
            background_processes_stopped: bool,
            #[serde(default)]
            agents_paused: bool,
        }

        let wire = WireState::deserialize(deserializer)?;
        let workflow_paused = wire
            .workflow_paused
            .or(wire.paused)
            .unwrap_or(wire.background_processes_stopped || wire.agents_paused);
        Ok(Self { workflow_paused })
    }
}

pub trait ProcessSupervisor {
    fn launch(&self, spec: ManagedProcessSpec) -> RefineResult<ManagedProcess>;
    fn signal(&self, process_id: &str, signal: &str) -> RefineResult<ManagedProcess>;
    fn wait(&self, process_id: &str) -> RefineResult<ManagedProcess>;
    fn stream(&self, process_id: &str) -> RefineResult<String>;
    fn observe_output(&self, enumerated: &ManagedProcess)
    -> RefineResult<ProcessOutputObservation>;
    fn inspect(&self, process_id: &str) -> RefineResult<ManagedProcess>;
    fn cleanup(&self, process_id: &str) -> RefineResult<()>;
    fn recover(&self) -> RefineResult<Vec<ManagedProcess>>;
}

#[derive(Clone, Debug)]
pub struct FileProcessSupervisor {
    pub runtime_root: PathBuf,
    pub allowed_commands: BTreeSet<String>,
    reaper_owned: Arc<Mutex<BTreeSet<String>>>,
}

#[derive(Debug)]
pub(crate) struct WorkflowProcessRegistrationLock {
    file: fs::File,
}

impl Drop for WorkflowProcessRegistrationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

impl FileProcessSupervisor {}

mod command_spec;
mod identity_persistence;
mod os_signals;
mod test_hooks;
mod workflow_registration;

use command_spec::*;
pub(crate) use identity_persistence::write_json_atomically;
use identity_persistence::*;
use os_signals::*;
#[cfg(test)]
pub(crate) use test_hooks::install_after_process_enumeration_hook;
use test_hooks::*;
pub(crate) use workflow_registration::acquire_workflow_process_registration_lock;
pub use workflow_registration::managed_pid_is_alive;
use workflow_registration::*;

mod execution;
mod output;
mod registry;
mod supervision;
mod termination;
#[cfg(test)]
mod tests;
