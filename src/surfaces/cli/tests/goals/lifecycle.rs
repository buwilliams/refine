use super::*;

#[test]
fn goal_edit_note_delete_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-goal-edit-note-delete");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Original",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "edit",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
            "--name",
            "Renamed",
            "--priority",
            "medium",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "note",
            "GOAL1",
            "CLI note",
            "--target-root",
            target_root.to_str().unwrap(),
            "--author",
            "Reviewer",
        ])
        .unwrap(),
    )
    .unwrap();

    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"name\": \"Renamed\""));
    assert!(written.contains("\"priority\": \"medium\""));
    assert!(written.contains("\"body\": \"CLI note\""));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "delete",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn goal_approve_and_undo_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-goal-merge-undo");
    let target_root = temp_root.clone();
    fs::create_dir_all(&target_root).unwrap();
    run_git(&target_root, &["init", "-b", "main"]);
    run_git(&target_root, &["config", "user.email", "test@example.com"]);
    run_git(&target_root, &["config", "user.name", "Test User"]);
    fs::write(target_root.join("app.txt"), "base\n").unwrap();
    run_git(&target_root, &["add", "app.txt"]);
    run_git(&target_root, &["commit", "-m", "initial"]);
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Merge Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();
    let branch = "refine/GOAL1/round-1";
    let worktree = target_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    run_git(
        &target_root,
        &["worktree", "add", "-b", branch, worktree.to_str().unwrap()],
    );
    fs::write(worktree.join("approved.txt"), "approved\n").unwrap();
    run_git(&worktree, &["add", "approved.txt"]);
    run_git(&worktree, &["commit", "-m", "candidate"]);
    let base_commit = run_git_stdout(&target_root, &["rev-parse", "main"]);
    let candidate_commit = run_git_stdout(&worktree, &["rev-parse", "HEAD"]);
    run_git(&target_root, &["merge", "--no-ff", "--no-edit", branch]);
    let target_commit = run_git_stdout(&target_root, &["rev-parse", "HEAD"]);
    let service = FileWorkItemService::new(&refine_dir);
    service
        .append_goal_round_summary("GOAL1", "Buddy", "Implement")
        .unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    service
        .update_goal_git_refs(
            "GOAL1",
            branch,
            "main",
            base_commit.trim(),
            Some(candidate_commit.trim()),
        )
        .unwrap();
    service
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &serde_json::json!({
                "workflow_quality_timing": "pre_merge",
                "workflow_git_remote": "origin",
                "workflow_integration": {
                    "candidate_commit": candidate_commit.trim(),
                    "target_branch": "main",
                    "target_commit": target_commit.trim(),
                    "remote": "origin",
                    "pushed": false,
                    "integrated_at": "2026-07-23T00:00:00Z",
                    "merge": {"ok": true, "conflicts": [], "message": "test integration"}
                }
            }),
        )
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Governance)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Review)
        .unwrap();
    let goal_path = refine_dir.join("goals/GO/AL1/goal.json");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "approve",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let written = fs::read_to_string(&goal_path).unwrap();
    assert!(written.contains("\"status\": \"done\""));
    assert_eq!(
        fs::read_to_string(target_root.join("approved.txt")).unwrap(),
        "approved\n"
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "undo",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let written = fs::read_to_string(&goal_path).unwrap();
    assert!(written.contains("\"status\": \"review\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_transfer_command_moves_feature_and_member_goals_between_nodes() {
    let temp_root = unique_temp_dir("cli-feature-node-transfer");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    for argv in [
        vec![
            "refine",
            "node",
            "create",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "feature",
            "create",
            "Transfer Feature",
            "--id",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "goal",
            "create",
            "Feature Goal",
            "--id",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "feature",
            "add-goal",
            "FEA1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let direct_goal = dispatch(
        Cli::try_parse_from([
            "refine",
            "node",
            "transfer",
            "node-1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap_err();
    assert!(
        direct_goal
            .to_string()
            .contains("transfer the Feature instead"),
        "{direct_goal}"
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "transfer",
            "FEA1",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let feature = fs::read_to_string(refine_dir.join("features/FE/A1/feature.json")).unwrap();
    assert!(feature.contains("\"node_id\": \"node-1\""));
    let goal = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(goal.contains("\"node_id\": \"node-1\""));

    fs::remove_dir_all(temp_root).unwrap();
}
