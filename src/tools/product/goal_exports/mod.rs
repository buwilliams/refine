use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitChange};
use crate::tools::product::work_items::{BulkGoalSelection, FileWorkItemService};

const JIRA_DESCRIPTION_LIMIT: usize = 30_000;
const IDENTITY_RESERVE: usize = 2_500;
const TRACEABILITY_RESERVE: usize = 3_000;
const COMMIT_RESERVE: usize = 3_500;
const OUTCOME_RESERVE: usize = 7_000;
const REQUEST_RESERVE: usize = 5_000;
const IMPLEMENTATION_RESERVE: usize = 5_000;
const GUIDANCE_RESERVE: usize = 1_200;
const NOTES_RESERVE: usize = 1_800;
const JIRA_HEADERS: [&str; 10] = [
    "Summary",
    "Description",
    "Work Type",
    "Priority",
    "Labels",
    "Refine Goal ID",
    "Refine Status",
    "Refine Branch",
    "Base Commit",
    "Candidate Commit",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JiraGoalExport {
    pub format: String,
    pub filename: String,
    pub content_type: String,
    pub goal_id: String,
    pub commit_count: usize,
    pub csv: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JiraGoalsExport {
    pub format: String,
    pub filename: String,
    pub content_type: String,
    pub goal_ids: Vec<String>,
    pub goal_count: usize,
    pub commit_count: usize,
    pub csv: String,
}

#[derive(Clone, Debug)]
pub struct FileGoalExportService {
    refine_dir: PathBuf,
    target_root: PathBuf,
    runtime_root: Option<PathBuf>,
    operation_id: Option<String>,
}

impl FileGoalExportService {
    pub fn new(refine_dir: impl Into<PathBuf>, target_root: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            target_root: target_root.into(),
            runtime_root: None,
            operation_id: None,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            target_root: target_root.into(),
            runtime_root: Some(runtime_root.into()),
            operation_id: None,
        }
    }

    pub fn with_operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub fn export_jira_csv(&self, goal_id: &str) -> RefineResult<JiraGoalExport> {
        let work_items = self.work_items();
        let goal = work_items.show_goal_detail(goal_id)?;
        let commits = self.goal_commits(&goal)?;
        jira_export_from_goal(&goal, &commits)
    }

    pub fn export_bulk_jira_csv(
        &self,
        selection: &BulkGoalSelection,
    ) -> RefineResult<JiraGoalsExport> {
        self.export_bulk_jira_csv_with_progress(selection, |_, _, _| Ok(()))
    }

    pub fn export_bulk_jira_csv_with_progress<F>(
        &self,
        selection: &BulkGoalSelection,
        mut report: F,
    ) -> RefineResult<JiraGoalsExport>
    where
        F: FnMut(&str, usize, usize) -> RefineResult<()>,
    {
        let work_items = self.work_items();
        let goal_ids = work_items
            .select_bulk_goal_ids(selection)?
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if goal_ids.is_empty() {
            return Err(RefineError::InvalidInput(
                "Select at least one Goal to export for Jira".to_string(),
            ));
        }

        let goal_total = goal_ids.len();
        report("Loading selected Goal evidence", 0, goal_total)?;
        let mut goals = Vec::with_capacity(goal_total);
        for (index, goal_id) in goal_ids.iter().enumerate() {
            goals.push(work_items.show_goal_detail(goal_id)?);
            report("Loading selected Goal evidence", index + 1, goal_total)?;
        }

        let ranges = goals
            .iter()
            .filter_map(goal_commit_range)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        report("Looking up commit evidence", 0, goal_total)?;
        let commits_by_range = self.git().changes_between_many(&ranges)?;

        let mut rows = Vec::with_capacity(goal_ids.len());
        let mut commit_count = 0;
        for (index, goal) in goals.iter().enumerate() {
            let commits = commits_for_goal(goal, &commits_by_range);
            commit_count += commits.len();
            rows.push(jira_row(goal, commits)?);
            report("Building Jira CSV", index + 1, goal_total)?;
        }
        let csv = format!("{}\r\n{}\r\n", JIRA_HEADERS.join(","), rows.join("\r\n"));

        Ok(JiraGoalsExport {
            format: "jira_csv".to_string(),
            filename: "refine-goals-jira.csv".to_string(),
            content_type: "text/csv; charset=utf-8".to_string(),
            goal_count: goal_ids.len(),
            goal_ids,
            commit_count,
            csv,
        })
    }

    fn work_items(&self) -> FileWorkItemService {
        match &self.runtime_root {
            Some(runtime_root) => FileWorkItemService::with_projection_cache(
                &self.refine_dir,
                runtime_root.join("cache"),
            ),
            None => FileWorkItemService::new(&self.refine_dir),
        }
    }

    fn goal_commits(&self, goal: &Value) -> RefineResult<Vec<GitChange>> {
        let Some(range) = goal_commit_range(goal) else {
            return Ok(Vec::new());
        };
        self.git().changes_between(&range.0, &range.1)
    }

    fn git(&self) -> FileGitWorktreeService {
        let git = match &self.runtime_root {
            Some(runtime_root) => {
                FileGitWorktreeService::with_runtime_root(&self.target_root, runtime_root)
            }
            None => FileGitWorktreeService::new(&self.target_root),
        };
        match &self.operation_id {
            Some(operation_id) => git.with_operation_id(operation_id),
            None => git,
        }
    }
}

fn goal_commit_range(goal: &Value) -> Option<(String, String)> {
    Some((
        nonempty_string(goal, "base_commit")?.to_string(),
        nonempty_string(goal, "candidate_commit")?.to_string(),
    ))
}

fn commits_for_goal<'a>(
    goal: &Value,
    commits_by_range: &'a BTreeMap<(String, String), Vec<GitChange>>,
) -> &'a [GitChange] {
    goal_commit_range(goal)
        .and_then(|range| commits_by_range.get(&range))
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn jira_export_from_goal(goal: &Value, commits: &[GitChange]) -> RefineResult<JiraGoalExport> {
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

fn jira_row(goal: &Value, commits: &[GitChange]) -> RefineResult<String> {
    let goal_id = required_string(goal, "id")?;
    let summary = required_string(goal, "name")?;
    let description = jira_description(goal, commits);
    if description.chars().count() > JIRA_DESCRIPTION_LIMIT {
        return Err(RefineError::Serialization(format!(
            "Goal {goal_id} Jira description could not be rendered within Jira's \
             {JIRA_DESCRIPTION_LIMIT} character limit; report this renderer invariant failure"
        )));
    }

    let priority = title_case(nonempty_string(goal, "priority").unwrap_or("low"));
    let values = [
        summary,
        description.as_str(),
        "Task",
        priority.as_str(),
        "refine-soc2-evidence",
        goal_id,
        nonempty_string(goal, "status").unwrap_or("unknown"),
        nonempty_string(goal, "branch_name").unwrap_or(""),
        nonempty_string(goal, "base_commit").unwrap_or(""),
        nonempty_string(goal, "candidate_commit").unwrap_or(""),
    ];
    Ok(values
        .iter()
        .map(|value| csv_cell(value))
        .collect::<Vec<_>>()
        .join(","))
}

fn jira_description(goal: &Value, commits: &[GitChange]) -> String {
    let mut sections = Vec::<EvidenceSection>::new();
    let mut overview = vec!["Refine delivery evidence".to_string()];
    push_bounded_line(
        &mut overview,
        "Goal ID",
        string_or(goal, "id", "Unknown"),
        256,
    );
    push_bounded_line(
        &mut overview,
        "Status",
        string_or(goal, "status", "Unknown"),
        128,
    );
    push_bounded_line(
        &mut overview,
        "Priority",
        string_or(goal, "priority", "Unknown"),
        128,
    );
    push_bounded_line(
        &mut overview,
        "Reporter",
        string_or(goal, "reporter", "Unreported"),
        256,
    );
    push_bounded_line(
        &mut overview,
        "Assignee",
        string_or(goal, "assignee", "Unassigned"),
        256,
    );
    push_bounded_line(
        &mut overview,
        "Created",
        string_or(goal, "created", "Unknown"),
        128,
    );
    push_bounded_line(
        &mut overview,
        "Updated",
        string_or(goal, "updated", "Unknown"),
        128,
    );
    push_bounded_optional_line(&mut overview, "Feature", goal, "feature_id", 256);
    push_bounded_optional_line(&mut overview, "Node", goal, "node_id", 256);
    sections.push(EvidenceSection::new(
        "Goal identity",
        overview.join("\n"),
        IDENTITY_RESERVE,
    ));

    let mut traceability = vec!["Branches and commit anchors".to_string()];
    push_bounded_optional_line(
        &mut traceability,
        "Target branch",
        goal,
        "target_branch",
        600,
    );
    push_bounded_optional_line(
        &mut traceability,
        "Implementation branch",
        goal,
        "branch_name",
        600,
    );
    push_bounded_optional_line(&mut traceability, "Base commit", goal, "base_commit", 600);
    push_bounded_optional_line(
        &mut traceability,
        "Candidate commit",
        goal,
        "candidate_commit",
        600,
    );
    sections.push(EvidenceSection::new(
        "branch and commit anchor evidence",
        traceability.join("\n"),
        TRACEABILITY_RESERVE,
    ));

    let mut commit_evidence = vec!["Delivered commits".to_string()];
    if commits.is_empty() {
        commit_evidence.push("Commits delivered: None recorded".to_string());
    } else {
        commit_evidence.push(format!("Commits delivered: {}", commits.len()));
        for commit in commits {
            commit_evidence.push(format!(
                "- {} | {} | {}",
                commit.commit, commit.committed_time, commit.subject
            ));
        }
    }
    sections.push(EvidenceSection::new(
        "delivered commit evidence",
        commit_evidence.join("\n"),
        COMMIT_RESERVE,
    ));

    if let Some(rounds) = goal.get("rounds").and_then(Value::as_array) {
        let round_count = rounds.len().max(1);
        let request_limit = (12_000 / round_count).max(512);
        let implementation_limit = (8_000 / round_count).max(512);
        let guidance_limit = (2_000 / round_count).max(256);
        let mut outcomes = vec!["Round outcomes, Quality, and governance".to_string()];
        let mut requested_work = vec!["Round requested work".to_string()];
        let mut implementation = vec!["Round implementation outcomes".to_string()];
        let mut guidance = vec!["Round guidance decisions".to_string()];
        for (index, round) in rounds.iter().enumerate() {
            let round_number = index + 1;
            outcomes.push(format!("Round {round_number}"));
            push_bounded_optional_line(&mut outcomes, "Reporter", round, "reporter", 256);
            push_bounded_optional_line(&mut outcomes, "Assignee", round, "assignee", 256);
            push_bounded_optional_line(&mut outcomes, "Created", round, "created", 128);
            push_bounded_optional_line(&mut outcomes, "Updated", round, "updated", 128);
            push_bounded_optional_line(
                &mut outcomes,
                "Implementation reported at",
                round,
                "implementation_reported_at",
                128,
            );
            push_quality_summary(&mut outcomes, round);
            push_bounded_optional_line(
                &mut outcomes,
                "Governance checked at",
                round,
                "governance_checked_at",
                128,
            );
            push_governance_summary(&mut outcomes, round);

            if let Some(prompt) = nonempty_string(round, "prompt") {
                requested_work.push(format!(
                    "Round {round_number}:\n{}",
                    truncate_with_marker(
                        prompt,
                        request_limit,
                        &format!("Round {round_number} requested work")
                    )
                ));
            }
            if let Some(report) = nonempty_string(round, "implementation_report") {
                implementation.push(format!(
                    "Round {round_number}:\n{}",
                    truncate_with_marker(
                        report,
                        implementation_limit,
                        &format!("Round {round_number} implementation outcome")
                    )
                ));
            }
            if let Some(decision) = nonempty_string(round, "guidance_decision") {
                guidance.push(format!(
                    "Round {round_number}:\n{}",
                    truncate_with_marker(
                        decision,
                        guidance_limit,
                        &format!("Round {round_number} guidance decision")
                    )
                ));
            }
        }
        if outcomes.len() > 1 {
            sections.push(EvidenceSection::new(
                "round outcome, Quality, and governance evidence",
                outcomes.join("\n"),
                OUTCOME_RESERVE,
            ));
        }
        if requested_work.len() > 1 {
            sections.push(EvidenceSection::new(
                "round requested work",
                requested_work.join("\n\n"),
                REQUEST_RESERVE,
            ));
        }
        if implementation.len() > 1 {
            sections.push(EvidenceSection::new(
                "round implementation outcomes",
                implementation.join("\n\n"),
                IMPLEMENTATION_RESERVE,
            ));
        }
        if guidance.len() > 1 {
            sections.push(EvidenceSection::new(
                "round guidance decisions",
                guidance.join("\n\n"),
                GUIDANCE_RESERVE,
            ));
        }
    }

    if let Some(notes) = goal.get("notes").and_then(Value::as_array)
        && !notes.is_empty()
    {
        let mut lines = vec!["Notes".to_string()];
        for note in notes {
            let author = string_or(note, "author", "Unknown");
            let created = string_or(note, "created", "Unknown");
            let body = string_or(note, "body", "");
            lines.push(format!("- {created} | {author} | {body}"));
        }
        sections.push(EvidenceSection::new(
            "Goal notes",
            lines.join("\n"),
            NOTES_RESERVE,
        ));
    }

    render_budgeted_sections(&sections, JIRA_DESCRIPTION_LIMIT)
}

#[derive(Debug)]
struct EvidenceSection {
    omission_label: &'static str,
    text: String,
    reserve: usize,
}

impl EvidenceSection {
    fn new(omission_label: &'static str, text: String, reserve: usize) -> Self {
        Self {
            omission_label,
            text,
            reserve,
        }
    }
}

fn render_budgeted_sections(sections: &[EvidenceSection], limit: usize) -> String {
    let mut rendered = String::new();
    for (index, section) in sections.iter().enumerate() {
        let separator_len = usize::from(index > 0) * 2;
        let later_reserve = sections[index + 1..]
            .iter()
            .map(|later| {
                let full_len = later.text.chars().count() + 2;
                full_len.min(later.reserve)
            })
            .sum::<usize>();
        let used = rendered.chars().count();
        let available = limit.saturating_sub(used).saturating_sub(later_reserve);
        if available <= separator_len {
            continue;
        }
        if separator_len > 0 {
            rendered.push_str("\n\n");
        }
        let content_limit = available - separator_len;
        rendered.push_str(&truncate_with_marker(
            &section.text,
            content_limit,
            section.omission_label,
        ));
    }
    rendered
}

fn truncate_with_marker(value: &str, limit: usize, label: &str) -> String {
    let total = value.chars().count();
    if total <= limit {
        return value.to_string();
    }

    let shortest_marker = format!("[omitted: {label}]");
    if shortest_marker.chars().count() > limit {
        return shortest_marker.chars().take(limit).collect();
    }

    let mut retained = limit.saturating_sub(shortest_marker.chars().count());
    loop {
        let omitted = total.saturating_sub(retained);
        let marker = format!("\n[shortened: {label}; {omitted} characters omitted]");
        let next_retained = limit.saturating_sub(marker.chars().count());
        if next_retained == retained {
            return value.chars().take(retained).chain(marker.chars()).collect();
        }
        retained = next_retained;
    }
}

fn required_string<'a>(value: &'a Value, key: &str) -> RefineResult<&'a str> {
    nonempty_string(value, key).ok_or_else(|| {
        RefineError::Serialization(format!("Goal export requires a non-empty {key}"))
    })
}

