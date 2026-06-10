use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::gap::GapPriority;
use crate::model::log::ActivityEntry;
use crate::model::workflow::GapStatus;

use super::types::*;

pub(super) fn activity_searchable_text(entry: &ActivityEntry) -> String {
    let mut parts = vec![
        entry.message.clone(),
        entry.severity.clone(),
        entry.category.clone(),
    ];
    if let Some(actor) = &entry.actor {
        parts.push(actor.clone());
    }
    if let Some(gap_id) = &entry.gap_id {
        parts.push(gap_id.clone());
    }
    if let Some(details) = &entry.details
        && let Ok(encoded) = serde_json::to_string(details)
    {
        parts.push(encoded);
    }
    parts.join("\n")
}

pub(super) fn change_searchable_text(change: &ChangeSummaryProjection) -> String {
    [
        Some(change.commit.as_str()),
        Some(change.subject.as_str()),
        change.branch.as_deref(),
        change.gap_id.as_deref(),
        change.gap_name.as_deref(),
        change.gap_priority.as_deref(),
        change.gap_assignee.as_deref(),
        change.gap_status.as_ref().map(GapStatus::as_str),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
}

pub(super) fn change_projection_key(change: &ChangeSummaryProjection) -> String {
    format!(
        "{}:{}",
        change.branch.as_deref().unwrap_or(""),
        change.commit
    )
}

pub(super) fn matching_change_gap<'a>(
    gaps: &'a BTreeMap<String, GapSummaryProjection>,
    branch: Option<&str>,
    subject: &str,
) -> Option<&'a GapSummaryProjection> {
    gaps.values().find(|gap| {
        subject.contains(&gap.gap.id) || branch.is_some_and(|branch| branch.contains(&gap.gap.id))
    })
}

pub(super) fn fingerprint_content_hash(path: &Path) -> RefineResult<String> {
    let bytes = fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read {} for fingerprint: {error}",
            path.display()
        ))
    })?;
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(format!("{hash:016x}"))
}

pub(super) fn text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(value.to_string()),
        _ => None,
    }
}

pub(super) fn nullable_text(value: Option<&Value>) -> Option<String> {
    text(value).and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub(super) fn nullable_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(value)) => value.as_i64(),
        Some(Value::String(value)) => value.parse().ok(),
        _ => None,
    }
}

pub(super) fn gap_status(value: Option<&Value>) -> GapStatus {
    match nullable_text(value).as_deref() {
        Some("todo") => GapStatus::Todo,
        Some("in-progress") => GapStatus::InProgress,
        Some("qa") => GapStatus::Qa,
        Some("ready-merge") => GapStatus::ReadyMerge,
        Some("awaiting-rebuild") => GapStatus::AwaitingRebuild,
        Some("review") => GapStatus::Review,
        Some("done") => GapStatus::Done,
        Some("failed") => GapStatus::Failed,
        Some("cancelled") => GapStatus::Cancelled,
        _ => GapStatus::Backlog,
    }
}

pub(super) fn gap_priority(value: Option<&Value>) -> GapPriority {
    match nullable_text(value).as_deref() {
        Some("medium") => GapPriority::Medium,
        Some("high") => GapPriority::High,
        _ => GapPriority::Low,
    }
}

pub(super) fn gap_status_counts<'a>(
    statuses: impl Iterator<Item = &'a GapStatus>,
) -> BTreeMap<GapStatus, usize> {
    let mut counts = BTreeMap::new();
    for status in statuses {
        *counts.entry(status.clone()).or_default() += 1;
    }
    counts
}
