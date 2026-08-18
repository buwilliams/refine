use super::*;

use sha2::{Digest, Sha256};

use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationPlan, ImplementationPlanBinding,
    ImplementationPlanPhase, ImplementationPlanState,
};

#[test]
fn file_work_item_service_edits_notes_and_deletes_goal_json() {
    let temp_root = unique_temp_dir("work-item-edit-note-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    crate::infrastructure::process::supervisor::config::FileReporterService::new(&refine_dir)
        .create("Existing")
        .unwrap();
    service
        .create_goal_summary("Original", Some("GOAL1"))
        .unwrap();

    let edited = service
        .update_goal_metadata_summary(
            "GOAL1",
            Some("Renamed"),
            Some("high"),
            Some("Reporter"),
            None,
        )
        .unwrap();
    assert_eq!(edited.goal.name, "Renamed");
    assert_eq!(edited.goal.priority, GoalPriority::High);
    assert_eq!(edited.goal.reporter.as_deref(), Some("Reporter"));
    let reporters =
        crate::infrastructure::process::supervisor::config::FileReporterService::new(&refine_dir)
            .list()
            .unwrap();
    let reporter_names = reporters["reporters"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|reporter| reporter["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(reporter_names, vec!["Existing", "Reporter"]);

    service
        .add_goal_note_summary("GOAL1", "Reviewer", "Needs a note")
        .unwrap();
    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"author\": \"Reviewer\""));
    assert!(written.contains("\"body\": \"Needs a note\""));

    service.delete_goal_record("GOAL1").unwrap();
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_appends_and_edits_latest_round() {
    let temp_root = unique_temp_dir("work-item-rounds");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Round Goal", Some("GOAL1"))
        .unwrap();

    let goal = service
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    assert_eq!(goal.goal.round_count, 1);
    let goal = service
        .edit_latest_goal_round_summary(
            "GOAL1",
            Some("Reviewer"),
            Some("Reviewer"),
            Some("New prompt"),
        )
        .unwrap();
    assert_eq!(goal.goal.reporter.as_deref(), Some("Reviewer"));
    assert_eq!(goal.goal.assignee.as_deref(), Some("Reviewer"));
    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));
    assert!(written.contains("\"assignee\": \"Reviewer\""));
    assert!(written.contains("\"prompt\": \"New prompt\""));
    assert!(written.contains("\"rule_state\": \"unclassified\""));

    let error = service
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({"implementation_plan": {"state": "completed"}}),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("Workflow-owned evidence"),
        "{error}"
    );
    assert!(
        service.show_goal_detail("GOAL1").unwrap()["rounds"][0]["implementation_plan"].is_null()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_retains_quality_recovery_evidence() {
    let temp_root = unique_temp_dir("work-item-quality-recovery-evidence");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Quality recovery evidence", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();

    service
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({
                "quality_agent_report": "Reviewed and corrected the candidate.",
                "quality_candidate_commit": "candidate123",
                "quality_recovery_analysis": "The configured check still fails.",
                "quality_recovery_round_prompt": "Correct the check and add a regression test.",
                "quality_recovery_details": {"phase": "quality_recovery"},
                "quality_recovery_checked_at": "2026-08-13T12:00:00Z"
            }),
        )
        .unwrap();

    let detail = service.show_goal_detail("GOAL1").unwrap();
    let round = &detail["rounds"][0];
    assert_eq!(round["quality_candidate_commit"], "candidate123");
    assert_eq!(
        round["quality_recovery_details"]["phase"],
        "quality_recovery"
    );
    assert_eq!(
        round["quality_recovery_round_prompt"],
        "Correct the check and add a regression test."
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_records_latest_round_implementation_report() {
    let temp_root = unique_temp_dir("work-item-implementation-report");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Reported Goal", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Implement it")
        .unwrap();

    service
        .update_latest_goal_round_implementation_report(
            "GOAL1",
            "  Changed the Goal detail so reviewers can see why.\nVerification: cargo test passed.  ",
        )
        .unwrap();

    let detail = service.show_goal_detail("GOAL1").unwrap();
    let round = &detail["rounds"][0];
    assert_eq!(
        round["implementation_report"],
        "Changed the Goal detail so reviewers can see why.\nVerification: cargo test passed."
    );
    assert!(
        round["implementation_reported_at"]
            .as_str()
            .is_some_and(|value| value.starts_with("20") && value.ends_with('Z')),
        "{detail:#}"
    );
    assert!(
        service
            .update_latest_goal_round_implementation_report("GOAL1", "   ")
            .is_err()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn implementation_plan_round_trips_and_rejects_stale_or_rebound_updates() {
    let temp_root = unique_temp_dir("work-item-implementation-plan");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Planned Goal", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Implement it")
        .unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    service
        .update_goal_git_refs("GOAL1", "refine/GOAL1/round-1", "main", "base123", None)
        .unwrap();
    let context = json!({
        "version": 1,
        "goal": {"id": "GOAL1"},
        "previous_rounds": [],
        "current_round": {"round": 1, "prompt": "Implement it"}
    });
    service
        .update_goal_round_evaluation_summary("GOAL1", 0, &json!({"agent_context": context}))
        .unwrap();
    let context_digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&context).unwrap())
    );
    let initial = ImplementationPlan {
        schema_version: IMPLEMENTATION_PLAN_SCHEMA_VERSION,
        state: ImplementationPlanState::InProgress,
        phase: ImplementationPlanPhase::Plan,
        binding: ImplementationPlanBinding {
            goal_id: "GOAL1".to_string(),
            round_idx: 0,
            context_version: 1,
            context_digest,
            implementation_branch: "refine/GOAL1/round-1".to_string(),
            target_branch: "main".to_string(),
            base_commit: "base123".to_string(),
        },
        started_at: "2026-08-11T10:00:00Z".to_string(),
        phase_started_at: "2026-08-11T10:00:00Z".to_string(),
        updated_at: "2026-08-11T10:00:00Z".to_string(),
        completed_at: None,
        proposal: None,
        criticism: None,
        final_plan: None,
        implementation: None,
        failure: None,
        invalid_output_attempts: Vec::new(),
        provider_session_id: None,
        governance_precheck: None,
    };
    service
        .replace_goal_round_implementation_plan("GOAL1", 0, None, &initial)
        .unwrap();
    assert_eq!(
        service.show_goal_detail("GOAL1").unwrap()["rounds"][0]["implementation_plan"]["phase"],
        "plan"
    );

    let mut criticized = initial.clone();
    criticized.phase = ImplementationPlanPhase::Criticize;
    criticized.updated_at = "2026-08-11T10:01:00Z".to_string();
    service
        .replace_goal_round_implementation_plan("GOAL1", 0, Some(&initial), &criticized)
        .unwrap();
    let mut stale = initial.clone();
    stale.phase = ImplementationPlanPhase::Revise;
    assert!(
        service
            .replace_goal_round_implementation_plan("GOAL1", 0, Some(&initial), &stale)
            .unwrap_err()
            .to_string()
            .contains("authority changed")
    );

    service
        .update_goal_git_refs("GOAL1", "refine/GOAL1/rebound", "main", "base123", None)
        .unwrap();
    assert!(
        service
            .replace_goal_round_implementation_plan("GOAL1", 0, Some(&criticized), &stale)
            .unwrap_err()
            .to_string()
            .contains("branch_name changed")
    );
    service
        .update_goal_git_refs("GOAL1", "refine/GOAL1/round-1", "main", "base456", None)
        .unwrap();
    assert!(
        service
            .replace_goal_round_implementation_plan("GOAL1", 0, Some(&criticized), &stale)
            .unwrap_err()
            .to_string()
            .contains("base_commit changed")
    );
    service
        .update_goal_git_refs("GOAL1", "refine/GOAL1/round-1", "main", "base123", None)
        .unwrap();
    service
        .update_goal_git_refs("GOAL1", "refine/GOAL1/round-1", "release", "base123", None)
        .unwrap();
    assert!(
        service
            .replace_goal_round_implementation_plan("GOAL1", 0, Some(&criticized), &stale)
            .unwrap_err()
            .to_string()
            .contains("target_branch changed")
    );
    service
        .update_goal_git_refs("GOAL1", "refine/GOAL1/round-1", "main", "base123", None)
        .unwrap();

    service
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({"agent_context": {"version": 2, "goal": {"id": "GOAL1"}}}),
        )
        .unwrap();
    assert!(
        service
            .replace_goal_round_implementation_plan("GOAL1", 0, Some(&criticized), &stale)
            .unwrap_err()
            .to_string()
            .contains("context changed")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn implementation_plan_rejects_a_null_pinned_agent_context() {
    let temp_root = unique_temp_dir("work-item-implementation-plan-null-context");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Planned Goal", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Implement it")
        .unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    service
        .update_goal_git_refs("GOAL1", "refine/GOAL1/round-1", "main", "base123", None)
        .unwrap();
    // A no-signal decode can persist agent_context as literal null; planning
    // must treat that as a lost pin, not hash the null.
    service
        .update_goal_round_evaluation_summary("GOAL1", 0, &json!({"agent_context": null}))
        .unwrap();
    let plan = ImplementationPlan {
        schema_version: IMPLEMENTATION_PLAN_SCHEMA_VERSION,
        state: ImplementationPlanState::InProgress,
        phase: ImplementationPlanPhase::Plan,
        binding: ImplementationPlanBinding {
            goal_id: "GOAL1".to_string(),
            round_idx: 0,
            context_version: 1,
            context_digest: format!("{:x}", Sha256::digest(b"null")),
            implementation_branch: "refine/GOAL1/round-1".to_string(),
            target_branch: "main".to_string(),
            base_commit: "base123".to_string(),
        },
        started_at: "2026-08-11T10:00:00Z".to_string(),
        phase_started_at: "2026-08-11T10:00:00Z".to_string(),
        updated_at: "2026-08-11T10:00:00Z".to_string(),
        completed_at: None,
        proposal: None,
        criticism: None,
        final_plan: None,
        implementation: None,
        failure: None,
        invalid_output_attempts: Vec::new(),
        provider_session_id: None,
        governance_precheck: None,
    };

    let error = service
        .replace_goal_round_implementation_plan("GOAL1", 0, None, &plan)
        .unwrap_err();

    assert!(
        error.to_string().contains("lost its pinned agent context"),
        "{error}"
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_goal_authoring_applies_latest_round_duplicate_decisions_in_parity() {
    let temp_root = unique_temp_dir("goal-authoring-duplicate-parity");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    service
        .author_goal(GoalAuthoringRequest {
            id: Some("ORIGINAL".to_string()),
            name: Some("Original".to_string()),
            prompt: "Earlier round prompt".to_string(),
            reporter: "Buddy".to_string(),
            assignee: Some("Alice".to_string()),
            priority: "medium".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap();
    service
        .append_goal_round_summary_with_assignee(
            "ORIGINAL",
            "Buddy",
            Some("Alice"),
            "Latest round prompt",
        )
        .unwrap();

    let earlier_round = service
        .author_goal(GoalAuthoringRequest {
            id: Some("EARLIER".to_string()),
            prompt: "Earlier round prompt".to_string(),
            reporter: "Buddy".to_string(),
            priority: "low".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap();
    assert!(earlier_round.created, "only the latest round may match");

    let ordinary_prompt = || GoalAuthoringRequest {
        prompt: "Latest round prompt".to_string(),
        reporter: "Buddy".to_string(),
        priority: "low".to_string(),
        ..GoalAuthoringRequest::default()
    };
    let feature_prompt = || FeatureGoalAuthoringRequest {
        prompt: "Latest round prompt".to_string(),
        reporter: "Buddy".to_string(),
        priority: "low".to_string(),
        ..FeatureGoalAuthoringRequest::default()
    };

    let ordinary_detected = service.author_goal(ordinary_prompt()).unwrap();
    let feature_detected = service
        .author_feature_goal("FEA1", feature_prompt())
        .unwrap();
    assert!(ordinary_detected.requires_duplicate_decision);
    assert!(feature_detected.requires_duplicate_decision);
    assert_eq!(ordinary_detected.duplicate, feature_detected.duplicate);
    assert_eq!(ordinary_detected.duplicate.unwrap().id, "ORIGINAL");

    let mut ordinary_skip = ordinary_prompt();
    ordinary_skip.duplicate_decision = "duplicate".to_string();
    let mut feature_skip = feature_prompt();
    feature_skip.duplicate_decision = "duplicate".to_string();
    for result in [
        service.author_goal(ordinary_skip).unwrap(),
        service.author_feature_goal("FEA1", feature_skip).unwrap(),
    ] {
        assert!(!result.created);
        assert_eq!(result.duplicate_action.as_deref(), Some("duplicate"));
    }

    service
        .transition_goal_status("ORIGINAL", GoalStatus::Todo)
        .unwrap();
    let mut ordinary_move = ordinary_prompt();
    ordinary_move.duplicate_decision = "move_original_to_backlog".to_string();
    let ordinary_move = service.author_goal(ordinary_move).unwrap();
    assert!(ordinary_move.move_result.unwrap().moved);
    service
        .transition_goal_status("ORIGINAL", GoalStatus::Todo)
        .unwrap();
    let mut feature_move = feature_prompt();
    feature_move.duplicate_decision = "move_original_to_backlog".to_string();
    let feature_move = service.author_feature_goal("FEA1", feature_move).unwrap();
    assert!(feature_move.move_result.unwrap().moved);

    let mut ordinary_create = ordinary_prompt();
    ordinary_create.duplicate_decision = "original".to_string();
    let ordinary_created = service.author_goal(ordinary_create).unwrap();
    let mut feature_create = feature_prompt();
    feature_create.duplicate_decision = "original".to_string();
    feature_create.placement = FeatureGoalPlacement::First;
    let feature_created = service.author_feature_goal("FEA1", feature_create).unwrap();
    assert!(ordinary_created.created);
    assert!(feature_created.created);
    assert_eq!(
        feature_created.goal.unwrap().feature_order,
        Some(1),
        "Feature placement stays part of the same authoring operation"
    );

    let mut ordinary_invalid = ordinary_prompt();
    ordinary_invalid.duplicate_decision = "unknown".to_string();
    let mut feature_invalid = feature_prompt();
    feature_invalid.duplicate_decision = "unknown".to_string();
    assert_eq!(
        service
            .author_goal(ordinary_invalid)
            .unwrap_err()
            .to_string(),
        service
            .author_feature_goal("FEA1", feature_invalid)
            .unwrap_err()
            .to_string()
    );

    fs::remove_dir_all(temp_root).unwrap();
}
