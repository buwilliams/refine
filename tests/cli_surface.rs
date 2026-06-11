mod support;

use std::fs;

use support::integration::IntegrationFixture;

#[test]
#[ignore = "daemon-backed surface test; run through `cargo run --manifest-path xtask/Cargo.toml -- test-cli`"]
fn cli_surface_suite() {
    let fixture = IntegrationFixture::start("cli");

    system_status_reports_healthy_daemon(&fixture);
    project_status_is_attached_to_test_app(&fixture);
    project_doctor_runs(&fixture);
    project_registry_lifecycle_commands(&fixture);
    system_doctor_and_api_groups_run(&fixture);
    gap_create_list_show_edit_note_round_delete(&fixture);
    gap_feature_assignment_and_round_edit_latest(&fixture);
    gap_workflow_actions_start_retry_verify_merge_undo(&fixture);
    feature_create_membership_rollup_and_delete(&fixture);
    feature_show_edit_reorder_move_cancel_and_import(&fixture);
    node_create_activate_archive(&fixture);
    node_show_rename_settings_and_transfer(&fixture);
    cluster_local_registry_commands(&fixture);
    log_commands_query_public_activity(&fixture);
    agent_commands_use_smoke_ai(&fixture);
}

fn system_status_reports_healthy_daemon(fixture: &IntegrationFixture) {
    let port = fixture.port.to_string();
    let runtime_root = fixture.runtime_root.display().to_string();
    let output = fixture.run_refine(&[
        "system",
        "status",
        "--port",
        &port,
        "--runtime-root",
        &runtime_root,
    ]);
    fixture.assert_success("system status", &output);
    let payload = fixture.json_stdout(&output);
    assert!(
        payload["running_ports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_u64() == Some(fixture.port.into())),
        "{payload:#}"
    );
    let status = payload["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["port"].as_u64() == Some(fixture.port.into()))
        .expect("test daemon port should be listed");
    assert_eq!(status["daemon_healthy"], true);
    assert_eq!(status["web_available"], true);
}

fn project_status_is_attached_to_test_app(fixture: &IntegrationFixture) {
    let output = fixture.run_refine(&["project", "status"]);
    fixture.assert_success("project status", &output);
    let payload = fixture.json_stdout(&output);
    assert_eq!(payload["attached"], true, "{payload:#}");
    assert!(
        payload["client_repo"]
            .as_str()
            .unwrap_or_default()
            .ends_with("rust-test-app"),
        "{payload:#}"
    );
    assert_eq!(payload["schema"]["compatible"], true, "{payload:#}");
}

fn project_doctor_runs(fixture: &IntegrationFixture) {
    let output = fixture.run_refine(&["project", "doctor"]);
    fixture.assert_success("project doctor", &output);
    let payload = fixture.json_stdout(&output);
    assert!(payload.is_object() || payload.is_array(), "{payload:#}");
}

fn project_registry_lifecycle_commands(fixture: &IntegrationFixture) {
    let primary_app = fixture.app_root.display().to_string();
    let registered_app = fixture.create_git_app("rust-registered-app");
    let registered_app_path = registered_app.display().to_string();
    let clone_source = fixture.create_git_app("rust-clone-source");
    let clone_destination = fixture.app_workspace_root().join("rust-cloned-app");
    let _ = fs::remove_dir_all(&clone_destination);
    let clone_destination_path = clone_destination.display().to_string();

    let register = fixture.run_refine(&["project", "register", "registered", &registered_app_path]);
    fixture.assert_success("project register", &register);
    let register_payload = fixture.json_stdout(&register);
    assert_eq!(register_payload["ok"], true);
    assert!(project_apps(&register_payload).iter().any(|app| {
        app["name"].as_str() == Some("registered")
            && app["path"].as_str() == Some(registered_app_path.as_str())
    }));

    let switch = fixture.run_refine(&["project", "switch", "registered"]);
    fixture.assert_success("project switch", &switch);
    let switch_payload = fixture.json_stdout(&switch);
    assert_eq!(switch_payload["attached"], true);
    assert_eq!(switch_payload["client_repo"], registered_app_path);

    let detach = fixture.run_refine(&["project", "detach"]);
    fixture.assert_success("project detach", &detach);
    let detach_payload = fixture.json_stdout(&detach);
    assert_eq!(detach_payload["attached"], false);
    assert!(
        detach_payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("No refine project is attached")
    );

    let attach = fixture.run_refine(&["project", "attach", &primary_app]);
    fixture.assert_success("project attach primary", &attach);
    assert_eq!(fixture.json_stdout(&attach)["client_repo"], primary_app);

    let migrate = fixture.run_refine(&["project", "migrate"]);
    fixture.assert_success("project migrate", &migrate);
    let migrate_payload = fixture.json_stdout(&migrate);
    assert_eq!(migrate_payload["ok"], true);
    assert_eq!(migrate_payload["migrated"], false);

    let sync = fixture.run_refine(&["project", "sync"]);
    fixture.assert_success("project sync", &sync);
    assert!(fixture.json_stdout(&sync).is_object());

    let clone = fixture.run_refine(&[
        "project",
        "clone",
        clone_source.to_str().unwrap(),
        &clone_destination_path,
        "--name",
        "cloned",
        "--make-current",
    ]);
    fixture.assert_success("project clone", &clone);
    let clone_payload = fixture.json_stdout(&clone);
    assert_eq!(clone_payload["attached"], true);
    assert_eq!(clone_payload["client_repo"], clone_destination_path);

    let restore = fixture.run_refine(&["project", "switch", &primary_app]);
    fixture.assert_success("project switch primary", &restore);
    assert_eq!(fixture.json_stdout(&restore)["client_repo"], primary_app);

    let remove_registered = fixture.run_refine(&["project", "remove", "registered"]);
    fixture.assert_success("project remove registered", &remove_registered);
    assert!(
        !project_apps(&fixture.json_stdout(&remove_registered))
            .iter()
            .any(|app| app["name"].as_str() == Some("registered"))
    );

    let remove_cloned = fixture.run_refine(&["project", "remove", "cloned"]);
    fixture.assert_success("project remove cloned", &remove_cloned);
    assert!(
        !project_apps(&fixture.json_stdout(&remove_cloned))
            .iter()
            .any(|app| app["name"].as_str() == Some("cloned"))
    );
}

fn project_apps(payload: &serde_json::Value) -> Vec<serde_json::Value> {
    payload["apps"]
        .as_array()
        .cloned()
        .or_else(|| payload["apps"]["apps"].as_array().cloned())
        .unwrap_or_default()
}

fn system_doctor_and_api_groups_run(fixture: &IntegrationFixture) {
    let runtime_root = fixture.runtime_root.display().to_string();
    let repo_root = fixture.app_root.display().to_string();
    let doctor = fixture.run_refine(&[
        "system",
        "doctor",
        "--runtime-root",
        &runtime_root,
        "--repo-root",
        &repo_root,
    ]);
    fixture.assert_success("system doctor", &doctor);
    assert!(fixture.json_stdout(&doctor).is_object());

    let api_groups = fixture.run_refine(&["system", "api-groups"]);
    fixture.assert_success("system api-groups", &api_groups);
    let payload = fixture.json_stdout(&api_groups);
    assert!(
        payload
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["prefix"].as_str() == Some("/work"))
    );
}

fn gap_create_list_show_edit_note_round_delete(fixture: &IntegrationFixture) {
    let gap_id = fixture.create_gap("cli surface gap");
    assert_eq!(fixture.gap_field(&gap_id, "status"), "backlog");
    assert_eq!(fixture.gap_field(&gap_id, "priority"), "low");
    assert_eq!(fixture.gap_field(&gap_id, "name"), "cli surface gap");
    assert_eq!(fixture.gap_field(&gap_id, "node_id"), "default");

    let list = fixture.run_refine(&["gap", "list"]);
    fixture.assert_success("gap list", &list);
    let payload = fixture.json_stdout(&list);
    assert!(
        payload["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["id"].as_str() == Some(gap_id.as_str()))
    );

    let edit = fixture.run_refine(&[
        "gap",
        "edit",
        &gap_id,
        "--name",
        "cli surface renamed gap",
        "--priority",
        "high",
    ]);
    fixture.assert_success("gap edit", &edit);
    assert_eq!(
        fixture.gap_field(&gap_id, "name"),
        "cli surface renamed gap"
    );
    assert_eq!(fixture.gap_field(&gap_id, "priority"), "high");

    let note = fixture.run_refine(&[
        "gap",
        "note",
        &gap_id,
        "needs a closer look",
        "--author",
        "refine-smoke",
    ]);
    fixture.assert_success("gap note", &note);
    assert_eq!(fixture.json_stdout(&note)["gap"]["id"], gap_id);
    let shown_after_note = fixture.run_refine(&["gap", "show", &gap_id]);
    fixture.assert_success("gap show after note", &shown_after_note);
    let note_id = fixture.json_stdout(&shown_after_note)["gap"]["notes"][0]["id"]
        .as_str()
        .expect("gap show should expose note id")
        .to_string();
    let note_edit = fixture.run_refine(&[
        "gap",
        "note-edit",
        &gap_id,
        &note_id,
        "needs a closer look after edit",
    ]);
    fixture.assert_success("gap note-edit", &note_edit);
    let shown_after_note_edit = fixture.run_refine(&["gap", "show", &gap_id]);
    fixture.assert_success("gap show after note edit", &shown_after_note_edit);
    assert_eq!(
        fixture.json_stdout(&shown_after_note_edit)["gap"]["notes"][0]["body"],
        "needs a closer look after edit"
    );
    let note_delete = fixture.run_refine(&["gap", "note-delete", &gap_id, &note_id]);
    fixture.assert_success("gap note-delete", &note_delete);
    let shown_after_note_delete = fixture.run_refine(&["gap", "show", &gap_id]);
    fixture.assert_success("gap show after note delete", &shown_after_note_delete);
    assert_eq!(
        fixture.json_stdout(&shown_after_note_delete)["gap"]["notes"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    assert_eq!(fixture.gap_field(&gap_id, "round_count"), 0);
    let round = fixture.run_refine(&[
        "gap",
        "round",
        &gap_id,
        "--reporter",
        "refine-smoke",
        "--actual",
        "observed actual",
        "--target",
        "desired target",
    ]);
    fixture.assert_success("gap round", &round);
    assert_eq!(fixture.gap_field(&gap_id, "round_count"), 1);

    let delete = fixture.run_refine(&["gap", "delete", &gap_id]);
    fixture.assert_success("gap delete", &delete);
    let payload = fixture.json_stdout(&delete);
    assert_eq!(payload["deleted"], true);
    assert_eq!(payload["id"], gap_id);

    let after = fixture.run_refine(&["gap", "show", &gap_id]);
    assert!(!after.status.success());
    assert!(
        String::from_utf8_lossy(&after.stderr)
            .to_lowercase()
            .contains("not found")
    );
}

fn gap_feature_assignment_and_round_edit_latest(fixture: &IntegrationFixture) {
    let gap_id = fixture.create_gap("cli feature assignment gap");
    let feature = fixture.run_refine(&[
        "feature",
        "create",
        "cli assignment feature",
        "--description",
        "Feature used by the CLI assignment regression.",
        "--reporter",
        "refine-smoke",
    ]);
    fixture.assert_success("feature create assignment", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let assign = fixture.run_refine(&["gap", "assign-feature", &gap_id, &feature_id]);
    fixture.assert_success("gap assign-feature", &assign);
    assert_eq!(fixture.gap_field(&gap_id, "feature_id"), feature_id);

    let remove = fixture.run_refine(&["gap", "remove-feature", &gap_id]);
    fixture.assert_success("gap remove-feature", &remove);
    assert!(fixture.gap_field(&gap_id, "feature_id").is_null());

    let round = fixture.run_refine(&[
        "gap",
        "round",
        &gap_id,
        "--reporter",
        "refine-smoke",
        "--actual",
        "first actual",
        "--target",
        "first target",
    ]);
    fixture.assert_success("gap round assignment", &round);
    let edit = fixture.run_refine(&[
        "gap",
        "round",
        &gap_id,
        "--edit-latest",
        "--reporter",
        "refine-smoke",
        "--actual",
        "edited actual",
        "--target",
        "edited target",
    ]);
    fixture.assert_success("gap round edit latest", &edit);
    let shown = fixture.run_refine(&["gap", "show", &gap_id]);
    fixture.assert_success("gap show after round edit", &shown);
    let gap = fixture.json_stdout(&shown);
    assert_eq!(gap["gap"]["round_count"], 1);
    assert!(gap.to_string().contains("edited actual"), "{gap:#}");

    fixture.assert_success(
        "gap delete assignment",
        &fixture.run_refine(&["gap", "delete", &gap_id]),
    );
    fixture.assert_success(
        "feature delete assignment",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
    );
}

fn gap_workflow_actions_start_retry_verify_merge_undo(fixture: &IntegrationFixture) {
    let started_id = fixture.create_gap("gap action start");
    let started = fixture.run_refine(&["gap", "start", &started_id]);
    fixture.assert_success("gap start", &started);
    assert_eq!(
        fixture.json_stdout(&started)["gap"]["status"],
        "in-progress"
    );
    assert_eq!(fixture.gap_field(&started_id, "status"), "in-progress");
    fixture.assert_success(
        "gap cancel started",
        &fixture.run_refine(&["gap", "cancel", &started_id]),
    );
    fixture.assert_success(
        "gap undo started cancel",
        &fixture.run_refine(&["gap", "undo", &started_id]),
    );
    fixture.assert_success(
        "gap delete started",
        &fixture.run_refine(&["gap", "delete", &started_id]),
    );

    let quality_id = fixture.create_gap("gap action quality retry");
    seed_gap_status(fixture, &quality_id, "failed");
    let retried_quality = fixture.run_refine(&["gap", "retry", &quality_id, "--stage", "quality"]);
    fixture.assert_success("gap retry quality", &retried_quality);
    assert_eq!(fixture.json_stdout(&retried_quality)["gap"]["status"], "qa");
    let verified_quality = fixture.run_refine(&["gap", "verify", &quality_id]);
    fixture.assert_success("gap verify qa", &verified_quality);
    assert_eq!(
        fixture.json_stdout(&verified_quality)["gap"]["status"],
        "done"
    );
    let undone_done = fixture.run_refine(&["gap", "undo", &quality_id]);
    fixture.assert_success("gap undo done", &undone_done);
    assert_eq!(fixture.json_stdout(&undone_done)["gap"]["status"], "review");
    let undone_review = fixture.run_refine(&["gap", "undo", &quality_id]);
    fixture.assert_success("gap undo review", &undone_review);
    assert_eq!(fixture.json_stdout(&undone_review)["gap"]["status"], "todo");
    fixture.assert_success(
        "gap delete quality retry",
        &fixture.run_refine(&["gap", "delete", &quality_id]),
    );

    let merge_id = fixture.create_gap("gap action merge retry");
    seed_gap_status(fixture, &merge_id, "failed");
    let retried_merge = fixture.run_refine(&["gap", "retry", &merge_id, "--stage", "merge"]);
    fixture.assert_success("gap retry merge", &retried_merge);
    assert_eq!(
        fixture.json_stdout(&retried_merge)["gap"]["status"],
        "ready-merge"
    );
    let merged = fixture.run_refine(&["gap", "merge", &merge_id]);
    fixture.assert_success("gap merge", &merged);
    assert_eq!(fixture.json_stdout(&merged)["gap"]["status"], "done");
    let merge_undone = fixture.run_refine(&["gap", "undo", &merge_id]);
    fixture.assert_success("gap undo merged", &merge_undone);
    assert_eq!(
        fixture.json_stdout(&merge_undone)["gap"]["status"],
        "review"
    );
    fixture.assert_success(
        "gap delete merge retry",
        &fixture.run_refine(&["gap", "delete", &merge_id]),
    );

    let cancelled_id = fixture.create_gap("gap action undo cancelled");
    let cancelled = fixture.run_refine(&["gap", "cancel", &cancelled_id]);
    fixture.assert_success("gap cancel for undo", &cancelled);
    let reopened = fixture.run_refine(&["gap", "undo", &cancelled_id]);
    fixture.assert_success("gap undo cancelled", &reopened);
    assert_eq!(fixture.json_stdout(&reopened)["gap"]["status"], "todo");
    fixture.assert_success(
        "gap delete cancel undo",
        &fixture.run_refine(&["gap", "delete", &cancelled_id]),
    );
}

fn seed_gap_status(fixture: &IntegrationFixture, gap_id: &str, status: &str) {
    let payload = fixture.api_json(
        "POST",
        "/api/work/gaps/bulk",
        serde_json::json!({
            "selected_ids": [gap_id],
            "exclude_ids": [],
            "update": {
                "status": status
            }
        }),
    );
    assert_eq!(payload["updated"], 1, "{payload:#}");
    assert_eq!(fixture.gap_field(gap_id, "status"), status);
}

fn feature_create_membership_rollup_and_delete(fixture: &IntegrationFixture) {
    let feature = fixture.run_refine(&["feature", "create", "cli surface feature"]);
    fixture.assert_success("feature create", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let gap_id = fixture.create_gap("feature member gap");

    let list = fixture.run_refine(&["feature", "list"]);
    fixture.assert_success("feature list", &list);
    assert!(feature_entry(&fixture.json_stdout(&list), &feature_id).is_some());

    let add = fixture.run_refine(&["feature", "add-gap", &feature_id, &gap_id]);
    fixture.assert_success("feature add-gap", &add);
    let list = fixture.run_refine(&["feature", "list"]);
    let payload = fixture.json_stdout(&list);
    let entry = feature_entry(&payload, &feature_id).expect("feature should be listed");
    assert!(
        entry["gap_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(gap_id.clone()))
    );
    assert_eq!(entry["rollup"]["gap_count"], 1);

    let remove = fixture.run_refine(&["feature", "remove-gap", &feature_id, &gap_id]);
    fixture.assert_success("feature remove-gap", &remove);
    let list = fixture.run_refine(&["feature", "list"]);
    let payload = fixture.json_stdout(&list);
    let entry = feature_entry(&payload, &feature_id).expect("feature should be listed");
    assert!(
        !entry["gap_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(gap_id.clone()))
    );

    let delete_gap = fixture.run_refine(&["gap", "delete", &gap_id]);
    fixture.assert_success("gap delete feature member", &delete_gap);
    let delete_feature = fixture.run_refine(&["feature", "delete", &feature_id]);
    fixture.assert_success("feature delete", &delete_feature);
}

fn feature_show_edit_reorder_move_cancel_and_import(fixture: &IntegrationFixture) {
    let feature = fixture.run_refine(&[
        "feature",
        "create",
        "cli extended feature",
        "--description",
        "Initial description",
        "--reporter",
        "refine-smoke",
    ]);
    fixture.assert_success("feature create extended", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let first_gap = fixture.create_gap("feature reorder first gap");
    let second_gap = fixture.create_gap("feature reorder second gap");

    let show = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show", &show);
    assert_eq!(fixture.json_stdout(&show)["feature"]["id"], feature_id);

    let edit = fixture.run_refine(&[
        "feature",
        "edit",
        &feature_id,
        "--name",
        "cli extended feature renamed",
        "--description",
        "Edited description",
        "--reporter",
        "refine-smoke",
    ]);
    fixture.assert_success("feature edit", &edit);
    let shown = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after edit", &shown);
    let shown_payload = fixture.json_stdout(&shown);
    assert_eq!(
        shown_payload["feature"]["name"],
        "cli extended feature renamed"
    );
    assert_eq!(
        shown_payload["feature"]["description"],
        "Edited description"
    );

    fixture.assert_success(
        "feature add first gap",
        &fixture.run_refine(&["feature", "add-gap", &feature_id, &first_gap]),
    );
    fixture.assert_success(
        "feature add second gap",
        &fixture.run_refine(&["feature", "add-gap", &feature_id, &second_gap]),
    );
    let reorder = fixture.run_refine(&["feature", "reorder-gap", &feature_id, &second_gap, "1"]);
    fixture.assert_success("feature reorder-gap", &reorder);
    let reordered = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after reorder", &reordered);
    let gap_ids = reordered_gap_ids(&fixture.json_stdout(&reordered));
    assert_eq!(
        gap_ids.first().map(String::as_str),
        Some(second_gap.as_str())
    );

    let move_todo = fixture.run_refine(&["feature", "move", &feature_id, "todo"]);
    fixture.assert_success("feature move todo", &move_todo);
    let moved = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after move", &moved);
    assert_eq!(fixture.json_stdout(&moved)["feature"]["status"], "todo");

    let cancel = fixture.run_refine(&["feature", "cancel", &feature_id]);
    fixture.assert_success("feature cancel", &cancel);
    let cancelled = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after cancel", &cancelled);
    assert_eq!(
        fixture.json_stdout(&cancelled)["feature"]["status"],
        "cancelled"
    );

    let import = fixture.run_refine(&[
        "feature",
        "import",
        "--csv",
        "--text",
        "actual,target,priority\nimport actual,import target,low\n",
        "--reporter",
        "refine-smoke",
        "--feature-id",
        &feature_id,
    ]);
    fixture.assert_success("feature import csv", &import);
    let import_payload = fixture.json_stdout(&import);
    assert_eq!(import_payload["count"], 1, "{import_payload:#}");

    for gap_id in [first_gap, second_gap] {
        let _ = fixture.run_refine(&["gap", "delete", &gap_id]);
    }
    let imported_ids = import_payload["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|gap| gap["id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    for gap_id in imported_ids {
        let _ = fixture.run_refine(&["gap", "delete", &gap_id]);
    }
    fixture.assert_success(
        "feature delete extended",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
    );
}

fn reordered_gap_ids(payload: &serde_json::Value) -> Vec<String> {
    let Some(gap_ids) = payload["gap_ids"]
        .as_array()
        .or_else(|| payload["feature"]["gap_ids"].as_array())
    else {
        return Vec::new();
    };
    gap_ids
        .iter()
        .filter_map(|gap_id| gap_id.as_str().map(str::to_string))
        .collect()
}

fn feature_entry<'a>(
    payload: &'a serde_json::Value,
    feature_id: &str,
) -> Option<&'a serde_json::Value> {
    payload["features"]
        .as_array()?
        .iter()
        .find(|entry| entry["feature"]["id"].as_str() == Some(feature_id))
}

fn node_create_activate_archive(fixture: &IntegrationFixture) {
    let list = fixture.run_refine(&["node", "list"]);
    fixture.assert_success("node list", &list);
    let payload = fixture.json_stdout(&list);
    assert_eq!(payload["active_node_id"], "default");
    assert!(node_ids(&payload).contains(&"default".to_string()));

    let create = fixture.run_refine(&["node", "create", "smoke-node"]);
    fixture.assert_success("node create", &create);
    let list = fixture.run_refine(&["node", "list"]);
    assert!(node_ids(&fixture.json_stdout(&list)).contains(&"smoke-node".to_string()));

    let activate = fixture.run_refine(&["node", "activate", "smoke-node"]);
    fixture.assert_success("node activate", &activate);
    let list = fixture.run_refine(&["node", "list"]);
    assert_eq!(fixture.json_stdout(&list)["active_node_id"], "smoke-node");

    let restore = fixture.run_refine(&["node", "activate", "default"]);
    fixture.assert_success("node restore", &restore);
    let archive = fixture.run_refine(&["node", "archive", "smoke-node"]);
    fixture.assert_success("node archive", &archive);
    let list = fixture.run_refine(&["node", "list"]);
    assert_eq!(fixture.json_stdout(&list)["active_node_id"], "default");
}

fn node_show_rename_settings_and_transfer(fixture: &IntegrationFixture) {
    let create = fixture.run_refine(&["node", "create", "transfer-node"]);
    fixture.assert_success("node create transfer", &create);

    let show = fixture.run_refine(&["node", "show", "transfer-node"]);
    fixture.assert_success("node show", &show);
    assert_eq!(fixture.json_stdout(&show)["node"]["id"], "transfer-node");

    let rename = fixture.run_refine(&["node", "rename", "transfer-node", "Transfer Node"]);
    fixture.assert_success("node rename", &rename);
    let shown = fixture.run_refine(&["node", "show", "transfer-node"]);
    fixture.assert_success("node show after rename", &shown);
    assert_eq!(
        fixture.json_stdout(&shown)["node"]["display_name"],
        "Transfer Node"
    );

    let settings = fixture.run_refine(&["node", "settings", "transfer-node"]);
    fixture.assert_success("node settings", &settings);
    let settings_payload = fixture.json_stdout(&settings);
    assert_eq!(settings_payload["node_id"], "transfer-node");
    assert!(
        settings_payload["settings"].is_object(),
        "{settings_payload:#}"
    );

    let gap_id = fixture.create_gap("node transfer gap");
    let transfer = fixture.run_refine(&["node", "transfer", "transfer-node", &gap_id]);
    fixture.assert_success("node transfer", &transfer);
    assert_eq!(fixture.gap_field(&gap_id, "node_id"), "transfer-node");

    fixture.assert_success(
        "node activate transfer for cleanup",
        &fixture.run_refine(&["node", "activate", "transfer-node"]),
    );
    fixture.assert_success(
        "gap delete transferred",
        &fixture.run_refine(&["gap", "delete", &gap_id]),
    );
    fixture.assert_success(
        "node activate default after transfer cleanup",
        &fixture.run_refine(&["node", "activate", "default"]),
    );
    fixture.assert_success(
        "node archive transfer",
        &fixture.run_refine(&["node", "archive", "transfer-node"]),
    );
}

fn cluster_local_registry_commands(fixture: &IntegrationFixture) {
    let list = fixture.run_refine(&["cluster", "list"]);
    fixture.assert_success("cluster list", &list);
    assert!(fixture.json_stdout(&list)["nodes"].is_array());

    let add = fixture.run_refine(&["cluster", "add-node", "cluster-smoke"]);
    fixture.assert_success("cluster add-node", &add);
    let duplicate = fixture.run_refine(&["cluster", "add-node", "cluster-smoke"]);
    assert!(
        !duplicate.status.success(),
        "duplicate cluster node succeeded"
    );
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("already exists"),
        "stderr:\n{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    let invalid_host = fixture.run_refine(&[
        "cluster",
        "edit-node",
        "cluster-smoke",
        "--ssh-host",
        "deploy@example.com",
    ]);
    assert!(
        !invalid_host.status.success(),
        "invalid cluster ssh host succeeded"
    );
    assert!(
        String::from_utf8_lossy(&invalid_host.stderr).contains("ssh_host"),
        "stderr:\n{}",
        String::from_utf8_lossy(&invalid_host.stderr)
    );
    let edit = fixture.run_refine(&[
        "cluster",
        "edit-node",
        "cluster-smoke",
        "--display-name",
        "Cluster Smoke",
        "--ssh-host",
        "127.0.0.1",
        "--ssh-port",
        "22",
        "--target-app-path",
        fixture.app_root.to_str().unwrap(),
        "--refine-port",
        &fixture.port.to_string(),
        "--enabled",
        "true",
    ]);
    fixture.assert_success("cluster edit-node", &edit);
    let show = fixture.run_refine(&["cluster", "show", "cluster-smoke"]);
    fixture.assert_success("cluster show", &show);
    let shown = fixture.json_stdout(&show);
    assert_eq!(shown["node"]["display_name"], "Cluster Smoke");
    assert_eq!(shown["node"]["enabled"], true);

    fixture.assert_success(
        "cluster disable-node",
        &fixture.run_refine(&["cluster", "disable-node", "cluster-smoke"]),
    );
    fixture.assert_success(
        "cluster enable-node",
        &fixture.run_refine(&["cluster", "enable-node", "cluster-smoke"]),
    );
    let sync = fixture.run_refine(&["cluster", "sync"]);
    fixture.assert_success("cluster sync", &sync);
    let sync_payload = fixture.json_stdout(&sync);
    assert_eq!(sync_payload["ok"], true, "{sync_payload:#}");
    assert_eq!(sync_payload["synced"], 1, "{sync_payload:#}");
    assert!(
        sync_payload["cluster"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == "cluster-smoke"),
        "{sync_payload:#}"
    );
    let maintenance = fixture.run_refine(&["cluster", "maintenance"]);
    fixture.assert_success("cluster maintenance", &maintenance);
    let maintenance_payload = fixture.json_stdout(&maintenance);
    assert_eq!(maintenance_payload["ok"], true, "{maintenance_payload:#}");
    assert_eq!(
        maintenance_payload["maintenance"]["active"], true,
        "{maintenance_payload:#}"
    );
    assert!(
        maintenance_payload["cluster"]["nodes"].is_array(),
        "{maintenance_payload:#}"
    );

    let gap_id = fixture.create_gap("cluster transfer gap");
    let transfer = fixture.run_refine(&["cluster", "transfer", "cluster-smoke", &gap_id]);
    fixture.assert_success("cluster transfer", &transfer);
    let transfer_payload = fixture.json_stdout(&transfer);
    assert_eq!(transfer_payload["target_node_id"], "cluster-smoke");
    assert_eq!(transfer_payload["updated"], 1);
    assert_eq!(fixture.gap_field(&gap_id, "node_id"), "cluster-smoke");

    let missing_run = fixture.run_refine(&["cluster", "run", "missing-cluster-node", "printf ok"]);
    assert!(
        !missing_run.status.success(),
        "cluster run unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&missing_run.stderr).contains("missing-cluster-node"),
        "stderr:\n{}",
        String::from_utf8_lossy(&missing_run.stderr)
    );

    fixture.assert_success(
        "node create cluster cleanup",
        &fixture.run_refine(&["node", "create", "cluster-smoke"]),
    );
    fixture.assert_success(
        "node activate cluster cleanup",
        &fixture.run_refine(&["node", "activate", "cluster-smoke"]),
    );
    fixture.assert_success(
        "gap delete cluster transferred",
        &fixture.run_refine(&["gap", "delete", &gap_id]),
    );
    fixture.assert_success(
        "node activate default after cluster cleanup",
        &fixture.run_refine(&["node", "activate", "default"]),
    );
    fixture.assert_success(
        "node archive cluster cleanup",
        &fixture.run_refine(&["node", "archive", "cluster-smoke"]),
    );
    fixture.assert_success(
        "cluster remove-node",
        &fixture.run_refine(&["cluster", "remove-node", "cluster-smoke"]),
    );
}

fn log_commands_query_public_activity(fixture: &IntegrationFixture) {
    let gap_id = fixture.create_gap("log command gap");
    let recorded = fixture.api_json(
        "POST",
        "/api/activity/ui-error",
        serde_json::json!({
            "message": "log command gap activity",
            "source": "cli-surface"
        }),
    );
    assert_eq!(recorded["recorded"], true, "{recorded:#}");
    let list = fixture.run_refine(&["log", "list", "--limit", "20"]);
    fixture.assert_success("log list", &list);
    let list_payload = fixture.json_stdout(&list);
    let entries = list_payload["entries"].as_array().unwrap();
    assert!(!entries.is_empty(), "{list_payload:#}");
    let entry_id = entries
        .iter()
        .find_map(|entry| entry["id"].as_str())
        .expect("activity entries should expose an id")
        .to_string();

    let tail = fixture.run_refine(&["log", "tail", "--limit", "5"]);
    fixture.assert_success("log tail", &tail);
    assert_eq!(fixture.json_stdout(&tail)["tail"], true);

    let show = fixture.run_refine(&["log", "show", &entry_id]);
    fixture.assert_success("log show", &show);
    assert_eq!(fixture.json_stdout(&show)["entry"]["id"], entry_id);

    let query = fixture.run_refine(&["log", "query", "gap", "--limit", "20"]);
    fixture.assert_success("log query", &query);
    assert!(fixture.json_stdout(&query)["entries"].is_array());

    let export = fixture.run_refine(&["log", "export"]);
    fixture.assert_success("log export", &export);
    assert!(
        fixture.json_stdout(&export)["exported"]
            .as_u64()
            .unwrap_or(0)
            >= 1
    );

    let bundle = fixture.run_refine(&["log", "bundle"]);
    fixture.assert_success("log bundle", &bundle);
    let bundle_payload = fixture.json_stdout(&bundle);
    assert_eq!(bundle_payload["redacted"], true);
    assert!(
        bundle_payload["path"]
            .as_str()
            .unwrap_or_default()
            .contains("support-bundle-"),
        "{bundle_payload:#}"
    );

    fixture.assert_success(
        "gap delete log command",
        &fixture.run_refine(&["gap", "delete", &gap_id]),
    );
}

fn agent_commands_use_smoke_ai(fixture: &IntegrationFixture) {
    let detect = fixture.run_refine(&["agent", "detect"]);
    fixture.assert_success("agent detect", &detect);
    let detect_payload = fixture.json_stdout(&detect);
    let smoke = detect_payload["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|provider| provider["name"].as_str() == Some("smoke-ai"))
        .expect("agent detect should list smoke-ai when REFINE_SMOKE_AI_PATH is set");
    assert_eq!(smoke["installed"], true, "{detect_payload:#}");

    let configure = fixture.run_refine(&["agent", "configure", "--provider", "smoke-ai"]);
    fixture.assert_success("agent configure smoke-ai", &configure);
    let configure_payload = fixture.json_stdout(&configure);
    assert_eq!(configure_payload["ok"], true);
    assert_eq!(configure_payload["configured"], true);
    assert_eq!(configure_payload["provider"], "smoke-ai");

    let auth = fixture.run_refine(&["agent", "auth", "--provider", "smoke-ai"]);
    fixture.assert_success("agent auth smoke-ai", &auth);
    let auth_payload = fixture.json_stdout(&auth);
    assert_eq!(auth_payload["ok"], true);
    assert_eq!(auth_payload["authenticated"], true);
    assert_eq!(auth_payload["provider"], "smoke-ai");

    let diagnose = fixture.run_refine(&["agent", "diagnose", "--provider", "smoke-ai"]);
    fixture.assert_success("agent diagnose smoke-ai", &diagnose);
    let diagnose_payload = fixture.json_stdout(&diagnose);
    assert_eq!(diagnose_payload["ok"], true);
    assert_eq!(diagnose_payload["provider"], "smoke-ai");
    assert!(
        diagnose_payload["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message
                .as_str()
                .unwrap_or_default()
                .contains("Smoke AI CLI found")),
        "{diagnose_payload:#}"
    );

    let output = fixture.run_refine(&[
        "agent",
        "invoke",
        "Start a chat conversation for CLI parity.",
        "--provider",
        "smoke-ai",
        "--cwd",
        fixture.app_root.to_str().unwrap(),
    ]);
    fixture.assert_success("agent invoke smoke-ai", &output);
    let payload = fixture.json_stdout(&output);
    assert_eq!(payload["ok"], true);
    assert!(
        payload["output"]
            .as_str()
            .unwrap_or_default()
            .contains("smoke-ai chat response"),
        "{payload:#}"
    );

    let resume =
        fixture.run_refine(&["agent", "resume", "smoke-session", "--provider", "smoke-ai"]);
    assert!(
        !resume.status.success(),
        "agent resume unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&resume.stderr)
            .contains("does not support provider-session resume"),
        "stderr:\n{}",
        String::from_utf8_lossy(&resume.stderr)
    );
}

fn node_ids(payload: &serde_json::Value) -> Vec<String> {
    payload["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_string))
        .collect()
}
