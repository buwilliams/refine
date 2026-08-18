use super::*;
use crate::infrastructure::process::subprocess::{ManagedProcess, ProcessOwner};

#[test]
fn process_summary_preserves_active_work_during_workflow_pause() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-paused-process-summary-{}",
        uuid::Uuid::new_v4()
    ));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .register(ManagedProcess {
            id: "active-agent".to_string(),
            owner: ProcessOwner::Agent,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Active Goal agent".to_string()),
            details: Some(json!({"goal_id": "GOAL1", "round_idx": 0}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let operations = FileOperationRegistry::new(&runtime_root);
    operations.register("governance-integration:GOAL1").unwrap();
    operations.register("import:extract:plan").unwrap();
    supervisor.set_workflow_paused(true).unwrap();

    let paused = process_summary_value(&runtime_root).unwrap();
    for alias in [
        "workflow_paused",
        "paused",
        "background_processes_stopped",
        "agents_paused",
    ] {
        assert_eq!(
            paused[alias], true,
            "{alias} must derive from workflow_paused"
        );
    }
    assert_eq!(
        paused["processes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|process| process["id"] == "active-agent")
            .unwrap()["status"],
        "running"
    );
    for kind in ["governance_integrator", "plan_draft_extractor"] {
        assert_eq!(
            paused["runner_work"]
                .as_array()
                .unwrap()
                .iter()
                .find(|work| work["kind"] == kind)
                .unwrap()["status"],
            "running"
        );
    }
    assert!(
        paused["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|work| !matches!(
                work["kind"].as_str(),
                Some("governance_integrator" | "plan_draft_extractor")
            ))
            .all(|work| work["status"] == "idle")
    );

    supervisor.set_workflow_paused(false).unwrap();
    let resumed = process_summary_value(&runtime_root).unwrap();
    assert_eq!(resumed["workflow_paused"], false);
    assert_eq!(resumed["processes"][0]["status"], "running");
    assert!(
        resumed["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|work| matches!(
                work["kind"].as_str(),
                Some("governance_integrator" | "plan_draft_extractor")
            ))
            .all(|work| work["status"] == "running")
    );
    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn runtime_projection_uses_canonical_pause_precedence_for_aliases() {
    let runtime = RuntimeProjection {
        supervisor: Some(JsonObject::from_iter([
            ("workflow_paused".to_string(), json!(false)),
            ("paused".to_string(), json!(true)),
            ("agents_paused".to_string(), json!(true)),
        ])),
        ..RuntimeProjection::default()
    };
    let summary = runtime_process_summary_value(&runtime);
    for alias in [
        "workflow_paused",
        "paused",
        "background_processes_stopped",
        "agents_paused",
    ] {
        assert_eq!(
            summary[alias], false,
            "{alias} must follow the canonical gate"
        );
    }
}

#[test]
fn process_summary_exposes_stable_background_workers_and_live_resources() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-process-manager-summary-{}",
        uuid::Uuid::new_v4()
    ));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .register(ManagedProcess {
            id: "git-sync-live".to_string(),
            owner: ProcessOwner::Runner,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("git-sync".to_string()),
            details: Some(json!({"kind": "runner", "worker_kind": "git-sync"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .set_background_worker_enabled("development-requests", false)
        .unwrap();

    let summary = FileProcessStatusService::new(&runtime_root)
        .summary()
        .unwrap();
    let workers = summary["background_workers"].as_array().unwrap();
    assert_eq!(workers.len(), BACKGROUND_RUNNERS.len());
    let git_sync = workers
        .iter()
        .find(|worker| worker["worker_kind"] == "git-sync")
        .unwrap();
    assert_eq!(git_sync["status"], "running");
    assert_eq!(git_sync["pid"], std::process::id());
    assert!(git_sync["memory_used_bytes"].as_u64().unwrap() > 0);
    assert!(git_sync["processor_used_percent"].as_f64().is_some());
    let workflow = workers
        .iter()
        .find(|worker| worker["worker_kind"] == "workflow")
        .unwrap();
    assert_eq!(
        workflow["management_actions"],
        json!(["start_background_worker", "pause_workflow"])
    );
    let development_requests = workers
        .iter()
        .find(|worker| worker["worker_kind"] == "development-requests")
        .unwrap();
    assert_eq!(development_requests["status"], "stopped");
    assert_eq!(development_requests["disabled"], true);
    assert_eq!(
        summary["disabled_background_workers"],
        json!(["development-requests"])
    );

    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn process_summary_discovers_future_background_worker_kinds() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-process-manager-discovered-worker-{}",
        uuid::Uuid::new_v4()
    ));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .register(ManagedProcess {
            id: "future-worker-live".to_string(),
            owner: ProcessOwner::Runner,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("future worker".to_string()),
            details: Some(json!({"kind": "runner", "worker_kind": "future-worker"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .set_background_worker_enabled("future-disabled-worker", false)
        .unwrap();

    let summary = process_summary_value(&runtime_root).unwrap();
    let workers = summary["background_workers"].as_array().unwrap();
    let live = workers
        .iter()
        .find(|worker| worker["worker_kind"] == "future-worker")
        .expect("live discovered worker row");
    assert_eq!(live["status"], "running");
    assert_eq!(
        live["management_actions"],
        json!(["stop_background_worker"])
    );
    let disabled = workers
        .iter()
        .find(|worker| worker["worker_kind"] == "future-disabled-worker")
        .expect("disabled discovered worker row");
    assert_eq!(disabled["status"], "stopped");
    assert_eq!(
        disabled["management_actions"],
        json!(["start_background_worker"])
    );

    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn process_summary_keeps_repository_reconciliation_internal() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-process-manager-internal-reconcile-{}",
        uuid::Uuid::new_v4()
    ));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    for (id, details) in [
        (
            "repository-reconcile",
            json!({"kind": "repository_reconcile", "command": "git ls-remote"}),
        ),
        (
            "source-update",
            json!({"kind": "source_update", "command": "git fetch"}),
        ),
    ] {
        supervisor
            .register(ManagedProcess {
                id: id.to_string(),
                owner: ProcessOwner::Maintenance,
                pid: Some(std::process::id()),
                state: "running".to_string(),
                label: Some("git".to_string()),
                details: Some(details.to_string()),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: None,
            })
            .unwrap();
    }

    let summary = process_summary_value(&runtime_root).unwrap();
    let processes = summary["processes"].as_array().unwrap();
    assert!(
        processes
            .iter()
            .all(|process| process["id"] != "repository-reconcile")
    );
    assert!(
        processes
            .iter()
            .any(|process| process["id"] == "source-update")
    );

    std::fs::remove_dir_all(runtime_root).unwrap();
}

#[test]
fn repository_disk_usage_includes_git_owned_worktree_storage() {
    let repository_root = std::env::temp_dir().join(format!(
        "refine-process-manager-disk-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&repository_root).unwrap();
    let initialized = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repository_root)
        .status()
        .unwrap();
    assert!(initialized.success());
    std::fs::write(repository_root.join("app.bin"), vec![1_u8; 8 * 1024]).unwrap();
    let worktree_storage = repository_root.join(".git/refine-worktrees/example");
    std::fs::create_dir_all(&worktree_storage).unwrap();
    std::fs::write(worktree_storage.join("build.bin"), vec![2_u8; 16 * 1024]).unwrap();

    // The request path never walks the repository: the first ask registers
    // demand and reports pending, a refresher pass measures off-path.
    let pending = repository_disk_usage_value(&repository_root);
    assert_eq!(pending["bytes"], Value::Null);
    refresh_repository_disk_usage_once();

    let usage = repository_disk_usage_value(&repository_root);
    assert_eq!(usage["includes_git_worktrees"], true);
    assert!(usage["bytes"].as_u64().unwrap() >= 24 * 1024);
    assert!(usage["git_common_dir"].as_str().unwrap().ends_with(".git"));

    std::fs::remove_dir_all(repository_root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn proc_resource_observations_report_this_process() {
    let mut pids = BTreeSet::new();
    pids.insert(std::process::id());
    // A pid that cannot exist must simply be absent, not an error.
    pids.insert(u32::MAX);

    let observations = process_resource_observations(&pids);

    let (memory_used_bytes, processor_used_percent) =
        observations.get(&std::process::id()).unwrap();
    assert!(*memory_used_bytes > 0);
    assert!(*processor_used_percent >= 0.0);
    assert!(!observations.contains_key(&u32::MAX));
}
