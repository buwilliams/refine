use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{FileOperationRegistry, OperationRegistry};
use crate::process::supervisor::runtime::RuntimeRoot;

pub const DAEMON_STATUS_FILE: &str = "daemon-status.json";
const BACKGROUND_DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(120);
const BACKGROUND_DAEMON_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);

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

pub trait DaemonLifecycleService {
    fn start(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn stop(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn restart(&self, port: u16) -> RefineResult<DaemonStatus>;
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
        let deadline = Instant::now() + BACKGROUND_DAEMON_READY_TIMEOUT;
        loop {
            relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset);
            let managed = supervisor.wait(&process.id)?;
            if managed.state != "running" {
                relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset);
                return Err(RefineError::Conflict(format!(
                    "daemon process exited before becoming reachable: {}",
                    managed.state
                )));
            }
            if http_probe(port).is_ok() {
                relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset);
                return self.status(port);
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(BACKGROUND_DAEMON_READY_POLL_INTERVAL);
        }
        relay_daemon_startup_output(process.stderr_path.as_deref(), &mut stderr_offset);
        Err(RefineError::Degraded(format!(
            "daemon did not become reachable on 127.0.0.1:{port}"
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
}

pub fn http_probe(port: u16) -> RefineResult<()> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| {
        RefineError::Io(format!(
            "daemon is not reachable on 127.0.0.1:{port}: {error}"
        ))
    })?;
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

fn relay_daemon_startup_output(path: Option<&str>, offset: &mut usize) {
    let Some(path) = path else {
        return;
    };
    let Ok(output) = fs::read_to_string(path) else {
        return;
    };
    if output.len() <= *offset {
        return;
    }
    let next = &output[*offset..];
    *offset = output.len();
    for line in next.lines().filter(|line| !line.trim().is_empty()) {
        eprintln!("{line}");
    }
}

impl DaemonLifecycleService for FileDaemonLifecycleService {
    fn start(&self, port: u16) -> RefineResult<DaemonStatus> {
        let status = running_status(port);
        self.write_status(&status)?;
        Ok(status)
    }

    fn stop(&self, port: u16) -> RefineResult<DaemonStatus> {
        let supervisor = FileProcessSupervisor::new(self.runtime_root.port_root(port));
        for process in supervisor.list()? {
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
        self.write_status(&status)?;
        Ok(status)
    }

    fn restart(&self, port: u16) -> RefineResult<DaemonStatus> {
        let _ = self.stop(port)?;
        self.start(port)
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
        let port_root = self.runtime_root.port_root(port);
        let supervisor = FileProcessSupervisor::new(&port_root);
        let before_recovery = supervisor.list()?;
        let recovered = supervisor.recover()?;
        let interrupted_operations = FileOperationRegistry::new(&port_root).interrupt_active()?;
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
        if !interrupted_operations.is_empty() {
            status
                .degraded_integrations
                .push("operation-recovery-interrupted".to_string());
        }
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
    if let Ok(value) = std::env::var("REFINE_LAUNCH_EXECUTABLE")
        && !value.trim().is_empty()
    {
        return Some(value);
    }
    std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
}

pub fn daemon_executable_path() -> RefineResult<PathBuf> {
    if current_launch_mode() == "binary"
        && let Some(path) = current_launch_executable()
        && path != "cargo"
    {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe()
        .map_err(|error| RefineError::Io(format!("failed to locate current executable: {error}")))
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
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_lifecycle_persists_port_scoped_status() {
        let temp_root = unique_temp_dir("lifecycle-status");
        let runtime_root = RuntimeRoot {
            root: temp_root.join("run"),
        };
        let service = FileDaemonLifecycleService::new(runtime_root);

        let started = service.start(4555).unwrap();
        assert!(started.daemon_healthy);
        assert!(service.status_path(4555).exists());
        assert_eq!(service.status(4555).unwrap().worker_state, "idle");

        let stopped = service.stop(4555).unwrap();
        assert!(!stopped.daemon_healthy);
        assert_eq!(service.status(4555).unwrap().worker_state, "stopped");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn background_daemon_launch_records_are_port_scoped() {
        let temp_root = unique_temp_dir("lifecycle-background");
        let runtime_root = RuntimeRoot {
            root: temp_root.join("run"),
        };
        let service = FileDaemonLifecycleService::new(runtime_root.clone());

        let result = service.start_background_daemon(BackgroundDaemonConfig {
            port: 4555,
            bind_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            cache_dir: None,
            static_root: None,
        });

        assert!(result.is_err());
        assert!(!runtime_root.root.join("processes").exists());
        assert!(!runtime_root.root.join("security-audit.jsonl").exists());
        assert!(runtime_root.port_root(4555).join("processes").exists());
        assert!(
            !runtime_root
                .port_root(4555)
                .join("security-audit.jsonl")
                .exists()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn detached_daemon_command_uses_executable_directly() {
        let exe = PathBuf::from("/usr/local/bin/refine");
        let (command, args) = detached_command_parts(&exe);
        assert_eq!(command, exe.display().to_string());
        assert!(args.is_empty());
    }

    #[test]
    fn file_lifecycle_recover_reconciles_port_scoped_processes() {
        let temp_root = unique_temp_dir("lifecycle-recover");
        let runtime_root = RuntimeRoot {
            root: temp_root.join("run"),
        };
        let supervisor = FileProcessSupervisor::new(runtime_root.port_root(4555));
        supervisor
            .register(crate::process::subprocess::ManagedProcess {
                id: "missing-pid".to_string(),
                owner: crate::process::subprocess::ProcessOwner::Agent,
                pid: None,
                state: "running".to_string(),
                label: Some("agent".to_string()),
                details: None,
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();
        let operation_registry = FileOperationRegistry::new(runtime_root.port_root(4555));
        let operation = operation_registry
            .register("gap GAP1 implementation")
            .unwrap();
        let service = FileDaemonLifecycleService::new(runtime_root);

        let status = service.recover(4555).unwrap();
        assert!(status.daemon_healthy);
        assert!(
            status
                .degraded_integrations
                .contains(&"process-recovery-reconciled".to_string())
        );
        assert!(supervisor.inspect("missing-pid").is_err());
        assert!(
            status
                .degraded_integrations
                .contains(&"operation-recovery-interrupted".to_string())
        );
        assert_eq!(
            operation_registry
                .status(&operation.id)
                .unwrap()
                .state
                .as_api_status(),
            "interrupted"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