fn nonempty_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn string_or<'a>(value: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    nonempty_string(value, key).unwrap_or(fallback)
}

fn push_bounded_optional_line(
    lines: &mut Vec<String>,
    label: &str,
    value: &Value,
    key: &str,
    value_limit: usize,
) {
    if let Some(value) = nonempty_string(value, key) {
        push_bounded_line(lines, label, value, value_limit);
    }
}

fn push_bounded_line(lines: &mut Vec<String>, label: &str, value: &str, value_limit: usize) {
    lines.push(format!(
        "{label}: {}",
        truncate_with_marker(value, value_limit, label)
    ));
}

fn push_quality_summary(lines: &mut Vec<String>, round: &Value) {
    push_bounded_optional_line(lines, "Quality state", round, "quality_state", 128);
    push_bounded_optional_line(lines, "Quality result", round, "quality_message", 768);
    push_bounded_optional_line(
        lines,
        "Quality checked at",
        round,
        "quality_checked_at",
        128,
    );

    let Some(details) = round
        .get("quality_details")
        .filter(|value| !value.is_null())
    else {
        return;
    };
    if let Some(detail) = compact_scalar(details) {
        push_unique_line(
            lines,
            format!(
                "Quality detail: {}",
                truncate_with_marker(&detail, 768, "Quality detail")
            ),
        );
        return;
    }
    for (key, label) in [
        ("evaluation_scope", "scope"),
        ("command", "command"),
        ("exit_code", "exit code"),
        ("cwd", "working directory"),
        ("source_candidate_commit", "source candidate"),
        ("candidate_commit", "evaluated commit"),
        ("operation_id", "operation"),
    ] {
        if let Some(value) = details.get(key).and_then(compact_scalar) {
            push_unique_line(
                lines,
                format!(
                    "Quality {label}: {}",
                    truncate_with_marker(&value, 768, &format!("Quality {label}"))
                ),
            );
        }
    }
    if let Some(results) = details.get("results").and_then(Value::as_array)
        && !results.is_empty()
    {
        let mut states = BTreeMap::<String, usize>::new();
        for result in results {
            let state = ["status", "state", "result"]
                .iter()
                .find_map(|key| result.get(key).and_then(compact_scalar))
                .unwrap_or_else(|| "recorded".to_string());
            *states.entry(state).or_default() += 1;
        }
        let counts = states
            .into_iter()
            .map(|(state, count)| format!("{state}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        push_unique_line(
            lines,
            format!("Quality checks: {} ({counts})", results.len()),
        );
    }
}

fn push_governance_summary(lines: &mut Vec<String>, round: &Value) {
    let states = [
        ("rule_state", "rule"),
        ("product_state", "product"),
        ("constitution_state", "constitution"),
        ("meta_rule_state", "meta-rule"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        nonempty_string(round, key).map(|value| {
            format!(
                "{label}={}",
                truncate_with_marker(value, 128, &format!("{label} governance state"))
            )
        })
    })
    .collect::<Vec<_>>();
    if !states.is_empty() {
        lines.push(format!("Governance states: {}", states.join(", ")));
    }
    push_bounded_optional_line(lines, "Governance result", round, "governance_message", 768);

    let details = round
        .get("governance_details")
        .filter(|value| !value.is_null());
    if let Some(detail) = details.and_then(compact_scalar) {
        push_unique_line(
            lines,
            format!(
                "Governance detail: {}",
                truncate_with_marker(&detail, 768, "Governance detail")
            ),
        );
    } else if let Some(details) = details {
        for (key, label) in [
            ("phase", "phase"),
            ("configured", "configured"),
            ("rules_checked", "rules checked"),
        ] {
            if let Some(value) = details.get(key).and_then(compact_scalar) {
                push_unique_line(
                    lines,
                    format!(
                        "Governance {label}: {}",
                        truncate_with_marker(&value, 256, &format!("Governance {label}"))
                    ),
                );
            }
        }
    }

    let explicit_actions = round
        .get("governance_rule_actions")
        .and_then(Value::as_array)
        .filter(|actions| !actions.is_empty());
    let actions = explicit_actions.or_else(|| {
        details.and_then(|details| {
            ["failed_actions", "violations", "rule_violations"]
                .iter()
                .find_map(|key| {
                    details
                        .get(key)
                        .and_then(Value::as_array)
                        .filter(|actions| !actions.is_empty())
                })
                .or_else(|| {
                    details.get("verdict").and_then(|verdict| {
                        ["failed_actions", "violations", "rule_violations"]
                            .iter()
                            .find_map(|key| {
                                verdict
                                    .get(key)
                                    .and_then(Value::as_array)
                                    .filter(|actions| !actions.is_empty())
                            })
                    })
                })
        })
    });
    let Some(actions) = actions else {
        return;
    };
    let mut seen = BTreeSet::new();
    for action in actions {
        let rendered = compact_governance_action(action);
        if !rendered.is_empty() && seen.insert(rendered.clone()) {
            lines.push(format!(
                "Governance action: {}",
                truncate_with_marker(&rendered, 1_200, "governance action")
            ));
        }
    }
}

fn compact_governance_action(action: &Value) -> String {
    if let Some(value) = compact_scalar(action) {
        return value;
    }
    [
        ("rule_id", "rule"),
        ("action", "action"),
        ("status", "status"),
        ("message", "message"),
        ("reason", "reason"),
        ("summary", "summary"),
        ("text", "text"),
        ("rule", "requirement"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        action
            .get(key)
            .and_then(compact_scalar)
            .map(|value| format!("{label}={value}"))
    })
    .collect::<Vec<_>>()
    .join("; ")
}

fn compact_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Value::Bool(_) | Value::Number(_) => Some(value.to_string()),
        _ => None,
    }
}

