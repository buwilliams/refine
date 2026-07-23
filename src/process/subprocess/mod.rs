use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationLaunchGuard};
use crate::process::supervisor::security::FileSecurityService;

const PROCESS_IDENTITIES_DIR: &str = "process-identities";

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
    pub waited_ms: u128,
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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessPauseState {
    pub background_processes_stopped: bool,
    pub agents_paused: bool,
}

pub trait ProcessSupervisor {
    fn launch(&self, spec: ManagedProcessSpec) -> RefineResult<ManagedProcess>;
    fn signal(&self, process_id: &str, signal: &str) -> RefineResult<ManagedProcess>;
    fn wait(&self, process_id: &str) -> RefineResult<ManagedProcess>;
    fn stream(&self, process_id: &str) -> RefineResult<String>;
    fn inspect(&self, process_id: &str) -> RefineResult<ManagedProcess>;
    fn cleanup(&self, process_id: &str) -> RefineResult<()>;
    fn recover(&self) -> RefineResult<Vec<ManagedProcess>>;
}

#[derive(Clone, Debug)]
pub struct FileProcessSupervisor {
    pub runtime_root: PathBuf,
    pub allowed_commands: BTreeSet<String>,
}

impl FileProcessSupervisor {
    /// Requests termination without removing the managed-process record. The process runner owns
    /// final reaping and artifact cleanup, so callers can keep capacity reserved until the real
    /// child has exited.
    pub fn request_termination(
        &self,
        process_id: &str,
        signal: &str,
    ) -> RefineResult<ManagedProcess> {
        if !matches!(signal, "stop" | "terminate" | "kill") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported termination signal {signal}"
            )));
        }
        let process = self.inspect(process_id)?;
        if let Some(pid) = process.pid {
            let _ = signal_os_process(pid, signal, process_owns_group(&process))?;
        }
        // The existing durable record remains authoritative while the runner reaps the child.
        // Rewriting it here can resurrect a stale running record if reaping removes the file
        // between signal delivery and this call.
        Ok(process)
    }

    pub fn process_is_alive(process: &ManagedProcess) -> RefineResult<bool> {
        process
            .pid
            .map(pid_alive)
            .transpose()
            .map(|alive| alive.unwrap_or(false))
    }

    /// Terminates a managed process and does not return until its recorded PID is confirmed dead.
    ///
    /// Recovery callers must retain the process record until exit is known: removing registration
    /// before that point would make an orphaned worker invisible and could allow overlapping work.
    pub fn terminate_and_confirm_exit(
        &self,
        process: &ManagedProcess,
        timeout: Duration,
    ) -> RefineResult<()> {
        if !Self::process_is_alive(process)? {
            return Ok(());
        }
        if let Err(error) = self.request_termination(&process.id, "terminate") {
            if !Self::process_is_alive(process)? {
                return Ok(());
            }
            return Err(error);
        }

        let deadline = Instant::now() + timeout;
        while Self::process_is_alive(process)? {
            if Instant::now() >= deadline {
                return Err(RefineError::Degraded(format!(
                    "managed process {} did not exit within {} ms after termination",
                    process.id,
                    timeout.as_millis()
                )));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Signals the exact process registration supplied by the caller and waits until that owned
    /// OS process exits. PID reuse, registry replacement, and delayed termination are causal
    /// failures: the process record and its identity evidence remain available for recovery.
    pub fn terminate_owned_and_confirm_exit(
        &self,
        expected: &ManagedProcess,
        signal: &str,
        timeout: Duration,
    ) -> RefineResult<ConfirmedProcessExit> {
        if !matches!(signal, "stop" | "terminate" | "kill") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported termination signal {signal}"
            )));
        }
        let started = Instant::now();
        let identity = self.ensure_process_identity(expected)?;
        self.ensure_expected_registration(expected, &identity)?;
        match self.owned_process_state(expected, &identity)? {
            OwnedProcessState::Exited => {
                let _ = self.recover();
                return Ok(confirmed_process_exit(expected, signal, &identity, started));
            }
            OwnedProcessState::IdentityMismatch(actual) => {
                return Err(process_identity_mismatch(
                    expected,
                    &identity,
                    actual.as_deref(),
                ));
            }
            OwnedProcessState::Alive => {}
        }

        if let Some(pid) = expected.pid
            && let Some(message) =
                signal_os_process(pid, signal, process_owns_group(expected))?
        {
            match self.owned_process_state(expected, &identity)? {
                OwnedProcessState::Exited => {}
                OwnedProcessState::IdentityMismatch(actual) => {
                    return Err(process_identity_mismatch(
                        expected,
                        &identity,
                        actual.as_deref(),
                    ));
                }
                OwnedProcessState::Alive => {
                    return Err(RefineError::Degraded(format!(
                        "failed to signal managed process {}: {message}; its process record and identity evidence were retained for recovery",
                        expected.id
                    )));
                }
            }
        }

        let deadline = Instant::now() + timeout;
        loop {
            match self.owned_process_state(expected, &identity)? {
                OwnedProcessState::Exited => {
                    let _ = self.recover();
                    return Ok(confirmed_process_exit(expected, signal, &identity, started));
                }
                OwnedProcessState::IdentityMismatch(actual) => {
                    return Err(process_identity_mismatch(
                        expected,
                        &identity,
                        actual.as_deref(),
                    ));
                }
                OwnedProcessState::Alive if Instant::now() >= deadline => {
                    return Err(RefineError::Degraded(format!(
                        "managed process {} did not exit within {} ms after {signal}; its process record and identity evidence were retained for recovery",
                        expected.id,
                        timeout.as_millis()
                    )));
                }
                OwnedProcessState::Alive => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            allowed_commands: BTreeSet::new(),
        }
    }

    pub fn with_allowed_commands(
        runtime_root: impl Into<PathBuf>,
        allowed_commands: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            allowed_commands: allowed_commands
                .into_iter()
                .map(|command| command.into())
                .collect(),
        }
    }

    pub fn processes_dir(&self) -> PathBuf {
        self.runtime_root.join("processes")
    }

    /// Hold transient process artifacts while a workflow-owned consumer finishes reading them.
    ///
    /// The filesystem lock is released automatically if the consumer exits, so later recovery can
    /// still remove abandoned artifacts.
    pub(crate) fn begin_artifact_handoff(&self, process_id: &str) -> RefineResult<fs::File> {
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let path = self.artifact_handoff_path(process_id);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open process artifact handoff {}: {error}",
                    path.display()
                ))
            })?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to lock process artifact handoff {}: {error}",
                path.display()
            ))
        })?;
        Ok(file)
    }

    pub(crate) fn finish_artifact_handoff(&self, handoff: fs::File) -> RefineResult<()> {
        drop(handoff);
        Ok(())
    }

    pub(crate) fn artifact_handoff_path(&self, process_id: &str) -> PathBuf {
        self.processes_dir()
            .join(format!("{process_id}.artifact-handoff.lock"))
    }

    fn process_identities_dir(&self) -> PathBuf {
        self.runtime_root.join(PROCESS_IDENTITIES_DIR)
    }

    fn process_identity_path(&self, process_id: &str) -> PathBuf {
        self.process_identities_dir()
            .join(format!("{process_id}.json"))
    }

    pub fn pause_state_path(&self) -> PathBuf {
        self.runtime_root.join("process-control.json")
    }

    pub fn list(&self) -> RefineResult<Vec<ManagedProcess>> {
        let dir = self.processes_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut processes = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to read process registry {}: {error}",
                dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!("failed to inspect process registry entry: {error}"))
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                if is_stale_process_temp(&path) {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(RefineError::Io(format!(
                                "failed to remove stale process temp {}: {error}",
                                path.display()
                            )));
                        }
                    }
                }
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read process {}: {error}",
                    path.display()
                ))
            })?;
            match serde_json::from_slice::<ManagedProcess>(&bytes) {
                Ok(process) if process.state == "running" => processes.push(process),
                Ok(process) => {
                    self.remove_process_artifacts(&process)?;
                }
                Err(_) if bytes.is_empty() => continue,
                Err(_) => continue,
            }
        }
        processes.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(processes)
    }

    pub fn recover_owner(&self, owner: ProcessOwner) -> RefineResult<Vec<ManagedProcess>> {
        let mut recovered = Vec::new();
        for mut process in self.list()? {
            if process.owner == owner
                && process.state == "running"
                && !self.recover_running_process(&mut process)?
            {
                continue;
            }
            recovered.push(process);
        }
        Ok(recovered)
    }

    pub fn pause_state(&self) -> RefineResult<ProcessPauseState> {
        let path = self.pause_state_path();
        if !path.exists() {
            return Ok(ProcessPauseState::default());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read process control {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse process control {}: {error}",
                path.display()
            ))
        })
    }

    pub fn set_background_processes_stopped(
        &self,
        stopped: bool,
    ) -> RefineResult<ProcessPauseState> {
        let mut state = self.pause_state()?;
        state.background_processes_stopped = stopped;
        self.write_pause_state(&state)?;
        Ok(state)
    }

    pub fn set_agents_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        let mut state = self.pause_state()?;
        state.agents_paused = paused;
        self.write_pause_state(&state)?;
        Ok(state)
    }

    fn write_pause_state(&self, state: &ProcessPauseState) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process control: {error}"))
        })?;
        let path = self.pause_state_path();
        write_json_atomically(&path, &encoded, "process control")
    }

    fn write_process(&self, process: &ManagedProcess) -> RefineResult<()> {
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let path = self.processes_dir().join(format!("{}.json", process.id));
        let encoded = serde_json::to_vec_pretty(process).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process: {error}"))
        })?;
        write_json_atomically(&path, &encoded, "process")
    }

    fn write_process_identity(&self, identity: &ManagedProcessIdentity) -> RefineResult<()> {
        fs::create_dir_all(self.process_identities_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process identity registry {}: {error}",
                self.process_identities_dir().display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(identity).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process identity: {error}"))
        })?;
        write_json_atomically(
            &self.process_identity_path(&identity.process_id),
            &encoded,
            "process identity",
        )
    }

    fn ensure_process_identity(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<ManagedProcessIdentity> {
        let path = self.process_identity_path(&process.id);
        match fs::read(&path) {
            Ok(bytes) => {
                let identity =
                    serde_json::from_slice::<ManagedProcessIdentity>(&bytes).map_err(|error| {
                        RefineError::Serialization(format!(
                            "failed to parse process identity {}: {error}",
                            path.display()
                        ))
                    })?;
                Ok(identity)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let identity = ManagedProcessIdentity {
                    process_id: process.id.clone(),
                    owner: process.owner.clone(),
                    pid: process.pid,
                    os_identity: process.pid.map(os_process_identity).transpose()?.flatten(),
                    registered_at: now_millis_string(),
                };
                self.write_process_identity(&identity)?;
                Ok(identity)
            }
            Err(error) => Err(RefineError::Io(format!(
                "failed to read process identity {}: {error}",
                path.display()
            ))),
        }
    }

    fn ensure_expected_registration(
        &self,
        expected: &ManagedProcess,
        identity: &ManagedProcessIdentity,
    ) -> RefineResult<()> {
        if identity.process_id != expected.id
            || identity.owner != expected.owner
            || identity.pid != expected.pid
        {
            return Err(RefineError::Conflict(format!(
                "managed process {} identity evidence does not match its registry record; termination was not requested and both records were retained for recovery",
                expected.id
            )));
        }
        match self.inspect(&expected.id) {
            Ok(current)
                if current.id == expected.id
                    && current.owner == expected.owner
                    && current.pid == expected.pid
                    && current.started_at == expected.started_at =>
            {
                Ok(())
            }
            Ok(_) => Err(RefineError::Conflict(format!(
                "managed process {} registry identity changed before termination; termination was not requested and current evidence was retained for recovery",
                expected.id
            ))),
            Err(RefineError::NotFound(_)) => match self.owned_process_state(expected, identity)? {
                OwnedProcessState::Exited => Ok(()),
                OwnedProcessState::Alive | OwnedProcessState::IdentityMismatch(_) => {
                    Err(RefineError::Conflict(format!(
                        "managed process {} is alive without its expected registry record; termination was not requested and identity evidence was retained for recovery",
                        expected.id
                    )))
                }
            },
            Err(error) => Err(error),
        }
    }

    fn owned_process_state(
        &self,
        process: &ManagedProcess,
        identity: &ManagedProcessIdentity,
    ) -> RefineResult<OwnedProcessState> {
        let Some(pid) = process.pid else {
            return Ok(OwnedProcessState::Exited);
        };
        if !pid_alive(pid)? {
            return Ok(OwnedProcessState::Exited);
        }
        let actual = os_process_identity(pid)?;
        match (&identity.os_identity, &actual) {
            (Some(expected), Some(actual)) if expected != actual => {
                Ok(OwnedProcessState::IdentityMismatch(Some(actual.clone())))
            }
            (Some(_), None) => Ok(OwnedProcessState::IdentityMismatch(None)),
            _ => Ok(OwnedProcessState::Alive),
        }
    }

    fn remove_process_artifacts(&self, process: &ManagedProcess) -> RefineResult<()> {
        let handoff_path = self.artifact_handoff_path(&process.id);
        // A live workflow consumer owns the transcript through this lease. Reconciliation may
        // already persist a truthful terminal state, but deletion waits until consumption ends.
        let handoff = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&handoff_path)
        {
            Ok(file) => match file.try_lock_exclusive() {
                Ok(()) => Some(file),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to lock process artifact handoff {} for cleanup: {error}",
                        handoff_path.display()
                    )));
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to open process artifact handoff {} for cleanup: {error}",
                    handoff_path.display()
                )));
            }
        };
        for path in [
            process.stdout_path.as_deref(),
            process.stderr_path.as_deref(),
            process.stdin_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove process artifact {path}: {error}"
                    )));
                }
            }
        }
        if let Some(handoff) = handoff {
            drop(handoff);
            match fs::remove_file(&handoff_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove process artifact handoff {}: {error}",
                        handoff_path.display()
                    )));
                }
            }
        }
        let path = self.processes_dir().join(format!("{}.json", process.id));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove process {}: {error}",
                    path.display()
                )));
            }
        }
        let identity_path = self.process_identity_path(&process.id);
        match fs::remove_file(&identity_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove process identity {}: {error}",
                    identity_path.display()
                )));
            }
        }
        Ok(())
    }

    pub fn register(&self, process: ManagedProcess) -> RefineResult<ManagedProcess> {
        if process.state != "running" {
            self.write_process(&process)?;
            self.remove_process_artifacts(&process)?;
            return Ok(process);
        }
        self.write_process(&process)?;
        if let Err(error) = self.ensure_process_identity(&process) {
            let _ = self.remove_process_artifacts(&process);
            return Err(error);
        }
        Ok(process)
    }

    /// Apply the same pause and host-command authorization gates used by ordinary managed
    /// processes before an interactive PTY process is spawned by a surface adapter.
    pub fn validate_interactive_launch(&self, spec: &ManagedProcessSpec) -> RefineResult<()> {
        self.validate_launch(spec)
    }

    pub fn run_to_completion(
        &self,
        spec: ManagedProcessSpec,
    ) -> RefineResult<ManagedProcessOutput> {
        self.run_to_completion_with_output(spec, |_, _| {})
    }

    pub fn run_to_completion_with_output<F>(
        &self,
        spec: ManagedProcessSpec,
        mut on_output: F,
    ) -> RefineResult<ManagedProcessOutput>
    where
        F: FnMut(ManagedProcessOutputStream, &[u8]),
    {
        self.validate_launch(&spec)?;
        let launch_guard = self.operation_launch_guard(&spec)?;
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let process_id = new_process_id();
        let stdout_path = self
            .processes_dir()
            .join(format!("{process_id}.stdout.log"));
        let stderr_path = self
            .processes_dir()
            .join(format!("{process_id}.stderr.log"));

        let mut command = process_command(&spec);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }

        let mut child = command.spawn().map_err(|error| {
            RefineError::Io(format!(
                "failed to launch managed process {}: {error}",
                spec.command
            ))
        })?;
        let stdin_path = if let Some(stdin) = spec.stdin.as_deref() {
            let path = self.processes_dir().join(format!("{process_id}.stdin.txt"));
            if !spec.sensitive {
                fs::write(&path, stdin).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to write process stdin {}: {error}",
                        path.display()
                    ))
                })?;
            }
            if let Some(mut child_stdin) = child.stdin.take() {
                child_stdin.write_all(stdin.as_bytes()).map_err(|error| {
                    RefineError::Io(format!("failed to send managed process stdin: {error}"))
                })?;
            }
            if spec.sensitive {
                None
            } else {
                Some(path.display().to_string())
            }
        } else {
            None
        };
        let mut process = ManagedProcess {
            id: process_id,
            owner: spec.owner.clone(),
            pid: Some(child.id()),
            state: "running".to_string(),
            label: Some(spec.command.clone()),
            details: Some(process_details(&spec)),
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path,
            limits: spec.limits,
            started_at: now_millis_string(),
            exit_code: None,
        };
        if let Err(error) = self.write_process(&process) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = self.ensure_process_identity(&process) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = self.remove_process_artifacts(&process);
            return Err(error);
        }
        drop(launch_guard);

        let stdout = child.stdout.take().ok_or_else(|| {
            RefineError::Io(format!(
                "managed process {} did not expose stdout",
                process.id
            ))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            RefineError::Io(format!(
                "managed process {} did not expose stderr",
                process.id
            ))
        })?;
        let (tx, rx) = mpsc::channel();
        let stdout_thread =
            spawn_output_reader(stdout, ManagedProcessOutputStream::Stdout, tx.clone());
        let stderr_thread = spawn_output_reader(stderr, ManagedProcessOutputStream::Stderr, tx);
        let mut stdout_file = fs::File::create(&stdout_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stdout log {}: {error}",
                stdout_path.display()
            ))
        })?;
        let mut stderr_file = fs::File::create(&stderr_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stderr log {}: {error}",
                stderr_path.display()
            ))
        })?;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut reader_done = 0usize;
        let mut reader_error = None;
        let mut status = None;
        while reader_done < 2 || status.is_none() {
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(ProcessOutputEvent::Chunk { stream, bytes }) => {
                    match stream {
                        ManagedProcessOutputStream::Stdout => {
                            stdout_file.write_all(&bytes).map_err(|error| {
                                RefineError::Io(format!(
                                    "failed to write process stdout log {}: {error}",
                                    stdout_path.display()
                                ))
                            })?;
                            stdout_bytes.extend_from_slice(&bytes);
                        }
                        ManagedProcessOutputStream::Stderr => {
                            stderr_file.write_all(&bytes).map_err(|error| {
                                RefineError::Io(format!(
                                    "failed to write process stderr log {}: {error}",
                                    stderr_path.display()
                                ))
                            })?;
                            stderr_bytes.extend_from_slice(&bytes);
                        }
                    }
                    on_output(stream, &bytes);
                }
                Ok(ProcessOutputEvent::Done) => reader_done += 1,
                Ok(ProcessOutputEvent::Error(error)) => {
                    reader_done += 1;
                    if reader_error.is_none() {
                        reader_error = Some(error);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if status.is_none() {
                status = child.try_wait().map_err(|error| {
                    RefineError::Io(format!(
                        "failed to inspect managed process {}: {error}",
                        process.id
                    ))
                })?;
            }
        }
        let status = match status {
            Some(status) => status,
            None => child.wait().map_err(|error| {
                RefineError::Io(format!(
                    "failed to wait for managed process {}: {error}",
                    process.id
                ))
            })?,
        };
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        stdout_file.flush().map_err(|error| {
            RefineError::Io(format!(
                "failed to flush process stdout log {}: {error}",
                stdout_path.display()
            ))
        })?;
        stderr_file.flush().map_err(|error| {
            RefineError::Io(format!(
                "failed to flush process stderr log {}: {error}",
                stderr_path.display()
            ))
        })?;
        process.state = if status.success() {
            "exited".to_string()
        } else {
            "failed".to_string()
        };
        process.exit_code = status.code();
        self.remove_process_artifacts(&process)?;
        if let Some(error) = reader_error {
            return Err(error);
        }
        Ok(ManagedProcessOutput {
            process,
            stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        })
    }

    fn validate_launch(&self, spec: &ManagedProcessSpec) -> RefineResult<()> {
        let pause_state = self.pause_state()?;
        if pause_state.agents_paused && spec.owner == ProcessOwner::Agent {
            return Err(RefineError::Conflict(
                "agent process launch is paused".to_string(),
            ));
        }
        if pause_state.background_processes_stopped && is_background_owner(&spec.owner) {
            return Err(RefineError::Conflict(
                "background process launch is stopped".to_string(),
            ));
        }
        if spec.command.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "managed process command is required".to_string(),
            ));
        }
        let authorization_command = spec
            .authorization_command
            .clone()
            .unwrap_or_else(|| process_command_line(spec));
        FileSecurityService::with_allowed_commands(
            &self.runtime_root,
            self.allowed_commands.iter().cloned(),
        )
        .authorize_host_command("process_supervisor", &authorization_command)
    }

    fn operation_launch_guard(
        &self,
        spec: &ManagedProcessSpec,
    ) -> RefineResult<Option<OperationLaunchGuard>> {
        let Some(operation_id) = spec
            .metadata
            .get("operation_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        FileOperationRegistry::new(&self.runtime_root)
            .active_launch_guard(operation_id)
            .map(Some)
    }
}

