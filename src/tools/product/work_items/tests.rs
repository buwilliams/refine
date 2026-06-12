use crate::model::gap::GapPriority;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

use super::*;
use crate::model::workflow::GapStatus;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn file_work_item_service_transitions_gap_via_refine_json() {
    let temp_root = unique_temp_dir("work-item-transition");
    let refine_dir = temp_root.join(".refine");
    let gap_dir = refine_dir.join("gaps").join("01").join("GAP1");
    fs::create_dir_all(&gap_dir).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
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
        FileWorkItemService::new(&refine_dir).transition_gap_status("GAP1", GapStatus::Todo);
    assert_eq!(updated.unwrap().gap.status, GapStatus::Todo);
    let written = fs::read_to_string(gap_dir.join("gap.json")).unwrap();
    assert!(written.contains("\"status\": \"todo\""));
    assert!(written.contains("\"updated\": \"20"));
    assert!(written.contains("Z\""));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_creates_and_lists_gap_json() {
    let temp_root = unique_temp_dir("work-item-create");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);

    let gap = service
        .create_gap_summary("Created from Rust", Some("GAP1"))
        .unwrap();
    assert_eq!(gap.gap.id, "GAP1");
    assert_eq!(gap.gap.status, GapStatus::Backlog);
    assert!(refine_dir.join("gaps/GA/P1/gap.json").exists());
    assert_eq!(service.list_gap_summaries().unwrap().len(), 1);
    assert_eq!(
        service.show_gap_summary("GAP1").unwrap().gap.name,
        "Created from Rust"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_edits_notes_and_deletes_gap_json() {
    let temp_root = unique_temp_dir("work-item-edit-note-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_gap_summary("Original", Some("GAP1"))
        .unwrap();

    let edited = service
        .update_gap_metadata_summary(
            "GAP1",
            Some("Renamed"),
            Some("high"),
            Some("Reporter"),
            None,
        )
        .unwrap();
    assert_eq!(edited.gap.name, "Renamed");
    assert_eq!(edited.gap.priority, GapPriority::High);
    assert_eq!(edited.gap.reporter.as_deref(), Some("Reporter"));

    service
        .add_gap_note_summary("GAP1", "Reviewer", "Needs a note")
        .unwrap();
    let written = fs::read_to_string(refine_dir.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"author\": \"Reviewer\""));
    assert!(written.contains("\"body\": \"Needs a note\""));

    service.delete_gap_record("GAP1").unwrap();
    assert!(!refine_dir.join("gaps/GA/P1/gap.json").exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_appends_and_edits_latest_round() {
    let temp_root = unique_temp_dir("work-item-rounds");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_gap_summary("Round Gap", Some("GAP1"))
        .unwrap();

    let gap = service
        .append_gap_round_summary("GAP1", "Reporter", "Actual", "Target")
        .unwrap();
    assert_eq!(gap.gap.round_count, 1);
    let gap = service
        .edit_latest_gap_round_summary(
            "GAP1",
            Some("Reviewer"),
            Some("Reviewer"),
            Some("New actual"),
            None,
        )
        .unwrap();
    assert_eq!(gap.gap.reporter.as_deref(), Some("Reviewer"));
    assert_eq!(gap.gap.assignee.as_deref(), Some("Reviewer"));
    let written = fs::read_to_string(refine_dir.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));
    assert!(written.contains("\"assignee\": \"Reviewer\""));
    assert!(written.contains("\"actual\": \"New actual\""));
    assert!(written.contains("\"target\": \"Target\""));
    assert!(written.contains("\"rule_state\": \"unclassified\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_creates_features_and_updates_gap_membership() {
    let temp_root = unique_temp_dir("work-item-feature");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service.create_gap_summary("Gap A", Some("GAP1")).unwrap();
    service.create_gap_summary("Gap B", Some("GAP2")).unwrap();

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

    let feature = service.assign_gap_to_feature("FEA1", "GAP1").unwrap();
    assert_eq!(feature.gap_ids, vec!["GAP1"]);
    let feature = service.assign_gap_to_feature("FEA1", "GAP2").unwrap();
    assert_eq!(feature.gap_ids, vec!["GAP1", "GAP2"]);
    assert_eq!(
        service.show_gap_summary("GAP2").unwrap().gap.feature_order,
        Some(2)
    );

    let feature = service.unorder_gap_in_feature("FEA1", "GAP1").unwrap();
    assert_eq!(feature.gap_ids, vec!["GAP2", "GAP1"]);
    assert_eq!(
        service.show_gap_summary("GAP1").unwrap().gap.feature_order,
        None
    );
    assert_eq!(
        service.show_gap_summary("GAP2").unwrap().gap.feature_order,
        Some(1)
    );

    let feature = service.order_gap_in_feature("FEA1", "GAP1").unwrap();
    assert_eq!(feature.gap_ids, vec!["GAP2", "GAP1"]);
    assert_eq!(
        service.show_gap_summary("GAP1").unwrap().gap.feature_order,
        Some(2)
    );

    let feature = service.remove_gap_from_feature("FEA1", "GAP1").unwrap();
    assert_eq!(feature.gap_ids, vec!["GAP2"]);
    assert_eq!(
        service.show_gap_summary("GAP2").unwrap().gap.feature_order,
        Some(1)
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_reorders_and_moves_feature_workflow() {
    let temp_root = unique_temp_dir("work-item-feature-workflow");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service.create_gap_summary("Gap A", Some("GAP1")).unwrap();
    service.create_gap_summary("Gap B", Some("GAP2")).unwrap();
    service.create_gap_summary("Gap C", Some("GAP3")).unwrap();
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_gap_to_feature("FEA1", "GAP1").unwrap();
    service.assign_gap_to_feature("FEA1", "GAP2").unwrap();
    service.assign_gap_to_feature("FEA1", "GAP3").unwrap();

    let reordered = service.reorder_gap_in_feature("FEA1", "GAP3", 1).unwrap();
    assert_eq!(reordered.gap_ids, vec!["GAP3", "GAP1", "GAP2"]);
    service
        .transition_gap_status("GAP2", GapStatus::Todo)
        .unwrap();
    let moved = service
        .move_feature_workflow("FEA1", GapStatus::Backlog)
        .unwrap();
    assert_eq!(moved.rollup.status, GapStatus::Backlog);
    assert_eq!(
        service.show_gap_summary("GAP2").unwrap().gap.status,
        GapStatus::Backlog
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_exposes_failed_feature_blocking_notice_on_gap_detail() {
    let temp_root = unique_temp_dir("work-item-feature-blocking-notice");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service.create_gap_summary("Gap A", Some("GAP1")).unwrap();
    service.create_gap_summary("Gap B", Some("GAP2")).unwrap();
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.assign_gap_to_feature("FEA1", "GAP1").unwrap();
    service.assign_gap_to_feature("FEA1", "GAP2").unwrap();
    service
        .transition_gap_status("GAP1", GapStatus::Todo)
        .unwrap();
    service
        .advance_automated_gap_status("GAP1", GapStatus::InProgress)
        .unwrap();
    service
        .advance_automated_gap_status("GAP1", GapStatus::Failed)
        .unwrap();
    service
        .transition_gap_status("GAP2", GapStatus::Todo)
        .unwrap();

    let detail = service.show_gap_detail("GAP1").unwrap();
    let notice = &detail["feature_blocking_notice"];
    assert_eq!(notice["feature_id"], "FEA1");
    assert_eq!(notice["blocking_gap_id"], "GAP1");
    assert_eq!(notice["blocked_count"], 1);
    assert_eq!(notice["blocked_gap_ids"], json!(["GAP2"]));
    assert!(
        notice["message"]
            .as_str()
            .unwrap_or("")
            .contains("blocking the next Gap")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_cancels_and_deletes_features_through_gap_paths() {
    let temp_root = unique_temp_dir("work-item-feature-cancel-delete");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GAP1", "Backlog Gap"),
        ("GAP2", "Todo Gap"),
        ("GAP3", "Done Gap"),
    ] {
        service.create_gap_summary(name, Some(id)).unwrap();
    }
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    for gap_id in ["GAP1", "GAP2", "GAP3"] {
        service.assign_gap_to_feature("FEA1", gap_id).unwrap();
    }
    service
        .transition_gap_status("GAP2", GapStatus::Todo)
        .unwrap();
    service
        .set_gap_status_unchecked("GAP3", &GapStatus::Done)
        .unwrap();

    let cancelled = service.cancel_feature_summary("FEA1").unwrap();
    assert_eq!(cancelled.rollup.cancelled_count, 2);
    assert_eq!(
        service.show_gap_summary("GAP1").unwrap().gap.status,
        GapStatus::Cancelled
    );
    assert_eq!(
        service.show_gap_summary("GAP2").unwrap().gap.status,
        GapStatus::Cancelled
    );
    assert_eq!(
        service.show_gap_summary("GAP3").unwrap().gap.status,
        GapStatus::Done
    );

    service.delete_feature_record("FEA1").unwrap();
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("gaps/GA/P1/gap.json").exists());
    assert!(!refine_dir.join("gaps/GA/P2/gap.json").exists());
    assert!(!refine_dir.join("gaps/GA/P3/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_merges_and_undoes_gap_workflow() {
    let temp_root = unique_temp_dir("work-item-merge-undo");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_gap_summary("Merge Gap", Some("GAP1"))
        .unwrap();
    service
        .set_gap_status_unchecked("GAP1", &GapStatus::ReadyMerge)
        .unwrap();

    let merged = service.merge_gap_summary("GAP1").unwrap();
    assert_eq!(merged.gap.status, GapStatus::Done);

    let undone = service.undo_gap_summary("GAP1").unwrap();
    assert_eq!(undone.gap.status, GapStatus::Review);
    let undone = service.undo_gap_summary("GAP1").unwrap();
    assert_eq!(undone.gap.status, GapStatus::Todo);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_bulk_updates_deletes_and_assigns_gaps() {
    let temp_root = unique_temp_dir("work-item-bulk");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    for (id, name) in [
        ("GAP1", "Bulk one"),
        ("GAP2", "Bulk two"),
        ("GAP3", "Skip me"),
    ] {
        service.create_gap_summary(name, Some(id)).unwrap();
        service
            .append_gap_round_summary(id, "Original", "Actual", "Target")
            .unwrap();
    }
    service
        .set_gap_status_unchecked("GAP3", &GapStatus::Qa)
        .unwrap();

    let status_result = service
        .bulk_update_gaps(
            BulkGapSelection {
                selected_ids: Some(vec![
                    "GAP1".to_string(),
                    "GAP2".to_string(),
                    "GAP3".to_string(),
                ]),
                ..Default::default()
            },
            BulkGapUpdate::Status("todo".to_string()),
        )
        .unwrap();
    assert_eq!(status_result.updated, 2);
    assert_eq!(status_result.skipped, 1);
    assert_eq!(
        service.show_gap_summary("GAP1").unwrap().gap.status,
        GapStatus::Todo
    );
    assert_eq!(
        service.show_gap_summary("GAP3").unwrap().gap.status,
        GapStatus::Qa
    );

    let reporter_result = service
        .bulk_update_gaps(
            BulkGapSelection {
                selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
                ..Default::default()
            },
            BulkGapUpdate::Reporter("Reviewer".to_string()),
        )
        .unwrap();
    assert_eq!(reporter_result.updated, 2);
    let written = fs::read_to_string(refine_dir.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));

    let assignee_result = service
        .bulk_update_gaps(
            BulkGapSelection {
                selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
                ..Default::default()
            },
            BulkGapUpdate::Assignee("Assignee".to_string()),
        )
        .unwrap();
    assert_eq!(assignee_result.updated, 2);
    assert_eq!(
        service
            .show_gap_summary("GAP1")
            .unwrap()
            .gap
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
            "Feature Reviewer",
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
    let assign_result = service
        .bulk_assign_gaps_to_feature(
            "FEA1",
            BulkGapSelection {
                selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(assign_result.updated, 2);
    assert_eq!(
        service.show_feature_summary("FEA1").unwrap().gap_ids,
        vec!["GAP1", "GAP2"]
    );

    let delete_result = service
        .bulk_delete_gaps(BulkGapSelection {
            selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(delete_result.deleted, 2);
    assert!(!refine_dir.join("gaps/GA/P1/gap.json").exists());
    assert!(!refine_dir.join("gaps/GA/P2/gap.json").exists());

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
    let local_gap = service
        .create_gap_summary("Remote-owned", Some("GAP1"))
        .unwrap();
    assert_eq!(local_gap.gap.node_id.as_deref(), Some("remote-node"));
    let local_feature = service
        .create_feature_summary("Remote feature", Some("FEA1"), None, None, None)
        .unwrap();
    assert_eq!(
        local_feature.feature.node_id.as_deref(),
        Some("remote-node")
    );

    nodes.activate("default").unwrap();
    let err = service
        .update_gap_metadata_summary("GAP1", Some("Blocked"), None, None, None)
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
        .bulk_transfer_gaps_to_node(
            "default",
            BulkGapSelection {
                selected_ids: Some(vec!["GAP1".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    let updated = service
        .update_gap_metadata_summary("GAP1", Some("Default-owned"), None, None, None)
        .unwrap();
    assert_eq!(updated.gap.name, "Default-owned");

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
    service.create_gap_summary("First", Some("GAP1")).unwrap();
    service.create_gap_summary("Second", Some("GAP2")).unwrap();
    service.assign_gap_to_feature("FEA1", "GAP1").unwrap();
    service.assign_gap_to_feature("FEA1", "GAP2").unwrap();

    let direct_gap = service
        .transfer_gap_to_node("remote-node", "GAP1")
        .unwrap_err();
    assert!(
        direct_gap
            .to_string()
            .contains("transfer the Feature instead"),
        "{direct_gap}"
    );
    let bulk = service
        .bulk_transfer_gaps_to_node(
            "remote-node",
            BulkGapSelection {
                selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(bulk.updated, 0);
    assert_eq!(bulk.skipped, 2);
    assert_eq!(bulk.skipped_details[0].reason, "feature:FEA1");

    let transferred = service
        .transfer_feature_to_node("remote-node", "FEA1")
        .unwrap();
    assert_eq!(transferred.updated, 3);
    assert_eq!(transferred.ids, vec!["FEA1", "GAP1", "GAP2"]);
    assert_eq!(
        service
            .show_feature_summary("FEA1")
            .unwrap()
            .feature
            .node_id
            .as_deref(),
        Some("remote-node")
    );
    for gap_id in ["GAP1", "GAP2"] {
        assert_eq!(
            service
                .show_gap_summary(gap_id)
                .unwrap()
                .gap
                .node_id
                .as_deref(),
            Some("remote-node")
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_rejects_feature_transfer_with_active_member_gap() {
    let temp_root = unique_temp_dir("work-item-feature-transfer-active");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature A", Some("FEA1"), None, None, None)
        .unwrap();
    service.create_gap_summary("Active", Some("GAP1")).unwrap();
    service.assign_gap_to_feature("FEA1", "GAP1").unwrap();
    service
        .transition_gap_status("GAP1", GapStatus::Todo)
        .unwrap();
    service
        .advance_automated_gap_status("GAP1", GapStatus::InProgress)
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
            .show_gap_summary("GAP1")
            .unwrap()
            .gap
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
    let gap_dir = refine_dir.join("gaps").join("01").join("GAP1");
    fs::create_dir_all(&gap_dir).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "Transition me",
              "status": "backlog",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();

    let err = FileWorkItemService::new(&refine_dir)
        .transition_gap_status("GAP1", GapStatus::InProgress)
        .unwrap_err();
    assert_eq!(
        err.category(),
        crate::process::supervisor::errors::ErrorCategory::InvalidInput
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
