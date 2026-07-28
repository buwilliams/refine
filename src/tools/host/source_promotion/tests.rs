use super::*;

struct FailingHelperLauncher;

impl RestartSafeHandoffLauncher for FailingHelperLauncher {
    fn launch(
        &self,
        _handoff: &RestartSafeHandoff,
        _service_manager: Option<&str>,
    ) -> RefineResult<()> {
        Err(RefineError::Io("mock helper launch denied".to_string()))
    }
}

#[derive(Default)]
struct RecordingHelperLauncher {
    handoffs: std::cell::RefCell<Vec<(RestartSafeHandoff, Option<String>)>>,
}

impl RestartSafeHandoffLauncher for RecordingHelperLauncher {
    fn launch(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<()> {
        self.handoffs
            .borrow_mut()
            .push((handoff.clone(), service_manager.map(str::to_string)));
        Ok(())
    }
}

#[derive(Default)]
struct FakeHost {
    fail: Option<&'static str>,
    rollback_fail: Option<&'static str>,
    calls: Vec<String>,
}

impl FakeHost {
    fn call(&mut self, stage: &str) -> RefineResult<()> {
        self.calls.push(stage.to_string());
        if self.fail == Some(stage) || self.rollback_fail == Some(stage) {
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
    fn prepare_restart(
        &mut self,
        executable: &Path,
    ) -> RefineResult<Option<ServiceRegistrationUpdate>> {
        self.call("register")?;
        Ok(Some(ServiceRegistrationUpdate {
            service_manager: "systemd_user".to_string(),
            candidate_executable: executable.to_path_buf(),
        }))
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
    fn verify_daemon(
        &mut self,
        _expected: &str,
        expected_executable: &Path,
    ) -> RefineResult<PathBuf> {
        self.call("verify")?;
        Ok(expected_executable.to_path_buf())
    }
    fn complete_restart_registration(&mut self) -> RefineResult<()> {
        self.call("complete_registration")
    }
    fn restore_restart_registration(&mut self) -> RefineResult<bool> {
        self.call("restore_registration")?;
        Ok(true)
    }
    fn verify_previous_registration(&mut self) -> RefineResult<PathBuf> {
        self.call("verify_registration")?;
        Ok(PathBuf::from("/previous/refine"))
    }
    fn verify_previous_source(&mut self, _expected_commit: &str) -> RefineResult<()> {
        self.call("verify_previous_source")
    }
    fn rollback(&mut self, _from: &str, _to: &str) -> RefineResult<()> {
        self.call("rollback")
    }
    fn restart_previous_daemon(&mut self) -> RefineResult<()> {
        self.call("restart_previous")
    }
    fn observe_previous_daemon(&mut self) -> SourcePromotionDaemonObservation {
        self.calls.push("observe_previous".to_string());
        match self.rollback_fail {
            Some("observe_unknown") => SourcePromotionDaemonObservation {
                reachability: "reachable".to_string(),
                expected_executable: "/previous/refine".to_string(),
                observed_executable: None,
                identity_matches: None,
                error: Some("mock live identity unavailable".to_string()),
            },
            Some("observe_candidate") => SourcePromotionDaemonObservation {
                reachability: "reachable".to_string(),
                expected_executable: "/previous/refine".to_string(),
                observed_executable: Some("/candidate/refine".to_string()),
                identity_matches: Some(false),
                error: Some("mock candidate remained live".to_string()),
            },
            _ => SourcePromotionDaemonObservation {
                reachability: "reachable".to_string(),
                expected_executable: "/previous/refine".to_string(),
                observed_executable: Some("/previous/refine".to_string()),
                identity_matches: Some(true),
                error: None,
            },
        }
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
        candidate_executable: None,
        service_manager: None,
        registration_updated: false,
        registration_rollback_succeeded: None,
        observed_executable: None,
        rollback_evidence: None,
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
fn source_promotion_queues_through_restart_safe_handoff() {
    let root = test_directory("source-helper-handoff");
    let checkout = root.join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    let service = FileSourcePromotionService::new(&checkout, root.join("runtime/8080"), 8080);
    let snapshot = test_snapshot(&checkout);
    let launcher = RecordingHelperLauncher::default();

    let operation = service
        .queue_validated(&snapshot, Path::new("/mock/refine"), &launcher)
        .unwrap();

    let handoffs = launcher.handoffs.borrow();
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].0.executable, PathBuf::from("/mock/refine"));
    assert_eq!(handoffs[0].0.args[1], "source-promote-helper");
    assert_eq!(handoffs[0].0.args.last(), Some(&operation.id));
    assert_eq!(service.load_operation().unwrap(), Some(operation));

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
            "register",
            "stop",
            "activate",
            "restart",
            "verify",
            "complete_registration"
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
            "register",
            "stop",
            "activate",
            "restart",
            "rollback",
            "restore_registration",
            "verify_registration",
            "restart_previous",
            "observe_previous",
            "complete_registration"
        ]
    );
    assert_eq!(operation.rollback_succeeded, Some(true));
    assert!(operation.recovery.as_deref().unwrap().contains("restored"));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.registration_verified, Some(true));
    assert_eq!(evidence.replacement_succeeded, Some(true));
    assert_eq!(evidence.identity_matches, Some(true));
    assert_eq!(
        evidence.observed_executable.as_deref(),
        Some("/previous/refine")
    );
}

#[test]
fn source_promotion_rollback_restart_failure_never_reports_success() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("restart_previous"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert!(evidence.replacement_attempted);
    assert_eq!(evidence.replacement_succeeded, Some(false));
    assert!(
        evidence
            .errors
            .iter()
            .any(|error| error.contains("forced prior daemon replacement failed"))
    );
}

#[test]
fn source_promotion_rollback_indeterminate_identity_never_reports_success() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("observe_unknown"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.reachability, "reachable");
    assert_eq!(evidence.identity_matches, None);
    assert_eq!(evidence.observed_executable, None);
    assert!(
        operation
            .recovery
            .as_deref()
            .unwrap()
            .contains("partial or indeterminate")
    );
}

