use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationRegistry};
use crate::process::supervisor::runtime::RuntimeRoot;

pub const DAEMON_STATUS_FILE: &str = "daemon-status.json";
pub const DAEMON_STARTUP_PROGRESS_FILE: &str = "daemon-startup-progress";
/// How long startup may go without observable progress before it is called a
/// failure. This is a stall budget, not a total budget: a host slow enough to
/// need several minutes to bind its port is still starting, not broken, and
/// reporting it as failed is what makes recovery machinery act on healthy work.
const BACKGROUND_DAEMON_READY_STALL_TIMEOUT: Duration = Duration::from_secs(120);
const BACKGROUND_DAEMON_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Tracks whether a startup is still making observable progress.
///
/// A fixed wall-clock deadline measures the machine rather than the work: the
/// same daemon that starts well inside the budget on a developer workstation
/// exceeds it on a loaded two-core node, and is then killed and recovered as
/// though it had failed. Progress resets the budget, so slower hosts take
/// longer and only genuine silence is treated as a stall.
pub(crate) struct ReadinessProgress {
    last_progress: Instant,
    stall_timeout: Duration,
}

impl ReadinessProgress {
    pub(crate) fn new(stall_timeout: Duration, now: Instant) -> Self {
        Self {
            last_progress: now,
            stall_timeout,
        }
    }

    pub(crate) fn record_progress(&mut self, now: Instant) {
        self.last_progress = now;
    }

