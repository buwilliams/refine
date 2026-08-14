use super::*;

impl LocalHttpDaemon {
    #[cfg(not(test))]
    pub(super) fn start_agent_automation_loop(&self, interval: Duration) -> AgentWorkflowLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let runtime_root = self.server.runtime_root.clone();
        let project_registry_root = self.server.app_registry_runtime_root();
        let interval = interval.max(Duration::from_millis(100));
        let handle = thread::spawn(move || {
            let mut last_reported_failure: Option<String> = None;
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(runtime_root) = &runtime_root {
                    let mut workers = FileRunnerWorkerService::new(runtime_root);
                    if let Some(project_registry_root) = &project_registry_root {
                        workers = workers.with_project_registry_root(project_registry_root);
                    }
                    // Supervise these independently: cleanup must keep running
                    // even if workflow execution itself cannot be launched.
                    let workflow_error = ensure_worker_failure(&workers, WORKFLOW_RUNNER);
                    let cleanup_error = ensure_worker_failure(&workers, WORKTREE_CLEANUP_RUNNER);
                    let development_request_error =
                        match load_self_development_email_config(runtime_root) {
                            Ok(Some(_)) => {
                                ensure_worker_failure(&workers, DEVELOPMENT_REQUEST_RUNNER)
                            }
                            Ok(None) => None,
                            Err(error) => Some(format!("self-development email contract: {error}")),
                        };
                    let failures = [workflow_error, cleanup_error, development_request_error]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>();
                    let error = (!failures.is_empty()).then(|| failures.join("; "));
                    if let Some(error) = error {
                        // A stall otherwise looks exactly like an idle queue.
                        // Report it only when it changes: this loop runs every second.
                        if last_reported_failure.as_deref() != Some(error.as_str()) {
                            eprintln!(
                                "refine runner supervision: could not ensure a background runner is running: {error}"
                            );
                            last_reported_failure = Some(error);
                        }
                    } else {
                        last_reported_failure = None;
                    }
                }
                sleep_until_stopped(&thread_stop, interval);
            }
        });
        AgentWorkflowLoop {
            stop,
            handle: Some(handle),
        }
    }

    #[cfg(not(test))]
    pub(super) fn start_git_sync_loop(&self) -> GitSyncLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let runtime_root = self.server.runtime_root.clone();
        let project_registry_root = self.server.app_registry_runtime_root();
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(runtime_root) = &runtime_root {
                    let mut workers = FileRunnerWorkerService::new(runtime_root);
                    if let Some(project_registry_root) = &project_registry_root {
                        workers = workers.with_project_registry_root(project_registry_root);
                    }
                    if let Some(error) = ensure_worker_failure(&workers, GIT_SYNC_RUNNER) {
                        eprintln!("refine git sync supervision: {error}");
                    }
                }
                sleep_until_stopped(&thread_stop, Duration::from_secs(1));
            }
        });
        GitSyncLoop {
            stop,
            handle: Some(handle),
        }
    }
}

impl LocalHttpDaemon {
    /// Hourly retention sweep. The activity and metrics cleanup routines
    /// existed but were reachable only from manual HTTP calls, and several
    /// runtime datasets had no deletion path at all, so disk usage grew
    /// monotonically for the life of an installation.
    pub(super) fn start_retention_loop(&self) -> RetentionLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let server = self.server.clone();
        let handle = thread::spawn(move || {
            // The first sweep waits out the boot window; retention is never
            // urgent enough to compete with startup.
            sleep_until_stopped(&thread_stop, RETENTION_FIRST_SWEEP_DELAY);
            while !thread_stop.load(Ordering::Relaxed) {
                run_retention_sweep(&server);
                sleep_until_stopped(&thread_stop, RETENTION_SWEEP_INTERVAL);
            }
        });
        RetentionLoop {
            stop,
            handle: Some(handle),
        }
    }
}

const RETENTION_FIRST_SWEEP_DELAY: Duration = Duration::from_secs(60);
const RETENTION_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const RETENTION_DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// Idempotency records replay a response to a retried request; retries arrive
/// within seconds, not days.
const IDEMPOTENCY_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const ACTIVITY_RETENTION_DAYS: i64 = 30;
const SECURITY_AUDIT_ROTATE_BYTES: u64 = 5_000_000;

