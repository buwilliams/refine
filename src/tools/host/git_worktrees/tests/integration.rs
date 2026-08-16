use super::*;

#[test]
fn commit_is_ancestor_distinguishes_negative_results_from_inspection_failures() {
    let temp_root = unique_temp_dir("git-ancestor-result");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "README.md", "base\n", "initial");
    let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
    commit_file(&repo, "next.txt", "next\n", "next");
    let next = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let service = FileGitWorktreeService::new(&repo);

    assert!(service.commit_is_ancestor(&base, &next).unwrap());
    assert!(!service.commit_is_ancestor(&next, &base).unwrap());
    let error = service
        .commit_is_ancestor("0000000000000000000000000000000000000000", &next)
        .unwrap_err();
    assert!(
        error.to_string().contains("Git ancestry inspection failed"),
        "{error}"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn changed_paths_since_reports_the_committed_candidate_diff() {
    let temp_root = unique_temp_dir("git-changed-paths");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "README.md", "base\n", "initial");
    let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
    fs::create_dir_all(repo.join("src")).unwrap();
    commit_file(&repo, "src/lib.rs", "pub fn changed() {}\n", "code");
    let candidate = git_stdout(&repo, &["rev-parse", "HEAD"]);

    let paths = FileGitWorktreeService::new(&repo)
        .changed_paths_since(&base, &candidate)
        .unwrap();

    assert_eq!(paths, vec!["src/lib.rs"]);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_cleans_merged_branch_worktree() {
    let temp_root = unique_temp_dir("git-worktree-cleanup");
    let repo = temp_root.join("repo");
    let worktree_path = temp_root.join("repo-refine-GOAL1-round-1");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");

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
    commit_file(&worktree_path, "feature.txt", "change\n", "feature");
    git(&repo, &["merge", "--no-edit", branch]).unwrap();

    let service = FileGitWorktreeService::new(&repo);
    let cleanup = service.cleanup_merged_branch(branch).unwrap();
    assert_eq!(cleanup.branch, branch);
    assert_eq!(
        cleanup.worktree_path.as_deref(),
        Some(worktree_path.to_str().unwrap())
    );
    assert!(cleanup.worktree_removed);
    assert!(cleanup.branch_deleted);
    assert!(!worktree_path.exists());
    assert!(!git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains(branch));
    assert!(!git_succeeds(
        &repo,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
    ));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_merges_rebases_and_recovers_conflicts() {
    let temp_root = unique_temp_dir("git-conflicts");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");

    git(&repo, &["switch", "-c", "merge-side"]).unwrap();
    commit_file(&repo, "app.txt", "merge side\n", "merge side");
    git(&repo, &["switch", "main"]).unwrap();
    commit_file(&repo, "app.txt", "main side\n", "main side");

    let service = FileGitWorktreeService::new(&repo);
    let merge = service.merge("merge-side").unwrap();
    assert!(!merge.ok);
    assert_eq!(merge.conflicts, vec!["app.txt"]);
    assert!(
        fs::read_to_string(repo.join("app.txt"))
            .unwrap()
            .contains("<<<<<<<")
    );
    let recovered = service.recover().unwrap();
    assert!(recovered.ok);
    assert_eq!(service.conflicts().unwrap(), Vec::<String>::new());

    git(&repo, &["switch", "-c", "rebase-side", "HEAD~1"]).unwrap();
    commit_file(&repo, "app.txt", "rebase side\n", "rebase side");
    let rebase = service.rebase("main").unwrap();
    assert!(!rebase.ok);
    assert_eq!(rebase.conflicts, vec!["app.txt"]);
    assert!(
        fs::read_to_string(repo.join("app.txt"))
            .unwrap()
            .contains("<<<<<<<")
    );
    let recovered = service.recover().unwrap();
    assert!(recovered.ok);
    assert_eq!(service.conflicts().unwrap(), Vec::<String>::new());

    let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
    assert!(audit.contains("\"action\":\"merge\""));
    assert!(audit.contains("\"action\":\"rebase\""));
    assert!(audit.contains("\"action\":\"recover\""));
    assert!(audit.contains("\"status\":\"conflict\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_merges_and_rebases_cleanly() {
    let temp_root = unique_temp_dir("git-clean-integrations");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "base.txt", "base\n", "initial");

    git(&repo, &["switch", "-c", "merge-clean"]).unwrap();
    commit_file(&repo, "merge.txt", "merge\n", "merge clean");
    git(&repo, &["switch", "main"]).unwrap();
    let service = FileGitWorktreeService::new(&repo);
    let merge = service.merge("merge-clean").unwrap();
    assert!(merge.ok);
    assert!(repo.join("merge.txt").exists());

    git(&repo, &["switch", "-c", "rebase-clean"]).unwrap();
    commit_file(&repo, "rebase.txt", "rebase\n", "rebase clean");
    git(&repo, &["switch", "main"]).unwrap();
    commit_file(&repo, "main.txt", "main\n", "main clean");
    git(&repo, &["switch", "rebase-clean"]).unwrap();
    let rebase = service.rebase("main").unwrap();
    assert!(rebase.ok);
    assert!(repo.join("main.txt").exists());
    assert!(repo.join("rebase.txt").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_reports_dirty_worktree_merge_failure() {
    let temp_root = unique_temp_dir("git-dirty-merge");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");
    git(&repo, &["switch", "-c", "incoming"]).unwrap();
    commit_file(&repo, "app.txt", "incoming\n", "incoming");
    git(&repo, &["switch", "main"]).unwrap();
    fs::write(repo.join("app.txt"), "dirty local\n").unwrap();

    let result = FileGitWorktreeService::new(&repo)
        .merge("incoming")
        .unwrap();
    assert!(!result.ok);
    assert!(result.message.unwrap_or_default().contains("local changes"));
    assert_eq!(
        fs::read_to_string(repo.join("app.txt")).unwrap(),
        "dirty local\n"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn ensure_detached_worktree_is_idempotent_and_locked() {
    let temp_root = unique_temp_dir("git-detached-worktree");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let path = temp_root.join("integration");

    let service = FileGitWorktreeService::new(&repo);
    service.ensure_detached_worktree(&path, &head).unwrap();
    assert!(path.join("app.txt").exists());
    assert_eq!(git_stdout(&path, &["rev-parse", "HEAD"]), head);
    assert_eq!(current_branch(&path), "", "worktree must stay detached");

    service.ensure_detached_worktree(&path, &head).unwrap();
    assert_eq!(git_stdout(&path, &["rev-parse", "HEAD"]), head);

    assert!(
        !git_succeeds(&repo, &["worktree", "remove", path.to_str().unwrap()]),
        "lock must refuse removal without force"
    );
    assert!(path.join("app.txt").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn update_ref_cas_advances_and_loses_when_the_ref_moved() {
    let temp_root = unique_temp_dir("git-update-ref-cas");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "one\n", "initial");
    let first = git_stdout(&repo, &["rev-parse", "HEAD"]);
    commit_file(&repo, "app.txt", "two\n", "second");
    let second = git_stdout(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "target", &first]).unwrap();

    let service = FileGitWorktreeService::new(&repo);
    service
        .update_ref_cas("refs/heads/target", &second, &first)
        .unwrap();
    assert_eq!(
        git_stdout(&repo, &["rev-parse", "refs/heads/target"]),
        second
    );

    let error = service
        .update_ref_cas("refs/heads/target", &first, &first)
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("target advanced"), "{message}");
    assert!(message.contains(&second), "{message}");
    assert_eq!(
        git_stdout(&repo, &["rev-parse", "refs/heads/target"]),
        second,
        "losing the race must not move the ref"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn update_ref_cas_moves_a_branch_checked_out_in_the_main_worktree() {
    let temp_root = unique_temp_dir("git-update-ref-checked-out");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");
    let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["switch", "-c", "side"]).unwrap();
    commit_file(&repo, "side.txt", "side\n", "side");
    let advanced = git_stdout(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["switch", "main"]).unwrap();

    // `branch -f main` would refuse here because main is checked out; the
    // detached-worktree integration lane depends on update-ref not refusing.
    FileGitWorktreeService::new(&repo)
        .update_ref_cas("refs/heads/main", &advanced, &base)
        .unwrap();
    assert_eq!(
        git_stdout(&repo, &["rev-parse", "refs/heads/main"]),
        advanced
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn read_tree_merge_update_applies_clean_delta_and_refuses_collision() {
    let temp_root = unique_temp_dir("git-read-tree-merge");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");
    let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
    commit_file(&repo, "delta.txt", "delta\n", "delta");
    let delta = git_stdout(&repo, &["rev-parse", "HEAD"]);

    let clean = temp_root.join("clean");
    let repo_service = FileGitWorktreeService::new(&repo);
    repo_service
        .ensure_detached_worktree(&clean, &base)
        .unwrap();
    FileGitWorktreeService::new(&clean)
        .read_tree_merge_update(&base, &delta)
        .unwrap();
    assert_eq!(
        fs::read_to_string(clean.join("delta.txt")).unwrap(),
        "delta\n"
    );

    let colliding = temp_root.join("colliding");
    repo_service
        .ensure_detached_worktree(&colliding, &base)
        .unwrap();
    fs::write(colliding.join("delta.txt"), "untracked collision\n").unwrap();
    let error = FileGitWorktreeService::new(&colliding)
        .read_tree_merge_update(&base, &delta)
        .unwrap_err();
    assert!(error.to_string().contains("delta.txt"), "{error}");
    assert_eq!(
        fs::read_to_string(colliding.join("delta.txt")).unwrap(),
        "untracked collision\n",
        "refusal must leave the colliding file untouched"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn symbolic_head_branch_distinguishes_a_branch_from_detachment() {
    let temp_root = unique_temp_dir("git-symbolic-head");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");

    let service = FileGitWorktreeService::new(&repo);
    assert_eq!(
        service.symbolic_head_branch().unwrap().as_deref(),
        Some("refs/heads/main")
    );

    git(&repo, &["checkout", "--detach"]).unwrap();
    assert_eq!(service.symbolic_head_branch().unwrap(), None);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_revert_conflict_and_recover_preserves_history() {
    let temp_root = unique_temp_dir("git-revert-conflict");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "one\n", "initial");
    commit_file(&repo, "app.txt", "two\n", "second");
    let second = git_stdout(&repo, &["rev-parse", "HEAD"]);
    commit_file(&repo, "app.txt", "three\n", "third");

    let service = FileGitWorktreeService::new(&repo);
    let reverted = service.revert_commit(&second).unwrap();
    assert!(!reverted.ok);
    assert_eq!(reverted.conflicts, vec!["app.txt"]);
    assert!(
        fs::read_to_string(repo.join("app.txt"))
            .unwrap()
            .contains("<<<<<<<")
    );
    let recovered = service.recover().unwrap();
    assert!(recovered.ok);
    assert_eq!(fs::read_to_string(repo.join("app.txt")).unwrap(), "three\n");

    fs::remove_dir_all(temp_root).unwrap();
}
