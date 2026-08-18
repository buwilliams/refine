use super::*;

#[cfg(unix)]
pub(super) fn metadata_change_unix_ns(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .ctime()
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(metadata.ctime_nsec()))
}

#[cfg(not(unix))]
pub(super) fn metadata_change_unix_ns(metadata: &fs::Metadata) -> Option<i64> {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
}

pub(super) fn round_log_activity_entry(
    goal_id: &str,
    index: usize,
    mut log: RoundLogEntry,
) -> ActivityEntry {
    let round_idx = log.round_idx.unwrap_or(0);
    let details = log.entry.details.take();
    ActivityEntry {
        id: format!("round-log:{goal_id}:{round_idx}:{index}"),
        datetime: log.entry.datetime,
        severity: log.entry.severity,
        category: log.entry.category,
        message: log.entry.message,
        goal_id: Some(goal_id.to_string()),
        actor: log.entry.actor,
        details,
        actions: log.entry.actions,
    }
}

pub(super) fn goal_reporter(
    object: &serde_json::Map<String, Value>,
    valid_rounds: &[&Value],
) -> Option<String> {
    nullable_text(object.get("reporter"))
        .or_else(|| {
            valid_rounds.first().and_then(|round| {
                round
                    .as_object()
                    .and_then(|object| nullable_text(object.get("reporter")))
            })
        })
        .or_else(|| nullable_text(object.get("assignee")))
}

pub(super) fn latest_round_assignee(
    object: &serde_json::Map<String, Value>,
    valid_rounds: &[&Value],
) -> Option<String> {
    valid_rounds
        .last()
        .and_then(|round| {
            round
                .as_object()
                .and_then(|object| nullable_text(object.get("assignee")))
        })
        .or_else(|| nullable_text(object.get("assignee")))
        .or_else(|| {
            valid_rounds.last().and_then(|round| {
                round
                    .as_object()
                    .and_then(|object| nullable_text(object.get("reporter")))
            })
        })
}