#[test]
fn source_promotion_rollback_registration_verification_failure_withholds_restart() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("verify_registration"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    assert!(!host.calls.contains(&"restart_previous".to_string()));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.registration_restored, Some(true));
    assert_eq!(evidence.registration_verified, Some(false));
    assert!(!evidence.replacement_attempted);
}

#[test]
fn source_promotion_rollback_observed_candidate_never_reports_success() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("observe_candidate"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.reachability, "reachable");
    assert_eq!(evidence.identity_matches, Some(false));
    assert_eq!(
        evidence.observed_executable.as_deref(),
        Some("/candidate/refine")
    );
}

#[test]
fn source_activation_failure_verifies_prior_source_before_forced_restart() {
    let mut host = FakeHost {
        fail: Some("activate"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.stage, "activate_source");
    assert_eq!(operation.rollback_succeeded, Some(true));
    assert!(host.calls.contains(&"verify_previous_source".to_string()));
    assert!(host.calls.contains(&"restart_previous".to_string()));
}

#[test]
fn source_promotion_stop_failure_restores_prepared_service_registration() {
    let mut host = FakeHost {
        fail: Some("stop"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(
        host.calls,
        [
            "build",
            "preflight",
            "register",
            "stop",
            "restore_registration",
            "verify_registration",
            "complete_registration"
        ]
    );
    assert_eq!(operation.stage, "stop_daemon");
    assert_eq!(operation.registration_rollback_succeeded, Some(true));
    assert!(
        operation
            .recovery
            .as_deref()
            .unwrap()
            .contains("fresh lifecycle evidence")
    );
}

#[test]
fn source_promotion_registration_failure_never_stops_or_activates() {
    let mut host = FakeHost {
        fail: Some("register"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(host.calls, ["build", "preflight", "register"]);
    assert_eq!(operation.stage, "prepare_service_registration");
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

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an available systemd user manager; platform-gated source-promotion integration evidence"]
fn real_systemd_source_promotion_runs_registered_candidate_identity() {
    use std::net::{Ipv4Addr, TcpListener};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    if !Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("SKIP: systemd user manager is unavailable");
        return;
    }

    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let build = Command::new("cargo")
        .args(["build", "--bin", "refine"])
        .current_dir(&repository_root)
        .status()
        .unwrap();
    assert!(
        build.success(),
        "failed to build the production Refine binary"
    );
    let refine = repository_root.join("target/debug/refine");
    let root = test_directory("real-systemd-source-promotion");
    let runtime_root = root.join("run");
    let source = root.join("source");
    let origin = root.join("origin.git");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let unit_name = format!("refine-{port}.service");
    let config_root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .expect("HOME or XDG_CONFIG_HOME is required for systemd integration");
    let unit_path = config_root.join("systemd/user").join(&unit_name);
    let _cleanup = RealSystemdPromotionCleanup {
        unit_name: unit_name.clone(),
        unit_path: unit_path.clone(),
        root: root.clone(),
    };

    initialize_candidate_source_repository(&source, &origin);
    let command_env = [
        ("XDG_STATE_HOME", root.join("state").display().to_string()),
        ("XDG_CACHE_HOME", root.join("cache").display().to_string()),
        ("REFINE_LAUNCH_MODE", "binary".to_string()),
        ("REFINE_LAUNCH_EXECUTABLE", refine.display().to_string()),
        ("REFINE_DAEMON_PORT", port.to_string()),
    ];
    let install = run_refine_command(
        &refine,
        &[
            "system",
            "install",
            "--port",
            &port.to_string(),
            "--runtime-root",
            &runtime_root.display().to_string(),
        ],
        &command_env,
    );
    assert_command_succeeded("system install", &install);
    wait_for_reachable(port, Duration::from_secs(20));

    let pause = run_refine_command(&refine, &["workflow", "pause"], &command_env);
    assert_command_succeeded("workflow pause", &pause);
    let queue = run_refine_command(
        &refine,
        &[
            "system",
            "source-promote",
            "--checkout",
            &source.display().to_string(),
            "--port",
            &port.to_string(),
            "--runtime-root",
            &runtime_root.display().to_string(),
        ],
        &command_env,
    );
    assert_command_succeeded("system source-promote", &queue);

    let operation_path = runtime_root
        .join(port.to_string())
        .join(SOURCE_PROMOTION_STATE_FILE);
    let deadline = Instant::now() + Duration::from_secs(120);
    let operation: SourcePromotionOperation = loop {
        if let Ok(bytes) = fs::read(&operation_path)
            && let Ok(operation) = serde_json::from_slice::<SourcePromotionOperation>(&bytes)
            && matches!(operation.status.as_str(), "succeeded" | "failed")
        {
            break operation;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for source promotion {}",
            operation_path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        operation.status, "succeeded",
        "source promotion failed: {operation:?}"
    );
    assert_eq!(operation.service_manager.as_deref(), Some("systemd_user"));
    assert!(operation.registration_updated);
    let candidate = PathBuf::from(operation.candidate_executable.as_ref().unwrap());
    let observed = PathBuf::from(operation.observed_executable.as_ref().unwrap());
    assert_eq!(
        fs::canonicalize(&candidate).unwrap(),
        fs::canonicalize(&observed).unwrap()
    );
    let unit = fs::read_to_string(&unit_path).unwrap();
    assert!(
        unit.contains(&candidate.display().to_string()),
        "installed unit did not select candidate: {unit}"
    );
    let live = live_daemon_executable(port).unwrap();
    assert_eq!(
        fs::canonicalize(live).unwrap(),
        fs::canonicalize(candidate).unwrap()
    );
}

#[cfg(target_os = "linux")]
struct RealSystemdPromotionCleanup {
    unit_name: String,
    unit_path: PathBuf,
    root: PathBuf,
}

#[cfg(target_os = "linux")]
impl Drop for RealSystemdPromotionCleanup {
    fn drop(&mut self) {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", &self.unit_name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = fs::remove_file(&self.unit_path);
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(target_os = "linux")]
fn initialize_candidate_source_repository(source: &Path, origin: &Path) {
    fs::create_dir_all(source.join("src")).unwrap();
    git_ok(source, &["init", "--quiet", "."]).unwrap();
    git_ok(source, &["config", "user.email", "refine-test@example.com"]).unwrap();
    git_ok(source, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"refine\"\nversion = \"0.0.1\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(source.join("src/main.rs"), candidate_daemon_source("prior")).unwrap();
    Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(source)
        .status()
        .unwrap();
    git_ok(source, &["add", "."]).unwrap();
    git_ok(source, &["commit", "--quiet", "-m", "prior"]).unwrap();
    let prior = git_text(source, &["rev-parse", "HEAD"]).unwrap();
    git_ok(
        origin.parent().unwrap(),
        &["init", "--quiet", "--bare", origin.to_str().unwrap()],
    )
    .unwrap();
    git_ok(
        source,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    )
    .unwrap();
    git_ok(source, &["push", "--quiet", "-u", "origin", "HEAD:main"]).unwrap();
    fs::write(
        source.join("src/main.rs"),
        candidate_daemon_source("candidate"),
    )
    .unwrap();
    git_ok(source, &["add", "src/main.rs"]).unwrap();
    git_ok(source, &["commit", "--quiet", "-m", "candidate"]).unwrap();
    git_ok(source, &["push", "--quiet", "origin", "HEAD:main"]).unwrap();
    git_ok(source, &["reset", "--quiet", "--hard", &prior]).unwrap();
}

#[cfg(target_os = "linux")]
fn candidate_daemon_source(identity: &str) -> String {
    format!(
        r#"use std::io::{{Read, Write}};
use std::net::{{TcpListener, TcpStream}};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {{
    let args: Vec<String> = std::env::args().collect();
    let port = args.windows(2).find(|pair| pair[0] == "--port")
        .and_then(|pair| pair[1].parse::<u16>().ok()).expect("port");
    if args.iter().any(|arg| arg == "--foreground") {{
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        for stream in listener.incoming() {{
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            let executable = std::env::current_exe().unwrap().display().to_string()
                .replace('\\', "\\\\").replace('"', "\\\"");
            let body = format!("{{{{\"product\":\"refine\",\"version\":\"{identity}\",\"executable_path\":\"{{}}\"}}}}", executable);
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {{}}\r\nConnection: close\r\n\r\n{{}}", body.len(), body);
            stream.write_all(response.as_bytes()).unwrap();
        }}
        return;
    }}
    let unit = format!("refine-{{port}}.service");
    let status = Command::new("systemctl").args(["--user", "start", &unit]).status().unwrap();
    if !status.success() {{ std::process::exit(1); }}
    for _ in 0..200 {{
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {{ return; }}
        thread::sleep(Duration::from_millis(25));
    }}
    std::process::exit(2);
}}
"#
    )
}

#[cfg(target_os = "linux")]
fn run_refine_command(
    refine: &Path,
    args: &[&str],
    environment: &[(&str, String)],
) -> std::process::Output {
    let mut command = Command::new(refine);
    command.args(args);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().unwrap()
}

#[cfg(target_os = "linux")]
fn assert_command_succeeded(label: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{label} failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
fn wait_for_reachable(port: u16, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while crate::process::supervisor::lifecycle::http_reachability_probe(port)
        != crate::process::supervisor::lifecycle::DaemonReachability::Reachable
    {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for daemon on {port}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
