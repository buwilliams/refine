use std::collections::BTreeMap;

use serde_json::Value;

use crate::model::goal::GoalPriority;
use crate::model::log::ActivityEntry;
use crate::model::workflow::GoalStatus;

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
    if let Some(goal_id) = &entry.goal_id {
        parts.push(goal_id.clone());
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
        change.goal_id.as_deref(),
        change.goal_name.as_deref(),
        change.goal_priority.as_deref(),
        change.goal_assignee.as_deref(),
        change.goal_status.as_ref().map(GoalStatus::as_str),
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

/// Joins a commit to a Goal by finding a Goal id embedded in its subject or
/// branch. Ids are looked up token-by-token against the Goal map instead of
/// scanning every Goal per commit: with a thousand recent commits the old scan
/// was the largest cost of projecting changes. Ids appear delimited by
/// non-alphanumeric characters in every branch and subject Refine writes.
pub(super) fn matching_change_goal<'a>(
    goals: &'a BTreeMap<String, GoalSummaryProjection>,
    branch: Option<&str>,
    subject: &str,
) -> Option<&'a GoalSummaryProjection> {
    [Some(subject), branch]
        .into_iter()
        .flatten()
        .flat_map(|text| text.split(|ch: char| !ch.is_ascii_alphanumeric()))
        .find_map(|token| goals.get(token))
}

/// How much searchable text a Goal keeps resident in the projection.
///
/// Notes and Round prompts have no upper bound, and a Goal accumulates both for
/// as long as it is worked, so carrying all of them made this the largest
/// per-Goal cost in the projection by a wide margin. The resident tier is a
/// prefix that answers most queries outright; anything it cannot decide is
/// resolved by reading the record, so nothing becomes unsearchable.
pub(crate) const RESIDENT_SEARCH_TEXT_LIMIT: usize = 512;

/// Everything about a Goal that a text query can match, in full.
///
/// Shared by the resident tier and the on-demand tier so the two cannot drift:
/// a term the deep tier would match but the resident prefix truncated is the
/// only difference between them.
pub(crate) fn goal_searchable_parts(object: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut parts = vec![
        text(object.get("name")).unwrap_or_else(|| "Untitled Goal".to_string()),
        text(object.get("reporter")).unwrap_or_default(),
        text(object.get("assignee")).unwrap_or_default(),
    ];
    for note in object
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        if let Some(body) = text(note.get("body")) {
            parts.push(body);
        }
    }
    for round in object
        .get("rounds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        for key in ["reporter", "assignee", "prompt"] {
            if let Some(value) = text(round.get(key)) {
                parts.push(value);
            }
        }
    }
    parts
}

/// Truncate to the resident budget on a character boundary.
pub(crate) fn resident_search_text(full: &str) -> String {
    if full.len() <= RESIDENT_SEARCH_TEXT_LIMIT {
        return full.to_string();
    }
    let mut end = RESIDENT_SEARCH_TEXT_LIMIT;
    while end > 0 && !full.is_char_boundary(end) {
        end -= 1;
    }
    full[..end].to_string()
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

pub(super) fn goal_status(goal: &serde_json::Map<String, Value>) -> GoalStatus {
    let latest_round = goal
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.last());
    match nullable_text(goal.get("status")).as_deref() {
        Some("todo") => GoalStatus::Todo,
        Some("plan") => GoalStatus::Plan,
        Some("implement") => GoalStatus::Implement,
        Some("quality") => GoalStatus::Quality,
        Some("governance") => GoalStatus::Governance,
        Some("in-progress") => {
            let plan = latest_round.and_then(|round| round.get("implementation_plan"));
            if plan
                .and_then(|plan| plan.get("implementation"))
                .is_some_and(|value| !value.is_null())
                || goal
                    .get("candidate_commit")
                    .is_some_and(|value| !value.is_null())
            {
                GoalStatus::Quality
            } else if plan
                .and_then(|plan| plan.get("final_plan"))
                .is_some_and(|value| !value.is_null())
            {
                GoalStatus::Implement
            } else {
                GoalStatus::Plan
            }
        }
        Some("qa") => GoalStatus::Quality,
        Some("ready-merge") => {
            if latest_round
                .and_then(|round| round.get("workflow_integration"))
                .is_some_and(|value| !value.is_null())
            {
                GoalStatus::Review
            } else {
                GoalStatus::Governance
            }
        }
        Some("build") => {
            if latest_round
                .and_then(|round| round.get("quality_state"))
                .and_then(Value::as_str)
                == Some("passed")
            {
                GoalStatus::Review
            } else {
                GoalStatus::Quality
            }
        }
        Some("review") => GoalStatus::Review,
        Some("done") => GoalStatus::Done,
        Some("failed") => GoalStatus::Failed,
        Some("cancelled") => GoalStatus::Cancelled,
        _ => GoalStatus::Backlog,
    }
}

pub(super) fn goal_priority(value: Option<&Value>) -> GoalPriority {
    match nullable_text(value).as_deref() {
        Some("medium") => GoalPriority::Medium,
        Some("high") => GoalPriority::High,
        _ => GoalPriority::Low,
    }
}

pub(super) fn goal_status_counts<'a>(
    statuses: impl Iterator<Item = &'a GoalStatus>,
) -> BTreeMap<GoalStatus, usize> {
    let mut counts = BTreeMap::new();
    for status in statuses {
        *counts.entry(status.clone()).or_default() += 1;
    }
    counts
}

#[cfg(test)]
mod workflow_status_migration_tests {
    use super::*;
    use serde_json::json;

    fn inferred(value: Value) -> GoalStatus {
        goal_status(value.as_object().unwrap())
    }

    #[test]
    fn legacy_active_statuses_infer_the_safest_current_stage() {
        assert_eq!(
            inferred(json!({"status": "in-progress", "rounds": [{}]})),
            GoalStatus::Plan
        );
        assert_eq!(
            inferred(json!({
                "status": "in-progress",
                "rounds": [{"implementation_plan": {"final_plan": {"result": {}}}}]
            })),
            GoalStatus::Implement
        );
        assert_eq!(
            inferred(json!({
                "status": "in-progress",
                "candidate_commit": "abc",
                "rounds": [{}]
            })),
            GoalStatus::Quality
        );
        assert_eq!(
            inferred(json!({"status": "qa", "rounds": [{}]})),
            GoalStatus::Quality
        );
        assert_eq!(
            inferred(json!({"status": "ready-merge", "rounds": [{}]})),
            GoalStatus::Governance
        );
        assert_eq!(
            inferred(json!({
                "status": "ready-merge",
                "rounds": [{"workflow_integration": {"target_commit": "def"}}]
            })),
            GoalStatus::Review
        );
    }
}
