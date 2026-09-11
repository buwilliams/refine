use super::*;

#[test]
fn governance_push_failure_retries_without_duplicate_merge() {
    let temp_root = unique_temp_dir("governance-integration-push-retry");
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
    let integration_service = FileGovernanceIntegrationService::new(&runtime_root, &refine_dir);
    let error = integration_service
        .integrate_workflow_candidate("GOAL1", 0, "default", branch, &candidate_commit, "origin")
        .unwrap_err();
    assert!(
        error.to_string().contains("pre-receive hook declined"),
        "{error}"
    );
    let integrated_head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    assert!(
        !git_succeeds(&repo, &["rev-parse", "--verify", "MERGE_HEAD"]),
        "the human checkout must never carry the integration merge state"
    );
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
    let retried = integration_service
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
    // Merge porcelain now runs in the detached integration worktree, so its
    // audit trail lives in that worktree's private Git directory.
    let integration_worktree = repo.join(".git/refine-integration/target");
    let worktree_git_dir = PathBuf::from(git_stdout(
        &integration_worktree,
        &["rev-parse", "--absolute-git-dir"],
    ));
    let audit = fs::read_to_string(worktree_git_dir.join("refine-audit.jsonl")).unwrap();
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

#[test]
fn forced_integration_records_real_git_evidence_and_duplicate_requests_do_not_repeat_it() {
    let root = unique_temp_dir("forced-integration");
    let repo = root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "base");
    let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let remote = root.join("remote.git");
    git(
        &root,
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
    let candidate_dir = root.join("candidate");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            candidate_dir.to_str().unwrap(),
        ],
    )
    .unwrap();
    commit_file(&candidate_dir, "feature.txt", "candidate\n", "candidate");
    let candidate = git_stdout(&candidate_dir, &["rev-parse", "HEAD"]);
    git(&candidate_dir, &["push", "-u", "origin", branch]).unwrap();
    let state = prepare_refine_dir(&repo).unwrap();
    let work = FileWorkItemService::new(&state);
    work.create_goal_summary("Integrate explicitly", Some("GOAL1"))
        .unwrap();
    work.append_goal_round_summary("GOAL1", "Operator", "Implement")
        .unwrap();
    work.update_goal_git_refs("GOAL1", branch, "main", &base, Some(&candidate))
        .unwrap();
    work.update_goal_round_evaluation_summary("GOAL1", 0, &json!({"workflow_git_remote":"origin"}))
        .unwrap();
    work.set_goal_status_unchecked("GOAL1", &GoalStatus::Failed)
        .unwrap();
    let before = work.show_goal_detail("GOAL1").unwrap();
    let request = crate::application::work_items::WorkflowControl {
        to: GoalStatus::Governance,
        reason: "Operator verified this candidate".into(),
        context: String::new(),
        expected_revision: before["workflow_revision"].as_u64().unwrap(),
        request_id: "force-integrate-1".into(),
        force: true,
        actor: "Operator".into(),
        invocation_id: None,
    };
    let service =
        FileGovernanceIntegrationService::with_target_root(root.join("runtime"), &state, &repo);
    let first = service.force_integrate("GOAL1", &request).unwrap();
    let main = git_stdout(&repo, &["rev-parse", "main"]);
    assert!(git_succeeds(
        &repo,
        &["merge-base", "--is-ancestor", &candidate, "main"]
    ));
    assert_eq!(first["decision"]["integration_performed"], true);
    let after = work.show_goal_detail("GOAL1").unwrap();
    assert_eq!(after["status"], "review");
    assert_eq!(
        after["rounds"][0]["workflow_integration"]["candidate_commit"],
        candidate
    );
    assert_ne!(after["rounds"][0]["quality_state"], "passed");
    let repeated = service.force_integrate("GOAL1", &request).unwrap();
    assert_eq!(repeated, first);
    assert_eq!(git_stdout(&repo, &["rev-parse", "main"]), main);
    fs::remove_dir_all(root).unwrap();
}