fn push_unique_line(lines: &mut Vec<String>, line: String) {
    if !lines.iter().any(|existing| existing == &line) {
        lines.push(line);
    }
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn csv_cell(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    #[test]
    fn jira_export_contains_reports_quality_notes_and_exact_commits() {
        let root = unique_temp_dir("jira-goal-export");
        let refine_dir = root.join(".refine");
        fs::create_dir_all(&refine_dir).unwrap();
        git(&root, &["init"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        git(&root, &["config", "user.name", "Test User"]);
        fs::write(root.join("app.txt"), "before\n").unwrap();
        git(&root, &["add", "app.txt"]);
        git(&root, &["commit", "-m", "initial"]);
        let base = git_stdout(&root, &["rev-parse", "HEAD"]);
        fs::write(root.join("app.txt"), "after\n").unwrap();
        git(&root, &["commit", "-am", "GOAL1 implement evidence export"]);
        let candidate = git_stdout(&root, &["rev-parse", "HEAD"]);

        let goal_dir = refine_dir.join("goals/GO/AL1");
        fs::create_dir_all(&goal_dir).unwrap();
        fs::write(
            goal_dir.join("goal.json"),
            serde_json::to_vec_pretty(&json!({
                "id": "GOAL1",
                "name": "Export audit, evidence",
                "status": "review",
                "priority": "high",
                "reporter": "Auditor",
                "branch_name": "refine/GOAL1/round-1",
                "target_branch": "main",
                "base_commit": base,
                "candidate_commit": candidate,
                "created": "2026-01-01T00:00:00Z",
                "updated": "2026-01-02T00:00:00Z",
                "notes": [{
                    "id": "note-1",
                    "author": "Reviewer",
                    "body": "Preserve \"quotes\"",
                    "created": "2026-01-02T00:00:00Z",
                    "updated": "2026-01-02T00:00:00Z"
                }],
                "rounds": [{
                    "reporter": "Auditor",
                    "assignee": "Engineer",
                    "prompt": "Capture delivery evidence",
                    "created": "2026-01-01T00:00:00Z",
                    "updated": "2026-01-02T00:00:00Z",
                    "implementation_report": "Added export. cargo test passed.",
                    "implementation_reported_at": "2026-01-02T00:00:00Z",
                    "quality_state": "passed",
                    "quality_message": "All checks passed",
                    "quality_details": {"command": "cargo test", "exit_code": 0},
                    "rule_state": "passed",
                    "logs": []
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let export = FileGoalExportService::new(&refine_dir, &root)
            .export_jira_csv("GOAL1")
            .unwrap();
        assert_eq!(export.filename, "refine-goal-GOAL1-jira.csv");
        assert_eq!(export.content_type, "text/csv; charset=utf-8");
        assert_eq!(export.commit_count, 1);
        assert!(
            export
                .csv
                .starts_with("Summary,Description,Work Type,Priority")
        );
        assert!(export.csv.contains("Export audit, evidence"));
        assert!(export.csv.contains("Added export. cargo test passed."));
        assert!(export.csv.contains("GOAL1 implement evidence export"));
        assert!(export.csv.contains("\"\"quotes\"\""));
        assert!(export.csv.ends_with("\r\n"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn budgeted_renderer_handles_exact_boundary_overflow_and_unicode() {
        let exact = vec![EvidenceSection::new(
            "exact-boundary evidence",
            "x".repeat(JIRA_DESCRIPTION_LIMIT),
            JIRA_DESCRIPTION_LIMIT,
        )];
        let exact_rendered = render_budgeted_sections(&exact, JIRA_DESCRIPTION_LIMIT);
        assert_eq!(exact_rendered.chars().count(), JIRA_DESCRIPTION_LIMIT);
        assert_eq!(exact_rendered, exact[0].text);
        assert!(!exact_rendered.contains("[shortened:"));

        let over = vec![EvidenceSection::new(
            "Unicode narrative",
            "界".repeat(JIRA_DESCRIPTION_LIMIT + 500),
            JIRA_DESCRIPTION_LIMIT,
        )];
        let first = render_budgeted_sections(&over, JIRA_DESCRIPTION_LIMIT);
        let second = render_budgeted_sections(&over, JIRA_DESCRIPTION_LIMIT);
        assert_eq!(first, second);
        assert_eq!(first.chars().count(), JIRA_DESCRIPTION_LIMIT);
        assert!(first.contains("[shortened: Unicode narrative;"));
        assert!(first.ends_with("characters omitted]"));
        assert!(first.is_char_boundary(first.len()));
    }

    #[test]
    fn large_multi_round_goal_is_bounded_auditable_and_csv_round_trips() {
        let verbose_raw_output = format!(
            "RAW_PROVIDER_OUTPUT_MUST_NOT_BE_REPLAYED {}",
            "判定".repeat(2_500)
        );
        let rounds = (1..=6)
            .map(|round| {
                json!({
                    "reporter": "Buddy Williams",
                    "assignee": "Delivery Agent",
                    "created": format!("2026-07-{round:02}T10:00:00Z"),
                    "updated": format!("2026-07-{round:02}T11:00:00Z"),
                    "prompt": format!(
                        "ROUND-{round}-REQUEST preserve Jira evidence.\n{}",
                        "large requested work with Unicode 🧭, commas, and \"quotes\". ".repeat(75)
                    ),
                    "implementation_report": format!(
                        "ROUND-{round}-OUTCOME verified shared export behavior. {}",
                        "implementation evidence passed. ".repeat(25)
                    ),
                    "implementation_reported_at": format!("2026-07-{round:02}T11:00:00Z"),
                    "quality_state": "passed",
                    "quality_message": format!("Round {round} Quality passed"),
                    "quality_checked_at": format!("2026-07-{round:02}T11:10:00Z"),
                    "quality_details": {
                        "evaluation_scope": "source_candidate",
                        "command": "cargo test --lib",
                        "exit_code": 0,
                        "candidate_commit": "candidate456",
                        "results": [
                            {"test": "unit", "status": "passed", "output": verbose_raw_output},
                            {"test": "browser", "status": "passed", "output": verbose_raw_output}
                        ],
                        "raw_output": verbose_raw_output
                    },
                    "rule_state": "passed",
                    "product_state": "passed",
                    "constitution_state": "passed",
                    "meta_rule_state": "passed",
                    "governance_message": format!("Round {round} governance passed"),
                    "governance_checked_at": format!("2026-07-{round:02}T11:20:00Z"),
                    "governance_details": {
                        "phase": "post_implementation",
                        "configured": true,
                        "rules_checked": 9,
                        "raw_output": verbose_raw_output,
                        "verdict": {
                            "status": "passed",
                            "message": format!("Round {round} governance passed"),
                            "provider_payload": verbose_raw_output
                        },
                        "failed_actions": [{
                            "rule_id": "rule-6",
                            "action": "record",
                            "message": "Verification evidence retained"
                        }]
                    },
                    "governance_rule_actions": [{
                        "rule_id": "rule-6",
                        "action": "record",
                        "message": "Verification evidence retained"
                    }]
                })
            })
            .collect::<Vec<_>>();
        let goal = json!({
            "id": "00000SZ1T921F15X91MBZWE000",
            "name": "Large Jira \"evidence\", export",
            "status": "review",
            "priority": "high",
            "reporter": "Buddy Williams",
            "assignee": "Delivery Agent",
            "feature_id": "FEATURE-AUDIT",
            "node_id": "default",
            "target_branch": "main",
            "branch_name": "refine/large-jira/round-6",
            "base_commit": "base123",
            "candidate_commit": "candidate456",
            "created": "2026-07-01T10:00:00Z",
            "updated": "2026-07-06T11:20:00Z",
            "rounds": rounds,
            "notes": [{
                "author": "Reviewer",
                "created": "2026-07-06T11:30:00Z",
                "body": format!("Review note: {}", "retain audit context. ".repeat(100))
            }]
        });
        let commits = vec![GitChange {
            commit: "candidate456".to_string(),
            committed_time: "2026-07-06T11:00:00Z".to_string(),
            subject: "Keep Jira evidence importable".to_string(),
            branch: Some("refine/large-jira/round-6".to_string()),
        }];

        let description = jira_description(&goal, &commits);
        assert!(description.chars().count() <= JIRA_DESCRIPTION_LIMIT);
        for evidence in [
            "Goal ID: 00000SZ1T921F15X91MBZWE000",
            "Status: review",
            "Priority: high",
            "Reporter: Buddy Williams",
            "Assignee: Delivery Agent",
            "Feature: FEATURE-AUDIT",
            "Node: default",
            "Target branch: main",
            "Implementation branch: refine/large-jira/round-6",
            "Base commit: base123",
            "Candidate commit: candidate456",
            "Keep Jira evidence importable",
            "ROUND-6-OUTCOME verified shared export behavior",
            "Quality checks: 2 (passed=2)",
            "Governance action: rule=rule-6",
        ] {
            assert!(description.contains(evidence), "missing {evidence}");
        }
        assert!(description.contains("[shortened: Round 1 requested work;"));
        assert!(description.contains("characters omitted]"));
        assert!(!description.contains("RAW_PROVIDER_OUTPUT_MUST_NOT_BE_REPLAYED"));
        assert_eq!(
            description
                .matches("Governance action: rule=rule-6")
                .count(),
            6,
            "the explicit action must not be duplicated from governance details"
        );
        assert_eq!(description, jira_description(&goal, &commits));

        let row = jira_row(&goal, &commits).unwrap();
        let parsed = parse_csv_row(&row);
        assert_eq!(parsed.len(), JIRA_HEADERS.len());
        assert_eq!(parsed[0], "Large Jira \"evidence\", export");
        assert_eq!(parsed[1], description);
        assert_eq!(parsed[5], "00000SZ1T921F15X91MBZWE000");
    }

    #[test]
    fn bulk_jira_export_uses_shared_selection_and_stable_goal_order() {
        let root = unique_temp_dir("jira-bulk-export");
        let refine_dir = root.join(".refine");
        let work_items = FileWorkItemService::new(&refine_dir);
        for (id, name) in [
            ("GOAL2", "Selected second"),
            ("GOAL1", "Selected first"),
            ("GOAL3", "Ignored Goal"),
        ] {
            work_items.create_goal_summary(name, Some(id)).unwrap();
            work_items
                .append_goal_round_summary(id, "Auditor", &format!("Implement {id}"))
                .unwrap();
        }
        work_items
            .edit_latest_goal_round_summary(
                "GOAL1",
                None,
                None,
                Some(&format!(
                    "Oversized valid request {}",
                    "evidence ".repeat(5_000)
                )),
            )
            .unwrap();

        let service = FileGoalExportService::new(&refine_dir, &root);
        let selected = service
            .export_bulk_jira_csv(&BulkGoalSelection {
                selected_ids: Some(vec![
                    "GOAL2".to_string(),
                    "GOAL1".to_string(),
                    "GOAL2".to_string(),
                ]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(selected.filename, "refine-goals-jira.csv");
        assert_eq!(selected.goal_ids, vec!["GOAL1", "GOAL2"]);
        assert_eq!(selected.goal_count, 2);
        assert_eq!(selected.csv.matches("Summary,Description").count(), 1);
        assert!(
            selected.csv.find("Selected first").unwrap()
                < selected.csv.find("Selected second").unwrap()
        );
        assert!(selected.csv.contains("[shortened: Round 1 requested work;"));
        assert!(!selected.csv.contains("Ignored Goal"));

        let all_matching_except_one = service
            .export_bulk_jira_csv(&BulkGoalSelection {
                filter: crate::tools::product::work_items::BulkGoalFilter {
                    q: Some("Selected".to_string()),
                    ..Default::default()
                },
                exclude_ids: vec!["GOAL2".to_string()],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all_matching_except_one.goal_ids, vec!["GOAL1"]);
        assert!(all_matching_except_one.csv.contains("Selected first"));
        assert!(!all_matching_except_one.csv.contains("Selected second"));

        let error = service
            .export_bulk_jira_csv(&BulkGoalSelection {
                selected_ids: Some(Vec::new()),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Select at least one Goal to export for Jira"
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn parse_csv_row(record: &str) -> Vec<String> {
        let mut fields = Vec::new();
        let mut field = String::new();
        let mut chars = record.chars().peekable();
        let mut quoted = false;
        while let Some(ch) = chars.next() {
            match ch {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = !quoted,
                ',' if !quoted => {
                    fields.push(std::mem::take(&mut field));
                }
                _ => field.push(ch),
            }
        }
        assert!(!quoted, "CSV row ended inside a quoted field");
        fields.push(field);
        fields
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{label}-{nanos}"))
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
