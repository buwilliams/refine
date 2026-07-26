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

pub fn managed_pid_is_alive(pid: u32) -> RefineResult<bool> {
    pid_alive(pid)
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

pub(crate) fn acquire_workflow_process_registration_lock(
    runtime_root: &Path,
) -> RefineResult<WorkflowProcessRegistrationLock> {
    fs::create_dir_all(runtime_root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create workflow process registration root {}: {error}",
            runtime_root.display()
        ))
    })?;
    let path = runtime_root.join(WORKFLOW_PROCESS_REGISTRATION_LOCK_FILE);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open workflow process registration lock {}: {error}",
                path.display()
            ))
        })?;
    file.lock_exclusive().map_err(|error| {
        RefineError::Io(format!(
            "failed to lock workflow process registration {}: {error}",
            path.display()
        ))
    })?;
    Ok(WorkflowProcessRegistrationLock { file })
}

fn workflow_process_identity(metadata: &Map<String, Value>) -> Option<(&str, &str, &str)> {
    let claim_id = metadata
        .get("claim_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let execution_id = metadata
        .get("execution_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let goal_id = metadata
        .get("goal_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some((claim_id, execution_id, goal_id))
}

fn workflow_runtime_root(process_root: &Path) -> PathBuf {
    if process_root.join(WORKFLOW_AUTOMATION_STATE_FILE).exists() {
        return process_root.to_path_buf();
    }
    process_root
        .parent()
        .filter(|parent| parent.join(WORKFLOW_AUTOMATION_STATE_FILE).exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| process_root.to_path_buf())
}

fn validate_running_workflow_claim(
    runtime_root: &Path,
    claim_id: &str,
    execution_id: &str,
    goal_id: &str,
) -> RefineResult<()> {
    let path = runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE);
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Conflict(format!(
            "workflow process launch for Goal {goal_id} could not validate claim {claim_id} execution {execution_id} against {}: {error}; the process was not started",
            path.display()
        ))
    })?;
    let state: Value = serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse workflow process launch state {}: {error}",
            path.display()
        ))
    })?;
    let running = state
        .get("claims")
        .and_then(Value::as_array)
        .is_some_and(|claims| {
            claims.iter().any(|claim| {
                claim.get("claim_id").and_then(Value::as_str) == Some(claim_id)
                    && claim.get("execution_id").and_then(Value::as_str) == Some(execution_id)
                    && claim.get("goal_id").and_then(Value::as_str) == Some(goal_id)
                    && claim.get("state").and_then(Value::as_str) == Some("running")
            })
        });
    if running {
        Ok(())
    } else {
        Err(RefineError::Conflict(format!(
            "workflow process launch for Goal {goal_id} no longer owns running claim {claim_id} execution {execution_id}; the process was not started"
        )))
    }
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
    if spec.owner == ProcessOwner::Agent {
        // The user's configured environment first, so an agent authenticates the
        // same way it would from a terminal. Applied before `spec.env` so refine's
        // own per-process variables still win.
        command.envs(crate::process::agent_env::agent_env_overlay(None));
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

/// Write JSON through a temp file and a rename so concurrent readers observe
/// either the previous content or the new content, never a truncated file.
/// Settings and registry records are read by the daemon while workflow threads
/// write them, so a plain `fs::write` lets a reader observe zero bytes.
pub(crate) fn write_json_atomically(path: &Path, encoded: &[u8], label: &str) -> RefineResult<()> {
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
        registry_cleanup_completed: false,
        identity_cleanup_completed: false,
        waited_ms: started.elapsed().as_millis(),
    }
}

fn remove_file_if_present(path: &Path, label: &str) -> RefineResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RefineError::Io(format!(
            "failed to remove {label} {}: {error}",
            path.display()
        ))),
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

fn append_observed_stream_file(output: &mut String, label: &str, path: &str) -> RefineResult<bool> {
    let path = PathBuf::from(path);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read process {label} stream {}: {error}",
                path.display()
            )));
        }
    };
    if text.trim().is_empty() {
        return Ok(true);
    }
    output.push_str(&format!("== {label} ==\n"));
    output.push_str(&tail_text(&text, 16_000));
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(true)
}

#[cfg(test)]
type AfterProcessEnumerationHook = Box<dyn FnOnce() + Send + 'static>;

#[cfg(test)]
static AFTER_PROCESS_ENUMERATION_HOOKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeMap<PathBuf, AfterProcessEnumerationHook>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) fn install_after_process_enumeration_hook(
    runtime_root: &Path,
    hook: impl FnOnce() + Send + 'static,
) {
    AFTER_PROCESS_ENUMERATION_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(runtime_root.to_path_buf(), Box::new(hook));
}

#[cfg(test)]
fn run_after_process_enumeration_hook(runtime_root: &Path) {
    let hook = AFTER_PROCESS_ENUMERATION_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(runtime_root);
    if let Some(hook) = hook {
        hook();
    }
}

fn tail_text(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        value.to_string()
    } else {
        value.chars().skip(count - max_chars).collect()
    }
}

mod execution;
mod output;
mod registry;
mod supervision;
mod termination;
#[cfg(test)]
mod tests;