    pub(crate) fn stalled(&self, now: Instant) -> bool {
        now.duration_since(self.last_progress) >= self.stall_timeout
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonStatus {
    pub port: u16,
    pub daemon_healthy: bool,
    pub web_available: bool,
    pub worker_state: String,
    pub target_app_state: String,
    #[serde(default = "default_launch_mode")]
    pub launch_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    pub active_operations: Vec<String>,
    pub degraded_integrations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_evidence: Option<DaemonLifecycleEvidence>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonLifecycleEvidence {
    pub action: String,
    pub service_manager: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_error: Option<String>,
    #[serde(default)]
    pub observed_reachable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DaemonReachability {
    Reachable,
    Unreachable(String),
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct BackgroundDaemonConfig {
    pub port: u16,
    pub bind_address: IpAddr,
    pub cache_dir: Option<PathBuf>,
    pub static_root: Option<PathBuf>,
}

impl Default for BackgroundDaemonConfig {
    fn default() -> Self {
        Self {
            port: 0,
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            cache_dir: None,
            static_root: None,
        }
    }
}

/// Direct port-runtime primitives used by the host lifecycle authority.
///
/// This layer never chooses installed service-manager semantics and is not a
/// surface lifecycle API.
pub trait DaemonRuntimeService {
    fn stop_runtime(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn status(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn health(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn recover(&self, port: u16) -> RefineResult<DaemonStatus>;
}

#[derive(Clone, Debug)]
pub struct FileDaemonLifecycleService {
    pub runtime_root: RuntimeRoot,
}

impl FileDaemonLifecycleService {
    pub fn checkout_local(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: RuntimeRoot::checkout_local(repo_root.into()),
        }
    }

    pub fn new(runtime_root: RuntimeRoot) -> Self {
        Self { runtime_root }
    }

    pub fn status_path(&self, port: u16) -> PathBuf {
        self.runtime_root.port_root(port).join(DAEMON_STATUS_FILE)
    }

    pub fn startup_progress_path(&self, port: u16) -> PathBuf {
        self.runtime_root
            .port_root(port)
            .join(DAEMON_STARTUP_PROGRESS_FILE)
    }

    /// Publish a startup milestone so a waiter can tell slow from stalled.
    ///
    /// The direct launcher reads the daemon's stderr for this, but a
    /// service-managed daemon writes to the journal, which Refine cannot see.
    /// Its readiness wait therefore had nothing to observe: the status file is
    /// written once at "starting" and not again until "idle", so a slow start
    /// was indistinguishable from a hung one and was reported as a failure.
    ///
    /// Best effort. A daemon that cannot record progress should still start;
    /// the cost is only that its waiter falls back to timing out.
    pub fn record_startup_progress(&self, port: u16, step: &str) {
        let path = self.startup_progress_path(port);
        let Some(parent) = path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let _ = fs::write(&path, format!("{step}\n"));
    }

    /// A token that changes whenever startup reports progress.
    ///
    /// Content rather than modification time: a step repeating within one
    /// filesystem timestamp granule would otherwise look like no progress at
    /// all.
    pub fn startup_progress_token(&self, port: u16) -> Option<String> {
        let path = self.startup_progress_path(port);
        let content = fs::read_to_string(&path).ok()?;
        let modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        Some(format!("{modified}:{content}"))
    }

    pub fn running_statuses(&self) -> RefineResult<Vec<DaemonStatus>> {
        let mut statuses = self.known_statuses()?;
        statuses.retain(|status| {
            status.daemon_healthy && status.web_available && http_probe(status.port).is_ok()
        });
        Ok(statuses)
    }

    pub fn known_statuses(&self) -> RefineResult<Vec<DaemonStatus>> {
        let entries = match fs::read_dir(&self.runtime_root.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read runtime root {}: {error}",
                    self.runtime_root.root.display()
                )));
            }
        };
        let mut statuses = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read runtime root entry {}: {error}",
                    self.runtime_root.root.display()
                ))
            })?;
            let file_type = entry.file_type().map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect runtime root entry {}: {error}",
                    entry.path().display()
                ))
            })?;
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(port) = name.parse::<u16>() else {
                continue;
            };
            let status = self.status(port)?;
            statuses.push(status);
        }
        statuses.sort_by_key(|status| status.port);
        Ok(statuses)
    }

    pub fn start_background_daemon(
        &self,
        config: BackgroundDaemonConfig,
    ) -> RefineResult<DaemonStatus> {
        let port = config.port;
        if port == 0 {
            return Err(RefineError::InvalidInput(
                "background daemon start requires a concrete port".to_string(),
            ));
        }
        let runtime_root = &self.runtime_root.root;
        let port_runtime_root = self.runtime_root.port_root(port);
        let exe = daemon_executable_path()?;
        fs::create_dir_all(runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create daemon runtime root {}: {error}",
                runtime_root.display()
            ))
        })?;
        let (command, mut args) = detached_command_parts(&exe);
        args.extend([
            "system".to_string(),
            "start".to_string(),
            "--foreground".to_string(),
            "--port".to_string(),
            port.to_string(),
            "--bind-address".to_string(),
            config.bind_address.to_string(),
            "--runtime-root".to_string(),
            runtime_root.display().to_string(),
        ]);
        if let Some(cache_dir) = config.cache_dir {
            args.push("--cache-dir".to_string());
            args.push(cache_dir.display().to_string());
        }
        if let Some(static_root) = config.static_root {
            args.push("--static-root".to_string());
            args.push(static_root.display().to_string());
        }
        let supervisor = FileProcessSupervisor::new(&port_runtime_root);
        let process = supervisor.launch(ManagedProcessSpec {
            owner: ProcessOwner::Daemon,
            command,
            args,
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: Some("refine daemon start".to_string()),
            sensitive: true,
            metadata: Default::default(),
        })?;
        eprintln!(
            "refine: starting daemon at http://{}:{}",
            config.bind_address, port
        );
        let mut stderr_offset = 0;
        let mut progress =
            ReadinessProgress::new(BACKGROUND_DAEMON_READY_STALL_TIMEOUT, Instant::now());
        loop {
            if relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset) {
                progress.record_progress(Instant::now());
            }
            let managed = supervisor.wait(&process.id)?;
            if managed.state != "running" {
                relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset);
                return Err(RefineError::Conflict(format!(
                    "daemon process exited before becoming reachable: {}{}",
                    managed.state,
                    managed
                        .exit_code
                        .map(|code| format!(" (exit code {code})"))
                        .unwrap_or_default()
                )));
            }
            if http_probe(port).is_ok() {
                relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset);
                return self.status(port);
            }
            if progress.stalled(Instant::now()) {
                break;
            }
            thread::sleep(BACKGROUND_DAEMON_READY_POLL_INTERVAL);
        }
        relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset);
        Err(RefineError::Degraded(format!(
            "daemon produced no startup progress for {}s and is not reachable on 127.0.0.1:{port}",
            BACKGROUND_DAEMON_READY_STALL_TIMEOUT.as_secs()
        )))
    }

    fn write_status(&self, status: &DaemonStatus) -> RefineResult<()> {
        let path = self.status_path(status.port);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create runtime directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let encoded = serde_json::to_vec_pretty(status).map_err(|error| {
            RefineError::Serialization(format!("failed to encode daemon status: {error}"))
        })?;
        fs::write(&path, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write daemon status {}: {error}",
                path.display()
            ))
        })
    }

    fn read_status(&self, port: u16) -> RefineResult<DaemonStatus> {
        let path = self.status_path(port);
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read daemon status {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse daemon status {}: {error}",
                path.display()
            ))
        })
    }

    fn recovered_status(&self, port: u16) -> RefineResult<DaemonStatus> {
        let port_root = self.runtime_root.port_root(port);
        let supervisor = FileProcessSupervisor::new(&port_root);
        let before_recovery = supervisor.list()?;
        supervisor.recover()?;
        let recovered_operations =
            FileOperationRegistry::new(&port_root).recover_active_supervised()?;
        let recovered = supervisor.recover()?;
        let mut status = running_status(port);
        status.active_operations = recovered
            .iter()
            .filter(|process| process.state == "running")
            .map(|process| process.id.clone())
            .collect();
        status.active_operations.extend(
            FileOperationRegistry::new(&port_root)
                .recover()?
                .into_iter()
                .filter(|operation| {
                    matches!(operation.state.as_api_status(), "pending" | "running")
                })
                .map(|operation| operation.id),
        );
        if before_recovery
            .iter()
            .any(|before| !recovered.iter().any(|after| after.id == before.id))
        {
            status
                .degraded_integrations
                .push("process-recovery-reconciled".to_string());
        }
        if recovered_operations
            .iter()
            .any(|operation| operation.state.as_api_status() == "interrupted")
        {
            status
                .degraded_integrations
                .push("operation-recovery-interrupted".to_string());
        }
        if recovered_operations.iter().any(|operation| {
            operation.error.as_ref().is_some_and(|error| {
                error.get("code").and_then(serde_json::Value::as_str)
                    == Some("operation_recovery_process_termination_failed")
            })
        }) {
            status
                .degraded_integrations
                .push("operation-recovery-attention-required".to_string());
        }
        Ok(status)
    }

    /// Reconcile runtime state while publishing an explicitly non-ready daemon status.
    /// Foreground startup promotes this same status only after every recovery and cache
    /// warmup step succeeds.
    pub fn begin_start(&self, port: u16) -> RefineResult<DaemonStatus> {
        let mut status = running_status(port);
        status.daemon_healthy = false;
        status.web_available = false;
        status.worker_state = "starting".to_string();
        status.active_operations.clear();
        status.lifecycle_evidence = None;
        self.write_status(&status)?;
        Ok(status)
    }

    pub fn prepare_start(&self, port: u16) -> RefineResult<DaemonStatus> {
        let mut status = self.recovered_status(port)?;
        status.daemon_healthy = false;
        status.web_available = false;
        status.worker_state = "starting".to_string();
        status.lifecycle_evidence = None;
        self.write_status(&status)?;
        Ok(status)
    }

    pub fn mark_ready(&self, mut status: DaemonStatus) -> RefineResult<DaemonStatus> {
        status.daemon_healthy = true;
        status.web_available = true;
        status.worker_state = "idle".to_string();
        status.lifecycle_evidence = None;
        self.write_status(&status)?;
        Ok(status)
    }

    /// Persist the readiness established by an authoritative daemon probe.
    ///
    /// An idempotent service-manager start may not restart the daemon, so the
    /// foreground startup path has no opportunity to replace stale lifecycle
    /// state. Preserve unrelated degradation and operation evidence while
    /// discarding failures from superseded startup attempts.
    pub fn mark_observed_ready(&self, port: u16) -> RefineResult<DaemonStatus> {
        self.mark_observed_ready_with_evidence(port, None)
    }

    pub fn mark_observed_ready_with_evidence(
        &self,
        port: u16,
        evidence: Option<DaemonLifecycleEvidence>,
    ) -> RefineResult<DaemonStatus> {
        let mut status = self
            .read_status(port)
            .unwrap_or_else(|_| running_status(port));
        status.degraded_integrations.retain(|detail| {
            !detail.starts_with("startup-failed:") && !detail.starts_with("restart-failed:")
        });
        status.daemon_healthy = true;
        status.web_available = true;
        status.worker_state = "idle".to_string();
        status.lifecycle_evidence = evidence;
        self.write_status(&status)?;
        Ok(status)
    }

    pub fn mark_start_failed(&self, port: u16, error: &RefineError) -> RefineResult<DaemonStatus> {
        self.mark_start_failed_with_evidence(port, error, None)
    }

    pub fn mark_start_failed_with_evidence(
        &self,
        port: u16,
        error: &RefineError,
        evidence: Option<DaemonLifecycleEvidence>,
    ) -> RefineResult<DaemonStatus> {
        self.mark_observed_failed_with_evidence(port, format!("startup-failed:{error}"), evidence)
    }

    pub fn mark_observed_failed_with_evidence(
        &self,
        port: u16,
        degradation: String,
        evidence: Option<DaemonLifecycleEvidence>,
    ) -> RefineResult<DaemonStatus> {
        let mut status = self
            .read_status(port)
            .unwrap_or_else(|_| stopped_status(port, Vec::new()));
        status.daemon_healthy = false;
        status.web_available = false;
        status.worker_state = "failed".to_string();
        status.active_operations.clear();
        status.degraded_integrations.push(degradation);
        status.lifecycle_evidence = evidence;
        self.write_status(&status)?;
        Ok(status)
    }

    pub fn mark_observed_stopped(
        &self,
        port: u16,
        evidence: Option<DaemonLifecycleEvidence>,
    ) -> RefineResult<DaemonStatus> {
        let mut status = self
            .read_status(port)
            .unwrap_or_else(|_| stopped_status(port, Vec::new()));
        status.daemon_healthy = false;
        status.web_available = false;
        status.worker_state = "stopped".to_string();
        status.active_operations.clear();
        status.lifecycle_evidence = evidence;
        self.write_status(&status)?;
        Ok(status)
    }
}

