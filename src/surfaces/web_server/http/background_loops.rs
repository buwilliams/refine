use super::*;

impl LocalHttpDaemon {
    #[cfg(not(test))]
    pub(super) fn start_agent_automation_loop(&self, interval: Duration) -> AgentWorkflowLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let runtime_root = self.server.runtime_root.clone();
        let project_registry_root = self.server.app_registry_runtime_root();
        let interval = interval.max(Duration::from_millis(100));
        let mut workers = runtime_root.as_ref().map(FileRunnerWorkerService::new);
        if let (Some(workers), Some(registry)) = (&mut workers, project_registry_root) {
            *workers = workers.clone().with_project_registry_root(registry);
        }
        if let Some(workers) = workers.clone() {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Err(error) =
                        crate::application::workflow::health::admission::observe_admission(
                            &workers.runtime_root,
                            workers.project_registry_root.as_deref(),
                        )
                    {
                        eprintln!("refine admission observation: {error}");
                    }
                    sleep_until_stopped(&stop, interval);
                }
            });
        }
        if let Some(workers) = workers.clone() {
            {
                let kind = WORKTREE_CLEANUP_RUNNER;
                let workers = workers.clone();
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        if let Some(error) = ensure_worker_failure(&workers, kind) {
                            eprintln!("refine cleanup supervision: {error}");
                        }
                        sleep_until_stopped(&stop, interval);
                    }
                });
            }
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    crate::application::workers::maintenance::maintain_daemon(
                        &workers.runtime_root,
                    );
                    sleep_until_stopped(&stop, interval);
                }
            });
        }
        let handle = thread::spawn(move || {
            let mut last_failure = None;
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(workers) = &workers {
                    let failure = ensure_worker_failure(workers, WORKFLOW_RUNNER);
                    if failure != last_failure {
                        if let Some(error) = &failure {
                            eprintln!("refine workflow supervision: {error}");
                        }
                        last_failure = failure;
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
        let _ =
            crate::infrastructure::observability::activity::FileActivityService::new(&refine_dir)
                .cleanup(ACTIVITY_RETENTION_DAYS, false);
        remove_files_older_than(&refine_dir.join("support-bundles"), 30 * RETENTION_DAY);
    }
    let Some(runtime_root) = server.runtime_root.clone() else {
        return;
    };
    let _ = crate::infrastructure::observability::metrics::FileMetricsService::new(&runtime_root)
        .cleanup(false);
    let _ = crate::infrastructure::process::supervisor::operations::FileOperationRegistry::new(
        &runtime_root,
    )
    .cleanup_terminal_operations(7 * RETENTION_DAY);
    remove_files_older_than(&runtime_root.join("idempotency"), IDEMPOTENCY_RETENTION);
    remove_files_older_than(&runtime_root.join("process-history"), 30 * RETENTION_DAY);
    sweep_orphan_process_logs(&runtime_root.join("processes"), 7 * RETENTION_DAY);
    prune_all_but_newest(&runtime_root.join("source-promotion"), 2);
    rotate_if_large(
        &runtime_root
            .join(crate::infrastructure::process::supervisor::security::SECURITY_AUDIT_FILE),
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

/// Retention is a shared ownership decision, including absent primary records.
fn sweep_orphan_process_logs(processes_dir: &Path, retention: Duration) {
    if let Some(runtime) = processes_dir.parent() {
        crate::infrastructure::process::subprocess::FileProcessSupervisor::new(runtime)
            .retire_aged_process_logs(retention);
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
    use crate::infrastructure::process::subprocess::FileProcessSupervisor;

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

#[cfg(test)]
mod workflow_shutdown_tests {
    use super::*;
    #[test]
    fn workflow_degradation_does_not_trigger_lifecycle_shutdown() {
        let root =
            std::env::temp_dir().join(format!("refine-workflow-shutdown-{}", uuid::Uuid::new_v4()));
        let port = 4599;
        let runtime =
            crate::infrastructure::process::supervisor::runtime::RuntimeRoot { root: root.clone() };
        let lifecycle =
            crate::infrastructure::process::supervisor::lifecycle::FileDaemonLifecycleService::new(
                runtime.clone(),
            );
        let status = lifecycle.prepare_start(port).unwrap();
        let mut ready = lifecycle.mark_ready(status).unwrap();
        ready.daemon_healthy = false;
        std::fs::write(
            runtime.port_root(port).join("daemon-status.json"),
            serde_json::to_vec(&ready).unwrap(),
        )
        .unwrap();
        let mut shutdown = lifecycle_shutdown(lifecycle.clone(), port);
        thread::sleep(Duration::from_millis(650));
        assert!(matches!(
            shutdown.receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        lifecycle
            .mark_start_failed(port, &RefineError::Degraded("test shutdown".into()))
            .unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(2), shutdown.receiver)
                    .await
                    .unwrap()
                    .unwrap();
            });
        DAEMON_SHUTTING_DOWN.store(false, Ordering::SeqCst);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "background_loops_retention_tests.rs"]
mod retention_tests;
