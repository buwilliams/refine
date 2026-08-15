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
