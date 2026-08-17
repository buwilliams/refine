use super::*;

pub(super) fn run_git_sync_worker(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<()> {
    let mut active_root: Option<PathBuf> = None;
    let mut active_node_id: Option<String> = None;
    let mut last_observed_fingerprint = None;
    let mut pending_sync = None;
    // When the oldest unpublished change was first observed. The debounce is
    // rolling — each new change pushes the deadline out — so without a cap a
    // sustained mutation stream postponed publishing indefinitely.
    let mut pending_since: Option<Instant> = None;
    let mut next_remote_fetch = None;
    let mut active_schedule = None;
    let mut failure_backoff = GitSyncFailureBackoff::default();
    let mut next_attempt = Instant::now();
    loop {
        if background_automation_is_paused(runtime_root)? {
            return Ok(());
        }
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
            let observed_node_id = state_sync_node_id(runtime_root, &target_root)?;
            if active_root.as_ref() != Some(&root)
                || active_node_id.as_deref() != Some(observed_node_id.as_str())
            {
                // Resume from the persisted fingerprint: without it, every
                // worker restart looked like a state change and scheduled a
                // spurious sync.
                last_observed_fingerprint = load_persisted_fingerprint(runtime_root, &root);
                active_root = Some(root.clone());
                active_node_id = Some(observed_node_id.clone());
                pending_sync = None;
                pending_since = None;
                next_remote_fetch = None;
                active_schedule = None;
                failure_backoff.reset();
                bind_state_sync_health(runtime_root, &target_root, &observed_node_id)?;
            }
            let service = FileGitSyncService::new(&target_root, runtime_root);
            if let Ok(fingerprint) = service.durable_state_fingerprint() {
                failure_backoff.observe(&root, &observed_node_id, fingerprint);
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
                    let first_change = *pending_since.get_or_insert(now);
                    // Quiet-period debounce with a hard cap: the deadline
                    // follows the newest change but never drifts further than
                    // the cap from the oldest unpublished one.
                    let cap = first_change + max_publish_delay(schedule.debounce);
                    pending_sync = Some((now + schedule.debounce).min(cap));
                }
                let demand_due = pending_sync.is_some_and(|deadline| now >= deadline);
                let remote_fetch_due = next_remote_fetch.is_some_and(|deadline| now >= deadline);
                if demand_due || remote_fetch_due {
                    let node_id = state_sync_node_id(runtime_root, &target_root)?;
                    if !failure_backoff.allows_attempt(now, false) {
                        // Suppression is deliberately silent: no health attempt
                        // and no activity record exists for work not launched.
                        next_attempt = now;
                        thread::sleep(GIT_RECONCILE_POLL_INTERVAL);
                        continue;
                    }
                    let source = if remote_fetch_due {
                        "background_remote_fetch"
                    } else {
                        "background_publish"
                    };
                    let attempt_id =
                        record_state_sync_attempt(runtime_root, &target_root, &node_id, source)?;
                    let result = match run_background_repository_operation(
                        runtime_root,
                        GIT_SYNC_RUNNER,
                        || {
                            service.try_sync_with_attempt(
                                remote_fetch_due,
                                StateSyncAttemptContext::new(attempt_id.to_string(), source),
                            )
                        },
                    )? {
                        BackgroundOperationOutcome::Completed(result) => result,
                        BackgroundOperationOutcome::Paused => return Ok(()),
                    };
                    match result {
                        Ok(result) if !result.deferred => {
                            failure_backoff.reset();
                            record_state_sync_result(
                                runtime_root,
                                &target_root,
                                &node_id,
                                attempt_id,
                                source,
                                &result,
                            )?;
                            last_observed_fingerprint = service
                                .durable_state_fingerprint()
                                .ok()
                                .or(Some(fingerprint));
                            if let (Some(root), Some(fingerprint)) =
                                (active_root.as_ref(), last_observed_fingerprint)
                            {
                                persist_fingerprint(runtime_root, root, fingerprint);
                            }
                            pending_sync = None;
                            pending_since = None;
                            next_remote_fetch = schedule
                                .remote_fetch_interval
                                .map(|interval| now + interval);
                            next_attempt = now;
                            let _ = refresh_projection(runtime_root, &target_root);
                        }
                        Ok(result) => {
                            record_state_sync_result(
                                runtime_root,
                                &target_root,
                                &node_id,
                                attempt_id,
                                source,
                                &result,
                            )?;
                            next_attempt = now + GIT_RECONCILE_RETRY_INTERVAL;
                        }
                        Err(error) => {
                            let conflict_report = latest_state_sync_conflict_report(runtime_root)
                                .ok()
                                .flatten()
                                .filter(|report| {
                                    report.attempt_id == attempt_id.to_string()
                                        && report.attempt_source == source
                                });
                            let failure_context =
                                git_sync_failure_context(&error, conflict_report.as_ref());
                            record_state_sync_failure(
                                runtime_root,
                                &target_root,
                                &node_id,
                                attempt_id,
                                source,
                                &error,
                            )?;
                            failure_backoff.record_failure(now, &failure_context);
                            match attempt_automatic_state_recovery(
                                runtime_root,
                                &target_root,
                                &node_id,
                                &service,
                                &error,
                                conflict_report.is_some(),
                            )? {
                                AutoRecoveryOutcome::Recovered => {
                                    failure_backoff.reset();
                                    last_observed_fingerprint = service
                                        .durable_state_fingerprint()
                                        .ok()
                                        .or(Some(fingerprint));
                                    if let (Some(root), Some(fingerprint)) =
                                        (active_root.as_ref(), last_observed_fingerprint)
                                    {
                                        persist_fingerprint(runtime_root, root, fingerprint);
                                    }
                                    pending_sync = None;
                                    pending_since = None;
                                    let _ = refresh_projection(runtime_root, &target_root);
                                }
                                AutoRecoveryOutcome::Paused => return Ok(()),
                                AutoRecoveryOutcome::NotAttempted | AutoRecoveryOutcome::Failed => {
                                }
                            }
                            next_attempt = now;
                        }
                    }
                }
            }
        }
        thread::sleep(GIT_RECONCILE_POLL_INTERVAL);
    }
}

/// The hard ceiling on how long publishing may trail the oldest unpublished
/// change while new changes keep arriving.
fn max_publish_delay(debounce: Duration) -> Duration {
    debounce.saturating_mul(6)
}

fn fingerprint_state_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join("git-sync-fingerprint.json")
}

fn load_persisted_fingerprint(runtime_root: &Path, root: &Path) -> Option<u64> {
    let bytes = std::fs::read(fingerprint_state_path(runtime_root)).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    if value.get("root").and_then(serde_json::Value::as_str)
        != Some(root.display().to_string().as_str())
    {
        return None;
    }
    value.get("fingerprint").and_then(serde_json::Value::as_u64)
}

fn persist_fingerprint(runtime_root: &Path, root: &Path, fingerprint: u64) {
    let payload = serde_json::json!({
        "root": root.display().to_string(),
        "fingerprint": fingerprint,
    });
    let path = fingerprint_state_path(runtime_root);
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if std::fs::write(&temp, payload.to_string()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}
