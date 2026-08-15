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
    let recovery_kind = matches!(error, RefineError::StateSyncMissingBaseline(_))
        .then_some(crate::tools::host::state_sync_health::StateSyncRecoveryKind::MissingBaseline);
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
        recovery_kind,
    )?;
    append_state_sync_activity(target_root, node_id, activity, report.as_ref())
}

pub(crate) fn settle_state_recovery_success(
    runtime_root: &Path,
    target_root: &Path,
) -> RefineResult<()> {
    let node_id = state_sync_node_id(runtime_root, target_root)?;
    let activity =
        FileStateSyncHealthService::new(runtime_root).record_success(target_root, &node_id)?;
    append_state_sync_activity(target_root, &node_id, activity)
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
