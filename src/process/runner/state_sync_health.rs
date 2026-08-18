use super::*;

pub(super) fn state_sync_node_id(runtime_root: &Path, target_root: &Path) -> RefineResult<String> {
    let refine_dir = prepare_refine_dir(target_root)?;
    FileNodeRegistryService::with_active_root(refine_dir, runtime_root).active_node_id()
}

pub(super) fn bind_state_sync_health(
    runtime_root: &Path,
    target_root: &Path,
    node_id: &str,
) -> RefineResult<()> {
    FileStateSyncHealthService::new(runtime_root).bind(target_root, node_id)
}

pub(super) fn record_state_sync_attempt(
    runtime_root: &Path,
    target_root: &Path,
    node_id: &str,
    source: &str,
) -> RefineResult<u64> {
    FileStateSyncHealthService::new(runtime_root).begin_attempt(target_root, node_id, source)
}

pub(super) fn record_state_sync_result(
    runtime_root: &Path,
    target_root: &Path,
    node_id: &str,
    attempt_id: u64,
    source: &str,
    result: &GitSyncResult,
) -> RefineResult<()> {
    let health = FileStateSyncHealthService::new(runtime_root);
    let activity = if result.ok && result.attempted && result.remote_configured == Some(true) {
        health.settle_success(target_root, node_id, attempt_id, source)?
    } else {
        let outcome = if result.remote_configured == Some(false) {
            "unconfigured"
        } else if result.deferred {
            "deferred"
        } else {
            "skipped"
        };
        health.settle_neutral(
            target_root,
            node_id,
            attempt_id,
            source,
            outcome,
            result.remote_configured,
        )?;
        None
    };
    append_state_sync_activity(target_root, node_id, activity, None)
}

pub(super) fn record_state_sync_failure(
    runtime_root: &Path,
    target_root: &Path,
    node_id: &str,
    attempt_id: u64,
    source: &str,
    error: &RefineError,
) -> RefineResult<()> {
    let report = latest_state_sync_conflict_report(runtime_root)
        .ok()
        .flatten()
        .filter(|report| {
            report.attempt_id == attempt_id.to_string() && report.attempt_source == source
        });
    let activity = FileStateSyncHealthService::new(runtime_root).settle_failure(
        target_root,
        node_id,
        attempt_id,
        source,
        &error.to_string(),
        report.as_ref().map(|report| report.report_id.as_str()),
        report
            .as_ref()
            .map(|report| report.report_location.as_str()),
    )?;
    append_state_sync_activity(target_root, node_id, activity, report.as_ref())
}

/// The attempt source recorded for daemon-initiated recovery, so its health
/// episodes and activity are distinguishable from ordinary sync attempts.
pub(super) const AUTO_RECOVERY_SOURCE: &str = "auto_recovery";

/// Outcome of the automatic recovery that follows a rejected background sync.
pub(super) enum AutoRecoveryOutcome {
    /// The node opted out, or the rejection is not recoverable evidence.
    NotAttempted,
    Recovered,
    /// Recovery ran and failed; the failure was recorded as its own health
    /// attempt and the worker's ordinary backoff cadence retries it.
    Failed,
    Paused,
}

/// Converge a rejected background synchronization without operator action.
///
/// Basic syncing must not require CLI ceremony: when ordinary sync fails
/// closed with the conflict that attempt just recorded, the daemon runs the
/// same terminal `sync --authority` decision an operator would, with the ownership
/// policy read from the merge base so no side votes for itself: remote
/// authority by default, and a live override for each contested goal record
/// whose merge-base bytes name this node as `node_id`. The displaced side
/// stays reachable as a merge parent (or retained ref on a join). A node set
/// to `state_sync_auto_recovery: off` keeps the fail-closed behavior for
/// deliberate divergence work.
///
/// The recovery is its own health attempt under `auto_recovery`, so success
/// and failure flow through the same episode, reminder, and recovered
/// activity as every other sync attempt.
pub(super) fn attempt_automatic_state_recovery(
    runtime_root: &Path,
    target_root: &Path,
    node_id: &str,
    service: &FileGitSyncService,
    conflict_report: Option<&StateSyncConflictReport>,
) -> RefineResult<AutoRecoveryOutcome> {
    let Some(report) = conflict_report else {
        return Ok(AutoRecoveryOutcome::NotAttempted);
    };
    if !state_sync_auto_recovery_enabled(runtime_root, target_root) {
        return Ok(AutoRecoveryOutcome::NotAttempted);
    }
    let decision = service.merge_base_ownership_decision(report, node_id);
    let attempt_id =
        record_state_sync_attempt(runtime_root, target_root, node_id, AUTO_RECOVERY_SOURCE)?;
    let run = match run_background_repository_operation(runtime_root, GIT_SYNC_RUNNER, || {
        service.run_state_recovery(decision.clone())
    })? {
        BackgroundOperationOutcome::Completed(Ok(run)) => run,
        BackgroundOperationOutcome::Completed(Err(error)) => {
            record_state_sync_failure(
                runtime_root,
                target_root,
                node_id,
                attempt_id,
                AUTO_RECOVERY_SOURCE,
                &error,
            )?;
            return Ok(AutoRecoveryOutcome::Failed);
        }
        BackgroundOperationOutcome::Paused => return Ok(AutoRecoveryOutcome::Paused),
    };
    record_state_sync_result(
        runtime_root,
        target_root,
        node_id,
        attempt_id,
        AUTO_RECOVERY_SOURCE,
        &run.sync,
    )?;
    append_auto_recovery_activity(target_root, node_id, &run)?;
    Ok(AutoRecoveryOutcome::Recovered)
}

