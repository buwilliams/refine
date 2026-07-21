use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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
use crate::tools::product::project_registry::FileProjectRegistryService;
use crate::tools::product::project_state::{FileProjectStateStore, ProjectStateStore};
use crate::workflow::WorkflowEngine;

pub const WORKFLOW_RUNNER: &str = "workflow";
pub const GIT_SYNC_RUNNER: &str = "git-sync";
pub const PROJECT_SYNC_RUNNER: &str = "project-sync";

const WORKFLOW_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_REMOTE_FETCH_INTERVAL: Duration = Duration::from_secs(300);
const DEFAULT_GIT_SYNC_DEBOUNCE: Duration = Duration::from_secs(5);
const GIT_RECONCILE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const GIT_RECONCILE_RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct FileRunnerWorkerService {
    pub runtime_root: PathBuf,
}

impl FileRunnerWorkerService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub fn ensure_background_worker(&self, worker_kind: &str) -> RefineResult<ManagedProcess> {
        validate_worker_kind(worker_kind, false)?;
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        if let Some(process) = supervisor
            .recover_owner(ProcessOwner::Runner)?
            .into_iter()
            .find(|process| managed_worker_kind(process) == Some(worker_kind))
        {
            return Ok(process);
        }
        let executable = std::env::current_exe().map_err(|error| {
            RefineError::Io(format!("failed to locate runner executable: {error}"))
        })?;
        supervisor.launch(background_worker_spec(
            &executable,
            &self.runtime_root,
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
}

pub fn run_worker(
    worker_kind: &str,
    runtime_root: PathBuf,
    target_root: Option<PathBuf>,
    operation_id: Option<String>,
) -> RefineResult<()> {
    validate_worker_kind(worker_kind, true)?;
    match worker_kind {
        WORKFLOW_RUNNER => run_workflow_worker(&runtime_root),
        GIT_SYNC_RUNNER => run_git_sync_worker(&runtime_root),
        PROJECT_SYNC_RUNNER => {
            let target_root = target_root.ok_or_else(|| {
                RefineError::InvalidInput("project-sync worker requires --target-root".to_string())
            })?;
            let operation_id = operation_id.ok_or_else(|| {
                RefineError::InvalidInput("project-sync worker requires --operation-id".to_string())
            })?;
            run_project_sync_operation(&runtime_root, &target_root, &operation_id)
        }
        _ => unreachable!(),
    }
}

fn run_workflow_worker(runtime_root: &Path) -> RefineResult<()> {
    loop {
        if let Some(target_root) = current_target_root(runtime_root)? {
            match WorkflowEngine::with_target_root(runtime_root, &target_root).evaluate_workflow() {
                Ok(_) => {
                    let _ = refresh_projection(runtime_root, &target_root);
                }
                Err(RefineError::Conflict(message)) if message.contains("paused") => {}
                Err(error) => eprintln!("refine workflow runner: {error}"),
            }
        }
        thread::sleep(WORKFLOW_INTERVAL);
    }
}

fn run_git_sync_worker(runtime_root: &Path) -> RefineResult<()> {
    let mut active_root = None;
    let mut last_observed_fingerprint = None;
    let mut pending_sync = None;
    let mut next_remote_fetch = None;
    let mut active_schedule = None;
    let mut next_attempt = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_attempt {
            let Some(target_root) = current_target_root(runtime_root)? else {
                thread::sleep(GIT_RECONCILE_POLL_INTERVAL);
                continue;
            };
            let root = target_root
                .canonicalize()
                .unwrap_or_else(|_| target_root.clone());
            if active_root.as_ref() != Some(&root) {
                active_root = Some(root);
                last_observed_fingerprint = None;
                pending_sync = None;
                next_remote_fetch = None;
                active_schedule = None;
            }
            let service = FileGitSyncService::new(&target_root, runtime_root);
            if let Ok(fingerprint) = service.durable_state_fingerprint() {
                let schedule = git_sync_schedule(runtime_root, &target_root).unwrap_or_default();
                if active_schedule != Some(schedule) {
                    if pending_sync.is_some() {
                        pending_sync = Some(now + schedule.debounce);
                    }
                    next_remote_fetch = schedule
                        .remote_fetch_interval
                        .map(|interval| now + interval);
                    active_schedule = Some(schedule);
                }
                if last_observed_fingerprint != Some(fingerprint) {
                    last_observed_fingerprint = Some(fingerprint);
                    pending_sync = Some(now + schedule.debounce);
                }
                let demand_due = pending_sync.is_some_and(|deadline| now >= deadline);
                let remote_fetch_due = next_remote_fetch.is_some_and(|deadline| now >= deadline);
                if demand_due || remote_fetch_due {
                    let result = if remote_fetch_due {
                        service.try_sync()
                    } else {
                        service.try_sync_state()
                    };
                    match result {
                        Ok(result) if !result.deferred => {
                            last_observed_fingerprint = service
                                .durable_state_fingerprint()
                                .ok()
                                .or(Some(fingerprint));
                            pending_sync = None;
                            next_remote_fetch = schedule
                                .remote_fetch_interval
                                .map(|interval| now + interval);
                            next_attempt = now;
                            let _ = refresh_projection(runtime_root, &target_root);
                        }
                        Ok(_) | Err(_) => {
                            next_attempt = now + GIT_RECONCILE_RETRY_INTERVAL;
                        }
                    }
                }
            }
        }
        thread::sleep(GIT_RECONCILE_POLL_INTERVAL);
    }
}

fn run_project_sync_operation(
    runtime_root: &Path,
    target_root: &Path,
    operation_id: &str,
) -> RefineResult<()> {
    let registry = FileOperationRegistry::new(runtime_root);
    let result = (|| {
        registry.update_progress(
            operation_id,
            json!({"message": "Synchronizing Refine state"}),
        )?;
        let git_sync = FileGitSyncService::new(target_root, runtime_root).sync()?;
        registry.update_progress(operation_id, json!({"message": "Rebuilding projection"}))?;
        let projection = refresh_projection(runtime_root, target_root)?;
        Ok::<Value, RefineError>(project_sync_result(&git_sync, &projection))
    })();
    match result {
        Ok(result) => {
            registry.finish_with_result(operation_id, OperationState::Succeeded, result)?;
            Ok(())
        }
        Err(error) => {
            let _ = registry.fail_with_error(
                operation_id,
                json!({
                    "code": "project_sync_failed",
                    "message": error.to_string()
                }),
            );
            Err(error)
        }
    }
}

fn refresh_projection(
    runtime_root: &Path,
    target_root: &Path,
) -> RefineResult<crate::tools::product::project_state::ProjectionSnapshot> {
    let refine_dir = prepare_refine_dir(target_root)?;
    let store = FileProjectStateStore::with_runtime_root(&refine_dir, runtime_root);
    let projection = store.rebuild_projection()?;
    store.persist_projection_snapshot(&runtime_root.join("cache"), &projection)?;
    Ok(projection)
}

fn current_target_root(runtime_root: &Path) -> RefineResult<Option<PathBuf>> {
    Ok(FileProjectRegistryService::new(runtime_root, None)
        .load()?
        .active_app
        .map(PathBuf::from))
}

fn project_sync_result(
    git_sync: &GitSyncResult,
    projection: &crate::tools::product::project_state::ProjectionSnapshot,
) -> Value {
    json!({
        "http_status": 200,
        "ok": true,
        "message": "Refine state synchronized and projection rebuilt.",
        "projection_version": projection.version,
        "goal_count": projection.goals.len(),
        "feature_count": projection.features.len(),
        "git_sync": git_sync
    })
}

fn managed_worker_kind(process: &ManagedProcess) -> Option<&str> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| {
            details
                .get("worker_kind")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .and_then(|kind| match kind.as_str() {
            WORKFLOW_RUNNER => Some(WORKFLOW_RUNNER),
            GIT_SYNC_RUNNER => Some(GIT_SYNC_RUNNER),
            PROJECT_SYNC_RUNNER => Some(PROJECT_SYNC_RUNNER),
            _ => None,
        })
}

fn background_worker_spec(
    executable: &Path,
    runtime_root: &Path,
    worker_kind: &str,
) -> ManagedProcessSpec {
    runner_worker_spec(executable, runtime_root, worker_kind, None, None)
}

fn project_sync_worker_spec(
    executable: &Path,
    runtime_root: &Path,
    target_root: &Path,
    operation_id: &str,
) -> ManagedProcessSpec {
    runner_worker_spec(
        executable,
        runtime_root,
        PROJECT_SYNC_RUNNER,
        Some(target_root),
        Some(operation_id),
    )
}

fn runner_worker_spec(
    executable: &Path,
    runtime_root: &Path,
    worker_kind: &str,
    target_root: Option<&Path>,
    operation_id: Option<&str>,
) -> ManagedProcessSpec {
    let mut args = vec![
        "system".to_string(),
        "runner-worker".to_string(),
        "--kind".to_string(),
        worker_kind.to_string(),
        "--port-runtime-root".to_string(),
        runtime_root.display().to_string(),
    ];
    if let Some(target_root) = target_root {
        args.extend([
            "--target-root".to_string(),
            target_root.display().to_string(),
        ]);
    }
    if let Some(operation_id) = operation_id {
        args.extend(["--operation-id".to_string(), operation_id.to_string()]);
    }
    ManagedProcessSpec {
        owner: ProcessOwner::Runner,
        command: executable.display().to_string(),
        args,
        cwd: None,
        env: Vec::new(),
        stdin: None,
        limits: Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        }),
        authorization_command: Some(format!("refine runner {worker_kind}")),
        sensitive: false,
        metadata: serde_json::from_value(json!({
            "kind": "runner",
            "worker_kind": worker_kind,
            "operation_id": operation_id
        }))
        .unwrap_or_default(),
    }
}

