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
                    let workflow_error = workers
                        .ensure_background_worker(WORKFLOW_RUNNER)
                        .err()
                        .map(|error| format!("workflow runner: {error}"));
                    let cleanup_error = workers
                        .ensure_background_worker(WORKTREE_CLEANUP_RUNNER)
                        .err()
                        .map(|error| format!("worktree cleanup runner: {error}"));
                    let error = match (workflow_error, cleanup_error) {
                        (Some(workflow), Some(cleanup)) => Some(format!("{workflow}; {cleanup}")),
                        (Some(error), None) | (None, Some(error)) => Some(error),
                        (None, None) => None,
                    };
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
                    let _ = workers.ensure_background_worker(GIT_SYNC_RUNNER);
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