fn run_retention_sweep(server: &Arc<InProcessWebServer>) {
    if let Ok(Some(refine_dir)) = server.current_refine_dir() {
        let _ = crate::tools::observability::activity::FileActivityService::new(&refine_dir)
            .cleanup(ACTIVITY_RETENTION_DAYS, false);
        remove_files_older_than(&refine_dir.join("support-bundles"), 30 * RETENTION_DAY);
    }
    let Some(runtime_root) = server.runtime_root.clone() else {
        return;
    };
    let _ = crate::tools::observability::metrics::FileMetricsService::new(&runtime_root)
        .cleanup(false);
    let _ = crate::process::supervisor::operations::FileOperationRegistry::new(&runtime_root)
        .cleanup_terminal_operations(7 * RETENTION_DAY);
    remove_files_older_than(&runtime_root.join("idempotency"), IDEMPOTENCY_RETENTION);
    remove_files_older_than(&runtime_root.join("process-history"), 30 * RETENTION_DAY);
    sweep_orphan_process_logs(&runtime_root.join("processes"), 7 * RETENTION_DAY);
    prune_all_but_newest(&runtime_root.join("source-promotion"), 2);
    rotate_if_large(
        &runtime_root.join(crate::process::supervisor::security::SECURITY_AUDIT_FILE),
        SECURITY_AUDIT_ROTATE_BYTES,
    );
}

fn remove_files_older_than(dir: &Path, retention: Duration) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(retention)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let is_old_file = entry
            .metadata()
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .is_some_and(|modified| modified < cutoff);
        if is_old_file {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Output logs and stdin captures whose process registration is gone are
/// orphans: the exit path archives history but leaves them behind, so any
/// process that ended on its own left its full output on disk forever.
fn sweep_orphan_process_logs(processes_dir: &Path, retention: Duration) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(retention)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let Ok(entries) = std::fs::read_dir(processes_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(process_id) = name
            .strip_suffix(".stdout.log")
            .or_else(|| name.strip_suffix(".stderr.log"))
            .or_else(|| name.strip_suffix(".stdin.txt"))
        else {
            continue;
        };
        if processes_dir.join(format!("{process_id}.json")).exists() {
            continue;
        }
        let is_old = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .is_some_and(|modified| modified < cutoff);
        if is_old {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Staged promotion binaries are only needed for the current handoff and one
/// fallback; older stages are dead weight at ~10-20 MB each.
fn prune_all_but_newest(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files = entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().ok(), entry.path()))
        })
        .collect::<Vec<_>>();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

fn rotate_if_large(path: &Path, rotate_bytes: u64) {
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > rotate_bytes) {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        let _ = std::fs::rename(path, std::path::PathBuf::from(rotated));
    }
}

fn ensure_worker_failure(workers: &FileRunnerWorkerService, worker_kind: &str) -> Option<String> {
    match workers.ensure_background_worker(worker_kind) {
        Ok(BackgroundWorkerEnsure::Running(_))
        | Ok(BackgroundWorkerEnsure::Paused)
        | Ok(BackgroundWorkerEnsure::Disabled) => None,
        Err(error) => Some(format!("{worker_kind} runner: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::subprocess::FileProcessSupervisor;

    #[test]
    fn paused_workers_are_quiet_but_pause_state_faults_are_reported() {
        let runtime_root = std::env::temp_dir().join(format!(
            "refine-background-supervision-{}",
            uuid::Uuid::new_v4()
        ));
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        supervisor.set_workflow_paused(true).unwrap();
        let workers = FileRunnerWorkerService::new(&runtime_root);
        for worker_kind in [GIT_SYNC_RUNNER, WORKTREE_CLEANUP_RUNNER] {
            assert_eq!(ensure_worker_failure(&workers, worker_kind), None);
        }

        std::fs::write(supervisor.pause_state_path(), "{invalid").unwrap();
        for worker_kind in [GIT_SYNC_RUNNER, WORKTREE_CLEANUP_RUNNER] {
            let error = ensure_worker_failure(&workers, worker_kind).unwrap();
            assert!(error.contains("failed to parse process control"), "{error}");
        }
        std::fs::remove_dir_all(runtime_root).unwrap();
    }
}
