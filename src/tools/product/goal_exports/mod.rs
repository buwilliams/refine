use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitChange};
use crate::tools::product::work_items::{BulkGoalSelection, FileWorkItemService};

const JIRA_DESCRIPTION_LIMIT: usize = 30_000;
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
        return Err(RefineError::InvalidInput(format!(
            "Goal {goal_id} Jira description exceeds Jira's {JIRA_DESCRIPTION_LIMIT} character limit"
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
    let mut sections = Vec::new();
    let mut overview = vec![
        "Refine delivery evidence".to_string(),
        format!("Goal ID: {}", string_or(goal, "id", "Unknown")),
        format!("Status: {}", string_or(goal, "status", "Unknown")),
        format!("Priority: {}", string_or(goal, "priority", "Unknown")),
        format!("Reporter: {}", string_or(goal, "reporter", "Unreported")),
        format!("Assignee: {}", string_or(goal, "assignee", "Unassigned")),
        format!("Created: {}", string_or(goal, "created", "Unknown")),
        format!("Updated: {}", string_or(goal, "updated", "Unknown")),
    ];
    push_optional_line(&mut overview, "Feature", goal, "feature_id");
    push_optional_line(&mut overview, "Node", goal, "node_id");
    sections.push(overview.join("\n"));

    let mut commit_evidence = vec!["Commit evidence".to_string()];
    push_optional_line(&mut commit_evidence, "Target branch", goal, "target_branch");
    push_optional_line(
        &mut commit_evidence,
        "Implementation branch",
        goal,
        "branch_name",
    );
    push_optional_line(&mut commit_evidence, "Base commit", goal, "base_commit");
    push_optional_line(
        &mut commit_evidence,
        "Candidate commit",
        goal,
        "candidate_commit",
    );
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
    sections.push(commit_evidence.join("\n"));

    if let Some(rounds) = goal.get("rounds").and_then(Value::as_array) {
        for (index, round) in rounds.iter().enumerate() {
            let mut lines = vec![format!("Round {}", index + 1)];
            push_optional_line(&mut lines, "Reporter", round, "reporter");
            push_optional_line(&mut lines, "Assignee", round, "assignee");
            push_optional_line(&mut lines, "Created", round, "created");
            push_optional_line(&mut lines, "Updated", round, "updated");
            push_optional_block(&mut lines, "Requested work", round, "prompt");
            push_optional_block(&mut lines, "Guidance decision", round, "guidance_decision");
            push_optional_block(
                &mut lines,
                "What changed and verification",
                round,
                "implementation_report",
            );
            push_optional_line(
                &mut lines,
                "Implementation reported at",
                round,
                "implementation_reported_at",
            );
            push_optional_line(&mut lines, "Quality state", round, "quality_state");
            push_optional_block(&mut lines, "Quality result", round, "quality_message");
            push_optional_json(&mut lines, "Quality details", round, "quality_details");
            push_optional_line(
                &mut lines,
                "Quality checked at",
                round,
                "quality_checked_at",
            );
            push_optional_line(&mut lines, "Rule state", round, "rule_state");
            push_optional_line(&mut lines, "Product state", round, "product_state");
            push_optional_line(
                &mut lines,
                "Constitution state",
                round,
                "constitution_state",
            );
            push_optional_line(&mut lines, "Meta rule state", round, "meta_rule_state");
            push_optional_block(&mut lines, "Governance result", round, "governance_message");
            push_optional_json(
                &mut lines,
                "Governance details",
                round,
                "governance_details",
            );
            push_optional_json(
                &mut lines,
                "Governance rule actions",
                round,
                "governance_rule_actions",
            );
            push_optional_line(
                &mut lines,
                "Governance checked at",
                round,
                "governance_checked_at",
            );
            sections.push(lines.join("\n"));
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
        sections.push(lines.join("\n"));
    }

    sections.join("\n\n")
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

fn push_optional_line(lines: &mut Vec<String>, label: &str, value: &Value, key: &str) {
    if let Some(value) = nonempty_string(value, key) {
        lines.push(format!("{label}: {value}"));
    }
}

fn push_optional_block(lines: &mut Vec<String>, label: &str, value: &Value, key: &str) {
    if let Some(value) = nonempty_string(value, key) {
        lines.push(format!("{label}:\n{value}"));
    }
}

fn push_optional_json(lines: &mut Vec<String>, label: &str, value: &Value, key: &str) {
    let Some(value) = value.get(key).filter(|value| !value.is_null()) else {
        return;
    };
    if value.as_str().is_some_and(|value| value.trim().is_empty()) {
        return;
    }
    if value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
    {
        return;
    }
    let rendered = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    lines.push(format!("{label}:\n{rendered}"));
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