pub fn http_probe(port: u16) -> RefineResult<()> {
    match http_reachability_probe(port) {
        DaemonReachability::Reachable => Ok(()),
        DaemonReachability::Unreachable(detail) | DaemonReachability::Unknown(detail) => {
            Err(RefineError::Io(detail))
        }
    }
}

pub fn http_reachability_probe(port: u16) -> DaemonReachability {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_secs(1)) {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            return DaemonReachability::Unreachable(format!(
                "daemon is not reachable on 127.0.0.1:{port}: {error}"
            ));
        }
        Err(error) => {
            return DaemonReachability::Unknown(format!(
                "daemon reachability probe failed on 127.0.0.1:{port}: {error}"
            ));
        }
    };
    match probe_connected_daemon(&mut stream, port) {
        Ok(()) => DaemonReachability::Reachable,
        Err(error) => DaemonReachability::Unknown(error.to_string()),
    }
}

fn probe_connected_daemon(stream: &mut TcpStream, port: u16) -> RefineResult<()> {
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| RefineError::Io(format!("failed to set daemon probe timeout: {error}")))?;
    stream
        .write_all(b"GET /system/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|error| RefineError::Io(format!("failed to write daemon probe: {error}")))?;
    let mut response = Vec::new();
    let mut chunk = [0_u8; 512];
    while !response.windows(4).any(|window| window == b"\r\n\r\n") && response.len() < 8192 {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| RefineError::Io(format!("failed to read daemon probe: {error}")))?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
    }
    let response = String::from_utf8_lossy(&response);
    if response.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(RefineError::Degraded(format!(
            "daemon probe returned unexpected response on 127.0.0.1:{port}"
        )))
    }
}