pub fn managed_pid_is_alive(pid: u32) -> RefineResult<bool> {
    pid_alive(pid)
}

impl ProcessSupervisor for FileProcessSupervisor {
    fn launch(&self, spec: ManagedProcessSpec) -> RefineResult<ManagedProcess> {
        self.validate_launch(&spec)?;
        let launch_guard = self.operation_launch_guard(&spec)?;
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let process_id = new_process_id();
        let stdout_path = self
            .processes_dir()
            .join(format!("{process_id}.stdout.log"));
        let stderr_path = self
            .processes_dir()
            .join(format!("{process_id}.stderr.log"));
        let stdout = fs::File::create(&stdout_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stdout log {}: {error}",
                stdout_path.display()
            ))
        })?;
        let stderr = fs::File::create(&stderr_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stderr log {}: {error}",
                stderr_path.display()
            ))
        })?;

        let mut command = process_command(&spec);
        command.stdout(Stdio::from(stdout));
        command.stderr(Stdio::from(stderr));
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }

        let mut child = command.spawn().map_err(|error| {
            RefineError::Io(format!(
                "failed to launch managed process {}: {error}",
                spec.command
            ))
        })?;
        let stdin_path = if let Some(stdin) = spec.stdin.as_deref() {
            let path = self.processes_dir().join(format!("{process_id}.stdin.txt"));
            if !spec.sensitive {
                fs::write(&path, stdin).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to write process stdin {}: {error}",
                        path.display()
                    ))
                })?;
            }
            if let Some(mut child_stdin) = child.stdin.take() {
                child_stdin.write_all(stdin.as_bytes()).map_err(|error| {
                    RefineError::Io(format!("failed to send managed process stdin: {error}"))
                })?;
            }
            if spec.sensitive {
                None
            } else {
                Some(path.display().to_string())
            }
        } else {
            None
        };

        let process = ManagedProcess {
            id: process_id,
            owner: spec.owner.clone(),
            pid: Some(child.id()),
            state: "running".to_string(),
            label: Some(spec.command.clone()),
            details: Some(process_details(&spec)),
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path,
            limits: spec.limits,
            started_at: now_millis_string(),
            exit_code: None,
        };
        if let Err(error) = self.write_process(&process) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = self.ensure_process_identity(&process) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = self.remove_process_artifacts(&process);
            return Err(error);
        }
        drop(launch_guard);
        let reaper = self.clone();
        let reaped_process = process.clone();
        std::thread::spawn(move || {
            let mut reaped_process = reaped_process;
            match child.wait() {
                Ok(status) => {
                    reaped_process.state = if status.success() {
                        "exited".to_string()
                    } else {
                        "failed".to_string()
                    };
                    reaped_process.exit_code = status.code();
                    let _ = reaper.remove_process_artifacts(&reaped_process);
                }
                Err(error) => {
                    reaped_process.state = "failed".to_string();
                    reaped_process.details = Some(append_detail(
                        reaped_process.details.take(),
                        &format!("failed to reap managed process: {error}"),
                    ));
                    let _ = reaper.remove_process_artifacts(&reaped_process);
                }
            }
        });
        Ok(process)
    }

    fn signal(&self, process_id: &str, signal: &str) -> RefineResult<ManagedProcess> {
        let mut process = self.inspect(process_id)?;
        if matches!(signal, "stop" | "terminate" | "kill") {
            if let Some(pid) = process.pid
                && let Some(message) = signal_os_process(pid, signal, process_owns_group(&process))?
            {
                process.details = Some(match process.details {
                    Some(details) if !details.trim().is_empty() => {
                        format!("{details}; {message}")
                    }
                    _ => message,
                });
            }
            process.state = "stopped".to_string();
            self.write_process(&process)?;
            self.remove_process_artifacts(&process)?;
        } else {
            process.state = format!("signalled:{signal}");
            self.write_process(&process)?;
        }
        Ok(process)
    }

    fn wait(&self, process_id: &str) -> RefineResult<ManagedProcess> {
        let mut process = self.inspect(process_id)?;
        if process.state == "running"
            && let Some(pid) = process.pid
            && !pid_alive(pid)?
        {
            process.state = "exited".to_string();
            self.write_process(&process)?;
            self.remove_process_artifacts(&process)?;
        }
        Ok(process)
    }

    fn stream(&self, process_id: &str) -> RefineResult<String> {
        let process = self.inspect(process_id)?;
        let mut output = String::new();
        if let Some(path) = process.stdout_path.as_deref() {
            append_stream_file(&mut output, "stdout", path)?;
        }
        if let Some(path) = process.stderr_path.as_deref() {
            append_stream_file(&mut output, "stderr", path)?;
        }
        if output.trim().is_empty() {
            Ok(format!("No captured output for process {process_id}."))
        } else {
            Ok(output)
        }
    }

    fn inspect(&self, process_id: &str) -> RefineResult<ManagedProcess> {
        let path = self.processes_dir().join(format!("{process_id}.json"));
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                return RefineError::NotFound(format!("Process {process_id} was not found"));
            }
            RefineError::Io(format!(
                "failed to read process {}: {error}",
                path.display()
            ))
        })?;
        let process = serde_json::from_slice::<ManagedProcess>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse process {}: {error}",
                path.display()
            ))
        })?;
        if process.state != "running" {
            self.remove_process_artifacts(&process)?;
            return Err(RefineError::NotFound(format!(
                "Process {process_id} was not found"
            )));
        }
        Ok(process)
    }

    fn cleanup(&self, process_id: &str) -> RefineResult<()> {
        if let Ok(process) = self.inspect(process_id) {
            let _ = self.signal(process_id, "terminate");
            self.remove_process_artifacts(&process)?;
        }
        Ok(())
    }

    fn recover(&self) -> RefineResult<Vec<ManagedProcess>> {
        let mut recovered = Vec::new();
        for mut process in self.list()? {
            if process.state == "running" && !self.recover_running_process(&mut process)? {
                continue;
            }
            recovered.push(process);
        }
        Ok(recovered)
    }
}

