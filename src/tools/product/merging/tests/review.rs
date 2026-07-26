use super::*;

#[test]
fn ready_merge_integrates_once_and_review_approval_only_accepts() {
    let temp_root = unique_temp_dir("ready-merge-integration");
    let repo = temp_root.join("repo");
    let refine_dir = repo.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let worktree_path = temp_root.join("repo-refine-GOAL1-round-1");
    let remote = temp_root.join("remote.git");
    fs::create_dir_all(&refine_dir).unwrap();
    init_repo(&repo);
    let refine_dir = prepare_refine_dir(&repo).unwrap();
    commit_file(&repo, "app.txt", "base\n", "initial");
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
    commit_file(&worktree_path, "feature.txt", "change\n", "GOAL1");
    git(&worktree_path, &["push", "-u", "origin", branch]).unwrap();
    let base_commit = git_stdout(&repo, &["rev-parse", "main"]);
    let candidate_commit = git_stdout(&worktree_path, &["rev-parse", "HEAD"]);

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

    let (claim_id, execution_id) = start_ready_merge_claim(&runtime_root, &repo);
    let merger = FileMergerService::new(&runtime_root, &refine_dir);
    let wrong_round = merger
        .integrate_workflow_candidate(
            "GOAL1",
            1,
            &claim_id,
            &execution_id,
            "default",
            branch,
            &candidate_commit,
            "origin",
        )
        .unwrap_err();
    assert!(
        wrong_round.to_string().contains("round changed"),
        "{wrong_round}"
    );
    assert!(
        merger
            .integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "other-node",
                branch,
                &candidate_commit,
                "origin",
            )
            .unwrap_err()
            .to_string()
            .contains("no longer owns active claim")
    );
    let first = merger.clone();
    let second = merger.clone();
    let (first_result, second_result) = thread::scope(|scope| {
        let first = scope.spawn(|| {
            first.integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
        });
        let second = scope.spawn(|| {
            second.integrate_workflow_candidate(
                "GOAL1",
                0,
                &claim_id,
                &execution_id,
                "default",
                branch,
                &candidate_commit,
                "origin",
            )
        });
        (first.join().unwrap(), second.join().unwrap())
    });
    let integrated = first_result.unwrap();
    let concurrent_recovery = second_result.unwrap();
    assert!(integrated.merge.ok);
    assert_eq!(concurrent_recovery, integrated);
    let repeated = merger
        .integrate_workflow_candidate(
            "GOAL1",
            0,
            &claim_id,
            &execution_id,
            "default",
            branch,
            &candidate_commit,
            "origin",
        )
        .unwrap();
    assert_eq!(repeated, integrated);
    work_items
        .update_goal_round_evaluation_summary("GOAL1", 0, &json!({"workflow_integration": null}))
        .unwrap();
    let recovered = merger
        .integrate_workflow_candidate(
            "GOAL1",
            0,
            &claim_id,
            &execution_id,
            "default",
            branch,
            &candidate_commit,
            "origin",
        )
        .unwrap();
    assert_eq!(recovered.candidate_commit, candidate_commit);
    assert_eq!(recovered.target_commit, integrated.target_commit);
    assert!(recovered.pushed);
    assert!(worktree_path.exists());
    assert_eq!(
        fs::read_to_string(repo.join("feature.txt")).unwrap(),
        "change\n"
    );
    let reviewed_head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Build)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Qa)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Review)
        .unwrap();

    let approved = merger.approve_reviewed_goal("GOAL1").unwrap();
    assert_eq!(approved.goal.status, GoalStatus::Done);
    assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), reviewed_head);
    assert!(worktree_path.exists());
    assert!(git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains(branch));
    assert!(git_succeeds(
        &repo,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
    ));
    assert_eq!(
        fs::read_to_string(repo.join("feature.txt")).unwrap(),
        "change\n"
    );
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    assert!(git_stdout(&repo, &["ls-remote", "origin", "refs/heads/main"]).starts_with(&head));
    let audit = fs::read_to_string(repo.join(".git/refine-audit.jsonl")).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"action\":\"push\""))
            .count(),
        1,
        "Ready Merge must be the only target-branch push"
    );
    work_items.undo_goal_summary("GOAL1").unwrap();
    let next_round = work_items
        .append_goal_round_summary("GOAL1", "Reviewer", "Address review feedback")
        .unwrap();
    assert_eq!(next_round.goal.status, GoalStatus::Todo);
    assert_eq!(next_round.goal.round_count, 2);
    let next_detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        next_detail["rounds"][0]["workflow_integration"]["target_commit"],
        integrated.target_commit
    );
    assert!(next_detail["rounds"][1]["workflow_integration"].is_null());
    assert_eq!(git_stdout(&repo, &["rev-parse", "HEAD"]), reviewed_head);
    let audit = fs::read_to_string(repo.join(".git/refine-audit.jsonl")).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"action\":\"push\""))
            .count(),
        1,
        "opening another round must not repeat target integration"
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
