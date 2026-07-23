use crate::model::goal::GoalPriority;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

use super::*;
use crate::model::workflow::GoalStatus;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn file_work_item_service_creates_and_lists_goal_json() {
    let temp_root = unique_temp_dir("work-item-create");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);

    let goal = service
        .create_goal_summary("Created from Rust", Some("GOAL1"))
        .unwrap();
    assert_eq!(goal.goal.id, "GOAL1");
    assert_eq!(goal.goal.status, GoalStatus::Backlog);
    assert!(refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.name,
        "Created from Rust"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_edits_notes_and_deletes_goal_json() {
    let temp_root = unique_temp_dir("work-item-edit-note-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
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
fn file_work_item_service_creates_features_and_updates_goal_membership() {
    let temp_root = unique_temp_dir("work-item-feature");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Goal A", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Goal B", Some("GOAL2"))
        .unwrap();

    let feature = service
        .create_feature_summary(
            "Feature A",
            Some("FEA1"),
            Some("desc"),
            Some("Reporter"),
            Some("Reviewer"),
        )
        .unwrap();
    assert_eq!(feature.feature.id, "FEA1");
    assert_eq!(feature.feature.assignee.as_deref(), Some("Reviewer"));
    assert!(refine_dir.join("features/FE/A1/feature.json").exists());

    let feature = service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1"]);
    let feature = service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL2")
            .unwrap()
            .goal
            .feature_order,
        None
    );

    let feature = service.unorder_goal_in_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .feature_order,
        None
    );
    assert_eq!(
        service
            .show_goal_summary("GOAL2")
            .unwrap()
            .goal
            .feature_order,
        None
    );

    let feature = service.order_goal_in_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .feature_order,
        Some(1)
    );

    let feature = service.remove_goal_from_feature("FEA1", "GOAL1").unwrap();
    assert_eq!(feature.goal_ids, vec!["GOAL2"]);
    assert_eq!(
        service
            .show_goal_summary("GOAL2")
            .unwrap()
            .goal
            .feature_order,
        None
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_reorders_and_moves_feature_workflow() {
    let temp_root = unique_temp_dir("work-item-feature-workflow");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Goal A", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Goal B", Some("GOAL2"))
        .unwrap();
    service
        .create_goal_summary("Goal C", Some("GOAL3"))
        .unwrap();
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL3").unwrap();
    for goal_id in ["GOAL1", "GOAL2", "GOAL3"] {
        service.order_goal_in_feature("FEA1", goal_id).unwrap();
    }

    let reordered = service.reorder_goal_in_feature("FEA1", "GOAL3", 1).unwrap();
    assert_eq!(reordered.goal_ids, vec!["GOAL3", "GOAL1", "GOAL2"]);
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    let moved = service
        .move_feature_workflow("FEA1", GoalStatus::Backlog)
        .unwrap();
    assert_eq!(moved.rollup.status, GoalStatus::Backlog);
    assert_eq!(
        service.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Backlog
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_exposes_failed_feature_blocking_notice_on_goal_detail() {
    let temp_root = unique_temp_dir("work-item-feature-blocking-notice");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Goal A", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Goal B", Some("GOAL2"))
        .unwrap();
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL1").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL2").unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();

    let detail = service.show_goal_detail("GOAL1").unwrap();
    let notice = &detail["feature_blocking_notice"];
    assert_eq!(notice["feature_id"], "FEA1");
    assert_eq!(notice["blocking_goal_id"], "GOAL1");
    assert_eq!(notice["blocked_count"], 1);
    assert_eq!(notice["blocked_goal_ids"], json!(["GOAL2"]));
    assert!(
        notice["message"]
            .as_str()
            .unwrap_or("")
            .contains("blocking the next Goal")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_cancels_and_deletes_features_through_goal_paths() {
    let temp_root = unique_temp_dir("work-item-feature-cancel-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GOAL1", "Backlog Goal"),
        ("GOAL2", "Todo Goal"),
        ("GOAL3", "Done Goal"),
    ] {
        service.create_goal_summary(name, Some(id)).unwrap();
    }
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    for goal_id in ["GOAL1", "GOAL2", "GOAL3"] {
        service.assign_goal_to_feature("FEA1", goal_id).unwrap();
    }
    service
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    service
        .set_goal_status_unchecked("GOAL3", &GoalStatus::Done)
        .unwrap();

    let cancelled = service.cancel_feature_summary("FEA1").unwrap();
    assert_eq!(cancelled.rollup.cancelled_count, 2);
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Cancelled
    );
    assert_eq!(
        service.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Cancelled
    );
    assert_eq!(
        service.show_goal_summary("GOAL3").unwrap().goal.status,
        GoalStatus::Done
    );

    service.delete_feature_record("FEA1").unwrap();
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL2/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL3/goal.json").exists());

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
fn file_work_item_service_bulk_updates_deletes_and_assigns_goals() {
    let temp_root = unique_temp_dir("work-item-bulk");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GOAL1", "Bulk one"),
        ("GOAL2", "Bulk two"),
        ("GOAL3", "Skip me"),
    ] {
        service.create_goal_summary(name, Some(id)).unwrap();
        service
            .append_goal_round_summary(id, "Original", "Prompt")
            .unwrap();
    }
    service
        .set_goal_status_unchecked("GOAL3", &GoalStatus::Qa)
        .unwrap();

    let status_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec![
                    "GOAL1".to_string(),
                    "GOAL2".to_string(),
                    "GOAL3".to_string(),
                ]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("todo".to_string()),
        )
        .unwrap();
    assert_eq!(status_result.updated, 2);
    assert_eq!(status_result.skipped, 1);
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert_eq!(
        service.show_goal_summary("GOAL3").unwrap().goal.status,
        GoalStatus::Qa
    );

    let reporter_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Reporter("Reviewer".to_string()),
        )
        .unwrap();
    assert_eq!(reporter_result.updated, 2);
    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));

    let assignee_result = service
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Assignee("Assignee".to_string()),
        )
        .unwrap();
    assert_eq!(assignee_result.updated, 2);
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .assignee
            .as_deref(),
        Some("Assignee")
    );

    service
        .create_feature_summary("Bulk Feature", Some("FEA1"), None, None, None)
        .unwrap();
    let feature_assignee_result = service
        .bulk_update_features(
            BulkFeatureSelection {
                selected_ids: Some(vec!["FEA1".to_string()]),
                ..Default::default()
            },
            BulkFeatureUpdate::Assignee("Feature Reviewer".to_string()),
        )
        .unwrap();
    assert_eq!(feature_assignee_result.updated, 1);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .assignee
            .as_deref(),
        Some("Feature Reviewer")
    );
    let feature_reporter_result = service
        .bulk_update_features(
            BulkFeatureSelection {
                selected_ids: Some(vec!["FEA1".to_string()]),
                ..Default::default()
            },
            BulkFeatureUpdate::Reporter("Feature Reporter".to_string()),
        )
        .unwrap();
    assert_eq!(feature_reporter_result.updated, 1);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .reporter
            .as_deref(),
        Some("Feature Reporter")
    );
    let assign_result = service
        .bulk_assign_goals_to_feature(
            "FEA1",
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(assign_result.updated, 2);
    assert_eq!(
        service.show_feature_summary("FEA1").unwrap().goal_ids,
        vec!["GOAL1", "GOAL2"]
    );

    let delete_result = service
        .bulk_delete_goals(BulkGoalSelection {
            selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(delete_result.deleted, 2);
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL2/goal.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_bulk_deletes_features() {
    let temp_root = unique_temp_dir("work-item-feature-bulk-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Bulk Feature", Some("FEA1"), None, None, None)
        .unwrap();
    service.create_goal_summary("First", Some("GOAL1")).unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();

    let deleted = service
        .bulk_delete_features(BulkFeatureSelection {
            selected_ids: Some(vec!["FEA1".to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(deleted.deleted, 1);
    assert_eq!(deleted.ids, vec!["FEA1"]);
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_uses_active_node_and_rejects_foreign_mutations() {
    let temp_root = unique_temp_dir("work-item-node-ownership");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    nodes.activate("remote-node").unwrap();

    let service = FileWorkItemService::new(&refine_dir);
    let local_goal = service
        .create_goal_summary("Remote-owned", Some("GOAL1"))
        .unwrap();
    assert_eq!(local_goal.goal.node_id.as_deref(), Some("remote-node"));
    let local_feature = service
        .create_feature_summary("Remote feature", Some("FEA1"), None, None, None)
        .unwrap();
    assert_eq!(
        local_feature.feature.node_id.as_deref(),
        Some("remote-node")
    );

    nodes.activate("default").unwrap();
    let err = service
        .update_goal_metadata_summary("GOAL1", Some("Blocked"), None, None, None)
        .unwrap_err();
    assert_eq!(
        err.category(),
        crate::process::supervisor::errors::ErrorCategory::Conflict
    );
    let err = service
        .update_feature_metadata_summary("FEA1", Some("Blocked"), None, None, None)
        .unwrap_err();
    assert_eq!(
        err.category(),
        crate::process::supervisor::errors::ErrorCategory::Conflict
    );

    service
        .bulk_transfer_goals_to_node(
            "default",
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    let updated = service
        .update_goal_metadata_summary("GOAL1", Some("Default-owned"), None, None, None)
        .unwrap();
    assert_eq!(updated.goal.name, "Default-owned");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_transfers_features_as_node_owned_units() {
    let temp_root = unique_temp_dir("work-item-feature-transfer");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.create_goal_summary("First", Some("GOAL1")).unwrap();
    service
        .create_goal_summary("Second", Some("GOAL2"))
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL2").unwrap();

    let direct_goal = service
        .transfer_goal_to_node("remote-node", "GOAL1")
        .unwrap_err();
    assert!(
        direct_goal
            .to_string()
            .contains("transfer the Feature instead"),
        "{direct_goal}"
    );
    let bulk = service
        .bulk_transfer_goals_to_node(
            "remote-node",
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string(), "GOAL2".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(bulk.updated, 0);
    assert_eq!(bulk.skipped, 2);
    assert_eq!(bulk.skipped_details[0].reason, "feature:FEA1");

    let bulk_feature = service
        .bulk_transfer_features_to_node(
            "remote-node",
            BulkFeatureSelection {
                selected_ids: Some(vec!["FEA1".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(bulk_feature.updated, 3);
    assert_eq!(bulk_feature.ids, vec!["FEA1", "GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .node_id
            .as_deref(),
        Some("remote-node")
    );

    service.transfer_feature_to_node("default", "FEA1").unwrap();

    let transferred = service
        .transfer_feature_to_node("remote-node", "FEA1")
        .unwrap();
    assert_eq!(transferred.updated, 3);
    assert_eq!(transferred.ids, vec!["FEA1", "GOAL1", "GOAL2"]);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .node_id
            .as_deref(),
        Some("remote-node")
    );
    for goal_id in ["GOAL1", "GOAL2"] {
        assert_eq!(
            service
                .show_goal_summary(goal_id)
                .unwrap()
                .goal
                .node_id
                .as_deref(),
            Some("remote-node")
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_rejects_feature_transfer_with_active_member_goal() {
    let temp_root = unique_temp_dir("work-item-feature-transfer-active");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service
        .create_goal_summary("Active", Some("GOAL1"))
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();

    let err = service
        .transfer_feature_to_node("remote-node", "FEA1")
        .unwrap_err();
    assert!(err.to_string().contains("status:in-progress"), "{err}");
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .node_id
            .as_deref(),
        Some("default")
    );
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .node_id
            .as_deref(),
        Some("default")
    );

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
        .transition_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap_err();
    assert_eq!(
        err.category(),
        crate::process::supervisor::errors::ErrorCategory::InvalidInput
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_spreads_eligible_goals_evenly_across_nodes() {
    let temp_root = unique_temp_dir("distribute-spread");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("node-a").unwrap();
    nodes.create("node-b").unwrap();
    for index in 1..=6 {
        service
            .create_goal_summary(&format!("Goal {index}"), Some(&format!("GOAL{index}")))
            .unwrap();
    }

    let targets = vec![
        "default".to_string(),
        "node-a".to_string(),
        "node-b".to_string(),
    ];
    let result = service
        .distribute_goals_across_nodes(&targets, false, &std::collections::BTreeSet::new(), false)
        .unwrap();

    assert_eq!(result.strategy, "spread");
    assert_eq!(result.eligible, 6);
    let mut counts = std::collections::BTreeMap::new();
    for goal in service.list_goal_summaries().unwrap() {
        let owner = goal.goal.node_id.unwrap_or_else(|| "default".to_string());
        *counts.entry(owner).or_insert(0usize) += 1;
    }
    assert_eq!(counts.get("default"), Some(&2));
    assert_eq!(counts.get("node-a"), Some(&2));
    assert_eq!(counts.get("node-b"), Some(&2));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_converge_moves_only_reviewable_goals_to_review_node() {
    let temp_root = unique_temp_dir("distribute-converge");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("worker").unwrap();
    service
        .create_goal_summary("Reviewable", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Still backlog", Some("GOAL2"))
        .unwrap();
    service.transfer_goal_to_node("worker", "GOAL1").unwrap();
    service.transfer_goal_to_node("worker", "GOAL2").unwrap();
    // Review is a workflow-owned state; write it directly for the fixture.
    let goal_path = refine_dir.join("goals/GO/AL1/goal.json");
    let updated = fs::read_to_string(&goal_path)
        .unwrap()
        .replace("\"backlog\"", "\"review\"");
    fs::write(&goal_path, updated).unwrap();

    let targets = vec!["default".to_string()];
    let result = service
        .distribute_goals_across_nodes(&targets, true, &std::collections::BTreeSet::new(), false)
        .unwrap();

    assert_eq!(result.strategy, "converge");
    assert_eq!(result.moved, 1);
    assert_eq!(result.moves[0].goal_id, "GOAL1");
    assert_eq!(result.moves[0].to_node_id, "default");
    let backlog_goal = service.show_goal_summary("GOAL2").unwrap();
    assert_eq!(backlog_goal.goal.node_id.as_deref(), Some("worker"));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_skips_feature_and_claimed_goals_and_honors_dry_run() {
    let temp_root = unique_temp_dir("distribute-skips");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("node-a").unwrap();
    service
        .create_goal_summary("In feature", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Claimed", Some("GOAL2"))
        .unwrap();
    service.create_goal_summary("Free", Some("GOAL3")).unwrap();
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();

    let mut claimed = std::collections::BTreeSet::new();
    claimed.insert("GOAL2".to_string());
    let targets = vec!["node-a".to_string()];
    let result = service
        .distribute_goals_across_nodes(&targets, false, &claimed, true)
        .unwrap();

    assert_eq!(result.strategy, "fill");
    assert!(result.dry_run);
    assert_eq!(result.moved, 1);
    assert_eq!(result.moves[0].goal_id, "GOAL3");
    assert_eq!(result.skipped, 2);
    let reasons: Vec<&str> = result
        .skipped_details
        .iter()
        .map(|detail| detail.reason.as_str())
        .collect();
    assert!(reasons.contains(&"feature:FEA1"));
    assert!(reasons.contains(&"claimed"));
    let free_goal = service.show_goal_summary("GOAL3").unwrap();
    assert_eq!(
        free_goal
            .goal
            .node_id
            .unwrap_or_else(|| "default".to_string()),
        "default"
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_goal_authoring_centralizes_create_review_edit_and_placement() {
    let temp_root = unique_temp_dir("feature-goal-authoring");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    service
        .create_goal_summary("Foundation", Some("GOAL1"))
        .unwrap();
    service.assign_goal_to_feature("FEA1", "GOAL1").unwrap();
    service.order_goal_in_feature("FEA1", "GOAL1").unwrap();

    let created = service
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                prompt: "Implement the shared Feature Goal operation".to_string(),
                reporter: "Buddy".to_string(),
                priority: "high".to_string(),
                placement: FeatureGoalPlacement::After("GOAL1".to_string()),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap();
    assert!(created.created);
    let goal = created.goal.unwrap();
    assert_eq!(goal.name, "Implement the shared Feature Goal operation");
    assert_eq!(goal.priority, GoalPriority::High);
    assert_eq!(goal.feature_order, Some(2));
    let goal_id = goal.id;
    assert_eq!(
        service.show_goal_detail(&goal_id).unwrap()["rounds"][0]["prompt"],
        "Implement the shared Feature Goal operation"
    );

    let goal_path = refine_dir
        .join("goals")
        .join(&goal_id[..2])
        .join(&goal_id[2..])
        .join("goal.json");
    let mut review_goal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    review_goal["status"] = json!("review");
    fs::write(&goal_path, serde_json::to_vec_pretty(&review_goal).unwrap()).unwrap();
    let review_summary = service.show_goal_summary(&goal_id).unwrap();
    assert!(FileWorkItemService::feature_goal_authoring_capability(&review_summary).editable);

    let edited = service
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                goal_id: Some(goal_id.clone()),
                name: Some("Reviewed authoring".to_string()),
                prompt: "Revise the prompt while review is active".to_string(),
                reporter: "Buddy".to_string(),
                priority: "medium".to_string(),
                placement: FeatureGoalPlacement::First,
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap();
    assert!(!edited.created);
    let edited = edited.goal.unwrap();
    assert_eq!(edited.status, GoalStatus::Review);
    assert_eq!(edited.name, "Reviewed authoring");
    assert_eq!(edited.feature_order, Some(1));
    assert_eq!(
        service
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .feature_order,
        Some(2)
    );
    assert_eq!(
        service.show_goal_detail(&goal_id).unwrap()["rounds"][0]["prompt"],
        "Revise the prompt while review is active"
    );

    let mut done_goal: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    done_goal["status"] = json!("done");
    fs::write(&goal_path, serde_json::to_vec_pretty(&done_goal).unwrap()).unwrap();
    let done_summary = service.show_goal_summary(&goal_id).unwrap();
    assert!(!FileWorkItemService::feature_goal_authoring_capability(&done_summary).editable);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_goal_authoring_reports_duplicates_and_validates_before_writes() {
    let temp_root = unique_temp_dir("feature-goal-authoring-validation");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    let request = FeatureGoalAuthoringRequest {
        prompt: "Same prompt".to_string(),
        reporter: "Buddy".to_string(),
        priority: "low".to_string(),
        ..FeatureGoalAuthoringRequest::default()
    };
    service
        .author_feature_goal("FEA1", request.clone())
        .unwrap();
    let duplicate = service.author_feature_goal("FEA1", request).unwrap();
    assert!(duplicate.requires_duplicate_decision);
    assert_eq!(duplicate.duplicate.unwrap().prompt, "Same prompt");
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);

    let invalid = service.author_feature_goal(
        "FEA1",
        FeatureGoalAuthoringRequest {
            prompt: "A different prompt".to_string(),
            reporter: "Buddy".to_string(),
            priority: "low".to_string(),
            placement: FeatureGoalPlacement::After("MISSING".to_string()),
            ..FeatureGoalAuthoringRequest::default()
        },
    );
    assert!(invalid.is_err());
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_goal_authoring_keeps_ordinary_and_feature_creation_in_parity() {
    let ordinary_root = unique_temp_dir("ordinary-goal-authoring-parity");
    let feature_root = unique_temp_dir("feature-goal-authoring-parity");
    let ordinary = FileWorkItemService::new(ordinary_root.join(".refine"));
    let feature = FileWorkItemService::new(feature_root.join(".refine"));
    feature
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    feature
        .create_goal_summary("Foundation", Some("FOUNDATION"))
        .unwrap();
    feature
        .assign_goal_to_feature("FEA1", "FOUNDATION")
        .unwrap();
    feature.order_goal_in_feature("FEA1", "FOUNDATION").unwrap();

    let generated_prompt =
        "  Build   one shared Goal authoring capability with deterministic names.  ";
    let ordinary_generated = ordinary
        .author_goal(GoalAuthoringRequest {
            prompt: generated_prompt.to_string(),
            reporter: "Buddy".to_string(),
            assignee: Some("Alice".to_string()),
            priority: "high".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap()
        .goal
        .unwrap();
    let feature_generated = feature
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                prompt: generated_prompt.to_string(),
                reporter: "Buddy".to_string(),
                assignee: Some("Alice".to_string()),
                priority: "high".to_string(),
                placement: FeatureGoalPlacement::After("FOUNDATION".to_string()),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap()
        .goal
        .unwrap();
    assert_eq!(ordinary_generated.name, feature_generated.name);
    assert_eq!(ordinary_generated.priority, feature_generated.priority);
    assert_eq!(ordinary_generated.reporter, feature_generated.reporter);
    assert_eq!(ordinary_generated.assignee, feature_generated.assignee);
    assert_eq!(feature_generated.feature_order, Some(2));
    for (service, goal_id) in [
        (&ordinary, ordinary_generated.id.as_str()),
        (&feature, feature_generated.id.as_str()),
    ] {
        let detail = service.show_goal_detail(goal_id).unwrap();
        assert_eq!(detail["rounds"][0]["reporter"], "Buddy");
        assert_eq!(detail["rounds"][0]["assignee"], "Alice");
        assert_eq!(detail["rounds"][0]["prompt"], generated_prompt.trim());
    }

    let ordinary_explicit = ordinary
        .author_goal(GoalAuthoringRequest {
            name: Some("Explicit shared name".to_string()),
            prompt: "Ordinary explicit prompt".to_string(),
            reporter: "Buddy".to_string(),
            priority: "low".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap()
        .goal
        .unwrap();
    let feature_explicit = feature
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                name: Some("Explicit shared name".to_string()),
                prompt: "Feature explicit prompt".to_string(),
                reporter: "Buddy".to_string(),
                priority: "low".to_string(),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap()
        .goal
        .unwrap();
    assert_eq!(ordinary_explicit.name, "Explicit shared name");
    assert_eq!(feature_explicit.name, ordinary_explicit.name);

    let ordinary_before = ordinary.list_goal_summaries().unwrap().len();
    let feature_before = feature.list_goal_summaries().unwrap().len();
    let ordinary_invalid = ordinary
        .author_goal(GoalAuthoringRequest {
            name: Some("Invalid".to_string()),
            reporter: "Bad\nReporter".to_string(),
            priority: "low".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap_err();
    let feature_invalid = feature
        .author_feature_goal(
            "FEA1",
            FeatureGoalAuthoringRequest {
                name: Some("Invalid".to_string()),
                prompt: "Invalid reporter".to_string(),
                reporter: "Bad\nReporter".to_string(),
                priority: "low".to_string(),
                ..FeatureGoalAuthoringRequest::default()
            },
        )
        .unwrap_err();
    assert_eq!(ordinary_invalid.to_string(), feature_invalid.to_string());
    assert_eq!(
        ordinary.list_goal_summaries().unwrap().len(),
        ordinary_before
    );
    assert_eq!(feature.list_goal_summaries().unwrap().len(), feature_before);

    fs::remove_dir_all(ordinary_root).unwrap();
    fs::remove_dir_all(feature_root).unwrap();
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

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
