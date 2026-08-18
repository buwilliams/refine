use super::*;
use crate::tools::host::checkout::test_fixture::SyntheticSourceProduct;

#[test]
fn git_sync_schedule_exposes_validated_stale_threshold() {
    let defaults = GitSyncSchedule::default();
    assert!(
        defaults.stale_threshold > defaults.remote_fetch_interval.unwrap(),
        "the default stale boundary must tolerate the normal fetch cadence"
    );

    let temp_root = std::env::temp_dir().join(format!(
        "refine-state-sync-schedule-{}",
        uuid::Uuid::new_v4()
    ));
    let target_root = temp_root.join("target");
    let runtime_root = temp_root.join("run/8082");
    let refine_dir = target_root.join(".refine");
    std::fs::create_dir_all(&refine_dir).unwrap();
    FileSettingsService::with_active_root(&refine_dir, &runtime_root)
        .update(&json!({
            "state_sync_debounce_seconds": 7,
            "project_update_pulse_interval_seconds": 11,
            "state_sync_stale_threshold_seconds": 31
        }))
        .unwrap();

    let schedule = git_sync_schedule(&runtime_root, &target_root).unwrap();

    assert_eq!(schedule.debounce, Duration::from_secs(7));
    assert_eq!(
        schedule.remote_fetch_interval,
        Some(Duration::from_secs(11))
    );
    assert_eq!(schedule.stale_threshold, Duration::from_secs(31));
    assert_eq!(
        state_sync_stale_threshold(&runtime_root, &target_root).unwrap(),
        Duration::from_secs(31)
    );
    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn state_sync_health_emits_one_failure_episode_and_one_recovery_activity() {
    let temp_root =
        std::env::temp_dir().join(format!("refine-state-sync-health-{}", uuid::Uuid::new_v4()));
    let target_root = temp_root.join("target");
    let runtime_root = temp_root.join("run/8082");
    std::fs::create_dir_all(target_root.join(".refine")).unwrap();
    let error =
        RefineError::Conflict("git fetch https://user:secret@example.com failed".to_string());
    let first = record_state_sync_attempt(&runtime_root, &target_root, "default", "test").unwrap();
    record_state_sync_failure(
        &runtime_root,
        &target_root,
        "default",
        first,
        "test",
        &error,
    )
    .unwrap();
    let second = record_state_sync_attempt(&runtime_root, &target_root, "default", "test").unwrap();
    record_state_sync_failure(
        &runtime_root,
        &target_root,
        "default",
        second,
        "test",
        &error,
    )
    .unwrap();
    let recovered =
        record_state_sync_attempt(&runtime_root, &target_root, "default", "test").unwrap();
    record_state_sync_result(
        &runtime_root,
        &target_root,
        "default",
        recovered,
        "test",
        &GitSyncResult {
            ok: true,
            attempted: true,
            remote_configured: Some(true),
            ..GitSyncResult::default()
        },
    )
    .unwrap();

    let entries = FileActivityService::new(target_root.join(".refine"))
        .recent(10)
        .unwrap();
    assert_eq!(entries.len(), 2, "{entries:#?}");
    assert!(entries.iter().all(|entry| entry.category == "state_sync"));
    assert!(entries.iter().any(|entry| entry.message.contains("failed")));
    assert!(
        entries
            .iter()
            .any(|entry| entry.message.contains("recovered"))
    );
    assert!(
        entries
            .iter()
            .all(|entry| !entry.message.contains("secret"))
    );
    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn source_worker_inherits_debug_executable_without_installed_binary() {
    let checkout = SyntheticSourceProduct::new("source-worker-executable");
    let runtime_root = checkout.port_runtime_root(4557);
    let test_executable = checkout.debug_executable();

    let selected = runner_executable_for_invocation(&runtime_root, &test_executable).unwrap();

    assert_eq!(selected, test_executable.canonicalize().unwrap());
    assert_ne!(selected, checkout.installed_executable());
}

#[test]
fn installed_worker_inherits_the_stable_checkout_binary() {
    let checkout = std::env::temp_dir().join(format!(
        "refine-installed-worker-executable-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(checkout.join("src")).unwrap();
    std::fs::create_dir_all(checkout.join("docs/runbooks")).unwrap();
    std::fs::create_dir_all(checkout.join("bin")).unwrap();
    std::fs::write(checkout.join("Cargo.toml"), "[package]\nname='refine'\n").unwrap();
    std::fs::write(checkout.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(checkout.join("docs/runbooks/install.md"), "# Install\n").unwrap();
    std::fs::write(checkout.join("r"), "#!/bin/sh\n").unwrap();
    std::fs::write(checkout.join("bin/refine"), "installed\n").unwrap();
    std::fs::write(
        checkout.join(".refine-deployed"),
        "mode=deployed\nrelease_bin=bin/refine\n",
    )
    .unwrap();

    let selected =
        runner_executable_for_invocation(&checkout.join("run/4557"), &checkout.join("bin/refine"))
            .unwrap();

    assert_eq!(
        selected,
        checkout.join("bin/refine").canonicalize().unwrap()
    );
    std::fs::remove_dir_all(checkout).unwrap();
}

fn worker_record(state: &str, worker_kind: &str) -> ManagedProcess {
    ManagedProcess {
        id: format!("process-{state}-{worker_kind}"),
        owner: ProcessOwner::Runner,
        pid: Some(4242),
        state: state.to_string(),
        label: Some("workflow runner".to_string()),
        details: Some(json!({"worker_kind": worker_kind}).to_string()),
        stdout_path: None,
        stderr_path: None,
        stdin_path: None,
        limits: None,
        started_at: String::new(),
        exit_code: None,
    }
}

#[test]
fn settled_worker_records_are_not_adopted_as_a_running_workflow_runner() {
    assert!(adoptable_worker(
        &worker_record("running", WORKFLOW_RUNNER),
        WORKFLOW_RUNNER,
        None
    ));
    // Each of these means the worker is gone. Adopting one leaves nothing
    // ticking the workflow while supervision believes a runner exists.
    for state in ["exited", "failed", "stopped", "interrupted"] {
        assert!(
            !adoptable_worker(
                &worker_record(state, WORKFLOW_RUNNER),
                WORKFLOW_RUNNER,
                None
            ),
            "a {state} record must not be adopted as a live workflow runner"
        );
    }
    assert!(!adoptable_worker(
        &worker_record("running", GIT_SYNC_RUNNER),
        WORKFLOW_RUNNER,
        None
    ));
    let mut foreign_owner = worker_record("running", WORKFLOW_RUNNER);
    foreign_owner.owner = ProcessOwner::Agent;
    assert!(!adoptable_worker(&foreign_owner, WORKFLOW_RUNNER, None));

    let canonical_registry = Path::new("/tmp/run");
    let legacy_worker = worker_record("running", WORKFLOW_RUNNER);
    assert!(
        !adoptable_worker(&legacy_worker, WORKFLOW_RUNNER, Some(canonical_registry)),
        "a pre-canonical-registry worker must be replaced"
    );
    let mut current_worker = legacy_worker;
    current_worker.details = Some(
        json!({
            "worker_kind": WORKFLOW_RUNNER,
            "project_registry_root": canonical_registry
        })
        .to_string(),
    );
    assert!(adoptable_worker(
        &current_worker,
        WORKFLOW_RUNNER,
        Some(canonical_registry)
    ));
}

#[test]
fn paused_workflow_suppresses_background_repository_workers_until_resumed() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-paused-background-runners-{}",
        uuid::Uuid::new_v4()
    ));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor.set_workflow_paused(true).unwrap();

    for worker_kind in PAUSE_AWARE_BACKGROUND_RUNNERS {
        assert!(
            run_worker(worker_kind, runtime_root.clone(), None, None, None).is_ok(),
            "{worker_kind} must exit without scanning repositories while paused"
        );
        assert!(matches!(
            FileRunnerWorkerService::new(&runtime_root)
                .ensure_background_worker(worker_kind)
                .unwrap(),
            BackgroundWorkerEnsure::Paused
        ));
    }

    supervisor.set_workflow_paused(false).unwrap();
    assert!(
        !background_automation_is_paused(&runtime_root).unwrap(),
        "clearing the same pause gate must make background workers launchable again"
    );

    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn user_stop_kills_and_disables_a_background_worker_until_start() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-user-stopped-background-runner-{}",
        uuid::Uuid::new_v4()
    ));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let (command, args) = if cfg!(windows) {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), "ping -n 300 127.0.0.1 >NUL".to_string()],
        )
    } else {
        ("sleep".to_string(), vec!["300".to_string()])
    };
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command,
            args,
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(json!({
                "kind": "runner",
                "worker_kind": GIT_SYNC_RUNNER
            }))
            .unwrap(),
        })
        .unwrap();
    let service = FileRunnerWorkerService::new(&runtime_root);

    assert!(matches!(
        service
            .set_background_worker_enabled(GIT_SYNC_RUNNER, false)
            .unwrap(),
        BackgroundWorkerEnsure::Disabled
    ));
    assert!(!FileProcessSupervisor::process_is_alive(&process).unwrap());
    assert!(
        supervisor
            .pause_state()
            .unwrap()
            .disabled_background_workers
            .contains(GIT_SYNC_RUNNER)
    );

    supervisor.set_workflow_paused(true).unwrap();
    assert!(matches!(
        service
            .set_background_worker_enabled(GIT_SYNC_RUNNER, true)
            .unwrap(),
        BackgroundWorkerEnsure::Paused
    ));
    assert!(
        !supervisor
            .pause_state()
            .unwrap()
            .disabled_background_workers
            .contains(GIT_SYNC_RUNNER)
    );

    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn pause_at_ensure_is_quiet_and_invalid_pause_state_is_an_error() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-pause-at-background-ensure-{}",
        uuid::Uuid::new_v4()
    ));
    let pause_root = runtime_root.clone();
    let observed_root = runtime_root.clone();
    let _hook = install_background_worker_hook(move |root, worker_kind, boundary| {
        if root != observed_root {
            return;
        }
        if worker_kind == GIT_SYNC_RUNNER && boundary == BackgroundWorkerBoundary::EnsureLaunch {
            FileProcessSupervisor::new(&pause_root)
                .set_workflow_paused(true)
                .unwrap();
        }
    });
    assert!(matches!(
        FileRunnerWorkerService::new(&runtime_root)
            .ensure_background_worker(GIT_SYNC_RUNNER)
            .unwrap(),
        BackgroundWorkerEnsure::Paused
    ));

    std::fs::write(
        FileProcessSupervisor::new(&runtime_root).pause_state_path(),
        "{invalid",
    )
    .unwrap();
    for worker_kind in PAUSE_AWARE_BACKGROUND_RUNNERS {
        let error = FileRunnerWorkerService::new(&runtime_root)
            .ensure_background_worker(worker_kind)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to parse process control"),
            "{error}"
        );
    }
    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn running_background_repository_workers_quiesce_at_pause_boundaries() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-running-background-runners-{}",
        uuid::Uuid::new_v4()
    ));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();

    let git_runtime_root = runtime_root.clone();
    let git_finished = finished_tx.clone();
    let git_worker = std::thread::spawn(move || {
        git_finished
            .send((
                GIT_SYNC_RUNNER,
                run_git_sync_worker(&git_runtime_root, None),
            ))
            .unwrap();
    });
    let cleanup_runtime_root = runtime_root.clone();
    let cleanup_worker = std::thread::spawn(move || {
        finished_tx
            .send((
                WORKTREE_CLEANUP_RUNNER,
                run_worktree_cleanup_worker(&cleanup_runtime_root, None),
            ))
            .unwrap();
    });

    std::thread::sleep(Duration::from_millis(50));
    supervisor.set_workflow_paused(true).unwrap();
    for _ in PAUSE_AWARE_BACKGROUND_RUNNERS {
        let (worker_kind, result) = finished_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("background repository worker did not quiesce after workflow pause");
        assert!(result.is_ok(), "{worker_kind} exited with {result:?}");
        result.unwrap();
    }
    git_worker.join().unwrap();
    cleanup_worker.join().unwrap();

    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn repository_workers_skip_blocked_operations_then_do_work_after_resume() {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Condvar, Mutex, mpsc};

    let root = std::env::temp_dir().join(format!(
        "refine-paused-background-operation-{}",
        uuid::Uuid::new_v4()
    ));
    let target_root = root.join("app");
    let registry_root = root.join("registry");
    let runtime_root = root.join("runtime");
    std::fs::create_dir_all(&target_root).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test User"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&target_root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(target_root.join("README.md"), "base\n").unwrap();
    for args in [vec!["add", "README.md"], vec!["commit", "-m", "base"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&target_root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let target_root = target_root.canonicalize().unwrap();
    let registry = crate::model::project::AppRegistry {
        version: 1,
        active_app: Some(target_root.display().to_string()),
        apps: std::collections::BTreeMap::new(),
    };
    FileProjectRegistryService::new(&registry_root, None)
        .save(&registry)
        .unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    std::fs::create_dir_all(&refine_dir).unwrap();
    FileSettingsService::with_active_root(&refine_dir, &runtime_root)
        .update(&json!({
            "state_sync_debounce_seconds": "1",
            "project_update_pulse_interval_seconds": "0",
            "worktree_cleanup_after_seconds": "0"
        }))
        .unwrap();

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (event_tx, event_rx) = mpsc::channel();
    let hook_release = Arc::clone(&release);
    let blocked_root = runtime_root.clone();
    let _blocked_hook = install_background_worker_hook(move |root, worker_kind, boundary| {
        // Every test shares this process, so only this test's own workers
        // count: another test's background operation must not be read as
        // work this pause was supposed to stop.
        if root != blocked_root
            || !matches!(
                boundary,
                BackgroundWorkerBoundary::BeforeOperation
                    | BackgroundWorkerBoundary::AfterOperation
            )
        {
            return;
        }
        event_tx.send((worker_kind.to_string(), boundary)).unwrap();
        if boundary == BackgroundWorkerBoundary::BeforeOperation {
            let (lock, ready) = &*hook_release;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = ready.wait(released).unwrap();
            }
        }
    });
    let git_runtime = runtime_root.clone();
    let git_registry = registry_root.clone();
    let git_worker =
        std::thread::spawn(move || run_git_sync_worker(&git_runtime, Some(&git_registry)));
    let cleanup_runtime = runtime_root.clone();
    let cleanup_registry = registry_root.clone();
    let cleanup_worker = std::thread::spawn(move || {
        run_worktree_cleanup_worker(&cleanup_runtime, Some(&cleanup_registry))
    });
    let mut blocked_workers = BTreeSet::new();
    while blocked_workers.len() < PAUSE_AWARE_BACKGROUND_RUNNERS.len() {
        let (worker_kind, boundary) = event_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("both repository workers must reach their operation boundary");
        assert_eq!(boundary, BackgroundWorkerBoundary::BeforeOperation);
        blocked_workers.insert(worker_kind);
    }
    supervisor.set_workflow_paused(true).unwrap();
    let (lock, ready) = &*release;
    *lock.lock().unwrap() = true;
    ready.notify_all();
    assert!(git_worker.join().unwrap().is_ok());
    assert!(cleanup_worker.join().unwrap().is_ok());
    assert!(
        event_rx
            .try_iter()
            .all(|(_, boundary)| { boundary != BackgroundWorkerBoundary::AfterOperation })
    );
    drop(_blocked_hook);

    supervisor.set_workflow_paused(false).unwrap();
    let (event_tx, event_rx) = mpsc::channel();
    let resumed_root = runtime_root.clone();
    let _resumed_hook = install_background_worker_hook(move |root, worker_kind, boundary| {
        if root == resumed_root
            && matches!(
                boundary,
                BackgroundWorkerBoundary::BeforeOperation
                    | BackgroundWorkerBoundary::AfterOperation
            )
        {
            event_tx.send((worker_kind.to_string(), boundary)).unwrap();
        }
    });
    let git_runtime = runtime_root.clone();
    let git_registry = registry_root.clone();
    let git_worker =
        std::thread::spawn(move || run_git_sync_worker(&git_runtime, Some(&git_registry)));
    let cleanup_runtime = runtime_root.clone();
    let cleanup_registry = registry_root.clone();
    let cleanup_worker = std::thread::spawn(move || {
        run_worktree_cleanup_worker(&cleanup_runtime, Some(&cleanup_registry))
    });
    let mut completed_workers = BTreeSet::new();
    while completed_workers.len() < PAUSE_AWARE_BACKGROUND_RUNNERS.len() {
        let (worker_kind, boundary) = event_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("both resumed repository workers must perform an operation");
        if boundary == BackgroundWorkerBoundary::AfterOperation {
            completed_workers.insert(worker_kind);
        }
    }
    supervisor.set_workflow_paused(true).unwrap();
    assert!(git_worker.join().unwrap().is_ok());
    assert!(cleanup_worker.join().unwrap().is_ok());
    assert_eq!(
        completed_workers,
        BTreeSet::from([
            GIT_SYNC_RUNNER.to_string(),
            WORKTREE_CLEANUP_RUNNER.to_string()
        ])
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_unremovable_settled_record_cannot_stop_supervision_from_seeing_the_registry() {
    // A settled record whose artifacts refuse to be removed used to fail the
    // entire process listing. Every consumer reads through that listing, so
    // one leftover silently stopped supervision from noticing a dead workflow
    // runner — and nothing relaunched it short of a daemon restart.
    let runtime_root =
        std::env::temp_dir().join(format!("refine-stuck-record-{}", uuid::Uuid::new_v4()));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    std::fs::create_dir_all(supervisor.processes_dir()).unwrap();

    let mut settled = worker_record("exited", WORKFLOW_RUNNER);
    // A directory cannot be removed as a file, so cleanup of this record
    // fails the way an undeletable leftover does in the field.
    let undeletable = runtime_root.join("undeletable-stdout");
    std::fs::create_dir_all(&undeletable).unwrap();
    settled.stdout_path = Some(undeletable.display().to_string());
    std::fs::write(
        supervisor
            .processes_dir()
            .join(format!("{}.json", settled.id)),
        serde_json::to_vec_pretty(&settled).unwrap(),
    )
    .unwrap();

    let live = worker_record("running", GIT_SYNC_RUNNER);
    std::fs::write(
        supervisor.processes_dir().join(format!("{}.json", live.id)),
        serde_json::to_vec_pretty(&live).unwrap(),
    )
    .unwrap();

    let listed = supervisor
        .list()
        .expect("a settled record that will not clean up must not fail the listing");

    assert!(
        listed.iter().any(|process| process.id == live.id),
        "the running record must still be visible"
    );
    assert!(
        !listed.iter().any(|process| process.id == settled.id),
        "the settled record must not be reported as running"
    );
    std::fs::remove_dir_all(&runtime_root).unwrap();
}

#[test]
fn retired_supervisor_state_is_purged_before_workflow_evaluation() {
    let target_root = std::env::temp_dir().join(format!(
        "refine-retired-supervisor-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&target_root).unwrap();
    let initialized = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&target_root)
        .status()
        .unwrap();
    assert!(initialized.success());
    let refine_dir = prepare_refine_dir(&target_root).unwrap();
    std::fs::create_dir_all(&refine_dir).unwrap();
    let runtime_root = target_root.join("run/8082");
    for name in ["supervisor-agent.json", "supervisor-agent.lock"] {
        std::fs::write(refine_dir.join(name), "{}\n").unwrap();
    }

    retire_legacy_supervisor(&runtime_root, &target_root).unwrap();

    assert!(!refine_dir.join("supervisor-agent.json").exists());
    assert!(!refine_dir.join("supervisor-agent.lock").exists());
    std::fs::remove_dir_all(target_root).unwrap();
}

#[test]
fn runner_specs_create_real_runner_processes() {
    let background_spec = background_worker_spec(
        Path::new("/opt/refine"),
        Path::new("/tmp/run/8082"),
        Some(Path::new("/tmp/run")),
        GIT_SYNC_RUNNER,
    );
    assert!(
        background_spec
            .args
            .windows(2)
            .any(|args| args == ["--project-registry-root", "/tmp/run"])
    );
    let cleanup_spec = background_worker_spec(
        Path::new("/opt/refine"),
        Path::new("/tmp/run/8082"),
        Some(Path::new("/tmp/run")),
        WORKTREE_CLEANUP_RUNNER,
    );
    assert_eq!(
        cleanup_spec.metadata["worker_kind"],
        WORKTREE_CLEANUP_RUNNER
    );
    assert!(validate_worker_kind(WORKTREE_CLEANUP_RUNNER, false).is_ok());
    let development_request_spec = background_worker_spec(
        Path::new("/opt/refine"),
        Path::new("/tmp/run/8082"),
        Some(Path::new("/tmp/run")),
        DEVELOPMENT_REQUEST_RUNNER,
    );
    assert_eq!(
        development_request_spec.metadata["worker_kind"],
        DEVELOPMENT_REQUEST_RUNNER
    );
    assert!(validate_worker_kind(DEVELOPMENT_REQUEST_RUNNER, false).is_ok());

    let spec = project_sync_worker_spec(
        Path::new("/opt/refine"),
        Path::new("/tmp/run/8082"),
        Path::new("/tmp/app"),
        "OP1",
    );
    assert_eq!(spec.owner, ProcessOwner::Runner);
    assert_eq!(spec.metadata["kind"], "runner");
    assert_eq!(spec.metadata["worker_kind"], PROJECT_SYNC_RUNNER);
    assert!(spec.args.iter().any(|arg| arg == "--operation-id"));
    assert_eq!(
        spec.limits
            .as_ref()
            .map(|limits| limits.kill_on_parent_exit),
        Some(true)
    );

    let jira_spec =
        jira_export_worker_spec(Path::new("/opt/refine"), Path::new("/tmp/run/8082"), "OP2");
    assert_eq!(jira_spec.owner, ProcessOwner::Runner);
    assert_eq!(jira_spec.metadata["worker_kind"], JIRA_EXPORT_RUNNER);
    assert_eq!(jira_spec.metadata["operation_id"], "OP2");
    assert!(!jira_spec.args.iter().any(|arg| arg == "--target-root"));
    assert_eq!(
        jira_spec
            .limits
            .as_ref()
            .map(|limits| limits.kill_on_parent_exit),
        Some(false)
    );
}

#[test]
fn runner_target_resolution_uses_canonical_registry_over_stale_port_registry() {
    let root = std::env::temp_dir().join(format!(
        "refine-runner-project-registry-{}",
        uuid::Uuid::new_v4()
    ));
    let canonical_root = root.join("run");
    let port_runtime_root = canonical_root.join("8082");
    std::fs::create_dir_all(&port_runtime_root).unwrap();
    std::fs::write(
        canonical_root.join("apps.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "active_app": "/tmp/current-app",
            "apps": {}
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        port_runtime_root.join("apps.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "active_app": "/tmp/stale-app",
            "apps": {}
        }))
        .unwrap(),
    )
    .unwrap();

    assert_eq!(
        current_target_root(&port_runtime_root, Some(&canonical_root)).unwrap(),
        Some(PathBuf::from("/tmp/current-app"))
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cleanup_runner_applies_configured_inactive_worktree_retention() {
    let target_root = std::env::temp_dir().join(format!(
        "refine-runner-worktree-cleanup-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&target_root).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test User"],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&target_root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(target_root.join("README.md"), "base\n").unwrap();
    for args in [vec!["add", "README.md"], vec!["commit", "-m", "base"]] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&target_root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    std::fs::create_dir_all(&refine_dir).unwrap();
    let runtime_root = target_root.join("runtime");
    FileSettingsService::with_active_root(&refine_dir, &runtime_root)
        .update(&json!({"worktree_cleanup_after_seconds": "0"}))
        .unwrap();
    let work_items = crate::tools::product::work_items::FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Runner cleanup", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Tester", "Implement")
        .unwrap();
    work_items
        .set_goal_branch_name("GOAL1", "refine/GOAL1/round-1")
        .unwrap();
    work_items.cancel_goal_summary("GOAL1").unwrap();
    let worktree = target_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args([
                "worktree",
                "add",
                "-b",
                "refine/GOAL1/round-1",
                worktree.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success()
    );

    run_configured_worktree_cleanup(&runtime_root, &target_root);

    assert!(!worktree.exists());
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args(["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-1"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::remove_dir_all(target_root).unwrap();
}

#[test]
fn cleanup_runner_scans_every_registered_target_app_once() {
    let root = std::env::temp_dir().join(format!(
        "refine-runner-registered-cleanup-{}",
        uuid::Uuid::new_v4()
    ));
    let registry_root = root.join("registry");
    let first = root.join("first");
    let second = root.join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let first = first.canonicalize().unwrap();
    let second = second.canonicalize().unwrap();
    let registry = crate::model::project::AppRegistry {
        version: 1,
        active_app: Some(first.display().to_string()),
        apps: std::collections::BTreeMap::from([
            (
                first.display().to_string(),
                crate::model::project::RegisteredApp {
                    name: "first".to_string(),
                    path: first.display().to_string(),
                    added_at: "2026-01-01T00:00:00Z".to_string(),
                    last_used_at: None,
                },
            ),
            (
                second.display().to_string(),
                crate::model::project::RegisteredApp {
                    name: "second".to_string(),
                    path: second.display().to_string(),
                    added_at: "2026-01-01T00:00:00Z".to_string(),
                    last_used_at: None,
                },
            ),
        ]),
    };
    FileProjectRegistryService::new(&registry_root, None)
        .save(&registry)
        .unwrap();

    let roots = registered_target_roots(&root.join("runtime"), Some(&registry_root)).unwrap();

    assert_eq!(roots, vec![first, second]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn background_sync_conflict_auto_recovers_unless_the_node_opted_out() {
    let temp_root = std::env::temp_dir().join(format!(
        "refine-state-sync-auto-recovery-{}",
        uuid::Uuid::new_v4()
    ));
    let remote = temp_root.join("remote.git");
    let seed = temp_root.join("seed");
    let a = temp_root.join("a");
    let b = temp_root.join("b");
    let runtime_root = temp_root.join("run/8082");
    std::fs::create_dir_all(&seed).unwrap();
    let git = |root: &Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&temp_root, &["init", "--bare", remote.to_str().unwrap()]);
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "sync@test"]);
    git(&seed, &["config", "user.name", "Sync Test"]);
    std::fs::write(seed.join("app.txt"), "base\n").unwrap();
    git(&seed, &["add", "app.txt"]);
    git(&seed, &["commit", "-q", "-m", "initial"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-q", "-u", "origin", "main"]);
    for root in [&a, &b] {
        git(
            &temp_root,
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                root.to_str().unwrap(),
            ],
        );
        git(root, &["config", "user.email", "sync@test"]);
        git(root, &["config", "user.name", "Sync Test"]);
    }
    let write_shared = |root: &Path, value: &str| {
        let destination = refine_dir_for_target_root(root)
            .unwrap()
            .join("shared/state.json");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(destination, value).unwrap();
    };
    let write_goal = |root: &Path, name: &str| {
        let destination = refine_dir_for_target_root(root)
            .unwrap()
            .join("goals/OWNED/goal.json");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(
            destination,
            format!(r#"{{"id":"OWNED","name":"{name}","node_id":"default"}}"#),
        )
        .unwrap();
    };
    // Establish a shared baseline, then diverge the same members on both
    // nodes so structural reconciliation must fail closed. The goal record
    // names this node as its owner at the merge base.
    write_shared(&a, "base\n");
    write_goal(&a, "base");
    FileGitSyncService::new(&a, a.join("run")).sync().unwrap();
    let service = FileGitSyncService::new(&b, &runtime_root);
    service.sync().unwrap();
    write_shared(&a, "remote\n");
    write_goal(&a, "remote-edit");
    FileGitSyncService::new(&a, a.join("run")).sync().unwrap();
    write_shared(&b, "live\n");
    write_goal(&b, "live-edit");

    bind_state_sync_health(&runtime_root, &b, "default").unwrap();
    let refine_b = refine_dir_for_target_root(&b).unwrap();
    FileSettingsService::with_active_root(&refine_b, &runtime_root)
        .update(&json!({"state_sync_auto_recovery": "off"}))
        .unwrap();
    let attempt =
        record_state_sync_attempt(&runtime_root, &b, "default", "background_publish").unwrap();
    let error = service.sync().unwrap_err();
    record_state_sync_failure(
        &runtime_root,
        &b,
        "default",
        attempt,
        "background_publish",
        &error,
    )
    .unwrap();

    let report = crate::tools::host::git_sync::latest_state_sync_conflict_report(&runtime_root)
        .unwrap()
        .expect("the contested sync records a conflict report");

    // A failure without recorded conflict evidence never triggers recovery
    // regardless of the setting.
    let outcome =
        attempt_automatic_state_recovery(&runtime_root, &b, "default", &service, None).unwrap();
    assert!(matches!(outcome, AutoRecoveryOutcome::NotAttempted));

    // The opted-out node keeps its fail-closed conflict.
    let outcome =
        attempt_automatic_state_recovery(&runtime_root, &b, "default", &service, Some(&report))
            .unwrap();
    assert!(matches!(outcome, AutoRecoveryOutcome::NotAttempted));
    let health = FileStateSyncHealthService::new(&runtime_root)
        .inspect(&b, "default", Duration::from_secs(900))
        .unwrap();
    assert!(health.failure_since.is_some(), "{health:#?}");

    // With the default policy the same rejection converges on its own:
    // remote authority for the shared record, a live override for the goal
    // record whose merge-base bytes name this node.
    FileSettingsService::with_active_root(&refine_b, &runtime_root)
        .update(&json!({"state_sync_auto_recovery": "remote"}))
        .unwrap();
    let outcome =
        attempt_automatic_state_recovery(&runtime_root, &b, "default", &service, Some(&report))
            .unwrap();
    assert!(matches!(outcome, AutoRecoveryOutcome::Recovered));
    let health = FileStateSyncHealthService::new(&runtime_root)
        .inspect(&b, "default", Duration::from_secs(900))
        .unwrap();
    assert!(health.failure_since.is_none(), "{health:#?}");
    assert_eq!(
        health.last_attempt_source,
        Some(AUTO_RECOVERY_SOURCE.to_string())
    );
    assert_eq!(
        std::fs::read_to_string(refine_b.join("shared/state.json")).unwrap(),
        "remote\n"
    );
    let owned: serde_json::Value =
        serde_json::from_slice(&std::fs::read(refine_b.join("goals/OWNED/goal.json")).unwrap())
            .unwrap();
    assert_eq!(
        owned["name"], "live-edit",
        "merge-base ownership keeps this node's copy of its own goal"
    );
    let entries = FileActivityService::new(&refine_b).recent(10).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.message.contains("auto-recovered")
                && entry.message.contains("remote authority")),
        "{entries:#?}"
    );
    service.sync().unwrap();
    std::fs::remove_dir_all(temp_root).unwrap();
}

/// Agent resolution runs FIRST, inside the sync attempt itself; only its
/// `NeedsDecision` escalation falls through to the merge-base-ownership
/// auto-recovery, so overnight convergence still terminates.
#[test]
fn agent_resolution_runs_before_auto_recovery_and_needs_decision_falls_through() {
    use crate::tools::git::resolve::{
        ResolutionRequest, ResolverOutcome, ScriptedResolver, ScriptedStep,
        install_resolver_override,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let temp_root = std::env::temp_dir().join(format!(
        "refine-state-sync-resolution-order-{}",
        uuid::Uuid::new_v4()
    ));
    let remote = temp_root.join("remote.git");
    let seed = temp_root.join("seed");
    let a = temp_root.join("a");
    let b = temp_root.join("b");
    let runtime_root = temp_root.join("run/8082");
    std::fs::create_dir_all(&seed).unwrap();
    let git = |root: &Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&temp_root, &["init", "--bare", remote.to_str().unwrap()]);
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "sync@test"]);
    git(&seed, &["config", "user.name", "Sync Test"]);
    std::fs::write(seed.join("app.txt"), "base\n").unwrap();
    git(&seed, &["add", "app.txt"]);
    git(&seed, &["commit", "-q", "-m", "initial"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-q", "-u", "origin", "main"]);
    for root in [&a, &b] {
        git(
            &temp_root,
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                root.to_str().unwrap(),
            ],
        );
        git(root, &["config", "user.email", "sync@test"]);
        git(root, &["config", "user.name", "Sync Test"]);
    }
    let write_shared = |root: &Path, value: &str| {
        let destination = refine_dir_for_target_root(root)
            .unwrap()
            .join("shared/state.json");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(destination, value).unwrap();
    };
    write_shared(&a, "{\"node\":\"base\"}\n");
    FileGitSyncService::new(&a, a.join("run")).sync().unwrap();
    let service = FileGitSyncService::new(&b, &runtime_root);
    service.sync().unwrap();
    write_shared(&a, "{\"node\":\"remote\"}\n");
    FileGitSyncService::new(&a, a.join("run")).sync().unwrap();
    write_shared(&b, "{\"node\":\"live\"}\n");
    bind_state_sync_health(&runtime_root, &b, "default").unwrap();

    let resolver_calls = Arc::new(AtomicU32::new(0));
    let step = |calls: &Arc<AtomicU32>| -> ScriptedStep {
        let counted = Arc::clone(calls);
        Box::new(move |_request: &ResolutionRequest<'_>| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(ResolverOutcome::Failed(
                "both versions look intentional".to_string(),
            ))
        })
    };
    let _guard = install_resolver_override(
        &b,
        Arc::new(ScriptedResolver::new(vec![
            step(&resolver_calls),
            step(&resolver_calls),
        ])),
    );

    let attempt =
        record_state_sync_attempt(&runtime_root, &b, "default", "background_publish").unwrap();
    let error = service.sync().unwrap_err();
    record_state_sync_failure(
        &runtime_root,
        &b,
        "default",
        attempt,
        "background_publish",
        &error,
    )
    .unwrap();

    // Resolution ran first (both bounded attempts) and escalated in domain
    // terms; the report leads with the question.
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 2);
    assert!(error.to_string().contains("needs a decision"), "{error}");
    let report = crate::tools::host::git_sync::latest_state_sync_conflict_report(&runtime_root)
        .unwrap()
        .expect("the escalated conflict keeps its report");
    assert!(report.decision_question.is_some(), "{report:#?}");

    // Only the NeedsDecision outcome falls through to auto-recovery, which
    // still converges the node with the merge-base ownership policy.
    let outcome =
        attempt_automatic_state_recovery(&runtime_root, &b, "default", &service, Some(&report))
            .unwrap();
    assert!(matches!(outcome, AutoRecoveryOutcome::Recovered));
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 2);
    let refine_b = refine_dir_for_target_root(&b).unwrap();
    assert_eq!(
        std::fs::read_to_string(refine_b.join("shared/state.json")).unwrap(),
        "{\"node\":\"remote\"}\n"
    );
    let health = FileStateSyncHealthService::new(&runtime_root)
        .inspect(&b, "default", Duration::from_secs(900))
        .unwrap();
    assert!(health.failure_since.is_none(), "{health:#?}");
    std::fs::remove_dir_all(temp_root).unwrap();
}