fn detached_command_parts(exe: &std::path::Path) -> (String, Vec<String>) {
    (exe.display().to_string(), Vec::new())
}

/// Relays any new startup output and reports whether there was any. New output
/// is the daemon demonstrating it is still working, which is what distinguishes
/// a slow start from a stalled one.
fn relay_daemon_startup_output(path: Option<&str>, offset: &mut usize) -> bool {
    let Some(path) = path else {
        return false;
    };
    let Ok(output) = fs::read_to_string(path) else {
        return false;
    };
    if output.len() <= *offset {
        return false;
    }
    let next = &output[*offset..];
    *offset = output.len();
    for line in next.lines().filter(|line| !line.trim().is_empty()) {
        eprintln!("{line}");
    }
    true
}

impl DaemonRuntimeService for FileDaemonLifecycleService {
    fn stop_runtime(&self, port: u16) -> RefineResult<DaemonStatus> {
        let supervisor = FileProcessSupervisor::new(self.runtime_root.port_root(port));
        let processes = supervisor.list()?;
        for process in processes
            .iter()
            .filter(|process| process.owner != ProcessOwner::Daemon)
        {
            let process = supervisor.wait(&process.id)?;
            if process.state == "running" {
                let _ = supervisor.signal(&process.id, "terminate");
            }
        }
        for process in processes {
            if process.owner == ProcessOwner::Daemon {
                let process = supervisor.wait(&process.id)?;
                if process.state == "running" {
                    let _ = supervisor.signal(&process.id, "terminate");
                    thread::sleep(Duration::from_millis(100));
                    if supervisor
                        .wait(&process.id)
                        .map(|process| process.state == "running")
                        .unwrap_or(false)
                    {
                        let _ = supervisor.signal(&process.id, "kill");
                    }
                }
            }
        }
        let mut status = self
            .read_status(port)
            .unwrap_or_else(|_| stopped_status(port, vec!["daemon-status-missing".to_string()]));
        status.daemon_healthy = false;
        status.web_available = false;
        status.worker_state = "stopped".to_string();
        status.active_operations.clear();
        status.lifecycle_evidence = None;
        self.write_status(&status)?;
        Ok(status)
    }

