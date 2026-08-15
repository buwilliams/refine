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
use crate::tools::host::checkout::RefineCheckoutPaths;
use crate::tools::host::git_sync::{
    FileGitSyncService, GitSyncResult, StateSyncAttemptContext, latest_state_sync_conflict_report,
};
use crate::tools::host::project_layout::{prepare_refine_dir, refine_dir_for_target_root};
use crate::tools::host::state_sync_health::{FileStateSyncHealthService, StateSyncHealthActivity};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::product::chat::FileChatService;
use crate::tools::product::goal_exports::FileGoalExportService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_projection::FileProjectProjectionStore;
use crate::tools::product::project_registry::FileProjectRegistryService;
use crate::tools::product::work_items::BulkGoalSelection;
use crate::tools::product::worktree_cleanup::{
    FileWorktreeCleanupService, WorktreeCleanupOptions, automatic_cleanup_delay_seconds,
    cleanup_failure_summary,
};
use crate::workflow::WorkflowEngine;

pub const WORKFLOW_RUNNER: &str = "workflow";
pub const WORKTREE_CLEANUP_RUNNER: &str = "worktree-cleanup";
pub const GIT_SYNC_RUNNER: &str = "git-sync";
pub const PROJECT_SYNC_RUNNER: &str = "project-sync";
pub const JIRA_EXPORT_RUNNER: &str = "jira-export";
pub const DEVELOPMENT_REQUEST_RUNNER: &str = "development-requests";
pub const BACKGROUND_RUNNERS: [&str; 4] = [
    GIT_SYNC_RUNNER,
    WORKFLOW_RUNNER,
    WORKTREE_CLEANUP_RUNNER,
    DEVELOPMENT_REQUEST_RUNNER,
];
const PAUSE_AWARE_BACKGROUND_RUNNERS: [&str; 2] = [GIT_SYNC_RUNNER, WORKTREE_CLEANUP_RUNNER];

const WORKFLOW_INTERVAL: Duration = Duration::from_secs(1);
const WORKTREE_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const WORKTREE_CLEANUP_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_REMOTE_FETCH_INTERVAL: Duration = Duration::from_secs(300);
const DEFAULT_GIT_SYNC_DEBOUNCE: Duration = Duration::from_secs(5);
const DEFAULT_STATE_SYNC_STALE_THRESHOLD: Duration = Duration::from_secs(900);
const GIT_RECONCILE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const GIT_RECONCILE_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const DEVELOPMENT_REQUEST_POLL_INTERVAL: Duration = Duration::from_secs(1);

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

mod development_requests;
mod dispatch;
mod git_sync;
mod jira_export;
mod project_sync;
mod schedule;
mod state_sync_health;
#[cfg(test)]
mod test_hooks;
mod worker_specs;
mod workflow;
mod worktree_cleanup;

pub use dispatch::run_worker;