fn validate_worker_kind(worker_kind: &str, allow_one_shot: bool) -> RefineResult<()> {
    if matches!(worker_kind, WORKFLOW_RUNNER | GIT_SYNC_RUNNER)
        || (allow_one_shot && worker_kind == PROJECT_SYNC_RUNNER)
    {
        return Ok(());
    }
    Err(RefineError::InvalidInput(format!(
        "unknown runner worker kind {worker_kind}"
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GitSyncSchedule {
    debounce: Duration,
    remote_fetch_interval: Option<Duration>,
}

impl Default for GitSyncSchedule {
    fn default() -> Self {
        Self {
            debounce: DEFAULT_GIT_SYNC_DEBOUNCE,
            remote_fetch_interval: Some(DEFAULT_REMOTE_FETCH_INTERVAL),
        }
    }
}

fn git_sync_schedule(runtime_root: &Path, target_root: &Path) -> RefineResult<GitSyncSchedule> {
    let refine_dir = prepare_refine_dir(target_root)?;
    let settings = FileSettingsService::with_active_root(refine_dir, runtime_root).load()?;
    Ok(GitSyncSchedule {
        debounce: positive_duration(
            settings.get("state_sync_debounce_seconds"),
            DEFAULT_GIT_SYNC_DEBOUNCE,
        ),
        remote_fetch_interval: optional_positive_duration(
            settings.get("project_update_pulse_interval_seconds"),
            DEFAULT_REMOTE_FETCH_INTERVAL,
        ),
    })
}

fn positive_duration(value: Option<&Value>, fallback: Duration) -> Duration {
    let seconds = value
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback.as_secs() as i64);
    Duration::from_secs(seconds.max(1) as u64)
}

fn optional_positive_duration(value: Option<&Value>, fallback: Duration) -> Option<Duration> {
    let seconds = value
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback.as_secs() as i64);
    (seconds > 0).then(|| Duration::from_secs(seconds as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_specs_create_real_runner_processes() {
        let spec = project_sync_worker_spec(
            Path::new("/opt/refine"),
            Path::new("/tmp/run/8082"),
            Path::new("/tmp/app"),
            "OP1",
        );
        assert_eq!(spec.owner, ProcessOwner::Runner);
        assert_eq!(spec.metadata["kind"], "runner");
        assert_eq!(spec.metadata["worker_kind"], PROJECT_SYNC_RUNNER);
        assert!(spec.args.iter().any(|arg| arg == "--operation-id"));
        assert_eq!(
            spec.limits
                .as_ref()
                .map(|limits| limits.kill_on_parent_exit),
            Some(true)
        );
    }
}
