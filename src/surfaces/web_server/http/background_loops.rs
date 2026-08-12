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

fn ensure_worker_failure(workers: &FileRunnerWorkerService, worker_kind: &str) -> Option<String> {
    match workers.ensure_background_worker(worker_kind) {
        Ok(BackgroundWorkerEnsure::Running(_)) | Ok(BackgroundWorkerEnsure::Paused) => None,
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
