use super::*;

#[test]
fn file_work_item_service_transitions_goal_via_refine_json() {
    let temp_root = unique_temp_dir("work-item-transition");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("01").join("GOAL1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Transition me",
              "status": "backlog",
              "priority": "low",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();

    let updated =
        FileWorkItemService::new(&refine_dir).transition_goal_status("GOAL1", GoalStatus::Todo);
    assert_eq!(updated.unwrap().goal.status, GoalStatus::Todo);
    let written = fs::read_to_string(goal_dir.join("goal.json")).unwrap();
    assert!(written.contains("\"status\": \"todo\""));
    assert!(written.contains("\"updated\": \"20"));
    assert!(written.contains("Z\""));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn projected_status_transition_rejects_an_external_revision_change() {
    let temp_root = unique_temp_dir("work-item-projected-transition-conflict");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let projected = service
        .create_goal_summary("Concurrent transition", Some("GOAL1"))
        .unwrap();
    let goal_path = refine_dir.join(&projected.goal.json_path);
    let mut durable: serde_json::Value =
        serde_json::from_slice(&fs::read(&goal_path).unwrap()).unwrap();
    durable["status"] = json!("review");
    durable["updated"] = json!("2026-07-23T13:00:00Z");
    fs::write(&goal_path, serde_json::to_vec_pretty(&durable).unwrap()).unwrap();

    let error = service
        .transition_goal_status_from_projection(&projected.goal, GoalStatus::Todo)
        .unwrap_err();

    assert!(matches!(error, RefineError::Conflict(_)));
    let durable: serde_json::Value =
        serde_json::from_slice(&fs::read(&goal_path).unwrap()).unwrap();
    assert_eq!(durable["status"], "review");
    assert_eq!(durable["updated"], "2026-07-23T13:00:00Z");
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn authored_todo_start_rejects_round_revision_change_before_execution() {
    let temp_root = unique_temp_dir("authored-todo-fence");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Fenced Goal", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Original source")
        .unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    let (round_idx, revision, request) = service.authored_goal_commitment("GOAL1").unwrap();
    service
        .edit_latest_goal_round_summary("GOAL1", None, None, Some("Changed source"))
        .unwrap();

    let error = service
        .advance_authored_goal_status("GOAL1", GoalStatus::Plan, round_idx, revision, &request)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("authoring changed before execution")
    );
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert_eq!(
        service.show_goal_detail("GOAL1").unwrap()["rounds"][0]["prompt"],
        "Changed source"
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn authored_todo_start_atomically_pins_nonempty_round() {
    let temp_root = unique_temp_dir("authored-todo-start");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Fenced Goal", Some("GOAL1"))
        .unwrap();
    assert!(service.authored_goal_commitment("GOAL1").is_err());
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Authoritative source")
        .unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    let (round_idx, revision, request) = service.authored_goal_commitment("GOAL1").unwrap();
    let started = service
        .advance_authored_goal_status("GOAL1", GoalStatus::Plan, round_idx, revision, &request)
        .unwrap();
    assert_eq!(started.goal.status, GoalStatus::Plan);
    assert_eq!(round_idx, 0);
    assert_eq!(request, "Authoritative source");
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_verifies_and_undoes_goal_workflow() {
    let temp_root = unique_temp_dir("work-item-verify-undo");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Merge Goal", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Implementer", "Initial implementation")
        .unwrap();
    service
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({
                "workflow_integration": {
                    "candidate_commit": "candidate",
                    "target_branch": "main",
                    "target_commit": "target",
                    "remote": "origin",
                    "pushed": true,
                    "integrated_at": "2026-07-23T12:00:00Z",
                    "merge": {"ok": true, "conflicts": [], "message": "integrated"}
                }
            }),
        )
        .unwrap();
    service
        .set_goal_status_unchecked("GOAL1", &GoalStatus::Review)
        .unwrap();

    let verified = service.verify_goal_summary("GOAL1").unwrap();
    assert_eq!(verified.goal.status, GoalStatus::Done);

    let undone = service.undo_goal_summary("GOAL1").unwrap();
    assert_eq!(undone.goal.status, GoalStatus::Review);
    assert!(
        service
            .undo_goal_summary("GOAL1")
            .unwrap_err()
            .to_string()
            .contains("submit a new round")
    );
    let revised = service
        .append_goal_round_summary("GOAL1", "Reviewer", "Address review feedback")
        .unwrap();
    assert_eq!(revised.goal.status, GoalStatus::Todo);
    assert_eq!(revised.goal.round_count, 2);
    let detail = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_integration"]["candidate_commit"],
        "candidate"
    );
    assert!(detail["rounds"][1]["workflow_integration"].is_null());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_rejects_invalid_manual_transition() {
    let temp_root = unique_temp_dir("work-item-invalid-transition");
    let refine_dir = temp_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("01").join("GOAL1");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "Transition me",
              "status": "backlog",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();

    let err = FileWorkItemService::new(&refine_dir)
        .transition_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap_err();
    assert_eq!(
        err.category(),
        crate::process::supervisor::errors::ErrorCategory::InvalidInput
    );
    fs::remove_dir_all(temp_root).unwrap();
}
