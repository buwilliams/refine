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
                "implementation_plan": {
                    "schema_version": 1,
                    "state": "completed",
                    "phase": "implement",
                    "final_plan": {"result": {
                        "summary": "Export the shared planning evidence",
                        "checklist": [{"id": "P1", "description": "Expose evidence"}]
                    }}
                },
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
    assert!(
        export
            .csv
            .contains("Implementation planning state: completed")
    );
    assert!(
        export
            .csv
            .contains("Final implementation plan: Export the shared planning evidence")
    );
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
            filter: crate::application::work_items::BulkGoalFilter {
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
