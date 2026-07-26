use super::*;

pub(super) fn current_target_root(runtime_root: &Path) -> RefineResult<Option<PathBuf>> {
    Ok(FileProjectRegistryService::new(runtime_root, None)
        .load()?
        .active_app
        .map(PathBuf::from))
}

pub(super) fn project_sync_result(
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

/// Whether an existing record is a worker supervision can rely on to keep
/// ticking. Adopting anything else makes supervision believe a worker exists
/// that will never tick again, and nothing relaunches it short of a daemon
/// restart. Recovery already drops settled records and revives nothing, so the
/// state check is a guard rather than the load-bearing part; the owner check is
/// not, because `recover_owner` returns records belonging to every owner and
/// only the caller's filter has been keeping foreign ones out.
pub(super) fn adoptable_worker(process: &ManagedProcess, worker_kind: &str) -> bool {
    process.owner == ProcessOwner::Runner
        && process.state == "running"
        && managed_worker_kind(process) == Some(worker_kind)
}

pub(super) fn managed_worker_kind(process: &ManagedProcess) -> Option<&str> {
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

pub(super) fn background_worker_spec(
    executable: &Path,
    runtime_root: &Path,
    worker_kind: &str,
) -> ManagedProcessSpec {
    runner_worker_spec(executable, runtime_root, worker_kind, None, None)
}

pub(super) fn project_sync_worker_spec(
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

pub(super) fn jira_export_worker_spec(
    executable: &Path,
    runtime_root: &Path,
    operation_id: &str,
) -> ManagedProcessSpec {
    let mut spec = runner_worker_spec(
        executable,
        runtime_root,
        JIRA_EXPORT_RUNNER,
        None,
        Some(operation_id),
    );
    // The HTTP worker thread that queues this process is intentionally short-lived. The daemon's
    // process supervisor owns termination, so the Jira worker must not inherit thread-lifetime
    // parent-death behavior.
    if let Some(limits) = &mut spec.limits {
        limits.kill_on_parent_exit = false;
    }
    spec
}

#[cfg(test)]
pub(super) fn jira_export_test_worker_spec(
    runtime_root: &Path,
    operation_id: &str,
) -> RefineResult<ManagedProcessSpec> {
    let executable = std::env::current_exe().map_err(|error| {
        RefineError::Io(format!("failed to locate Jira export test worker: {error}"))
    })?;
    let mut spec = jira_export_worker_spec(&executable, runtime_root, operation_id);
    spec.args = vec![
        "--exact".to_string(),
        "surfaces::cli::tests::goals::exports::jira_export_worker_cli_process_helper".to_string(),
        "--nocapture".to_string(),
    ];
    spec.env = vec![
        (
            "REFINE_TEST_JIRA_RUNTIME_ROOT".to_string(),
            runtime_root.display().to_string(),
        ),
        (
            "REFINE_TEST_JIRA_OPERATION_ID".to_string(),
            operation_id.to_string(),
        ),
        (
            "REFINE_TEST_JIRA_WORKER_DELAY_MS".to_string(),
            "500".to_string(),
        ),
    ];
    Ok(spec)
}

pub(super) fn runner_worker_spec(
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
