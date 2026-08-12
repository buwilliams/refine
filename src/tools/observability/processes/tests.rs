use super::*;
use crate::process::subprocess::{ManagedProcess, ProcessOwner};

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
    operations.register("merger:GOAL1").unwrap();
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
    for kind in ["merger", "plan_draft_extractor"] {
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
                Some("merger" | "plan_draft_extractor")
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
                Some("merger" | "plan_draft_extractor")
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