enum ProcessOutputEvent {
    Chunk {
        stream: ManagedProcessOutputStream,
        bytes: Vec<u8>,
    },
    Done,
    Error(RefineError),
}

fn spawn_output_reader<R>(
    mut reader: R,
    stream: ManagedProcessOutputStream,
    tx: mpsc::Sender<ProcessOutputEvent>,
) -> std::thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = tx.send(ProcessOutputEvent::Done);
                    return;
                }
                Ok(read) => {
                    let _ = tx.send(ProcessOutputEvent::Chunk {
                        stream,
                        bytes: buffer[..read].to_vec(),
                    });
                }
                Err(error) => {
                    let _ = tx.send(ProcessOutputEvent::Error(RefineError::Io(format!(
                        "failed to read managed process {stream:?}: {error}"
                    ))));
                    return;
                }
            }
        }
    })
}

impl FileProcessSupervisor {
    fn recover_running_process(&self, process: &mut ManagedProcess) -> RefineResult<bool> {
        match process.pid {
            Some(pid) if pid_alive(pid)? => {}
            Some(_) => {
                process.state = "exited".to_string();
                process.details = Some(append_detail(
                    process.details.take(),
                    "process was not alive during recovery",
                ));
                self.write_process(process)?;
                self.remove_process_artifacts(process)?;
                return Ok(false);
            }
            None => {
                process.state = "interrupted".to_string();
                process.details = Some(append_detail(
                    process.details.take(),
                    "running process had no pid during recovery",
                ));
                self.write_process(process)?;
                self.remove_process_artifacts(process)?;
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn is_background_owner(owner: &ProcessOwner) -> bool {
    matches!(
        owner,
        ProcessOwner::Runner
            | ProcessOwner::TargetApp
            | ProcessOwner::Agent
            | ProcessOwner::Quality
            | ProcessOwner::Import
            | ProcessOwner::Maintenance
    )
}

fn process_command_line(spec: &ManagedProcessSpec) -> String {
    std::iter::once(spec.command.as_str())
        .chain(spec.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn process_details(spec: &ManagedProcessSpec) -> String {
    if spec.sensitive {
        return "redacted".to_string();
    }
    if !spec.metadata.is_empty() {
        let mut details = spec.metadata.clone();
        details
            .entry("command".to_string())
            .or_insert_with(|| json!(process_command_line(spec)));
        return serde_json::to_string(&details).unwrap_or_else(|_| spec.args.join(" "));
    }
    spec.args.join(" ")
}

fn process_command(spec: &ManagedProcessSpec) -> Command {
    let mut command = Command::new(&spec.command);
    command.args(&spec.args);
    if let Some(cwd) = spec.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
        command.current_dir(cwd);
    }
    command.envs(spec.env.iter().map(|(key, value)| (key, value)));
    if spec.owner == ProcessOwner::Agent {
        for key in AGENT_DIRECT_API_KEY_ENV {
            command.env_remove(key);
        }
    }
    configure_process_lifecycle(&mut command, spec);
    command
}

#[cfg(unix)]
fn configure_process_lifecycle(command: &mut Command, spec: &ManagedProcessSpec) {
    use std::os::unix::process::CommandExt;

    unsafe extern "C" {
        fn setsid() -> i32;
    }

    let detached_daemon = spec.owner == ProcessOwner::Daemon;
    let isolated_process_group = spec
        .metadata
        .get("isolated_process_group")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let kill_on_parent_exit = spec
        .limits
        .as_ref()
        .is_some_and(|limits| limits.kill_on_parent_exit);
    if !detached_daemon && !kill_on_parent_exit && !isolated_process_group {
        return;
    }
    unsafe {
        command.pre_exec(move || {
            if (detached_daemon || isolated_process_group) && setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            if kill_on_parent_exit {
                const PR_SET_PDEATHSIG: i32 = 1;
                const SIGTERM: i32 = 15;
                unsafe extern "C" {
                    fn prctl(option: i32, arg2: usize, ...) -> i32;
                    fn getppid() -> i32;
                }
                if prctl(PR_SET_PDEATHSIG, SIGTERM as usize) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if getppid() == 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "managed process parent exited during launch",
                    ));
                }
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_process_lifecycle(_command: &mut Command, _spec: &ManagedProcessSpec) {}

const AGENT_DIRECT_API_KEY_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_API_KEY",
    "CODEX_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_GENAI_API_KEY",
    "OPENAI_API_KEY",
];

fn process_isolation_label(limits: Option<&ProcessResourceLimits>) -> &'static str {
    if limits.is_some() {
        "requested"
    } else {
        "best_effort"
    }
}

fn process_actions(state: &str) -> Vec<&'static str> {
    if state == "running" {
        vec!["terminate", "kill"]
    } else {
        vec!["cleanup"]
    }
}

fn append_detail(existing: Option<String>, message: &str) -> String {
    match existing {
        Some(existing) if !existing.trim().is_empty() => format!("{existing}; {message}"),
        _ => message.to_string(),
    }
}

fn is_stale_process_temp(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !file_name.starts_with('.') || path.extension().and_then(|ext| ext.to_str()) != Some("tmp") {
        return false;
    }
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > Duration::from_secs(30))
}

fn now_millis_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn write_json_atomically(path: &Path, encoded: &[u8], label: &str) -> RefineResult<()> {
    let Some(parent) = path.parent() else {
        return Err(RefineError::Io(format!(
            "failed to write {label} {}: path has no parent",
            path.display()
        )));
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", new_process_id()));
    {
        let mut tmp = fs::File::create(&tmp_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create {label} temp file {}: {error}",
                tmp_path.display()
            ))
        })?;
        tmp.write_all(encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write {label} temp file {}: {error}",
                tmp_path.display()
            ))
        })?;
        tmp.sync_all().map_err(|error| {
            RefineError::Io(format!(
                "failed to sync {label} temp file {}: {error}",
                tmp_path.display()
            ))
        })?;
    }
    fs::rename(&tmp_path, path).map_err(|error| {
        let _ = fs::remove_file(&tmp_path);
        RefineError::Io(format!(
            "failed to write {label} {}: {error}",
            path.display()
        ))
    })
}

