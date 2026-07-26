use super::*;

struct FailingHelperLauncher;

impl SourcePromotionHelperLauncher for FailingHelperLauncher {
    fn launch(&self, _command: &mut Command) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "mock helper launch denied",
        ))
    }
}

#[derive(Default)]
struct FakeHost {
    fail: Option<&'static str>,
    calls: Vec<String>,
}

impl FakeHost {
    fn call(&mut self, stage: &str) -> RefineResult<()> {
        self.calls.push(stage.to_string());
        if self.fail == Some(stage) {
            Err(RefineError::Degraded(format!("{stage} failed")))
        } else {
            Ok(())
        }
    }
}

impl SourcePromotionHost for FakeHost {
    fn build_candidate(&mut self, _commit: &str) -> RefineResult<PathBuf> {
        self.call("build")?;
        Ok(PathBuf::from("/candidate/refine"))
    }
    fn verify_preconditions(&mut self, _from: &str, _to: &str) -> RefineResult<()> {
        self.call("preflight")
    }
    fn stop_daemon(&mut self) -> RefineResult<()> {
        self.call("stop")
    }
    fn activate(&mut self, _from: &str, _to: &str) -> RefineResult<()> {
        self.call("activate")
    }
    fn restart_daemon(&mut self, _executable: &Path) -> RefineResult<()> {
        self.call("restart")
    }
    fn verify_daemon(&mut self, _expected: &str) -> RefineResult<()> {
        self.call("verify")
    }
    fn rollback(&mut self, _from: &str, _to: &str) -> RefineResult<()> {
        self.call("rollback")
    }
    fn restart_previous_daemon(&mut self) -> RefineResult<()> {
        self.call("restart_previous")
    }
}

fn operation() -> SourcePromotionOperation {
    SourcePromotionOperation {
        id: "source-test".to_string(),
        status: "queued".to_string(),
        stage: "queued".to_string(),
        message: String::new(),
        checkout_path: "/refine".to_string(),
        from_commit: "aaa".to_string(),
        to_commit: "bbb".to_string(),
        started_at: now_timestamp(),
        updated_at: now_timestamp(),
        error: None,
        rollback_attempted: false,
        rollback_succeeded: None,
        recovery: None,
    }
}

fn test_snapshot(checkout: &Path) -> SourcePromotionSnapshot {
    SourcePromotionSnapshot {
        checkout_path: checkout.display().to_string(),
        current_commit: "aaa".to_string(),
        remote: "origin".to_string(),
        local_branch: "main".to_string(),
        branch: "main".to_string(),
        available_commit: "bbb".to_string(),
        clean: true,
        fast_forward: true,
        update_available: true,
        active_work: Vec::new(),
        operation: None,
    }
}

fn test_directory(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("refine-{label}-{}", Uuid::new_v4()))
}

