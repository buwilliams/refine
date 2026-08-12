use super::*;

#[test]
fn ready_merge_push_failure_retries_without_duplicate_merge() {
    let temp_root = unique_temp_dir("ready-merge-push-retry");
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
    commit_file(&worktree_path, "feature.txt", "candidate\n", "candidate");
    let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]);
    git(&worktree_path, &["push", "-u", "origin", branch]).unwrap();
    let hook = remote.join("hooks/pre-receive");
    fs::write(
            &hook,
            "#!/bin/sh\nwhile read old new ref; do\n  test \"$ref\" != refs/heads/main || exit 1\ndone\n",
        )
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
    }

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
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
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
        .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
        .unwrap();
    let merger = FileMergerService::new(&runtime_root, &refine_dir);
    let error = merger
        .integrate_workflow_candidate("GOAL1", 0, "default", branch, &candidate_commit, "origin")
        .unwrap_err();
    assert!(
        error.to_string().contains("pre-receive hook declined"),
        "{error}"
    );
    let integrated_head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    assert!(git_succeeds(
        &repo,
        &[
            "merge-base",
            "--is-ancestor",
            &candidate_commit,
            &integrated_head
        ]
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
    assert!(
        work_items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_integration"]
            .is_null()
    );

    fs::remove_file(&hook).unwrap();
    let retried = merger
        .integrate_workflow_candidate("GOAL1", 0, "default", branch, &candidate_commit, "origin")
        .unwrap();
    assert_eq!(retried.target_commit, integrated_head);
    assert!(retried.pushed);
    assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), integrated_head);
    assert!(git_succeeds(
        &repo,
        &[
            "merge-base",
            "--is-ancestor",
            &candidate_commit,
            "origin/main"
        ]
    ));
    let audit = fs::read_to_string(repo.join(".git/refine-audit.jsonl")).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"action\":\"merge_commit_no_ff\""))
            .count(),
        1
    );

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
