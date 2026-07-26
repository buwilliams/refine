use super::*;

pub(super) fn run_git_sync_worker(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<()> {
    let mut active_root = None;
    let mut last_observed_fingerprint = None;
    let mut pending_sync = None;
    let mut next_remote_fetch = None;
    let mut active_schedule = None;
    let mut next_attempt = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_attempt {
            let Some(target_root) = current_target_root(runtime_root, project_registry_root)?
            else {
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
                        Ok(_) => {
                            next_attempt = now + GIT_RECONCILE_RETRY_INTERVAL;
                        }
                        Err(_error) => {
                            next_attempt = now + GIT_RECONCILE_RETRY_INTERVAL;
                        }
                    }
                }
            }
        }
        thread::sleep(GIT_RECONCILE_POLL_INTERVAL);
    }
}
