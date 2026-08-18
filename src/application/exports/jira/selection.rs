use super::*;

pub(super) fn goal_commit_range(goal: &Value) -> Option<(String, String)> {
    Some((
        nonempty_string(goal, "base_commit")?.to_string(),
        nonempty_string(goal, "candidate_commit")?.to_string(),
    ))
}

pub(super) fn commits_for_goal<'a>(
    goal: &Value,
    commits_by_range: &'a BTreeMap<(String, String), Vec<GitChange>>,
) -> &'a [GitChange] {
    goal_commit_range(goal)
        .and_then(|range| commits_by_range.get(&range))
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub(super) fn jira_export_from_goal(
    goal: &Value,
    commits: &[GitChange],
) -> RefineResult<JiraGoalExport> {
    let goal_id = required_string(goal, "id")?;
    let csv = format!(
        "{}\r\n{}\r\n",
        JIRA_HEADERS.join(","),
        jira_row(goal, commits)?
    );

    Ok(JiraGoalExport {
        format: "jira_csv".to_string(),
        filename: format!("refine-goal-{goal_id}-jira.csv"),
        content_type: "text/csv; charset=utf-8".to_string(),
        goal_id: goal_id.to_string(),
        commit_count: commits.len(),
        csv,
    })
}