fn initialize_git_repository(root: &Path) -> String {
    fs::create_dir_all(root).unwrap();
    git_ok(root, &["init", "--quiet", "."]).unwrap();
    git_ok(root, &["config", "user.email", "refine-test@example.com"]).unwrap();
    git_ok(root, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(root.join("fixture.txt"), "candidate fixture\n").unwrap();
    git_ok(root, &["add", "fixture.txt"]).unwrap();
    git_ok(root, &["commit", "--quiet", "-m", "fixture"]).unwrap();
    git_text(root, &["rev-parse", "HEAD"]).unwrap()
}

struct PromotionRepository {
    root: PathBuf,
    checkout: PathBuf,
    from_commit: String,
    to_commit: String,
}

fn initialize_promotion_repository(label: &str) -> PromotionRepository {
    let root = test_directory(label);
    let checkout = root.join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    git_ok(&checkout, &["init", "--quiet", "."]).unwrap();
    git_ok(
        &checkout,
        &["config", "user.email", "refine-test@example.com"],
    )
    .unwrap();
    git_ok(&checkout, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(
        checkout.join("Cargo.toml"),
        "[package]\nname = \"source-promotion-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(checkout.join("fixture.txt"), "prior fixture\n").unwrap();
    git_ok(&checkout, &["add", "Cargo.toml", "fixture.txt"]).unwrap();
    git_ok(&checkout, &["commit", "--quiet", "-m", "prior source"]).unwrap();
    git_ok(&checkout, &["branch", "-M", "main"]).unwrap();
    let from_commit = git_text(&checkout, &["rev-parse", "HEAD"]).unwrap();

    fs::write(checkout.join("fixture.txt"), "promoted fixture\n").unwrap();
    git_ok(&checkout, &["add", "fixture.txt"]).unwrap();
    git_ok(&checkout, &["commit", "--quiet", "-m", "promoted source"]).unwrap();
    let to_commit = git_text(&checkout, &["rev-parse", "HEAD"]).unwrap();
    git_ok(
        &checkout,
        &["update-ref", "refs/remotes/origin/main", &to_commit],
    )
    .unwrap();
    git_ok(&checkout, &["reset", "--hard", "--quiet", &from_commit]).unwrap();

    PromotionRepository {
        root,
        checkout,
        from_commit,
        to_commit,
    }
}

fn assert_checked_out_commit(checkout: &Path, commit: &str, contents: &str) {
    assert_eq!(git_text(checkout, &["rev-parse", "HEAD"]).unwrap(), commit);
    assert_eq!(
        git_text(checkout, &["rev-parse", "refs/heads/main"]).unwrap(),
        commit
    );
    let tree = format!("{commit}^{{tree}}");
    assert_eq!(
        git_text(checkout, &["write-tree"]).unwrap(),
        git_text(checkout, &["rev-parse", &tree]).unwrap()
    );
    assert_eq!(
        fs::read_to_string(checkout.join("fixture.txt")).unwrap(),
        contents
    );
    assert_eq!(git_text(checkout, &["status", "--porcelain"]).unwrap(), "");
}

#[test]
fn helper_launch_failure_persists_terminal_retryable_operation() {
    let root = test_directory("source-helper-launch");
    let service = FileSourcePromotionService::new(&root, root.join("runtime/8080"), 8080);
    let snapshot = test_snapshot(&root);

    let error = service
        .queue_validated(&snapshot, Path::new("/mock/refine"), &FailingHelperLauncher)
        .unwrap_err();

    assert!(error.to_string().contains("mock helper launch denied"));
    let failed = service.load_operation().unwrap().unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.stage, "launch_helper");
    assert!(
        failed
            .error
            .as_deref()
            .unwrap()
            .contains("mock helper launch denied")
    );
    assert!(failed.recovery.as_deref().unwrap().contains("retry"));
    assert!(
        service
            .active_work()
            .unwrap()
            .iter()
            .all(|item| !item.starts_with("source promotion "))
    );
    let reconnected = FileSourcePromotionService::new(&root, root.join("runtime/8080"), 8080)
        .load_operation()
        .unwrap()
        .unwrap();
    assert_eq!(reconnected, failed);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_promotion_affordance_covers_target_readiness_and_persisted_operations() {
    let mut source = test_snapshot(Path::new("/refine"));

    let hidden = source_promotion_affordance(false, &source);
    assert!(!hidden.visible);
    assert!(!hidden.enabled);
    assert_eq!(hidden.state, "hidden");

    let available = source_promotion_affordance(true, &source);
    assert!(available.visible);
    assert!(available.enabled);
    assert_eq!(available.state, "available");
    assert!(available.title.contains("bbb"));

    source.clean = false;
    let blocked = source_promotion_affordance(true, &source);
    assert!(!blocked.enabled);
    assert_eq!(blocked.state, "blocked");
    assert!(blocked.title.contains("uncommitted changes"));

    source.clean = true;
    source.operation = Some(operation());
    let updating = source_promotion_affordance(true, &source);
    assert!(!updating.enabled);
    assert_eq!(updating.state, "updating");

    source.operation.as_mut().unwrap().status = "failed".to_string();
    let retryable = source_promotion_affordance(true, &source);
    assert!(retryable.enabled);
    assert_eq!(retryable.state, "available");

    source.operation = None;
    source.update_available = false;
    source.available_commit = source.current_commit.clone();
    let current = source_promotion_affordance(true, &source);
    assert!(!current.enabled);
    assert_eq!(current.state, "current");
}

#[test]
fn candidate_build_spawn_failure_cleans_worktree_and_allows_retry() {
    let root = test_directory("source-candidate-retry");
    let checkout = root.join("checkout");
    let commit = initialize_git_repository(&checkout);
    let service = FileSourcePromotionService::new(&checkout, root.join("runtime/8080"), 8080);
    let mut host = FileSourcePromotionHost::new(service.clone());
    host.candidate_builder = root.join("missing-candidate-builder");

    for _ in 0..2 {
        let error = host.build_candidate(&commit).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to launch candidate build"),
            "{error}"
        );
        let worktrees = git_text(&checkout, &["worktree", "list", "--porcelain"]).unwrap();
        assert_eq!(
            worktrees
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1,
            "{worktrees}"
        );
    }
    let artifact_root = service.port_runtime_root.join("source-promotion");
    assert_eq!(fs::read_dir(artifact_root).unwrap().count(), 0);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn candidate_build_reports_primary_and_unrecovered_cleanup_failures() {
    use std::os::unix::fs::PermissionsExt;

    let root = test_directory("source-candidate-cleanup-failure");
    let checkout = root.join("checkout");
    let commit = initialize_git_repository(&checkout);
    let service = FileSourcePromotionService::new(&checkout, root.join("runtime/8080"), 8080);
    let builder = root.join("fail-build");
    fs::write(
        &builder,
        "#!/bin/sh\nchmod 0555 ..\necho 'mock primary build failure' >&2\nexit 42\n",
    )
    .unwrap();
    fs::set_permissions(&builder, fs::Permissions::from_mode(0o755)).unwrap();
    let mut host = FileSourcePromotionHost::new(service.clone());
    host.candidate_builder = builder;

    let error = host.build_candidate(&commit).unwrap_err();
    let artifact_root = service.port_runtime_root.join("source-promotion");
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o755)).unwrap();
    let candidate = fs::read_dir(&artifact_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    assert!(
        cleanup_candidate_worktree(&checkout, &candidate, true).is_empty(),
        "test cleanup should recover after restoring permissions"
    );

    let message = error.to_string();
    assert!(message.contains("mock primary build failure"), "{message}");
    assert!(
        message.contains("candidate cleanup also failed"),
        "{message}"
    );
    assert!(message.contains("git worktree prune"), "{message}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_activation_advances_ref_index_and_worktree_together() {
    let repository = initialize_promotion_repository("source-activation");
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);

    host.activate(&repository.from_commit, &repository.to_commit)
        .unwrap();

    assert_checked_out_commit(
        &repository.checkout,
        &repository.to_commit,
        "promoted fixture\n",
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_activation_ref_failure_restores_prior_index_and_worktree() {
    let repository = initialize_promotion_repository("source-activation-ref-failure");
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);
    let lock = repository.checkout.join(".git/refs/heads/main.lock");
    fs::write(&lock, "locked for source-promotion test\n").unwrap();

    let error = host
        .activate(&repository.from_commit, &repository.to_commit)
        .unwrap_err();

    assert!(error.to_string().contains("update-ref"), "{error}");
    fs::remove_file(lock).unwrap();
    assert_checked_out_commit(
        &repository.checkout,
        &repository.from_commit,
        "prior fixture\n",
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_promotion_rollback_restores_prior_ref_index_and_worktree() {
    let repository = initialize_promotion_repository("source-rollback");
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);
    host.activate(&repository.from_commit, &repository.to_commit)
        .unwrap();

    host.rollback(&repository.from_commit, &repository.to_commit)
        .unwrap();

    assert_checked_out_commit(
        &repository.checkout,
        &repository.from_commit,
        "prior fixture\n",
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_activation_leaves_dirty_checkout_untouched() {
    let repository = initialize_promotion_repository("source-activation-dirty");
    fs::write(repository.checkout.join("fixture.txt"), "local work\n").unwrap();
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);

    let error = host
        .activate(&repository.from_commit, &repository.to_commit)
        .unwrap_err();

    assert!(error.to_string().contains("checkout changed"), "{error}");
    assert_eq!(
        git_text(&repository.checkout, &["rev-parse", "HEAD"]).unwrap(),
        repository.from_commit
    );
    assert_eq!(
        git_text(&repository.checkout, &["write-tree"]).unwrap(),
        git_text(
            &repository.checkout,
            &["rev-parse", &format!("{}^{{tree}}", repository.from_commit)]
        )
        .unwrap()
    );
    assert_eq!(
        fs::read_to_string(repository.checkout.join("fixture.txt")).unwrap(),
        "local work\n"
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_activation_leaves_diverged_checkout_untouched() {
    let repository = initialize_promotion_repository("source-activation-diverged");
    fs::write(
        repository.checkout.join("fixture.txt"),
        "diverged fixture\n",
    )
    .unwrap();
    git_ok(&repository.checkout, &["add", "fixture.txt"]).unwrap();
    git_ok(
        &repository.checkout,
        &["commit", "--quiet", "-m", "diverged source"],
    )
    .unwrap();
    let diverged_commit = git_text(&repository.checkout, &["rev-parse", "HEAD"]).unwrap();
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);

    let error = host
        .activate(&diverged_commit, &repository.to_commit)
        .unwrap_err();

    assert!(error.to_string().contains("fast-forward"), "{error}");
    assert_checked_out_commit(&repository.checkout, &diverged_commit, "diverged fixture\n");
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_promotion_builds_before_stopping_and_verifies_restart() {
    let mut host = FakeHost::default();
    let mut operation = operation();
    let mut states = Vec::new();
    run_source_promotion(&mut host, &mut operation, |state| {
        states.push((state.status.clone(), state.stage.clone()));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        host.calls,
        [
            "build",
            "preflight",
            "stop",
            "activate",
            "restart",
            "verify"
        ]
    );
    assert_eq!(operation.status, "succeeded");
    assert_eq!(operation.stage, "complete");
    assert_eq!(states.first().unwrap().1, "build_candidate");
}

#[test]
fn source_promotion_build_failure_never_stops_or_activates() {
    let mut host = FakeHost {
        fail: Some("build"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(host.calls, ["build"]);
    assert_eq!(operation.stage, "build_candidate");
    assert_eq!(operation.status, "failed");
}

#[test]
fn source_promotion_restart_failure_rolls_back_and_recovers_previous_daemon() {
    let mut host = FakeHost {
        fail: Some("restart"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(
        host.calls,
        [
            "build",
            "preflight",
            "stop",
            "activate",
            "restart",
            "rollback",
            "restart_previous"
        ]
    );
    assert_eq!(operation.rollback_succeeded, Some(true));
    assert!(operation.recovery.as_deref().unwrap().contains("restored"));
}

#[test]
fn source_promotion_active_work_after_build_never_stops_or_activates() {
    let mut host = FakeHost {
        fail: Some("preflight"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(host.calls, ["build", "preflight"]);
    assert_eq!(operation.stage, "verify_idle");
}

#[test]
fn validation_rejects_dirty_diverged_active_and_current_snapshots() {
    let base = SourcePromotionSnapshot {
        checkout_path: "/refine".to_string(),
        current_commit: "aaa".to_string(),
        remote: "origin".to_string(),
        local_branch: "main".to_string(),
        branch: "main".to_string(),
        available_commit: "bbb".to_string(),
        clean: true,
        fast_forward: true,
        update_available: true,
        active_work: Vec::new(),
        operation: None,
    };
    let mut dirty = base.clone();
    dirty.clean = false;
    assert!(
        validate_promotion(&dirty)
            .unwrap_err()
            .to_string()
            .contains("clean")
    );
    let mut diverged = base.clone();
    diverged.fast_forward = false;
    assert!(
        validate_promotion(&diverged)
            .unwrap_err()
            .to_string()
            .contains("fast-forward")
    );
    let mut active = base.clone();
    active.active_work.push("active Goal claim G1".to_string());
    assert!(
        validate_promotion(&active)
            .unwrap_err()
            .to_string()
            .contains("idle")
    );
    let mut current = base;
    current.update_available = false;
    assert!(
        validate_promotion(&current)
            .unwrap_err()
            .to_string()
            .contains("already")
    );
}
