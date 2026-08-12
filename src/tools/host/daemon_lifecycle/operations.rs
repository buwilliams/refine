use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::*;
use crate::process::supervisor::coordination::replace_file_durably;
use crate::process::supervisor::lifecycle::DaemonRuntimeService;
use crate::tools::host::checkout::RefineCheckoutPaths;

pub const DAEMON_LIFECYCLE_OPERATIONS_DIR: &str = "daemon-lifecycle-operations";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DaemonLifecycleOperation {
    pub id: String,
    pub action: String,
    pub status: String,
    pub port: u16,
    pub requested_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<DaemonStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileDaemonLifecycleOperationService {
    runtime_root: RuntimeRoot,
    current_version: String,
}

impl FileDaemonLifecycleOperationService {
    pub fn new(runtime_root: RuntimeRoot, current_version: impl Into<String>) -> Self {
        Self {
            runtime_root,
            current_version: current_version.into(),
        }
    }

    pub fn queue(
        &self,
        action: DaemonLifecycleAction,
        config: BackgroundDaemonConfig,
    ) -> RefineResult<DaemonLifecycleOperation> {
        let paths = RefineCheckoutPaths::from_runtime_root(&self.runtime_root.root)?;
        let executable = paths.binary;
        if !executable.is_file() {
            return Err(RefineError::NotFound(format!(
                "checkout-local daemon lifecycle helper is missing: {}",
                executable.display()
            )));
        }
        let installation = FileInstallationService::for_port(
            &self.runtime_root.root,
            &self.current_version,
            config.port,
        );
        let installed_action = installed_action(action)?;
        let service_manager = installation.installed_service_manager_for(installed_action)?;
        self.queue_with(
            action,
            config,
            &executable,
            service_manager.as_deref(),
            &HostRestartSafeHandoffLauncher,
        )
    }

    pub(crate) fn queue_with(
        &self,
        action: DaemonLifecycleAction,
        config: BackgroundDaemonConfig,
        executable: &Path,
        service_manager: Option<&str>,
        launcher: &dyn RestartSafeHandoffLauncher,
    ) -> RefineResult<DaemonLifecycleOperation> {
        let action_name = operation_action_name(action)?;
        let now = now_timestamp();
        let mut operation = DaemonLifecycleOperation {
            id: format!("daemon-lifecycle-{}", Uuid::new_v4()),
            action: action_name.to_string(),
            status: "queued".to_string(),
            port: config.port,
            requested_at: now.clone(),
            updated_at: now,
            result: None,
            error: None,
            recovery: None,
        };
        self.save(&operation)?;

        let handoff = RestartSafeHandoff {
            executable: executable.to_path_buf(),
            args: vec![
                "system".to_string(),
                "daemon-lifecycle-helper".to_string(),
                "--action".to_string(),
                action_name.to_string(),
                "--port".to_string(),
                config.port.to_string(),
                "--runtime-root".to_string(),
                self.runtime_root.root.display().to_string(),
                "--operation-id".to_string(),
                operation.id.clone(),
            ],
            cwd: handoff_cwd(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
            label: operation.id.clone(),
        };
        if let Err(error) = launcher.launch(&handoff, service_manager) {
            operation.status = "failed".to_string();
            operation.error = Some(error.to_string());
            operation.recovery = Some(
                "No daemon control was attempted; correct the restart-safe helper launch failure and retry"
                    .to_string(),
            );
            operation.updated_at = now_timestamp();
            self.save(&operation)?;
            return Err(error);
        }
        Ok(operation)
    }

    pub fn run_helper(
        &self,
        operation_id: &str,
        action: DaemonLifecycleAction,
        config: BackgroundDaemonConfig,
    ) -> RefineResult<DaemonLifecycleOperation> {
        self.run_helper_with(
            operation_id,
            action,
            config,
            Duration::from_millis(750),
            execute_daemon_lifecycle,
        )
    }

    pub(crate) fn run_helper_with(
        &self,
        operation_id: &str,
        action: DaemonLifecycleAction,
        config: BackgroundDaemonConfig,
        response_delay: Duration,
        execute: impl FnOnce(
            &FileHostDaemonLifecycleService,
            DaemonLifecycleAction,
            BackgroundDaemonConfig,
        ) -> RefineResult<DaemonStatus>,
    ) -> RefineResult<DaemonLifecycleOperation> {
        let mut operation = self.load(config.port, operation_id)?;
        let action_name = operation_action_name(action)?;
        if operation.action != action_name || operation.port != config.port {
            return Err(RefineError::Conflict(format!(
                "daemon lifecycle operation {operation_id} does not match {action_name} on port {}",
                config.port
            )));
        }

        // The durable receipt must leave the daemon before this independent helper
        // stops or restarts the service that accepted the request.
        thread::sleep(response_delay);
        operation.status = "running".to_string();
        operation.updated_at = now_timestamp();
        self.save(&operation)?;

        let lifecycle = FileHostDaemonLifecycleService::new(
            self.runtime_root.clone(),
            self.current_version.clone(),
        );
        match execute(&lifecycle, action, config) {
            Ok(status) => {
                operation.status = "succeeded".to_string();
                operation.recovery = status
                    .lifecycle_evidence
                    .as_ref()
                    .and_then(|evidence| evidence.recovery.clone());
                operation.result = Some(status);
                operation.error = None;
                operation.updated_at = now_timestamp();
                self.save(&operation)?;
                Ok(operation)
            }
            Err(error) => {
                let observed = lifecycle.runtime_lifecycle().status(operation.port).ok();
                operation.status = "failed".to_string();
                operation.recovery = observed
                    .as_ref()
                    .and_then(|status| status.lifecycle_evidence.as_ref())
                    .and_then(|evidence| evidence.recovery.clone())
                    .or_else(|| {
                        Some(
                            "Inspect the durable lifecycle evidence and service manager before retrying"
                                .to_string(),
                        )
                    });
                operation.result = observed;
                operation.error = Some(error.to_string());
                operation.updated_at = now_timestamp();
                self.save(&operation)?;
                Err(error)
            }
        }
    }

    pub fn load(&self, port: u16, operation_id: &str) -> RefineResult<DaemonLifecycleOperation> {
        validate_operation_id(operation_id)?;
        let path = self.operation_path(port, operation_id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RefineError::NotFound(format!(
                    "Daemon lifecycle operation {operation_id} was not found"
                ))
            } else {
                RefineError::Io(format!(
                    "failed to read daemon lifecycle operation {}: {error}",
                    path.display()
                ))
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse daemon lifecycle operation {}: {error}",
                path.display()
            ))
        })
    }

    fn save(&self, operation: &DaemonLifecycleOperation) -> RefineResult<()> {
        let encoded = serde_json::to_vec_pretty(operation).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode daemon lifecycle operation: {error}"
            ))
        })?;
        replace_file_durably(
            &self.operation_path(operation.port, &operation.id),
            &encoded,
        )
    }

    fn operation_path(&self, port: u16, operation_id: &str) -> PathBuf {
        self.runtime_root
            .port_root(port)
            .join(DAEMON_LIFECYCLE_OPERATIONS_DIR)
            .join(format!("{operation_id}.json"))
    }
}

fn installed_action(action: DaemonLifecycleAction) -> RefineResult<InstalledServiceAction> {
    match action {
        DaemonLifecycleAction::Stop => Ok(InstalledServiceAction::Stop),
        DaemonLifecycleAction::Restart => Ok(InstalledServiceAction::Restart),
        DaemonLifecycleAction::Start => Err(RefineError::InvalidInput(
            "HTTP start does not require a restart-safe lifecycle handoff".to_string(),
        )),
    }
}

fn operation_action_name(action: DaemonLifecycleAction) -> RefineResult<&'static str> {
    match action {
        DaemonLifecycleAction::Stop => Ok("stop"),
        DaemonLifecycleAction::Restart => Ok("restart"),
        DaemonLifecycleAction::Start => Err(RefineError::InvalidInput(
            "daemon lifecycle operations support only stop and restart".to_string(),
        )),
    }
}

fn validate_operation_id(operation_id: &str) -> RefineResult<()> {
    if operation_id.starts_with("daemon-lifecycle-")
        && operation_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(
            "invalid daemon lifecycle operation id".to_string(),
        ))
    }
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339()
}
