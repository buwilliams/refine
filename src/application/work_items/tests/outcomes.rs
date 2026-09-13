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
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Authored request")
        .unwrap();
    let plan = decision(&service, GoalStatus::Plan, "plan-1");
    let receipt = service.control_workflow("GOAL1", &plan).unwrap();
    assert_eq!(service.control_workflow("GOAL1", &plan).unwrap(), receipt);
    let goal = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(goal["workflow_controls"].as_array().unwrap().len(), 1);
    assert_eq!(goal["rounds"][0]["prompt"], "Authored request");
    assert_eq!(goal["workflow_context"], plan.context);
    assert_eq!(goal["status"], "plan");
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
    let before = service.show_goal_detail("GOAL1").unwrap();
    let prior_edge_approval = service
        .refine_dir
        .join("automation/approvals")
        .join(format!(
            "{}.json",
            crate::application::events::transitions::edge_key(&before, "done")
        ));
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
    // The atomic decision receipt itself bypasses the previous gates. An
    // approval written before that receipt could survive a failed Goal write
    // and let an ordinary transition bypass gates on the unchanged occurrence.
    assert!(!prior_edge_approval.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_control_repeated_assignments_are_noops() {
    let root = unique_temp_dir("workflow-control-noop");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Authored request")
        .unwrap();
    service
        .control_workflow("GOAL1", &decision(&service, GoalStatus::Todo, "select"))
        .unwrap();
    let before = service.show_goal_detail("GOAL1").unwrap();
    for number in 0..3 {
        let result = service
            .control_workflow(
                "GOAL1",
                &decision(&service, GoalStatus::Todo, &format!("noop-{number}")),
            )
            .unwrap();
        assert_eq!(result["noop"], true);
        assert_eq!(service.show_goal_detail("GOAL1").unwrap(), before);
    }
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
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Integrate the retained candidate")
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
fn unresolved_step_outcome_is_superseded_by_explicit_retry_with_evidence_retained() {
    let root = unique_temp_dir("failed-attempt-fence");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Authored request")
        .unwrap();
    service
        .control_workflow("GOAL1", &decision(&service, GoalStatus::Plan, "plan"))
        .unwrap();
    let (round, revision, prompt) = service.authored_goal_commitment("GOAL1").unwrap();
    let authority = service
        .claim_workflow_attempt("GOAL1", GoalStatus::Plan, round, revision, &prompt)
        .unwrap();
    let engine =
        crate::application::workflow::WorkflowEngine::with_target_root(root.join("runtime"), &root);
    let directory = root.join("runtime/workflow-failures");
    fs::create_dir_all(&directory).unwrap();
    let evidence = json!({"goal_id":"GOAL1", "target_root": root, "round_idx":authority.round_idx,
        "generation":authority.generation, "workflow_revision":authority.workflow_revision,
        "original_error":"provider failed", "settlement":{"unpersisted_evidence":"Goal write unavailable"}});
    let path = directory.join("retained.json");
    fs::write(&path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    assert!(
        engine
            .unresolved_workflow_outcome("GOAL1", &service.show_goal_detail("GOAL1").unwrap())
            .unwrap()
            .is_some()
    );
    service
        .control_workflow(
            "GOAL1",
            &decision(&service, GoalStatus::Todo, "explicit-retry"),
        )
        .unwrap();
    assert!(
        engine
            .unresolved_workflow_outcome("GOAL1", &service.show_goal_detail("GOAL1").unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap(),
        evidence
    );
    assert_eq!(
        service.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_attempt_authority"]["generation"],
        authority.generation
    );
    assert_eq!(
        service.show_goal_detail("GOAL1").unwrap()["rounds"][0]["prior_attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn human_assignment_can_select_every_step_without_candidate_or_new_round() {
    let root = unique_temp_dir("human-workflow-assignment");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Repairable", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Operator", "Keep this request")
        .unwrap();
    for status in [
        GoalStatus::Failed,
        GoalStatus::Todo,
        GoalStatus::Plan,
        GoalStatus::Implement,
        GoalStatus::Quality,
        GoalStatus::Governance,
        GoalStatus::Review,
        GoalStatus::Done,
        GoalStatus::Cancelled,
        GoalStatus::Backlog,
    ] {
        service
            .override_goal_status("GOAL1", status.clone())
            .unwrap();
        let goal = service.show_goal_detail("GOAL1").unwrap();
        assert_eq!(goal["status"], status.as_str());
        assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
        assert_eq!(goal["rounds"][0]["prompt"], "Keep this request");
        assert!(goal["candidate_commit"].is_null());
        assert!(goal["workflow_requested_step"].is_null());
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn human_assignment_stops_owned_agent_and_supersedes_its_claim() {
    use crate::infrastructure::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec};
    use crate::infrastructure::process::subprocess::{ProcessOwner, ProcessSupervisor};
    let root = unique_temp_dir("human-workflow-stop");
    let mut service = FileWorkItemService::new(root.join(".refine"));
    service.active_node_root = Some(root.join("run"));
    service
        .create_goal_summary("Repairable", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Operator", "Request")
        .unwrap();
    service
        .override_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    let (round, revision, prompt) = service.authored_goal_commitment("GOAL1").unwrap();
    let authority = service
        .claim_workflow_attempt("GOAL1", GoalStatus::Implement, round, revision, &prompt)
        .unwrap();
    let supervisor = FileProcessSupervisor::new(root.join("run/agents"));
    let process = supervisor.launch(ManagedProcessSpec {
        owner: ProcessOwner::Agent, command: "/bin/sleep".into(), args: vec!["60".into()],
        cwd: None, env: Vec::new(), stdin: None, limits: None, authorization_command: None, sensitive: false,
        metadata: serde_json::from_value(json!({"goal_id":"GOAL1","node_id":"default","round_idx":0,"isolated_process_group":true})).unwrap(),
    }).unwrap();
    service
        .override_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    assert!(
        supervisor.group_pending(&process).unwrap(),
        "selection commits before downstream cleanup"
    );
    assert!(
        service
            .verify_workflow_attempt("GOAL1", authority, GoalStatus::Implement, "default")
            .is_err()
    );
    assert_eq!(service.show_goal_detail("GOAL1").unwrap()["status"], "todo");
    let revision = service.show_goal_detail("GOAL1").unwrap()["workflow_revision"]
        .as_u64()
        .unwrap();
    service.delete_goal_round("GOAL1", 0, revision).unwrap();
    assert!(
        !supervisor
            .process_history_dir()
            .join(format!("{}.json", process.id))
            .exists()
    );
    assert!(
        !supervisor
            .processes_dir()
            .join(format!("{}.json", process.id))
            .exists()
    );
    assert!(
        !supervisor
            .owned_groups()
            .unwrap()
            .iter()
            .any(|group| group.process.id == process.id)
    );
    for path in [process.stdout_path, process.stderr_path]
        .into_iter()
        .flatten()
    {
        assert!(!std::path::Path::new(&path).exists());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn superseded_integration_records_actual_effect_without_advancing_reentered_governance() {
    let root = unique_temp_dir("superseded-integration-result");
    let service = FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Controlled", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Request")
        .unwrap();
    let mut integrate = decision(&service, GoalStatus::Governance, "integrate-old");
    integrate.force = true;
    service
        .control_workflow_operation("GOAL1", &integrate, true)
        .unwrap();
    service
        .override_goal_status("GOAL1", GoalStatus::Implement)
        .unwrap();
    service
        .override_goal_status("GOAL1", GoalStatus::Governance)
        .unwrap();
    let generation = service.show_goal_detail("GOAL1").unwrap()["event_generation"].clone();
    let result = Ok(json!({"candidate_commit":"actual-published-commit"}));
    let receipt = service
        .finish_controlled_integration("GOAL1", "integrate-old", &result)
        .unwrap();
    assert_eq!(receipt["integration_performed"], true);
    let goal = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "governance");
    assert_eq!(goal["event_generation"], generation);
    assert_eq!(goal["workflow_integration_control"]["state"], "redirected");
    assert_eq!(
        service
            .finish_controlled_integration("GOAL1", "integrate-old", &result)
            .unwrap(),
        receipt
    );
    fs::remove_dir_all(root).unwrap();
}
