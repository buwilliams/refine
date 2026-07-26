use super::*;

#[test]
fn web_server_reorders_and_moves_feature_workflow() {
    let temp_root = unique_temp_dir("http-feature-reorder-move");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    for (id, name) in [("GOAL1", "Goal One"), ("GOAL2", "Goal Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for goal_id in ["GOAL1", "GOAL2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/goals".to_string(),
            body: Some(json!({"goal_id": goal_id})),
        });
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/work/features/FEA1/goals/{goal_id}/order"),
            body: None,
        });
    }

    let reorder = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL2/reorder".to_string(),
        body: Some(json!({"order": 1})),
    });
    assert_eq!(reorder.status, 200);
    assert_eq!(reorder.body["goal_ids"], json!(["GOAL2", "GOAL1"]));

    let reorder_before = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/reorder".to_string(),
        body: Some(json!({"before": "GOAL2"})),
    });
    assert_eq!(reorder_before.status, 200);
    assert_eq!(reorder_before.body["goal_ids"], json!(["GOAL1", "GOAL2"]));

    let reorder_after = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/goals/GOAL1/reorder".to_string(),
        body: Some(json!({"after": "GOAL2"})),
    });
    assert_eq!(reorder_after.status, 200);
    assert_eq!(reorder_after.body["goal_ids"], json!(["GOAL2", "GOAL1"]));

    let move_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/move".to_string(),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(move_feature.status, 200);
    assert_eq!(move_feature.body["rollup"]["status"], "todo");
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_cancels_and_deletes_features() {
    let temp_root = unique_temp_dir("http-feature-cancel-delete");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(runtime_root.clone());
    for (id, name) in [("GOAL1", "Goal One"), ("GOAL2", "Goal Two")] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
    }
    server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    for goal_id in ["GOAL1", "GOAL2"] {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/goals".to_string(),
            body: Some(json!({"goal_id": goal_id})),
        });
    }

    let goal_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/goals/GOAL1/cancel".to_string(),
        body: None,
    });
    assert_eq!(goal_cancel.status, 200);
    assert_eq!(goal_cancel.body["goal"]["status"], "cancelled");

    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process = supervisor
        .register(ManagedProcess {
            id: "agent-goal2".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("agent".to_string()),
            details: Some("working on GOAL2".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "now".to_string(),
            exit_code: None,
        })
        .unwrap();
    let operation = FileOperationRegistry::new(&runtime_root)
        .register("feature FEA1 goal GOAL2")
        .unwrap();

    let feature_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/work/features/FEA1/cancel".to_string(),
        body: None,
    });
    assert_eq!(feature_cancel.status, 200);
    assert_eq!(feature_cancel.body["rollup"]["cancelled_count"], 2);
    assert_eq!(feature_cancel.body["runtime_reconciled"]["processes"], 1);
    assert_eq!(feature_cancel.body["runtime_reconciled"]["operations"], 1);
    assert!(supervisor.inspect(&process.id).is_err());
    assert_eq!(
        FileOperationRegistry::new(&runtime_root)
            .status(&operation.id)
            .unwrap()
            .state,
        OperationState::Cancelled
    );

    let feature_delete = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/work/features/FEA1".to_string(),
        body: None,
    });
    assert_eq!(feature_delete.status, 200);
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL2/goal.json").exists());

    remove_temp_dir(&temp_root);
}
