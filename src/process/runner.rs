use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
    ProcessSupervisor,
};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::tools::host::git_sync::{FileGitSyncService, GitSyncResult};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::product::chat::FileChatService;
use crate::tools::product::goal_exports::FileGoalExportService;
use crate::tools::product::project_registry::FileProjectRegistryService;
use crate::tools::product::project_state::{FileProjectStateStore, ProjectStateStore};
use crate::tools::product::work_items::BulkGoalSelection;
use crate::workflow::WorkflowEngine;

pub const WORKFLOW_RUNNER: &str = "workflow";
pub const GIT_SYNC_RUNNER: &str = "git-sync";
pub const PROJECT_SYNC_RUNNER: &str = "project-sync";
pub const JIRA_EXPORT_RUNNER: &str = "jira-export";

const WORKFLOW_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_REMOTE_FETCH_INTERVAL: Duration = Duration::from_secs(300);
const DEFAULT_GIT_SYNC_DEBOUNCE: Duration = Duration::from_secs(5);
const GIT_RECONCILE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const GIT_RECONCILE_RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct FileRunnerWorkerService {
    pub runtime_root: PathBuf,
    pub project_registry_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct JiraExportOperationRequest {
    refine_dir: PathBuf,
    target_root: PathBuf,
    selection: BulkGoalSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_identity: Option<String>,
}

mod dispatch;
mod git_sync;
mod jira_export;
mod project_sync;
mod schedule;
mod worker_specs;
mod workflow;

pub use dispatch::run_worker;

use git_sync::*;
use jira_export::*;
use project_sync::*;
use schedule::*;
use worker_specs::*;
use workflow::*;

impl FileRunnerWorkerService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            project_registry_root: None,
        }
    }

    pub fn with_project_registry_root(mut self, project_registry_root: impl Into<PathBuf>) -> Self {
        self.project_registry_root = Some(project_registry_root.into());
        self
    }

    pub fn ensure_background_worker(&self, worker_kind: &str) -> RefineResult<ManagedProcess> {
        validate_worker_kind(worker_kind, false)?;
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        let recovered = supervisor.recover_owner(ProcessOwner::Runner)?;
        if let Some(process) = recovered.iter().into_iter().find(|process| {
            adoptable_worker(process, worker_kind, self.project_registry_root.as_deref())
        }) {
            return Ok(process.clone());
        }
        for process in recovered.into_iter().filter(|process| {
            process.owner == ProcessOwner::Runner
                && process.state == "running"
                && managed_worker_kind(process) == Some(worker_kind)
        }) {
            let _ = supervisor.signal(&process.id, "terminate");
        }
        let executable = std::env::current_exe().map_err(|error| {
            RefineError::Io(format!("failed to locate runner executable: {error}"))
        })?;
        supervisor.launch(background_worker_spec(
            &executable,
            &self.runtime_root,
            self.project_registry_root.as_deref(),
            worker_kind,
        ))
    }

    pub fn queue_project_sync(&self, target_root: &Path) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let operation = registry.register("project:sync")?;
        let _ = registry.update_progress(
            &operation.id,
            json!({"message": "Waiting for the repository worker"}),
        );
        #[cfg(test)]
        {
            let runtime_root = self.runtime_root.clone();
            let target_root = target_root.to_path_buf();
            let operation_id = operation.id.clone();
            thread::spawn(move || {
                let _ = run_project_sync_operation(&runtime_root, &target_root, &operation_id);
            });
            registry.status(&operation.id)
        }
        #[cfg(not(test))]
        let executable = std::env::current_exe().map_err(|error| {
            RefineError::Io(format!("failed to locate runner executable: {error}"))
        })?;
        #[cfg(not(test))]
        let spec =
            project_sync_worker_spec(&executable, &self.runtime_root, target_root, &operation.id);
        #[cfg(not(test))]
        match FileProcessSupervisor::new(&self.runtime_root).launch(spec) {
            Ok(_) => registry.status(&operation.id),
            Err(error) => {
                let _ = registry.fail_with_error(
                    &operation.id,
                    json!({
                        "code": "runner_launch_failed",
                        "message": error.to_string()
                    }),
                );
                Err(error)
            }
        }
    }

    pub fn queue_jira_export(
        &self,
        refine_dir: &Path,
        target_root: &Path,
        selection: BulkGoalSelection,
    ) -> RefineResult<OperationHandle> {
        self.queue_jira_export_request(JiraExportOperationRequest {
            refine_dir: refine_dir.to_path_buf(),
            target_root: target_root.to_path_buf(),
            selection,
            recovery_of: None,
            retry_identity: None,
        })
    }

    pub fn retry_jira_export(&self, operation_id: &str) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let interrupted = registry.status(operation_id)?;
        if interrupted.owner != "goals:jira-export"
            || !matches!(interrupted.state, OperationState::Interrupted)
        {
            return Err(RefineError::Conflict(
                "Only interrupted Jira export operations can be recovered".to_string(),
            ));
        }
        let mut request = serde_json::from_value::<JiraExportOperationRequest>(interrupted.request)
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to recover Jira export request: {error}"
                ))
            })?;
        let retry_identity = format!("goals:jira-export:retry:{operation_id}");
        request.recovery_of = Some(operation_id.to_string());
        request.retry_identity = Some(retry_identity.clone());
        let registration = registry.find_or_register_replacement(
            "goals:jira-export",
            operation_id,
            &retry_identity,
            serde_json::to_value(&request).map_err(|error| {
                RefineError::Serialization(format!("failed to encode Jira export request: {error}"))
            })?,
        )?;
        if !registration.created {
            return Ok(registration.operation);
        }
        self.start_jira_export_operation(&registry, registration.operation)
    }

    fn queue_jira_export_request(
        &self,
        request: JiraExportOperationRequest,
    ) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let operation = registry.register_with_request(
            "goals:jira-export",
            serde_json::to_value(&request).map_err(|error| {
                RefineError::Serialization(format!("failed to encode Jira export request: {error}"))
            })?,
        )?;
        self.start_jira_export_operation(&registry, operation)
    }

    fn start_jira_export_operation(
        &self,
        registry: &FileOperationRegistry,
        operation: OperationHandle,
    ) -> RefineResult<OperationHandle> {
        registry.update_progress(
            &operation.id,
            json!({
                "stage": "queued",
                "message": "Waiting for the Jira export worker",
                "completed": 0,
                "total": 0
            }),
        )?;
        let operation = registry.status(&operation.id)?;
        #[cfg(test)]
        let spec = jira_export_test_worker_spec(&self.runtime_root, &operation.id)?;
        #[cfg(not(test))]
        let spec = {
            let executable = std::env::current_exe().map_err(|error| {
                RefineError::Io(format!("failed to locate runner executable: {error}"))
            })?;
            jira_export_worker_spec(&executable, &self.runtime_root, &operation.id)
        };
        self.launch_jira_export_worker(registry, &operation, spec)
    }

    fn launch_jira_export_worker(
        &self,
        registry: &FileOperationRegistry,
        operation: &OperationHandle,
        spec: ManagedProcessSpec,
    ) -> RefineResult<OperationHandle> {
        match FileProcessSupervisor::new(&self.runtime_root).launch(spec) {
            Ok(_) => registry.status(&operation.id),
            Err(error) => {
                let _ = registry.fail_with_error(
                    &operation.id,
                    json!({
                        "code": "runner_launch_failed",
                        "message": error.to_string()
                    }),
                );
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests;