    fn status(&self, port: u16) -> RefineResult<DaemonStatus> {
        match self.read_status(port) {
            Ok(status) => Ok(status),
            Err(_) => Ok(stopped_status(
                port,
                vec!["daemon-status-missing".to_string()],
            )),
        }
    }

    fn health(&self, port: u16) -> RefineResult<DaemonStatus> {
        self.status(port)
    }

    fn recover(&self, port: u16) -> RefineResult<DaemonStatus> {
        let status = self.recovered_status(port)?;
        self.write_status(&status)?;
        Ok(status)
    }
}

pub fn running_status(port: u16) -> DaemonStatus {
    DaemonStatus {
        port,
        daemon_healthy: true,
        web_available: true,
        worker_state: "idle".to_string(),
        target_app_state: "unknown".to_string(),
        launch_mode: current_launch_mode(),
        executable_path: current_launch_executable(),
        active_operations: Vec::new(),
        degraded_integrations: Vec::new(),
        lifecycle_evidence: None,
    }
}

pub fn stopped_status(port: u16, degraded_integrations: Vec<String>) -> DaemonStatus {
    DaemonStatus {
        port,
        daemon_healthy: false,
        web_available: false,
        worker_state: "stopped".to_string(),
        target_app_state: "unknown".to_string(),
        launch_mode: current_launch_mode(),
        executable_path: current_launch_executable(),
        active_operations: Vec::new(),
        degraded_integrations,
        lifecycle_evidence: None,
    }
}

pub fn current_launch_mode() -> String {
    match std::env::var("REFINE_LAUNCH_MODE")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "binary" | "release" | "deployed" => "binary".to_string(),
        "cargo" | "dev" | "development" => "cargo".to_string(),
        _ => infer_launch_mode_from_executable(),
    }
}

pub fn current_launch_executable() -> Option<String> {
    select_launch_executable(
        &current_launch_mode(),
        std::env::var("REFINE_LAUNCH_EXECUTABLE").ok(),
        std::env::current_exe().ok(),
    )
}

fn select_launch_executable(
    mode: &str,
    configured: Option<String>,
    actual: Option<PathBuf>,
) -> Option<String> {
    if mode == "cargo" {
        return actual.map(|path| path.display().to_string());
    }
    configured
        .filter(|value| !value.trim().is_empty() && value.trim() != "cargo")
        .or_else(|| actual.map(|path| path.display().to_string()))
}

pub fn daemon_executable_path() -> RefineResult<PathBuf> {
    current_launch_executable()
        .map(PathBuf::from)
        .ok_or_else(|| RefineError::NotFound("failed to locate current executable".to_string()))
}

pub fn daemon_executable_string() -> RefineResult<String> {
    daemon_executable_path().map(|path| path.display().to_string())
}

fn default_launch_mode() -> String {
    "unknown".to_string()
}

fn infer_launch_mode_from_executable() -> String {
    let path = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    if path.contains("/target/debug/") {
        "cargo".to_string()
    } else {
        "binary".to_string()
    }
}

#[cfg(test)]
mod tests;