fn new_process_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "proc-{}-{}-{}",
        now.as_millis(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum OwnedProcessState {
    Alive,
    Exited,
    IdentityMismatch(Option<String>),
}

fn confirmed_process_exit(
    process: &ManagedProcess,
    signal: &str,
    identity: &ManagedProcessIdentity,
    started: Instant,
) -> ConfirmedProcessExit {
    ConfirmedProcessExit {
        process_id: process.id.clone(),
        pid: process.pid,
        signal: signal.to_string(),
        os_identity: identity.os_identity.clone(),
        confirmed_exit: true,
        registry_retained_until_exit: true,
        waited_ms: started.elapsed().as_millis(),
    }
}

fn process_identity_mismatch(
    process: &ManagedProcess,
    expected: &ManagedProcessIdentity,
    actual: Option<&str>,
) -> RefineError {
    RefineError::Conflict(format!(
        "managed process {} PID identity mismatch (expected {}, observed {}); termination was not requested, and its process record and identity evidence were retained for recovery",
        process.id,
        expected.os_identity.as_deref().unwrap_or("unavailable"),
        actual.unwrap_or("unavailable")
    ))
}

#[cfg(target_os = "linux")]
fn os_process_identity(pid: u32) -> RefineResult<Option<String>> {
    let stat_path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = match fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read process identity {}: {error}",
                stat_path.display()
            )));
        }
    };
    let end = stat.rfind(')').ok_or_else(|| {
        RefineError::Serialization(format!(
            "failed to parse process identity {}: missing command terminator",
            stat_path.display()
        ))
    })?;
    let start_ticks = stat[end + 1..].split_whitespace().nth(19).ok_or_else(|| {
        RefineError::Serialization(format!(
            "failed to parse process identity {}: missing start time",
            stat_path.display()
        ))
    })?;
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(Some(format!("linux:{boot_id}:{start_ticks}")))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn os_process_identity(pid: u32) -> RefineResult<Option<String>> {
    let output = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect process identity {pid} with ps: {error}"
            ))
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let started = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!started.is_empty()).then(|| format!("unix:{started}")))
}

