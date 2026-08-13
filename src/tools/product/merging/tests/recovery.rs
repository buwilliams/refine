use super::*;

#[test]
fn governance_conflict_aborts_without_advancing_or_losing_candidate() {
    let temp_root = unique_temp_dir("ready-merge-conflict");
    let repo = temp_root.join("repo");
    let refine_dir = repo.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let worktree_path = temp_root.join("candidate");
    let remote = temp_root.join("remote.git");
    fs::create_dir_all(&refine_dir).unwrap();
    init_repo(&repo);
    let refine_dir = prepare_refine_dir(&repo).unwrap();
    commit_file(&repo, "app.txt", "base\n", "initial");
    let base_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
    git(
        &temp_root,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    )
    .unwrap();
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .unwrap();
    git(&repo, &["push", "-u", "origin", "main"]).unwrap();
    let branch = "refine/GOAL1/round-1";
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            worktree_path.to_str().unwrap(),
        ],
    )
    .unwrap();
    commit_file(&worktree_path, "app.txt", "candidate\n", "candidate");
    let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]);
    git(&worktree_path, &["push", "-u", "origin", branch]).unwrap();
    commit_file(&repo, "app.txt", "target\n", "target");
    let target_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["push", "origin", "main"]).unwrap();

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("GOAL1", Some("GOAL1"))
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
            branch,
            "main",
            &base_commit,
            Some(&candidate_commit),
        )
        .unwrap();
    work_items
        .update_goal_round_evaluation_summary("GOAL1", 0, &json!({"workflow_git_remote": "origin"}))
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Governance)
        .unwrap();

    let error = FileMergerService::new(&runtime_root, &refine_dir)
        .integrate_workflow_candidate("GOAL1", 0, "default", branch, &candidate_commit, "origin")
        .unwrap_err();
    assert!(
        error.to_string().contains("candidate integration failed"),
        "{error}"
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Governance
    );
    assert!(
        work_items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_integration"]
            .is_null()
    );
    assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), target_commit);
    assert_eq!(
        fs::read_to_string(repo.join("app.txt")).unwrap(),
        "target\n"
    );
    assert_eq!(
        fs::read_to_string(worktree_path.join("app.txt")).unwrap(),
        "candidate\n"
    );
    assert!(git_stdout(&repo, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
    assert!(!git_succeeds(
        &repo,
        &["rev-parse", "--verify", "MERGE_HEAD"]
    ));
    assert!(!git_succeeds(
        &repo,
        &[
            "merge-base",
            "--is-ancestor",
            &candidate_commit,
            "origin/main"
        ]
    ));

    git(
        &repo,
        &[
            "worktree",
            "remove",
            "--force",
            worktree_path.to_str().unwrap(),
        ],
    )
    .unwrap();
    fs::remove_dir_all(temp_root).unwrap();
}
