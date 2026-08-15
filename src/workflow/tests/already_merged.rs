use super::*;

#[test]
fn workflow_resolves_already_merged_quality_once_and_stops_resuming() {
    let temp_root = unique_temp_dir("workflow-already-merged-terminal");
    let target_root = temp_root.join("repo");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&target_root).unwrap();
    git(&target_root, &["init", "-b", "main"]).unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(target_root.join("app.txt"), "base\n").unwrap();
    git(&target_root, &["add", "app.txt"]).unwrap();
    git(&target_root, &["commit", "-m", "base"]).unwrap();
    let base = git_output(&target_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(&target_root, &["checkout", "-b", "refine/GOAL1/round-1"]).unwrap();
    fs::write(target_root.join("candidate.txt"), "candidate\n").unwrap();
    git(&target_root, &["add", "candidate.txt"]).unwrap();
    git(&target_root, &["commit", "-m", "candidate"]).unwrap();
    let candidate = git_output(&target_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(&target_root, &["checkout", "main"]).unwrap();
    git(&target_root, &["merge", "--no-ff", "--no-edit", &candidate]).unwrap();
    let integrated = git_output(&target_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    fs::write(target_root.join("sibling.txt"), "sibling\n").unwrap();
    git(&target_root, &["add", "sibling.txt"]).unwrap();
    git(&target_root, &["commit", "-m", "sibling descendant"]).unwrap();

    let refine_dir = test_refine_dir(&target_root);
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Already merged", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Buddy", "Implement")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL1",
            "refine/GOAL1/round-1",
            "main",
            &base,
            Some(&candidate),
        )
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    work_items
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({
                "quality_state": "passed",
                "quality_candidate_commit": candidate,
                "quality_checked_at": "2026-08-15T00:01:00Z",
                "quality_details": {
                    "candidate_commit": candidate,
                    "source_candidate_commit": candidate,
                    "evaluation_scope": "isolated_candidate"
                },
                "rule_state": "passed",
                "meta_rule_state": "passed",
                "product_state": "passed",
                "constitution_state": "passed",
                "governance_candidate_commit": candidate,
                "governance_checked_at": "2026-08-15T00:02:00Z",
                "workflow_integration": {
                    "candidate_commit": candidate,
                    "target_branch": "main",
                    "target_commit": integrated,
                    "remote": "origin",
                    "pushed": false,
                    "integrated_at": "2026-08-15T00:03:00Z",
                    "merge": {"ok": true, "conflicts": [], "message": "integrated"}
                }
            }),
        )
        .unwrap();

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let first = workflow.execute_work().unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].final_status, "review");
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["state"],
        "resolved"
    );
    assert!(workflow.execute_work().unwrap().is_empty());
    fs::remove_dir_all(temp_root).unwrap();
}