#[cfg(windows)]
fn os_process_identity(pid: u32) -> RefineResult<Option<String>> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).StartTime.Ticks"),
        ])
        .output()
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect process identity {pid} with PowerShell: {error}"
            ))
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let started = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!started.is_empty()).then(|| format!("windows:{started}")))
}

fn signal_os_process(pid: u32, signal: &str, process_group: bool) -> RefineResult<Option<String>> {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.arg("/PID").arg(pid.to_string());
        if process_group {
            command.arg("/T");
        }
        if signal == "kill" {
            command.arg("/F");
        }
        let status = command.status().map_err(|error| {
            RefineError::Io(format!(
                "failed to signal process {pid} with taskkill: {error}"
            ))
        })?;
        if status.success() {
            Ok(None)
        } else {
            Ok(Some(format!(
                "taskkill returned {status}; process may already have exited"
            )))
        }
    }
    #[cfg(not(windows))]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        const SIGKILL: i32 = 9;
        let target = if process_group {
            -(pid as i32)
        } else {
            pid as i32
        };
        let os_signal = if signal == "kill" { SIGKILL } else { SIGTERM };
        if unsafe { kill(target, os_signal) } == 0 {
            Ok(None)
        } else {
            let error = std::io::Error::last_os_error();
            Ok(Some(format!(
                "kill signal {os_signal} returned {error}; process may already have exited"
            )))
        }
    }
}