use development_requests::*;
use git_sync::*;
use jira_export::*;
use project_sync::*;
pub(crate) use schedule::state_sync_stale_threshold;
use schedule::*;
use state_sync_health::*;
#[cfg(test)]
use test_hooks::*;
use worker_specs::*;
use workflow::*;
use worktree_cleanup::*;

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

    pub fn ensure_background_worker(
        &self,
        worker_kind: &str,
    ) -> RefineResult<BackgroundWorkerEnsure> {
        validate_worker_kind(worker_kind, false)?;
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        if supervisor
            .pause_state()?
            .disabled_background_workers
            .contains(worker_kind)
        {
            return Ok(BackgroundWorkerEnsure::Disabled);
        }
        if PAUSE_AWARE_BACKGROUND_RUNNERS.contains(&worker_kind)
            && self.background_automation_paused()?
        {
            return Ok(BackgroundWorkerEnsure::Paused);
        }
        let recovered = supervisor.recover_owner(ProcessOwner::Runner)?;
        if let Some(process) = recovered.iter().into_iter().find(|process| {
            adoptable_worker(process, worker_kind, self.project_registry_root.as_deref())
        }) {
            return Ok(BackgroundWorkerEnsure::Running(Box::new(process.clone())));
        }
        for process in recovered.into_iter().filter(|process| {
            process.owner == ProcessOwner::Runner
                && process.state == "running"
                && managed_worker_kind(process) == Some(worker_kind)
        }) {
            let _ = supervisor.signal(&process.id, "terminate");
        }
        #[cfg(test)]
        run_background_worker_hook(worker_kind, BackgroundWorkerBoundary::EnsureLaunch);
        if supervisor
            .pause_state()?
            .disabled_background_workers
            .contains(worker_kind)
        {
            return Ok(BackgroundWorkerEnsure::Disabled);
        }
        if PAUSE_AWARE_BACKGROUND_RUNNERS.contains(&worker_kind)
            && self.background_automation_paused()?
        {
            return Ok(BackgroundWorkerEnsure::Paused);
        }
        let executable = runner_executable(&self.runtime_root)?;
        supervisor
            .launch(background_worker_spec(
                &executable,
                &self.runtime_root,
                self.project_registry_root.as_deref(),
                worker_kind,
            ))
            .map(Box::new)
            .map(BackgroundWorkerEnsure::Running)
    }

    pub fn background_automation_paused(&self) -> RefineResult<bool> {
        Ok(FileProcessSupervisor::new(&self.runtime_root)
            .pause_state()?
            .workflow_paused)
    }

    pub fn set_background_worker_enabled(
        &self,
        worker_kind: &str,
        enabled: bool,
    ) -> RefineResult<BackgroundWorkerEnsure> {
        validate_worker_kind(worker_kind, false)?;
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        supervisor.set_background_worker_enabled(worker_kind, enabled)?;
        if enabled {
            return self.ensure_background_worker(worker_kind);
        }

        for process in supervisor.recover_owner(ProcessOwner::Runner)? {
            if process.state == "running" && managed_worker_kind(&process) == Some(worker_kind) {
                let outcome = supervisor.terminate_owned_and_confirm_exit(
                    &process,
                    "kill",
                    Duration::from_secs(2),
                )?;
                supervisor
                    .cleanup_confirmed_exit(&process, outcome)
                    .map_err(|failure| failure.error)?;
            }
        }
        Ok(BackgroundWorkerEnsure::Disabled)
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
        let executable = runner_executable(&self.runtime_root)?;
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
            let executable = runner_executable(&self.runtime_root)?;
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

#[cfg(not(test))]
fn runner_executable(runtime_root: &Path) -> RefineResult<PathBuf> {
    let executable = std::env::current_exe()
        .map_err(|error| RefineError::Io(format!("failed to locate runner executable: {error}")))?;
    runner_executable_for_invocation(runtime_root, &executable)
}

fn runner_executable_for_invocation(
    runtime_root: &Path,
    executable: &Path,
) -> RefineResult<PathBuf> {
    let (paths, _) = RefineCheckoutPaths::from_port_runtime_root(runtime_root)?;
    paths.require_owned_executable(executable)
}

#[cfg(test)]
fn runner_executable(_runtime_root: &Path) -> RefineResult<PathBuf> {
    std::env::current_exe()
        .map_err(|error| RefineError::Io(format!("failed to locate runner executable: {error}")))
}

#[derive(Clone, Debug)]
pub enum BackgroundWorkerEnsure {
    Running(Box<ManagedProcess>),
    Paused,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackgroundOperationOutcome<T> {
    Completed(T),
    Paused,
}

fn run_background_repository_operation<T>(
    runtime_root: &Path,
    worker_kind: &'static str,
    operation: impl FnOnce() -> T,
) -> RefineResult<BackgroundOperationOutcome<T>> {
    let _ = worker_kind;
    if background_automation_is_paused(runtime_root)? {
        return Ok(BackgroundOperationOutcome::Paused);
    }
    #[cfg(test)]
    run_background_worker_hook(worker_kind, BackgroundWorkerBoundary::BeforeOperation);
    if background_automation_is_paused(runtime_root)? {
        return Ok(BackgroundOperationOutcome::Paused);
    }
    let result = operation();
    #[cfg(test)]
    run_background_worker_hook(worker_kind, BackgroundWorkerBoundary::AfterOperation);
    Ok(BackgroundOperationOutcome::Completed(result))
}

pub(super) fn background_automation_is_paused(runtime_root: &Path) -> RefineResult<bool> {
    FileRunnerWorkerService::new(runtime_root).background_automation_paused()
}

#[cfg(test)]
mod tests;
