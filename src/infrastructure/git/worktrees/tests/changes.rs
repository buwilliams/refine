use super::*;

#[test]
fn ensure_worktree_at_commit_recreates_checkout_after_its_directory_was_removed() {
    let temp_root = unique_temp_dir("git-exact-worktree-stale-registration");
    let repo = temp_root.join("repo");
    let target = temp_root.join("exact");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "base.txt", "base\n", "initial");
    let commit = git_stdout(&repo, &["rev-parse", "HEAD"]);

    let service = FileGitWorktreeService::new(&repo);
    let created = PathBuf::from(
        service
            .ensure_worktree_at_commit("refine/exact/round-1", &target, &commit)
            .unwrap(),
    );
    // External cleanup removes only the directory; the registration stays behind
    // and keeps the branch "checked out".
    fs::remove_dir_all(&created).unwrap();

    let recreated = PathBuf::from(
        service
            .ensure_worktree_at_commit("refine/exact/round-1", &target, &commit)
            .unwrap(),
    );
    assert_eq!(recreated, target);
    assert_eq!(current_branch(&recreated), "refine/exact/round-1");
    assert_eq!(git_stdout(&recreated, &["rev-parse", "HEAD"]), commit);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn changes_between_many_maps_multiple_ranges_from_one_commit_graph() {
    let temp_root = unique_temp_dir("git-change-ranges");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]).unwrap();
    git(&repo, &["config", "user.email", "test@example.com"]).unwrap();
    git(&repo, &["config", "user.name", "Test User"]).unwrap();
    fs::write(repo.join("app.txt"), "base\n").unwrap();
    git(&repo, &["add", "app.txt"]).unwrap();
    git(&repo, &["commit", "-m", "base"]).unwrap();
    let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("app.txt"), "first\n").unwrap();
    git(&repo, &["commit", "-am", "first delivery"]).unwrap();
    let first = git_stdout(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("app.txt"), "second\n").unwrap();
    git(&repo, &["commit", "-am", "second delivery"]).unwrap();
    let second = git_stdout(&repo, &["rev-parse", "HEAD"]);

    let ranges = vec![
        (base.clone(), first.clone()),
        (first.clone(), second.clone()),
        (base.clone(), second.clone()),
        (base.clone(), second.clone()),
    ];
    let runtime_root = temp_root.join("runtime");
    let operation =
        crate::infrastructure::process::supervisor::operations::OperationRegistry::register(
            &crate::infrastructure::process::supervisor::operations::FileOperationRegistry::new(
                &runtime_root,
            ),
            "git:change-ranges",
        )
        .unwrap();
    let changes = FileGitWorktreeService::with_runtime_root(&repo, &runtime_root)
        .with_operation_id(operation.id)
        .changes_between_many(&ranges)
        .unwrap();

    assert_eq!(changes.len(), 3);
    assert_eq!(changes[&(base.clone(), first.clone())].len(), 1);
    assert_eq!(changes[&(first.clone(), second.clone())].len(), 1);
    assert_eq!(
        changes[&(base.clone(), second.clone())]
            .iter()
            .map(|change| change.subject.as_str())
            .collect::<Vec<_>>(),
        vec!["first delivery", "second delivery"]
    );

    fs::remove_dir_all(temp_root).unwrap();
}
