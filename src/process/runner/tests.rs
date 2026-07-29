use super::*;

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
