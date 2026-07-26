use super::*;

impl LocalHttpDaemon {
    #[cfg(not(test))]
    pub(super) fn start_agent_automation_loop(&self, interval: Duration) -> AgentWorkflowLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let runtime_root = self.server.runtime_root.clone();
        let interval = interval.max(Duration::from_millis(100));
        let handle = thread::spawn(move || {
            let mut last_reported_failure: Option<String> = None;
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(runtime_root) = &runtime_root {
                    match FileRunnerWorkerService::new(runtime_root)
                        .ensure_background_worker(WORKFLOW_RUNNER)
                    {
                        Ok(_) => last_reported_failure = None,
                        // A stall here means nothing is ticking the workflow, which
                        // otherwise looks exactly like an idle queue. Report it, but
                        // only when it changes: this loop runs every second.
                        Err(error) => {
                            let error = error.to_string();
                            if last_reported_failure.as_deref() != Some(error.as_str()) {
                                eprintln!(
                                    "refine workflow supervision: could not ensure the workflow runner is running: {error}"
                                );
                                last_reported_failure = Some(error);
                            }
                        }
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
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(runtime_root) = &runtime_root {
                    let _ = FileRunnerWorkerService::new(runtime_root)
                        .ensure_background_worker(GIT_SYNC_RUNNER);
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