fn process_owns_group(process: &ManagedProcess) -> bool {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| {
            details
                .get("isolated_process_group")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn pid_alive(pid: u32) -> RefineResult<bool> {
    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .arg("/FI")
            .arg(format!("PID eq {pid}"))
            .output()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect process {pid} with tasklist: {error}"
                ))
            })?;
        Ok(String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
    }
    #[cfg(not(windows))]
    {
        if pid > i32::MAX as u32 {
            return Ok(false);
        }
        if unix_pid_is_zombie(pid)? {
            return Ok(false);
        }
        let status = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect process {pid} with kill -0: {error}"
                ))
            })?;
        Ok(status.success())
    }
}

#[cfg(not(windows))]
fn unix_pid_is_zombie(pid: u32) -> RefineResult<bool> {
    let status_path = PathBuf::from(format!("/proc/{pid}/status"));
    let status = match fs::read_to_string(&status_path) {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to inspect process status {}: {error}",
                status_path.display()
            )));
        }
    };
    Ok(status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .is_some_and(|state| state.trim_start().starts_with('Z')))
}

fn append_stream_file(output: &mut String, label: &str, path: &str) -> RefineResult<()> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read process {label} stream {}: {error}",
            path.display()
        ))
    })?;
    if text.trim().is_empty() {
        return Ok(());
    }
    output.push_str(&format!("== {label} ==\n"));
    output.push_str(&tail_text(&text, 16_000));
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(())
}

