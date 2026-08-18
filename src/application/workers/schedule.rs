use super::*;

pub(super) fn validate_worker_kind(worker_kind: &str, allow_one_shot: bool) -> RefineResult<()> {
    if matches!(
        worker_kind,
        WORKFLOW_RUNNER | WORKTREE_CLEANUP_RUNNER | GIT_SYNC_RUNNER | DEVELOPMENT_REQUEST_RUNNER
    ) || (allow_one_shot && matches!(worker_kind, PROJECT_SYNC_RUNNER | JIRA_EXPORT_RUNNER))
    {
        return Ok(());
    }
    Err(RefineError::InvalidInput(format!(
        "unknown runner worker kind {worker_kind}"
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GitSyncSchedule {
    pub(super) debounce: Duration,
    pub(super) remote_fetch_interval: Option<Duration>,
    pub(crate) stale_threshold: Duration,
}

impl Default for GitSyncSchedule {
    fn default() -> Self {
        Self {
            debounce: DEFAULT_GIT_SYNC_DEBOUNCE,
            remote_fetch_interval: Some(DEFAULT_REMOTE_FETCH_INTERVAL),
            stale_threshold: DEFAULT_STATE_SYNC_STALE_THRESHOLD,
        }
    }
}

pub(super) fn git_sync_schedule(
    runtime_root: &Path,
    target_root: &Path,
) -> RefineResult<GitSyncSchedule> {
    let refine_dir = prepare_refine_dir(target_root)?;
    let settings = FileSettingsService::with_active_root(refine_dir, runtime_root).load()?;
    Ok(GitSyncSchedule {
        debounce: positive_duration(
            settings.get("state_sync_debounce_seconds"),
            DEFAULT_GIT_SYNC_DEBOUNCE,
        ),
        remote_fetch_interval: optional_positive_duration(
            settings.get("project_update_pulse_interval_seconds"),
            DEFAULT_REMOTE_FETCH_INTERVAL,
        ),
        stale_threshold: positive_duration(
            settings.get("state_sync_stale_threshold_seconds"),
            DEFAULT_STATE_SYNC_STALE_THRESHOLD,
        ),
    })
}

pub(crate) fn state_sync_stale_threshold(
    runtime_root: &Path,
    target_root: &Path,
) -> RefineResult<Duration> {
    git_sync_schedule(runtime_root, target_root).map(|schedule| schedule.stale_threshold)
}

pub(super) fn positive_duration(value: Option<&Value>, fallback: Duration) -> Duration {
    let seconds = value
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback.as_secs() as i64);
    Duration::from_secs(seconds.max(1) as u64)
}

pub(super) fn optional_positive_duration(
    value: Option<&Value>,
    fallback: Duration,
) -> Option<Duration> {
    let seconds = value
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback.as_secs() as i64);
    (seconds > 0).then(|| Duration::from_secs(seconds as u64))
}
