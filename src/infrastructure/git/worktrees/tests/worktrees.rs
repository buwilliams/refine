use super::*;

#[test]
fn file_git_worktree_service_lists_status_and_reverts_commits() {
    let temp_root = unique_temp_dir("git-worktree");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]).unwrap();
    git(&repo, &["config", "user.email", "test@example.com"]).unwrap();
    git(&repo, &["config", "user.name", "Test User"]).unwrap();
    fs::write(repo.join("app.txt"), "one\n").unwrap();
    git(&repo, &["add", "app.txt"]).unwrap();
    git(&repo, &["commit", "-m", "initial"]).unwrap();
    fs::write(repo.join("app.txt"), "two\n").unwrap();
    git(&repo, &["commit", "-am", "update app"]).unwrap();

    let service = FileGitWorktreeService::new(&repo);
    let changes = service.recent_changes(10).unwrap();
    assert_eq!(changes[0].subject, "update app");
    let status = service.inspect("").unwrap();
    assert!(matches!(status.branch.as_deref(), Some("main" | "master")));
    assert!(!status.dirty_user_changes);

    let reverted = service.revert_commit(&changes[0].commit).unwrap();
    assert!(reverted.ok);
    assert_eq!(fs::read_to_string(repo.join("app.txt")).unwrap(), "one\n");
    let audit_path = service.audit_path().unwrap();
    assert_eq!(audit_path, repo.join(".git").join(GIT_AUDIT_FILE));
    let audit = fs::read_to_string(audit_path).unwrap();
    assert!(audit.contains("\"action\":\"revert\""));
    assert!(audit.contains("\"status\":\"ok\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn ensure_branch_at_head_tolerates_a_superseded_recovery_attempts_branch() {
    let temp_root = unique_temp_dir("git-branch-reuse");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).unwrap();
    git(&repo, &["config", "user.email", "test@example.com"]).unwrap();
    git(&repo, &["config", "user.name", "Test User"]).unwrap();
    fs::write(repo.join("app.txt"), "one\n").unwrap();
    git(&repo, &["add", "app.txt"]).unwrap();
    git(&repo, &["commit", "-m", "initial"]).unwrap();

    let service = FileGitWorktreeService::new(&repo);
    let head = service.head_ref().unwrap().commit.unwrap();

    // Fresh creation checks the branch out at HEAD.
    service
        .ensure_branch_at_head("refine/goal/round-2")
        .unwrap();
    assert_eq!(
        service.head_ref().unwrap().branch.as_deref(),
        Some("refine/goal/round-2")
    );

    // A reused recovery Round re-enters with the branch already checked out.
    service
        .ensure_branch_at_head("refine/goal/round-2")
        .unwrap();
    assert_eq!(service.head_ref().unwrap().commit.unwrap(), head);

    // A superseded attempt moved the branch; re-entry from the retained
    // candidate resets it to HEAD instead of dying on "already exists".
    fs::write(repo.join("app.txt"), "superseded\n").unwrap();
    git(&repo, &["commit", "-am", "superseded attempt work"]).unwrap();
    let superseded_tip = service.head_ref().unwrap().commit.unwrap();
    git(&repo, &["switch", "main"]).unwrap();
    service
        .ensure_branch_at_head("refine/goal/round-2")
        .unwrap();
    let after = service.head_ref().unwrap();
    assert_eq!(after.branch.as_deref(), Some("refine/goal/round-2"));
    assert_eq!(after.commit.unwrap(), head);
    assert_ne!(superseded_tip, head);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_reports_primary_worktree_refine_state_as_untracked() {
    let temp_root = unique_temp_dir("git-status");
    let repo = temp_root.join("repo");
    fs::create_dir_all(repo.join(".refine")).unwrap();
    git(&repo, &["init"]).unwrap();
    fs::write(repo.join(".refine/state.json"), "{}\n").unwrap();
    fs::write(repo.join("user.txt"), "user\n").unwrap();

    let status = FileGitWorktreeService::new(&repo).inspect("").unwrap();
    assert!(!status.dirty_user_changes);
    assert!(status.dirty_paths.is_empty());
    assert!(
        status
            .untracked_paths
            .iter()
            .any(|path| path == ".refine/state.json" || path == ".refine/")
    );
    assert!(status.untracked_paths.iter().any(|path| path == "user.txt"));
    assert!(status.refine_owned_artifacts.is_empty());
    assert!(!status.is_pristine());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_splits_tracked_dirt_from_untracked_files() {
    let temp_root = unique_temp_dir("git-status-split");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "committed\n", "initial");

    fs::write(repo.join("notes.txt"), "scratch\n").unwrap();
    let status = FileGitWorktreeService::new(&repo).inspect("").unwrap();
    assert!(!status.dirty_user_changes);
    assert!(status.dirty_paths.is_empty());
    assert_eq!(status.untracked_paths, vec!["notes.txt".to_string()]);
    assert!(!status.is_pristine());

    fs::write(repo.join("app.txt"), "modified\n").unwrap();
    fs::create_dir_all(repo.join("run/8080")).unwrap();
    fs::write(repo.join("run/8080/state.json"), "{}\n").unwrap();
    fs::create_dir_all(repo.join("target/tmp")).unwrap();
    fs::write(repo.join("target/tmp/build.txt"), "build\n").unwrap();
    let status = FileGitWorktreeService::new(&repo).inspect("").unwrap();
    assert!(status.dirty_user_changes);
    assert_eq!(status.dirty_paths, vec!["app.txt".to_string()]);
    assert_eq!(status.untracked_paths, vec!["notes.txt".to_string()]);
    assert!(status.refine_owned_artifacts.iter().any(|p| p == "run/"));
    assert!(status.refine_owned_artifacts.iter().any(|p| p == "target/"));
    let dirt = status.describe_dirt();
    assert!(dirt.contains("tracked: [app.txt]"), "{dirt}");
    assert!(dirt.contains("untracked: [notes.txt]"), "{dirt}");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_reports_literal_paths_for_renames_and_special_names() {
    let temp_root = unique_temp_dir("git-status-literal");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "committed\n", "initial");

    git(&repo, &["mv", "app.txt", "app2.txt"]).unwrap();
    fs::write(repo.join("New Document.txt"), "spaced\n").unwrap();
    fs::write(repo.join("foo[1].txt"), "glob-shaped\n").unwrap();

    let status = FileGitWorktreeService::new(&repo).inspect("").unwrap();
    // A rename reports both sides as separate literal paths, never the
    // porcelain display string "old -> new"; special names arrive without
    // porcelain C-quoting. Both feed back to Git as exact pathspecs.
    assert!(status.dirty_paths.iter().any(|path| path == "app.txt"));
    assert!(status.dirty_paths.iter().any(|path| path == "app2.txt"));
    assert_eq!(status.dirty_paths.len(), 2, "{:?}", status.dirty_paths);
    assert!(
        status
            .untracked_paths
            .iter()
            .any(|path| path == "New Document.txt"),
        "{:?}",
        status.untracked_paths
    );
    assert!(
        status
            .untracked_paths
            .iter()
            .any(|path| path == "foo[1].txt"),
        "{:?}",
        status.untracked_paths
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn quarantine_handles_renames_and_preserves_untracked_special_names() {
    let temp_root = unique_temp_dir("git-quarantine-literal");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "committed\n", "initial");

    git(&repo, &["mv", "app.txt", "app2.txt"]).unwrap();
    fs::write(repo.join("New Document.txt"), "spaced\n").unwrap();
    fs::write(repo.join("foo[1].txt"), "glob-shaped\n").unwrap();
    fs::write(repo.join("foo1.txt"), "user file\n").unwrap();

    let service = FileGitWorktreeService::new(&repo);
    let stash = service
        .quarantine_worktree_changes("quarantine tracked changes")
        .unwrap();

    assert!(stash.is_some());
    assert_eq!(
        fs::read_to_string(repo.join("app.txt")).unwrap(),
        "committed\n",
        "the staged rename must be quarantined, restoring the source path"
    );
    assert!(!repo.join("app2.txt").exists());
    // Untracked files — glob-shaped and space-bearing names included — are a
    // user's business and stay in place untouched.
    assert_eq!(
        fs::read_to_string(repo.join("New Document.txt")).unwrap(),
        "spaced\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("foo[1].txt")).unwrap(),
        "glob-shaped\n"
    );
    assert_eq!(
        fs::read_to_string(repo.join("foo1.txt")).unwrap(),
        "user file\n"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_hard_resets_tracked_changes() {
    let temp_root = unique_temp_dir("git-hard-reset");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    fs::write(repo.join("app.txt"), "committed\n").unwrap();
    git(&repo, &["add", "app.txt"]).unwrap();
    git(&repo, &["commit", "-m", "initial"]).unwrap();
    fs::write(repo.join("app.txt"), "dirty\n").unwrap();
    fs::write(repo.join("untracked.txt"), "keep\n").unwrap();
    fs::create_dir_all(repo.join(".refine")).unwrap();
    fs::write(repo.join(".refine/state.json"), "{}\n").unwrap();

    let service = FileGitWorktreeService::new(&repo);
    let reset = service.hard_reset().unwrap();
    assert!(reset.ok);
    assert_eq!(
        fs::read_to_string(repo.join("app.txt")).unwrap(),
        "committed\n"
    );
    assert!(!repo.join("untracked.txt").exists());
    assert!(!repo.join(".refine").exists());
    let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
    assert!(audit.contains("\"action\":\"hard_reset\""));
    assert!(audit.contains("\"status\":\"ok\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes() {
    let temp_root = unique_temp_dir("git-workflow-happy");
    let repo = temp_root.join("repo");
    let remote = temp_root.join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
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
    commit_file(&repo, "base.txt", "base\n", "initial");

    let service = FileGitWorktreeService::new(&repo);
    assert_eq!(
        service.ensure_branch_at_head("feature/pathspec").unwrap(),
        "feature/pathspec"
    );
    assert_eq!(current_branch(&repo), "feature/pathspec");
    commit_file(
        &repo,
        "tracked.txt",
        "base tracked\n",
        "track selected file",
    );
    fs::write(repo.join("tracked.txt"), "tracked\n").unwrap();
    fs::write(repo.join("ignored.txt"), "ignored\n").unwrap();
    let diff = service.diff(&["tracked.txt".to_string()]).unwrap();
    assert!(diff.contains("tracked"));
    assert!(!diff.contains("ignored"));

    let commit = service
        .commit("commit selected path", &["tracked.txt".to_string()])
        .unwrap();
    assert_eq!(
        git_stdout(&repo, &["show", "--pretty=format:", "--name-only", &commit]),
        "tracked.txt"
    );
    assert!(git_stdout(&repo, &["status", "--porcelain=v1"]).contains("?? ignored.txt"));
    service.push("origin", "feature/pathspec").unwrap();
    assert_eq!(
        git_stdout(&repo, &["rev-parse", "origin/feature/pathspec^{commit}"]),
        commit
    );

    git(&repo, &["switch", "main"]).unwrap();
    let worktree_path = PathBuf::from(service.worktree("feature/worktree").unwrap());
    assert_eq!(
        worktree_path,
        repo.join(".git/refine-worktrees/feature-worktree")
    );
    assert!(worktree_path.join(".git").exists());
    assert_eq!(current_branch(&worktree_path), "feature/worktree");
    let worktree_status = service.inspect(worktree_path.to_str().unwrap()).unwrap();
    assert_eq!(worktree_status.root, worktree_path.display().to_string());
    assert_eq!(worktree_status.branch.as_deref(), Some("feature/worktree"));
    let linked_service = FileGitWorktreeService::new(&worktree_path);
    fs::write(worktree_path.join("base.txt"), "linked change\n").unwrap();
    assert!(
        linked_service
            .diff(&["base.txt".to_string()])
            .unwrap()
            .contains("linked change")
    );
    let linked_audit_path = linked_service.audit_path().unwrap();
    assert!(linked_audit_path.exists());
    assert_ne!(
        linked_audit_path,
        worktree_path.join(".git").join(GIT_AUDIT_FILE)
    );

    let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
    for action in ["branch", "diff", "commit", "push", "worktree"] {
        assert!(audit.contains(&format!("\"action\":\"{action}\"")));
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn ensure_worktree_recreates_checkout_after_its_directory_was_removed() {
    let temp_root = unique_temp_dir("git-worktree-stale-registration");
    let repo = temp_root.join("repo");
    let target = temp_root.join("candidate");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "base.txt", "base\n", "initial");

    let service = FileGitWorktreeService::new(&repo);
    let created = PathBuf::from(
        service
            .ensure_worktree("refine/stale/round-1", &target)
            .unwrap(),
    );
    let tip = git_stdout(&created, &["rev-parse", "HEAD"]);
    // External cleanup removes only the directory; the registration stays behind
    // and keeps the branch "checked out".
    fs::remove_dir_all(&created).unwrap();

    let recreated = PathBuf::from(
        service
            .ensure_worktree("refine/stale/round-1", &target)
            .unwrap(),
    );
    assert_eq!(recreated, target);
    assert!(recreated.join(".git").exists());
    assert_eq!(current_branch(&recreated), "refine/stale/round-1");
    assert_eq!(git_stdout(&recreated, &["rev-parse", "HEAD"]), tip);
    let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
    assert!(audit.contains("\"action\":\"worktree_prune\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn locked_candidate_worktree_registration_survives_repo_wide_prune() {
    let temp_root = unique_temp_dir("git-worktree-lock-prune");
    let repo = temp_root.join("repo");
    let target = temp_root.join("candidate");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "base.txt", "base\n", "initial");

    let service = FileGitWorktreeService::new(&repo);
    let created = PathBuf::from(
        service
            .ensure_worktree("refine/locked/round-1", &target)
            .unwrap(),
    );
    assert!(git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains("locked"));

    // Repo-wide prune sweeps (state sync, source promotion) must not drop the
    // registration even after external cleanup removed the directory.
    fs::remove_dir_all(&created).unwrap();
    git(&repo, &["worktree", "prune"]).unwrap();
    assert!(
        git_stdout(&repo, &["worktree", "list", "--porcelain"])
            .contains(&created.display().to_string())
    );

    // Self-healing still recreates the checkout past its own retained lock.
    let recreated = PathBuf::from(
        service
            .ensure_worktree("refine/locked/round-1", &target)
            .unwrap(),
    );
    assert_eq!(recreated, target);
    assert_eq!(current_branch(&recreated), "refine/locked/round-1");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn remove_worktree_unlocks_a_locked_candidate_worktree_first() {
    let temp_root = unique_temp_dir("git-worktree-unlock-remove");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "base.txt", "base\n", "initial");

    let service = FileGitWorktreeService::new(&repo);
    let clean = PathBuf::from(
        service
            .ensure_worktree("refine/unlock/clean", &temp_root.join("clean"))
            .unwrap(),
    );
    service.remove_worktree(&clean, false).unwrap();
    assert!(!clean.exists());

    let forced = PathBuf::from(
        service
            .ensure_worktree("refine/unlock/forced", &temp_root.join("forced"))
            .unwrap(),
    );
    fs::write(forced.join("dirty.txt"), "dirty\n").unwrap();
    service.remove_worktree(&forced, true).unwrap();
    assert!(!forced.exists());
    assert!(!git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains("refine/unlock"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_bootstraps_unborn_repo_for_worktree() {
    let temp_root = unique_temp_dir("git-worktree-unborn");
    let repo = temp_root.join("repo");
    let target = temp_root.join("standalone");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    fs::write(repo.join("staged.txt"), "staged\n").unwrap();
    fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();
    git(&repo, &["add", "staged.txt"]).unwrap();

    let service = FileGitWorktreeService::new(&repo);
    let path = PathBuf::from(
        service
            .ensure_worktree("refine/standalone/test", &target)
            .unwrap(),
    );

    assert_eq!(path, target);
    assert!(path.join(".git").exists());
    assert_eq!(current_branch(&path), "refine/standalone/test");
    assert_eq!(
        git_stdout(&repo, &["log", "--pretty=%s", "-1"]),
        "Initialize Refine workspace"
    );
    assert_eq!(
        git_stdout(&repo, &["show", "--pretty=format:", "--name-only", "HEAD"]),
        ""
    );
    assert!(git_stdout(&repo, &["status", "--porcelain=v1"]).contains("A  staged.txt"));
    assert!(git_stdout(&repo, &["status", "--porcelain=v1"]).contains("?? untracked.txt"));
    assert!(!path.join("staged.txt").exists());
    assert!(!path.join("untracked.txt").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_does_not_track_read_only_git_probes() {
    let temp_root = unique_temp_dir("git-read-only-untracked");
    let repo = temp_root.join("repo");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");

    let service = FileGitWorktreeService::with_runtime_root(&repo, &runtime_root);
    service.inspect("").unwrap();
    service.recent_changes(10).unwrap();
    service.diff(&["app.txt".to_string()]).unwrap();

    let process_count = fs::read_dir(runtime_root.join("processes"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(process_count, 0);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_rejects_invalid_names_and_reports_git_failures() {
    let temp_root = unique_temp_dir("git-invalid");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "base\n", "initial");
    let service = FileGitWorktreeService::new(&repo);

    for name in ["", "-bad", "bad..name", "bad//name", "bad name"] {
        assert!(matches!(
            service.ensure_branch_at_head(name),
            Err(RefineError::InvalidInput(_))
        ));
        assert!(matches!(
            service.worktree(name),
            Err(RefineError::InvalidInput(_))
        ));
        assert!(matches!(
            service.merge(name),
            Err(RefineError::InvalidInput(_))
        ));
        assert!(matches!(
            service.rebase(name),
            Err(RefineError::InvalidInput(_))
        ));
        assert!(matches!(
            service.push("origin", name),
            Err(RefineError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        service.push("", "main"),
        Err(RefineError::InvalidInput(_))
    ));
    assert!(matches!(
        service.revert_commit("bad ref!"),
        Err(RefineError::InvalidInput(_))
    ));
    assert!(matches!(
        service.worktree("main"),
        Err(RefineError::Conflict(_))
    ));
    assert!(matches!(
        service.push("missing-remote", "main"),
        Err(RefineError::Conflict(_))
    ));
    let missing_revert = service.revert_commit("deadbeef").unwrap();
    assert!(!missing_revert.ok);
    assert!(
        missing_revert
            .message
            .unwrap_or_default()
            .contains("deadbeef")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_accepts_clean_branch_commit_since_base() {
    let temp_root = unique_temp_dir("git-existing-commit");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "base.txt", "base\n", "initial");
    git(&repo, &["switch", "-c", "feature/precommitted"]).unwrap();
    commit_file(&repo, "agent.txt", "agent\n", "agent commit");
    let precommitted = git_stdout(&repo, &["rev-parse", "HEAD"]);

    let service = FileGitWorktreeService::new(&repo);
    let commit = service
        .commit_or_current_if_clean_since("Refine commit wrapper", &[], "main")
        .unwrap();
    assert_eq!(commit, precommitted);
    assert_eq!(
        git_stdout(&repo, &["log", "--pretty=%s", "-1"]),
        "agent commit"
    );
    let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
    assert!(audit.contains("\"action\":\"commit_existing\""));
    assert!(audit.contains(&precommitted));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_git_worktree_service_hard_reset_preserves_runtime_and_removes_other_noise() {
    let temp_root = unique_temp_dir("git-hard-reset-runtime");
    let repo = temp_root.join("repo");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "committed\n", "initial");
    fs::write(repo.join("app.txt"), "dirty\n").unwrap();
    fs::write(repo.join("untracked.txt"), "remove\n").unwrap();
    fs::create_dir_all(runtime_root.join("processes")).unwrap();
    fs::write(runtime_root.join("processes/pid.json"), "{}\n").unwrap();
    fs::create_dir_all(repo.join("run/8080")).unwrap();
    fs::write(repo.join("run/8080/state.json"), "{}\n").unwrap();
    fs::create_dir_all(repo.join("target/tmp")).unwrap();
    fs::write(repo.join("target/tmp/build.txt"), "build\n").unwrap();

    let service = FileGitWorktreeService::with_runtime_root(&repo, &runtime_root);
    let status = service.inspect("").unwrap();
    assert!(status.dirty_user_changes);
    assert!(
        status
            .refine_owned_artifacts
            .iter()
            .any(|path| path == "run/")
    );
    assert!(
        status
            .refine_owned_artifacts
            .iter()
            .any(|path| path == "target/")
    );

    let reset = service.hard_reset().unwrap();
    assert!(reset.ok);
    assert_eq!(
        fs::read_to_string(repo.join("app.txt")).unwrap(),
        "committed\n"
    );
    assert!(!repo.join("untracked.txt").exists());
    assert!(runtime_root.join("processes/pid.json").exists());
    assert!(repo.join("run/8080/state.json").exists());
    assert!(repo.join("target/tmp/build.txt").exists());

    fs::remove_dir_all(temp_root).unwrap();
}