fn state_sync_auto_recovery_enabled(runtime_root: &Path, target_root: &Path) -> bool {
    // Errors fail closed: recovery mutates state, so an unreadable settings
    // store must not authorize it.
    let Ok(refine_dir) = prepare_refine_dir(target_root) else {
        return false;
    };
    let Ok(settings) = FileSettingsService::with_active_root(refine_dir, runtime_root).load()
    else {
        return false;
    };
    settings
        .get("state_sync_auto_recovery")
        .and_then(Value::as_str)
        .map(str::trim)
        != Some("off")
}

/// Record the audit trail for a recovery the daemon chose on its own. Every
/// displaced version stays reachable as a merge parent (or the named
/// retained ref on a join), so the entry carries the settled paths and any
/// ownership overrides the merge base granted this node.
fn append_auto_recovery_activity(
    target_root: &Path,
    node_id: &str,
    run: &StateRecoveryRunResult,
) -> RefineResult<()> {
    let Some(recovery) = &run.recovery else {
        // The rerun synchronized cleanly before recovery was needed; the
        // ordinary sync activity already tells that story.
        return Ok(());
    };
    let refine_dir = prepare_refine_dir(target_root)?;
    let service = FileActivityService::new(refine_dir);
    let mut entry = service.new_entry(
        format!(
            "State sync auto-recovered on node {node_id} with {} authority; displaced state stays reachable as a merge parent{}.",
            recovery.authority.as_str(),
            if recovery.retained_refs.is_empty() {
                String::new()
            } else {
                format!(" and at {}", recovery.retained_refs.join(", "))
            }
        ),
        "warn",
        "state_sync",
        None,
        Some("refine".to_string()),
    );
    entry.details = serde_json::json!({
        "event": "sync_auto_recovered",
        "node_id": node_id,
        "authority": recovery.authority.as_str(),
        "attempts": run.attempts,
        "settled_paths": recovery.settled_paths,
        "ownership_overrides": recovery.overrides,
        "retained_refs": recovery.retained_refs,
        "local_only": true
    })
    .as_object()
    .cloned();
    service.append(entry)
}

pub(crate) fn settle_state_recovery_success(
    runtime_root: &Path,
    target_root: &Path,
    expected_health: &crate::tools::host::state_sync_health::StateSyncHealth,
) -> RefineResult<(bool, crate::tools::host::state_sync_health::StateSyncHealth)> {
    let health_service = FileStateSyncHealthService::new(runtime_root);
    let (settled, activity) = health_service.record_recovery_success(
        target_root,
        &expected_health.node_id,
        expected_health.revision,
    )?;
    append_state_sync_activity(target_root, &expected_health.node_id, activity, None)?;
    let threshold = state_sync_stale_threshold(runtime_root, target_root)?;
    Ok((
        settled,
        health_service.inspect(target_root, &expected_health.node_id, threshold)?,
    ))
}

fn append_state_sync_activity(
    target_root: &Path,
    node_id: &str,
    activity: Option<StateSyncHealthActivity>,
    report: Option<&crate::tools::host::git_sync::StateSyncConflictReport>,
) -> RefineResult<()> {
    let Some(activity) = activity else {
        return Ok(());
    };
    let refine_dir = prepare_refine_dir(target_root)?;
    let service = FileActivityService::new(refine_dir);
    let (message, severity, event, error, failure_since) = match activity {
        StateSyncHealthActivity::FailureStarted { error } => (
            format!("State sync failed on node {node_id}: {error}"),
            "error",
            "sync_failed",
            Some(error),
            None,
        ),
        StateSyncHealthActivity::FailureReminder { error } => (
            format!("State sync remains unavailable on node {node_id}: {error}"),
            "warn",
            "sync_failure_reminder",
            Some(error),
            None,
        ),
        StateSyncHealthActivity::Recovered { failure_since } => (
            format!("State sync recovered on node {node_id}."),
            "info",
            "sync_recovered",
            None,
            Some(failure_since),
        ),
    };
    let mut entry = service.new_entry(
        message,
        severity,
        "state_sync",
        None,
        Some("refine".to_string()),
    );
    entry.details = serde_json::json!({
        "event": event,
        "node_id": node_id,
        "error": error,
        "failure_since": failure_since,
        "conflict_report_id": report.map(|report| report.report_id.as_str()),
        "conflict_report_location": report.map(|report| report.report_location.as_str()),
        "local_only": true
    })
    .as_object()
    .cloned();
    service.append(entry)
}
