use super::*;

fn decision(service: &FileWorkItemService, to: GoalStatus, request_id: &str) -> WorkflowControl {
    let goal = service.show_goal_detail("GOAL1").unwrap();
    WorkflowControl {
        to,
        reason: "Explicit operator decision".into(),
        context: "Retain the investigation for the next attempt".into(),
        expected_revision: goal["workflow_revision"].as_u64().unwrap_or(0),
        request_id: request_id.into(),
        force: false,
        actor: "Operator".into(),
        invocation_id: None,
    }
}

#[test]
fn workflow_control_is_surface_independent_idempotent_and_revision_fenced() {
    let root = unique_temp_dir("workflow-controls");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    let plan = decision(&service, GoalStatus::Plan, "plan-1");
    let receipt = service.control_workflow("GOAL1", &plan).unwrap();
    assert_eq!(service.control_workflow("GOAL1", &plan).unwrap(), receipt);
    let goal = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(goal["workflow_controls"].as_array().unwrap().len(), 1);
    assert!(
        goal["rounds"][0]["prompt"]
            .as_str()
            .unwrap()
            .contains(&plan.context)
    );
    let mut stale = plan.clone();
    stale.request_id = "stale-redirect".into();
    stale.to = GoalStatus::Failed;
    assert!(matches!(
        service.control_workflow("GOAL1", &stale),
        Err(RefineError::Conflict(_))
    ));
    let mut reused = plan;
    reused.context = "different request".into();
    assert!(matches!(
        service.control_workflow("GOAL1", &reused),
        Err(RefineError::Conflict(_))
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_control_forced_done_is_audited_and_does_not_invent_integration() {
    let root = unique_temp_dir("workflow-force-done");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    let mut request = decision(&service, GoalStatus::Done, "force-done");
    assert!(service.control_workflow("GOAL1", &request).is_err());
    request.force = true;
    let receipt = service.control_workflow("GOAL1", &request).unwrap();
    assert_eq!(receipt["forced"], true);
    assert_eq!(receipt["integration_performed"], false);
    assert!(
        !receipt["overridden_requirements"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let goal = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "done");
    assert!(goal["candidate_commit"].is_null());
    assert!(goal["integration"].is_null());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_control_repeated_attempts_have_flat_history_and_fresh_event_identity() {
    let root = unique_temp_dir("workflow-control-attempts");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    service
        .control_workflow("GOAL1", &decision(&service, GoalStatus::Plan, "plan"))
        .unwrap();
    let before = service.show_goal_detail("GOAL1").unwrap();
    for number in 0..3 {
        service
            .control_workflow(
                "GOAL1",
                &decision(&service, GoalStatus::Todo, &format!("retry-{number}")),
            )
            .unwrap();
    }
    let after = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        after["event_generation"].as_u64().unwrap(),
        before["event_generation"].as_u64().unwrap_or(0) + 3
    );
    let attempts = after["rounds"][0]["prior_attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 3);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.get("prior_attempts").is_none())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn interrupted_integration_releases_reservation_without_repeating_git_work() {
    let root = unique_temp_dir("interrupted-integration-control");
    let refine_dir = root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let projected = service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    let path = refine_dir.join(&projected.goal.json_path);
    let mut durable: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    durable["status"] = json!("governance");
    durable["workflow_integration_control"] = json!({"state":"pending","request_id":"interrupted"});
    durable["workflow_controls"] =
        json!([{"request_id":"interrupted","integration_performed":false}]);
    fs::write(&path, serde_json::to_vec(&durable).unwrap()).unwrap();
    service
        .interrupt_workflow("GOAL1", "Runner stopped")
        .unwrap();
    let after = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(after["status"], "failed");
    assert_eq!(
        after["workflow_integration_control"]["state"],
        "interrupted"
    );
    assert_eq!(
        after["workflow_controls"][0]["integration_result"]["state"],
        "interrupted"
    );
    assert_eq!(
        after["workflow_controls"][0]["integration_performed"],
        false
    );
    service
        .control_workflow(
            "GOAL1",
            &decision(&service, GoalStatus::Plan, "explicit-recovery"),
        )
        .unwrap();
    assert_eq!(
        service.show_goal_detail("GOAL1").unwrap()["rounds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn forced_integration_rejects_missing_inputs_before_recording_a_decision() {
    let root = unique_temp_dir("force-integration-preflight");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    let before = service.show_goal_detail("GOAL1").unwrap();
    let mut request = decision(&service, GoalStatus::Governance, "integrate-missing");
    request.force = true;
    let integration = crate::application::workflow::governance::integration::FileGovernanceIntegrationService::new(root.join("runtime"), root.join(".refine"));
    assert!(integration.force_integrate("GOAL1", &request).is_err());
    assert_eq!(service.show_goal_detail("GOAL1").unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_attempt_admission_fence_is_cleared_by_an_explicit_new_attempt() {
    let root = unique_temp_dir("failed-attempt-fence");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    service
        .control_workflow("GOAL1", &decision(&service, GoalStatus::Plan, "plan"))
        .unwrap();
    let (round, revision, prompt) = service.authored_goal_commitment("GOAL1").unwrap();
    let authority = service
        .claim_workflow_attempt("GOAL1", GoalStatus::Todo, round, revision, &prompt)
        .unwrap();
    let engine =
        crate::application::workflow::WorkflowEngine::with_target_root(root.join("runtime"), &root);
    engine.fence_failed_attempt("GOAL1", authority);
    assert!(engine.failed_attempt_is_fenced("GOAL1", &service.show_goal_detail("GOAL1").unwrap()));
    service
        .control_workflow(
            "GOAL1",
            &decision(&service, GoalStatus::Todo, "explicit-retry"),
        )
        .unwrap();
    assert!(!engine.failed_attempt_is_fenced("GOAL1", &service.show_goal_detail("GOAL1").unwrap()));
    assert_eq!(
        service.show_goal_detail("GOAL1").unwrap()["rounds"][0]["prior_attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}
