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
    fn activate_candidate_binary(&mut self, candidate: &Path) -> RefineResult<PathBuf> {
        self.call("activate_binary")?;
        Ok(candidate.to_path_buf())
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
    fn restore_previous_binary(&mut self) -> RefineResult<bool> {
        self.call("restore_binary")?;
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
        stashed_changes: None,
        agent_process_id: None,
        agent_worker_process_id: None,
        agent_provider: None,
        agent_context_path: None,
        agent_output: None,
        pre_upgrade_workflow_paused: None,
        workflow_pause_restored: None,
        primary_outcome: None,
        restoration_error: None,
        reconciliation_evidence: None,
        agent_decisions: Vec::new(),
        handoff_attempt: None,
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

fn spawn_single_http_probe() -> u16 {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
    });
    port
}

fn persist_cached_source(
    service: &FileSourcePromotionService,
    repo: &PromotionRepository,
) -> SourcePromotionSnapshot {
    let snapshot = service.inspect(false).unwrap();
    fs::create_dir_all(&service.port_runtime_root).unwrap();
    fs::write(
        service.update_check_state_path(),
        serde_json::to_vec_pretty(&SourceUpdateCheckState {
            last_successful_check_at: Some(now_timestamp()),
            current_source_identity: Some(repo.from_commit.clone()),
            available_source_identity: Some(repo.to_commit.clone()),
            freshness: "fresh".to_string(),
            source: Some(snapshot.clone()),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    snapshot
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
    let operation_arg = handoffs[0]
        .0
        .args
        .iter()
        .position(|arg| arg == "--operation-id")
        .unwrap();
    assert_eq!(
        handoffs[0].0.args.get(operation_arg + 1),
        Some(&operation.id)
    );
    assert_eq!(service.load_operation().unwrap(), Some(operation));

    fs::remove_dir_all(root).unwrap();
}

#[path = "agent_tests.rs"]
mod agent_tests;
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

    // A dirty checkout no longer blocks: queueing stashes and reports it.
    source.clean = false;
    let dirty = source_promotion_affordance(true, &source);
    assert!(dirty.enabled);
    assert_eq!(dirty.state, "available");

    source.fast_forward = false;
    let blocked = source_promotion_affordance(true, &source);
    assert!(!blocked.enabled);
    assert_eq!(blocked.state, "blocked");
    assert!(blocked.title.contains("fast-forward"));

    source.clean = true;
    source.fast_forward = true;
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
    assert!(current.enabled);
    assert_eq!(current.state, "current");
}

#[test]
fn validate_update_intent_ignores_cleanliness_and_gates_on_reachable_updates() {
    let mut snapshot = test_snapshot(Path::new("/refine"));
    snapshot.clean = false;
    assert!(validate_update_intent(&snapshot).is_ok());

    snapshot.fast_forward = false;
    let diverged = validate_update_intent(&snapshot).unwrap_err();
    assert!(diverged.to_string().contains("diverged"));

    snapshot.fast_forward = true;
    snapshot.update_available = false;
    let current = validate_update_intent(&snapshot).unwrap_err();
    assert!(current.to_string().contains("already at the latest"));
}

#[test]
fn stash_dirty_tree_preserves_work_in_a_named_stash_and_reports_it() {
    let root = test_directory("source-stash-dirty");
    initialize_git_repository(&root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"stash-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    git_ok(&root, &["add", "Cargo.toml"]).unwrap();
    git_ok(&root, &["commit", "--quiet", "-m", "manifest"]).unwrap();
    let service = FileSourcePromotionService::new(&root, root.join("run/8080"), 8080);

    assert_eq!(service.stash_dirty_tree().unwrap(), None);

    fs::write(root.join("fixture.txt"), "dirty tracked change\n").unwrap();
    fs::write(root.join("untracked.txt"), "untracked work\n").unwrap();
    let reference = service.stash_dirty_tree().unwrap().unwrap();
    assert!(
        reference.contains("refine-update-"),
        "stash reference should carry the label: {reference}"
    );
    assert!(
        git_text(&root, &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
    let stash_list = git_text(&root, &["stash", "list"]).unwrap();
    assert!(stash_list.contains("refine-update-"), "{stash_list}");
    assert!(!root.join("untracked.txt").exists());

    assert_eq!(service.stash_dirty_tree().unwrap(), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wait_for_terminal_reports_transitions_and_returns_the_terminal_operation() {
    let root = test_directory("source-wait-terminal");
    let runtime = root.join("run/8080");
    let service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let mut running = operation();
    running.status = "running".to_string();
    running.stage = "build_candidate".to_string();
    running.message = "Building the fetched source candidate before activation".to_string();
    service.save_operation(&running).unwrap();

    let writer_service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let mut later = running.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(700));
        later.stage = "stop_daemon".to_string();
        later.message = "Candidate built; stopping the Refine daemon".to_string();
        writer_service.save_operation(&later).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(700));
        later.status = "succeeded".to_string();
        later.stage = "complete".to_string();
        later.message = "Latest source promoted and Refine is healthy".to_string();
        writer_service.save_operation(&later).unwrap();
    });

    let mut transitions = Vec::new();
    let terminal = service
        .wait_for_terminal(
            "source-test",
            std::time::Duration::from_secs(20),
            &mut |observed| transitions.push((observed.stage.clone(), observed.status.clone())),
        )
        .unwrap();
    writer.join().unwrap();
    assert_eq!(terminal.status, "succeeded");
    assert_eq!(terminal.stage, "complete");
    assert!(transitions.len() >= 2, "transitions: {transitions:?}");
    assert_eq!(
        transitions.first().map(|(stage, _)| stage.as_str()),
        Some("build_candidate")
    );
    assert_eq!(
        transitions.last().map(|(stage, _)| stage.as_str()),
        Some("complete")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wait_for_terminal_rejects_a_superseding_operation_and_times_out_visibly() {
    let root = test_directory("source-wait-mismatch");
    let runtime = root.join("run/8080");
    let service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let mut running = operation();
    running.status = "running".to_string();
    running.stage = "build_candidate".to_string();
    service.save_operation(&running).unwrap();

    let mismatch = service
        .wait_for_terminal(
            "source-someone-else",
            std::time::Duration::from_secs(5),
            &mut |_observed| {},
        )
        .unwrap_err();
    assert!(mismatch.to_string().contains("superseded"));

    let timeout = service
        .wait_for_terminal(
            "source-test",
            std::time::Duration::from_millis(600),
            &mut |_observed| {},
        )
        .unwrap_err();
    assert!(timeout.to_string().contains("continues in the background"));
    fs::remove_dir_all(root).unwrap();
}

#[path = "promotion_tests.rs"]
mod promotion_tests;
#[path = "systemd_upgrade_tests.rs"]
mod systemd_upgrade_tests;