fn tail_text(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        value.to_string()
    } else {
        value.chars().skip(count - max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn file_process_supervisor_tracks_running_processes_and_pause_state() {
        let temp_root = unique_temp_dir("processes");
        let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
        let process = supervisor
            .launch(ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command: shell_binary().to_string(),
                args: long_running_shell_args().to_vec(),
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: Some(ProcessResourceLimits {
                    max_memory_bytes: Some(512 * 1024 * 1024),
                    cpu_priority: Some("normal".to_string()),
                    kill_on_parent_exit: false,
                }),
                authorization_command: None,
                sensitive: false,
                metadata: Default::default(),
            })
            .unwrap();
        assert_eq!(supervisor.list().unwrap().len(), 1);
        assert_eq!(process.api_json()["kind"], "agent");
        assert_eq!(process.state, "running");

        let stopped = supervisor.set_background_processes_stopped(true).unwrap();
        assert!(stopped.background_processes_stopped);
        let paused = supervisor.set_agents_paused(true).unwrap();
        assert!(paused.agents_paused);
        assert!(supervisor.pause_state_path().exists());

        let stopped = supervisor.signal(&process.id, "stop").unwrap();
        assert_eq!(stopped.state, "stopped");
        assert!(supervisor.inspect(&process.id).is_err());
        assert_eq!(supervisor.list().unwrap().len(), 0);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_reaps_launched_processes_after_exit() {
        let temp_root = unique_temp_dir("process-reaper");
        let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
        let process = supervisor
            .launch(ManagedProcessSpec {
                owner: ProcessOwner::Runner,
                command: shell_binary().to_string(),
                args: shell_args("exit 0"),
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: None,
                sensitive: false,
                metadata: Default::default(),
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while supervisor
            .processes_dir()
            .join(format!("{}.json", process.id))
            .exists()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(supervisor.list().unwrap().is_empty());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_recovery_does_not_steal_completion_status() {
        let temp_root = unique_temp_dir("process-completion-recovery");
        let runtime_root = temp_root.join("run/8080");
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        let output = supervisor
            .run_to_completion_with_output(
                ManagedProcessSpec {
                    owner: ProcessOwner::Agent,
                    command: shell_binary().to_string(),
                    args: shell_args("printf recovered-output").to_vec(),
                    cwd: None,
                    env: Vec::new(),
                    stdin: None,
                    limits: None,
                    authorization_command: None,
                    sensitive: false,
                    metadata: Default::default(),
                },
                |_, _| {
                    let _ = FileProcessSupervisor::new(&runtime_root).recover();
                },
            )
            .unwrap();

        assert!(output.success());
        assert_eq!(output.stdout, "recovered-output");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_signals_registered_os_process() {
        let temp_root = unique_temp_dir("process-signal");
        let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let process = supervisor
            .register(ManagedProcess {
                id: "sleep-test".to_string(),
                owner: ProcessOwner::TargetApp,
                pid: Some(child.id()),
                state: "running".to_string(),
                label: Some("sleep".to_string()),
                details: None,
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();

        let stopped = supervisor.signal(&process.id, "kill").unwrap();
        assert_eq!(stopped.state, "stopped");
        assert!(supervisor.inspect(&process.id).is_err());
        for _ in 0..20 {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(child.try_wait().unwrap().is_some());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn requested_termination_keeps_registry_truth_until_process_exit() {
        let temp_root = unique_temp_dir("process-request-termination");
        let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080/agents"));
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let process = supervisor
            .register(ManagedProcess {
                id: "managed-agent-stop".to_string(),
                owner: ProcessOwner::Agent,
                pid: Some(child.id()),
                state: "running".to_string(),
                label: Some("sleep".to_string()),
                details: Some(json!({"session_id": "CHAT1"}).to_string()),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();

        let stopping = supervisor
            .request_termination(&process.id, "terminate")
            .unwrap();
        assert_eq!(stopping.state, "running");
        assert!(supervisor.inspect(&process.id).is_ok());
        for _ in 0..40 {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(child.try_wait().unwrap().is_some());
        assert!(!FileProcessSupervisor::process_is_alive(&stopping).unwrap());
        assert!(supervisor.recover().unwrap().is_empty());
        assert!(supervisor.inspect(&process.id).is_err());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_streams_registered_output_files() {
        let temp_root = unique_temp_dir("process-stream");
        let runtime_root = temp_root.join("run/8080");
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        let stdout_path = runtime_root.join("stdout.log");
        let stderr_path = runtime_root.join("stderr.log");
        fs::create_dir_all(&runtime_root).unwrap();
        fs::write(&stdout_path, "hello stdout\n").unwrap();
        fs::write(&stderr_path, "warn stderr\n").unwrap();
        supervisor
            .register(ManagedProcess {
                id: "stream-test".to_string(),
                owner: ProcessOwner::Agent,
                pid: None,
                state: "running".to_string(),
                label: Some("stream".to_string()),
                details: None,
                stdout_path: Some(stdout_path.display().to_string()),
                stderr_path: Some(stderr_path.display().to_string()),
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();

        let stream = supervisor.stream("stream-test").unwrap();
        assert!(stream.contains("hello stdout"));
        assert!(stream.contains("warn stderr"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_applies_pause_gates_and_recovers_stale_processes() {
        let temp_root = unique_temp_dir("process-recover");
        let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
        supervisor.set_agents_paused(true).unwrap();
        let rejected = supervisor.launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: shell_binary().to_string(),
            args: shell_args("printf blocked").to_vec(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        });
        assert!(rejected.is_err());
        supervisor.set_agents_paused(false).unwrap();

        supervisor
            .register(ManagedProcess {
                id: "stale".to_string(),
                owner: ProcessOwner::Maintenance,
                pid: None,
                state: "running".to_string(),
                label: Some("stale".to_string()),
                details: None,
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();
        let recovered = supervisor.recover().unwrap();
        assert!(recovered.is_empty());
        assert!(supervisor.inspect("stale").is_err());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_cleans_deferred_artifacts_after_handoff_release() {
        let temp_root = unique_temp_dir("process-artifact-handoff");
        let runtime_root = temp_root.join("run/8080");
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        let process_id = "workflow-owned-transcript";
        let stdout_path = supervisor
            .processes_dir()
            .join(format!("{process_id}.stdout.log"));
        fs::create_dir_all(supervisor.processes_dir()).unwrap();
        fs::write(&stdout_path, "workflow result").unwrap();
        let handoff = supervisor.begin_artifact_handoff(process_id).unwrap();
        supervisor
            .register(ManagedProcess {
                id: process_id.to_string(),
                owner: ProcessOwner::Agent,
                pid: None,
                state: "running".to_string(),
                label: Some("Goal Agent".to_string()),
                details: None,
                stdout_path: Some(stdout_path.display().to_string()),
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();

        assert!(supervisor.recover().unwrap().is_empty());
        assert!(stdout_path.is_file());
        let process_path = supervisor
            .processes_dir()
            .join(format!("{process_id}.json"));
        let reconciled: ManagedProcess =
            serde_json::from_slice(&fs::read(&process_path).unwrap()).unwrap();
        assert_eq!(reconciled.state, "interrupted");

        supervisor.finish_artifact_handoff(handoff).unwrap();
        assert!(supervisor.recover().unwrap().is_empty());
        assert!(!stdout_path.exists());
        assert!(!process_path.exists());
        assert!(!supervisor.artifact_handoff_path(process_id).exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_enforces_allowed_commands() {
        let temp_root = unique_temp_dir("process-allowed");
        let runtime_root = temp_root.join("run/8080");
        let supervisor = FileProcessSupervisor::with_allowed_commands(&runtime_root, ["printf"]);

        let denied = supervisor.launch(ManagedProcessSpec {
            owner: ProcessOwner::UserHelper,
            command: shell_binary().to_string(),
            args: shell_args("rm -rf target").to_vec(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: Some("rm -rf target".to_string()),
            sensitive: false,
            metadata: Default::default(),
        });

        assert!(matches!(denied, Err(RefineError::Unauthorized(_))));
        let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
        assert!(audit.contains("\"outcome\":\"denied\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_redacts_sensitive_process_details_and_stdin() {
        let temp_root = unique_temp_dir("process-sensitive");
        let runtime_root = temp_root.join("run/8080");
        let supervisor = FileProcessSupervisor::new(&runtime_root);

        let process = supervisor
            .run_to_completion(ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: shell_binary().to_string(),
                args: shell_args("cat >/dev/null").to_vec(),
                cwd: None,
                env: Vec::new(),
                stdin: Some("secret-value".to_string()),
                limits: None,
                authorization_command: None,
                sensitive: true,
                metadata: Default::default(),
            })
            .unwrap()
            .process;

        assert_eq!(process.details.as_deref(), Some("redacted"));
        assert!(process.stdin_path.is_none());
        assert!(
            !supervisor
                .processes_dir()
                .join(format!("{}.json", process.id))
                .exists()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_process_supervisor_strips_direct_api_keys_from_agent_processes() {
        let temp_root = unique_temp_dir("process-agent-env");
        let runtime_root = temp_root.join("run/8080");
        let supervisor = FileProcessSupervisor::new(&runtime_root);

        let output = supervisor
            .run_to_completion(ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command: shell_binary().to_string(),
                args: shell_args(
                    "printf '%s:%s' \"${OPENAI_API_KEY-unset}\" \"${ANTHROPIC_API_KEY-unset}\"",
                )
                .to_vec(),
                cwd: None,
                env: vec![
                    ("OPENAI_API_KEY".to_string(), "bad-openai-key".to_string()),
                    (
                        "ANTHROPIC_API_KEY".to_string(),
                        "bad-anthropic-key".to_string(),
                    ),
                ],
                stdin: None,
                limits: None,
                authorization_command: None,
                sensitive: false,
                metadata: Default::default(),
            })
            .unwrap();

        assert_eq!(output.stdout, "unset:unset");
        fs::remove_dir_all(temp_root).unwrap();
    }

    fn shell_binary() -> &'static str {
        if cfg!(windows) { "cmd" } else { "sh" }
    }

    fn shell_args(script: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["/C".to_string(), script.to_string()]
        } else {
            vec!["-c".to_string(), script.to_string()]
        }
    }

    fn long_running_shell_args() -> Vec<String> {
        if cfg!(windows) {
            shell_args("ping -n 30 127.0.0.1 >NUL")
        } else {
            shell_args("sleep 30")
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
