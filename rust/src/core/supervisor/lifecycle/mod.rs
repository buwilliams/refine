use serde::{Deserialize, Serialize};

use crate::core::host::process_supervision::{FileProcessSupervisor, ProcessSupervisor};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::jobs::{FileJobRegistry, JobRegistry};
use crate::core::supervisor::runtime::RuntimeRoot;
use std::fs;
use std::path::PathBuf;

pub const DAEMON_STATUS_FILE: &str = "daemon-status.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonStatus {
    pub port: u16,
    pub daemon_healthy: bool,
    pub web_available: bool,
    pub worker_state: String,
    pub target_app_state: String,
    pub active_operations: Vec<String>,
    pub degraded_integrations: Vec<String>,
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

impl DaemonLifecycleService for FileDaemonLifecycleService {
    fn start(&self, port: u16) -> RefineResult<DaemonStatus> {
        let status = running_status(port);
        self.write_status(&status)?;
        Ok(status)
    }

    fn stop(&self, port: u16) -> RefineResult<DaemonStatus> {
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
        let recovered = FileProcessSupervisor::new(&port_root).recover()?;
        let interrupted_jobs = FileJobRegistry::new(&port_root).interrupt_active()?;
        let mut status = running_status(port);
        status.active_operations = recovered
            .iter()
            .filter(|process| process.state == "running")
            .map(|process| process.id.clone())
            .collect();
        status.active_operations.extend(
            FileJobRegistry::new(&port_root)
                .recover()?
                .into_iter()
                .filter(|job| matches!(job.state.as_api_status(), "pending" | "running"))
                .map(|job| job.id),
        );
        if recovered
            .iter()
            .any(|process| matches!(process.state.as_str(), "exited" | "interrupted"))
        {
            status
                .degraded_integrations
                .push("process-recovery-reconciled".to_string());
        }
        if !interrupted_jobs.is_empty() {
            status
                .degraded_integrations
                .push("job-recovery-interrupted".to_string());
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
        active_operations: Vec::new(),
        degraded_integrations,
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
    fn file_lifecycle_recover_reconciles_port_scoped_processes() {
        let temp_root = unique_temp_dir("lifecycle-recover");
        let runtime_root = RuntimeRoot {
            root: temp_root.join("run"),
        };
        let supervisor = FileProcessSupervisor::new(runtime_root.port_root(4555));
        supervisor
            .register(crate::core::host::process_supervision::ManagedProcess {
                id: "missing-pid".to_string(),
                owner: crate::core::host::process_supervision::ProcessOwner::Agent,
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
        let job_registry = FileJobRegistry::new(runtime_root.port_root(4555));
        let job = job_registry.register("gap GAP1 implementation").unwrap();
        let service = FileDaemonLifecycleService::new(runtime_root);

        let status = service.recover(4555).unwrap();
        assert!(status.daemon_healthy);
        assert!(
            status
                .degraded_integrations
                .contains(&"process-recovery-reconciled".to_string())
        );
        assert_eq!(
            supervisor.inspect("missing-pid").unwrap().state,
            "interrupted"
        );
        assert!(
            status
                .degraded_integrations
                .contains(&"job-recovery-interrupted".to_string())
        );
        assert_eq!(
            job_registry.status(&job.id).unwrap().state.as_api_status(),
            "interrupted"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}
