use super::*;

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(super) fn derive_goal_name(prompt: &str) -> Option<String> {
    let collapsed = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name = collapsed.chars().take(80).collect::<String>();
    if collapsed.chars().count() > 80 {
        name = name
            .trim_end_matches(|ch: char| !ch.is_alphanumeric())
            .to_string();
    }
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

pub(super) fn bulk_goal_matches_filter(
    refine_dir: Option<&std::path::Path>,
    goal: &GoalSummaryProjection,
    filter: &BulkGoalFilter,
) -> bool {
    if let Some(status) = filter
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && goal.goal.status.as_str() != status
    {
        return false;
    }
    if let Some(reporter) = filter
        .reporter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && goal.goal.reporter.as_deref() != Some(reporter)
    {
        return false;
    }
    if let Some(assignee) = filter
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && goal.goal.assignee.as_deref() != Some(assignee)
    {
        return false;
    }
    if let Some(feature) = filter
        .feature
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if feature == "standalone" {
            if goal.goal.feature_id.is_some() {
                return false;
            }
        } else if feature != "all" && goal.goal.feature_id.as_deref() != Some(feature) {
            return false;
        }
    }
    if let Some(min_rounds) = filter.rounds_gte
        && goal.goal.round_count < min_rounds
    {
        return false;
    }
    if let Some(max_rounds) = filter.rounds_lte
        && goal.goal.round_count > max_rounds
    {
        return false;
    }
    if let Some(node) = filter
        .node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && node != "all"
        && node != "current"
        && goal.goal.node_id.as_deref().unwrap_or("default") != node
    {
        return false;
    }
    if let Some(query) = filter.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let query = query.to_lowercase();
        let reporter = goal.goal.reporter.as_deref().unwrap_or("").to_lowercase();
        let assignee = goal.goal.assignee.as_deref().unwrap_or("").to_lowercase();
        // Matched against the same corpus the Goals view searches, so a bulk
        // selection cannot quietly cover fewer Goals than the list it was made
        // from.
        if !goal.goal.id.to_lowercase().contains(&query)
            && !reporter.contains(&query)
            && !assignee.contains(&query)
            && !goal_text_matches(
                refine_dir,
                &goal.goal.json_path,
                &goal.searchable_text,
                &query,
            )
        {
            return false;
        }
    }
    true
}

pub(super) fn goal_transfer_skip_reason(goal: &GoalSummaryProjection) -> Option<String> {
    if let Some(reason) = goal_status_transfer_skip_reason(goal) {
        return Some(reason);
    }
    goal.goal
        .feature_id
        .as_ref()
        .map(|feature_id| format!("feature:{feature_id}"))
}

pub(super) fn goal_status_transfer_skip_reason(goal: &GoalSummaryProjection) -> Option<String> {
    if matches!(
        goal.goal.status,
        GoalStatus::Plan | GoalStatus::Implement | GoalStatus::Quality | GoalStatus::Governance
    ) {
        Some(format!("status:{}", goal.goal.status.as_str()))
    } else {
        None
    }
}

pub(super) fn validate_goal_transfer_to_node(goal: &GoalSummaryProjection) -> RefineResult<()> {
    if let Some(feature_id) = goal.goal.feature_id.as_deref() {
        return Err(RefineError::Conflict(format!(
            "Goal {} is assigned to Feature {feature_id}; transfer the Feature instead",
            goal.goal.id
        )));
    }
    if let Some(reason) = goal_transfer_skip_reason(goal) {
        return Err(RefineError::Conflict(format!(
            "Goal {} is not transferable ({reason})",
            goal.goal.id
        )));
    }
    Ok(())
}

pub(super) fn bulk_feature_matches_filter(
    feature: &FeatureSummaryProjection,
    filter: &BulkFeatureFilter,
    active_node_id: &str,
) -> bool {
    if let Some(status) = filter
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && feature.status.as_str() != status
    {
        return false;
    }
    if let Some(reporter) = filter
        .reporter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && feature.feature.reporter.as_deref() != Some(reporter)
    {
        return false;
    }
    if let Some(assignee) = filter
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && feature.feature.assignee.as_deref() != Some(assignee)
    {
        return false;
    }
    if let Some(node) = filter
        .node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match node {
            "all" => {}
            "current" => {
                if feature.feature.node_id.as_deref().unwrap_or("default") != active_node_id {
                    return false;
                }
            }
            node => {
                if feature.feature.node_id.as_deref().unwrap_or("default") != node {
                    return false;
                }
            }
        }
    }
    if let Some(query) = filter.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let query = query.to_lowercase();
        let reporter = feature
            .feature
            .reporter
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        let assignee = feature
            .feature
            .assignee
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        let description = feature
            .feature
            .description
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        if !feature.feature.id.to_lowercase().contains(&query)
            && !feature.feature.name.to_lowercase().contains(&query)
            && !description.contains(&query)
            && !reporter.contains(&query)
            && !assignee.contains(&query)
        {
            return false;
        }
    }
    true
}

pub(super) fn valid_reporter_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 80 && !value.chars().any(|ch| ch.is_control())
}

pub(super) fn restore_last_workflow_status(status: &GoalStatus) -> GoalStatus {
    match status {
        GoalStatus::Failed | GoalStatus::Review | GoalStatus::Cancelled => GoalStatus::Todo,
        other => other.clone(),
    }
}
