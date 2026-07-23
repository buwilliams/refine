mod support;

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use refine::process::subprocess::{FileProcessSupervisor, ManagedProcess, ProcessOwner};
use serde_json::json;
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
    goal_create_list_show_edit_note_round_delete(&fixture);
    goal_feature_assignment_and_round_edit_latest(&fixture);
    goal_workflow_actions_start_retry_verify_merge_undo(&fixture);
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
        payload["target_root"]
            .as_str()
            .unwrap_or_default()
            .ends_with("rust-test-app"),
        "{payload:#}"
    );
    assert_eq!(payload["schema"]["compatible"], true, "{payload:#}");
}

fn project_doctor_runs(fixture: &IntegrationFixture) {
    let initial = fixture.run_refine(&["project", "doctor"]);
    fixture.assert_success("initial project doctor", &initial);
    let initial = fixture.json_stdout(&initial);
    assert_eq!(
        initial["processes"]["runner_reachable"], false,
        "{initial:#}"
    );

    let runtime_root = fixture.runtime_root.join(fixture.port.to_string());
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    supervisor
        .register(ManagedProcess {
            id: "cli-test-workflow-runner".to_string(),
            owner: ProcessOwner::Runner,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("CLI test workflow runner".to_string()),
            details: Some(json!({"kind": "runner", "worker_kind": "workflow"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = fixture.run_refine(&["project", "doctor"]);
        fixture.assert_success("project doctor", &output);
        let payload = fixture.json_stdout(&output);
        if payload["processes"]["runner_reachable"] == true {
            assert!(
                payload["processes"]["process_count"]
                    .as_u64()
                    .is_some_and(|count| count >= 2),
                "{payload:#}"
            );
            assert_eq!(
                payload["processes"]["running_process_count"],
                payload["processes"]["process_count"],
                "{payload:#}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "project doctor kept stale startup process health: {payload:#}"
        );
        thread::sleep(Duration::from_millis(100));
    }
    fs::remove_file(
        supervisor
            .processes_dir()
            .join("cli-test-workflow-runner.json"),
    )
    .unwrap();
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
    assert_eq!(switch_payload["target_root"], registered_app_path);

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
    assert_eq!(fixture.json_stdout(&attach)["target_root"], primary_app);

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
    assert_eq!(clone_payload["target_root"], clone_destination_path);

    let restore = fixture.run_refine(&["project", "switch", &primary_app]);
    fixture.assert_success("project switch primary", &restore);
    assert_eq!(fixture.json_stdout(&restore)["target_root"], primary_app);

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

fn goal_create_list_show_edit_note_round_delete(fixture: &IntegrationFixture) {
    let goal_id = fixture.create_goal("cli surface goal");
    assert_eq!(fixture.goal_field(&goal_id, "status"), "backlog");
    assert_eq!(fixture.goal_field(&goal_id, "priority"), "low");
    assert_eq!(fixture.goal_field(&goal_id, "name"), "cli surface goal");
    assert_eq!(fixture.goal_field(&goal_id, "node_id"), "default");

    let list = fixture.run_refine(&["goal", "list"]);
    fixture.assert_success("goal list", &list);
    let payload = fixture.json_stdout(&list);
    assert!(
        payload["goals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|goal| goal["id"].as_str() == Some(goal_id.as_str()))
    );

    let edit = fixture.run_refine(&[
        "goal",
        "edit",
        &goal_id,
        "--name",
        "cli surface renamed goal",
        "--priority",
        "high",
    ]);
    fixture.assert_success("goal edit", &edit);
    assert_eq!(
        fixture.goal_field(&goal_id, "name"),
        "cli surface renamed goal"
    );
    assert_eq!(fixture.goal_field(&goal_id, "priority"), "high");

    let note = fixture.run_refine(&[
        "goal",
        "note",
        &goal_id,
        "needs a closer look",
        "--author",
        "refine-smoke",
    ]);
    fixture.assert_success("goal note", &note);
    assert_eq!(fixture.json_stdout(&note)["goal"]["id"], goal_id);
    let shown_after_note = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after note", &shown_after_note);
    let note_id = fixture.json_stdout(&shown_after_note)["goal"]["notes"][0]["id"]
        .as_str()
        .expect("goal show should expose note id")
        .to_string();
    let note_edit = fixture.run_refine(&[
        "goal",
        "note-edit",
        &goal_id,
        &note_id,
        "needs a closer look after edit",
    ]);
    fixture.assert_success("goal note-edit", &note_edit);
    let shown_after_note_edit = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after note edit", &shown_after_note_edit);
    assert_eq!(
        fixture.json_stdout(&shown_after_note_edit)["goal"]["notes"][0]["body"],
        "needs a closer look after edit"
    );
    let note_delete = fixture.run_refine(&["goal", "note-delete", &goal_id, &note_id]);
    fixture.assert_success("goal note-delete", &note_delete);
    let shown_after_note_delete = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after note delete", &shown_after_note_delete);
    assert_eq!(
        fixture.json_stdout(&shown_after_note_delete)["goal"]["notes"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    assert_eq!(fixture.goal_field(&goal_id, "round_count"), 0);
    let round = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--reporter",
        "refine-smoke",
        "--prompt",
        "Implement the desired behavior",
    ]);
    fixture.assert_success("goal round", &round);
    assert_eq!(fixture.goal_field(&goal_id, "round_count"), 1);

    let jira_export_path = fixture.app_root.join("goal-evidence.csv");
    let jira_export = fixture.run_refine(&[
        "goal",
        "export",
        &goal_id,
        "--output",
        jira_export_path.to_str().unwrap(),
    ]);
    fixture.assert_success("goal Jira export", &jira_export);
    let jira_csv = fs::read_to_string(&jira_export_path).unwrap();
    assert!(jira_csv.starts_with("Summary,Description,Work Type,Priority"));
    assert!(jira_csv.contains("Implement the desired behavior"));
    fs::remove_file(jira_export_path).unwrap();

    let delete = fixture.run_refine(&["goal", "delete", &goal_id]);
    fixture.assert_success("goal delete", &delete);
    let payload = fixture.json_stdout(&delete);
    assert_eq!(payload["deleted"], true);
    assert_eq!(payload["id"], goal_id);

    let after = fixture.run_refine(&["goal", "show", &goal_id]);
    assert!(!after.status.success());
    assert!(
        String::from_utf8_lossy(&after.stderr)
            .to_lowercase()
            .contains("not found")
    );
}

fn goal_feature_assignment_and_round_edit_latest(fixture: &IntegrationFixture) {
    let goal_id = fixture.create_goal("cli feature assignment goal");
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

    let assign = fixture.run_refine(&["goal", "assign-feature", &goal_id, &feature_id]);
    fixture.assert_success("goal assign-feature", &assign);
    assert_eq!(fixture.goal_field(&goal_id, "feature_id"), feature_id);

    let remove = fixture.run_refine(&["goal", "remove-feature", &goal_id]);
    fixture.assert_success("goal remove-feature", &remove);
    assert!(fixture.goal_field(&goal_id, "feature_id").is_null());

    let round = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--reporter",
        "refine-smoke",
        "--prompt",
        "first prompt",
    ]);
    fixture.assert_success("goal round assignment", &round);
    let edit = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--edit-latest",
        "--reporter",
        "refine-smoke",
        "--prompt",
        "edited prompt",
    ]);
    fixture.assert_success("goal round edit latest", &edit);
    let shown = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after round edit", &shown);
    let goal = fixture.json_stdout(&shown);
    assert_eq!(goal["goal"]["round_count"], 1);
    assert!(goal.to_string().contains("edited prompt"), "{goal:#}");

    fixture.assert_success(
        "goal delete assignment",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
    );
    fixture.assert_success(
        "feature delete assignment",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
    );
}

fn goal_workflow_actions_start_retry_verify_merge_undo(fixture: &IntegrationFixture) {
    let started_id = fixture.create_goal("goal action start");
    let started = fixture.run_refine(&["goal", "start", &started_id]);
    fixture.assert_success("goal start", &started);
    assert_eq!(fixture.json_stdout(&started)["goal"]["status"], "todo");
    assert_eq!(fixture.goal_field(&started_id, "status"), "todo");
    fixture.assert_success(
        "goal cancel started",
        &fixture.run_refine(&["goal", "cancel", &started_id]),
    );
    fixture.assert_success(
        "goal undo started cancel",
        &fixture.run_refine(&["goal", "undo", &started_id]),
    );
    fixture.assert_success(
        "goal delete started",
        &fixture.run_refine(&["goal", "delete", &started_id]),
    );

    let quality_id = fixture.create_goal("goal action quality retry");
    seed_goal_status(fixture, &quality_id, "failed");
    let retried_quality = fixture.run_refine(&["goal", "retry", &quality_id, "--stage", "quality"]);
    fixture.assert_success("goal retry quality", &retried_quality);
    assert_eq!(
        fixture.json_stdout(&retried_quality)["goal"]["status"],
        "qa"
    );
    let verified_quality = fixture.run_refine(&["goal", "verify", &quality_id]);
    fixture.assert_success("goal verify qa", &verified_quality);
    assert_eq!(
        fixture.json_stdout(&verified_quality)["goal"]["status"],
        "done"
    );
    let undone_done = fixture.run_refine(&["goal", "undo", &quality_id]);
    fixture.assert_success("goal undo done", &undone_done);
    assert_eq!(
        fixture.json_stdout(&undone_done)["goal"]["status"],
        "review"
    );
    let undone_review = fixture.run_refine(&["goal", "undo", &quality_id]);
    fixture.assert_success("goal undo review", &undone_review);
    assert_eq!(
        fixture.json_stdout(&undone_review)["goal"]["status"],
        "todo"
    );
    fixture.assert_success(
        "goal delete quality retry",
        &fixture.run_refine(&["goal", "delete", &quality_id]),
    );

    let merge_id = fixture.create_goal("goal action merge retry");
    seed_goal_status(fixture, &merge_id, "failed");
    let retried_merge = fixture.run_refine(&["goal", "retry", &merge_id, "--stage", "merge"]);
    fixture.assert_success("goal retry merge", &retried_merge);
    assert_eq!(
        fixture.json_stdout(&retried_merge)["goal"]["status"],
        "ready-merge"
    );
    let merged = fixture.run_refine(&["goal", "merge", &merge_id]);
    assert!(
        !merged.status.success(),
        "goal merge without a reviewed candidate unexpectedly succeeded"
    );
    fixture.assert_success(
        "goal cancel merge retry",
        &fixture.run_refine(&["goal", "cancel", &merge_id]),
    );
    fixture.assert_success(
        "goal delete merge retry",
        &fixture.run_refine(&["goal", "delete", &merge_id]),
    );

    let cancelled_id = fixture.create_goal("goal action undo cancelled");
    let cancelled = fixture.run_refine(&["goal", "cancel", &cancelled_id]);
    fixture.assert_success("goal cancel for undo", &cancelled);
    let reopened = fixture.run_refine(&["goal", "undo", &cancelled_id]);
    fixture.assert_success("goal undo cancelled", &reopened);
    assert_eq!(fixture.json_stdout(&reopened)["goal"]["status"], "todo");
    fixture.assert_success(
        "goal delete cancel undo",
        &fixture.run_refine(&["goal", "delete", &cancelled_id]),
    );
}

fn seed_goal_status(fixture: &IntegrationFixture, goal_id: &str, status: &str) {
    let payload = fixture.api_json(
        "POST",
        "/api/goals/bulk",
        serde_json::json!({
            "selected_ids": [goal_id],
            "exclude_ids": [],
            "update": {
                "status": status
            }
        }),
    );
    assert_eq!(payload["updated"], 1, "{payload:#}");
    assert_eq!(fixture.goal_field(goal_id, "status"), status);
}

fn feature_create_membership_rollup_and_delete(fixture: &IntegrationFixture) {
    let feature = fixture.run_refine(&["feature", "create", "cli surface feature"]);
    fixture.assert_success("feature create", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let goal_id = fixture.create_goal("feature member goal");

    let list = fixture.run_refine(&["feature", "list"]);
    fixture.assert_success("feature list", &list);
    assert!(feature_entry(&fixture.json_stdout(&list), &feature_id).is_some());

    let add = fixture.run_refine(&["feature", "add-goal", &feature_id, &goal_id]);
    fixture.assert_success("feature add-goal", &add);
    let list = fixture.run_refine(&["feature", "list"]);
    let payload = fixture.json_stdout(&list);
    let entry = feature_entry(&payload, &feature_id).expect("feature should be listed");
    assert!(
        entry["goal_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(goal_id.clone()))
    );
    assert_eq!(entry["rollup"]["goal_count"], 1);

    let remove = fixture.run_refine(&["feature", "remove-goal", &feature_id, &goal_id]);
    fixture.assert_success("feature remove-goal", &remove);
    let list = fixture.run_refine(&["feature", "list"]);
    let payload = fixture.json_stdout(&list);
    let entry = feature_entry(&payload, &feature_id).expect("feature should be listed");
    assert!(
        !entry["goal_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(goal_id.clone()))
    );

    let delete_goal = fixture.run_refine(&["goal", "delete", &goal_id]);
    fixture.assert_success("goal delete feature member", &delete_goal);
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
    let first_goal = fixture.create_goal("feature reorder first goal");
    let second_goal = fixture.create_goal("feature reorder second goal");

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
        "feature add first goal",
        &fixture.run_refine(&["feature", "add-goal", &feature_id, &first_goal]),
    );
    fixture.assert_success(
        "feature add second goal",
        &fixture.run_refine(&["feature", "add-goal", &feature_id, &second_goal]),
    );
    fixture.assert_success(
        "feature order first goal",
        &fixture.run_refine(&["feature", "order-goal", &feature_id, &first_goal]),
    );
    fixture.assert_success(
        "feature order second goal",
        &fixture.run_refine(&["feature", "order-goal", &feature_id, &second_goal]),
    );
    let reorder = fixture.run_refine(&["feature", "reorder-goal", &feature_id, &second_goal, "1"]);
    fixture.assert_success("feature reorder-goal", &reorder);
    let reordered = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after reorder", &reordered);
    let goal_ids = reordered_goal_ids(&fixture.json_stdout(&reordered));
    assert_eq!(
        goal_ids.first().map(String::as_str),
        Some(second_goal.as_str())
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
        "prompt,priority\nimplement imported goal,low\n",
        "--reporter",
        "refine-smoke",
        "--feature-id",
        &feature_id,
    ]);
    fixture.assert_success("feature import csv", &import);
    let import_payload = fixture.json_stdout(&import);
    assert_eq!(import_payload["count"], 1, "{import_payload:#}");

    for goal_id in [first_goal, second_goal] {
        let _ = fixture.run_refine(&["goal", "delete", &goal_id]);
    }
    let imported_ids = import_payload["goals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|goal| goal["id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    for goal_id in imported_ids {
        let _ = fixture.run_refine(&["goal", "delete", &goal_id]);
    }
    fixture.assert_success(
        "feature delete extended",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
    );
}

fn reordered_goal_ids(payload: &serde_json::Value) -> Vec<String> {
    let Some(goal_ids) = payload["goal_ids"]
        .as_array()
        .or_else(|| payload["feature"]["goal_ids"].as_array())
    else {
        return Vec::new();
    };
    goal_ids
        .iter()
        .filter_map(|goal_id| goal_id.as_str().map(str::to_string))
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

    let goal_id = fixture.create_goal("node transfer goal");
    let transfer = fixture.run_refine(&["node", "transfer", "transfer-node", &goal_id]);
    fixture.assert_success("node transfer", &transfer);
    assert_eq!(fixture.goal_field(&goal_id, "node_id"), "transfer-node");

    let feature = fixture.run_refine(&["feature", "create", "cli node transfer feature"]);
    fixture.assert_success("feature create node transfer", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let feature_goal_id = fixture.create_goal("node transfer feature member goal");
    fixture.assert_success(
        "feature add node transfer goal",
        &fixture.run_refine(&["feature", "add-goal", &feature_id, &feature_goal_id]),
    );
    let direct_feature_goal_transfer =
        fixture.run_refine(&["node", "transfer", "transfer-node", &feature_goal_id]);
    assert!(
        !direct_feature_goal_transfer.status.success(),
        "Feature-owned Goal transfer unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&direct_feature_goal_transfer.stderr)
            .contains("transfer the Feature instead"),
        "stderr:\n{}",
        String::from_utf8_lossy(&direct_feature_goal_transfer.stderr)
    );
    fixture.assert_success(
        "feature transfer node",
        &fixture.run_refine(&["feature", "transfer", &feature_id, "transfer-node"]),
    );
    let transferred_feature = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show transferred node", &transferred_feature);
    assert_eq!(
        fixture.json_stdout(&transferred_feature)["feature"]["node_id"],
        "transfer-node"
    );
    assert_eq!(
        fixture.goal_field(&feature_goal_id, "node_id"),
        "transfer-node"
    );

    fixture.assert_success(
        "node activate transfer for cleanup",
        &fixture.run_refine(&["node", "activate", "transfer-node"]),
    );
    fixture.assert_success(
        "goal delete transferred",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
    );
    fixture.assert_success(
        "goal delete transferred feature member",
        &fixture.run_refine(&["goal", "delete", &feature_goal_id]),
    );
    fixture.assert_success(
        "feature delete transferred",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
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
    assert!(!duplicate.status.success(), "duplicate node succeeded");
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
    assert_eq!(
        sync_payload["operation"]["owner"], "project:sync",
        "{sync_payload:#}"
    );
    assert!(
        sync_payload["operation"]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
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

    let goal_id = fixture.create_goal("cluster transfer goal");
    let transfer = fixture.run_refine(&["cluster", "transfer", "cluster-smoke", &goal_id]);
    fixture.assert_success("cluster transfer", &transfer);
    let transfer_payload = fixture.json_stdout(&transfer);
    assert_eq!(transfer_payload["target_node_id"], "cluster-smoke");
    assert_eq!(transfer_payload["updated"], 1);
    assert_eq!(fixture.goal_field(&goal_id, "node_id"), "cluster-smoke");

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
        "node activate cluster cleanup",
        &fixture.run_refine(&["node", "activate", "cluster-smoke"]),
    );
    fixture.assert_success(
        "goal delete cluster transferred",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
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
    let goal_id = fixture.create_goal("log command goal");
    let recorded = fixture.api_json(
        "POST",
        "/api/activity/ui-error",
        serde_json::json!({
            "message": "log command goal activity",
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

    let query = fixture.run_refine(&["log", "query", "goal", "--limit", "20"]);
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
        "goal delete log command",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
    );
}

fn agent_commands_use_smoke_ai(fixture: &IntegrationFixture) {
    let supervisor = fixture.run_refine(&["agent", "supervisor"]);
    fixture.assert_success("agent supervisor", &supervisor);
    let supervisor_payload = fixture.json_stdout(&supervisor);
    assert!(supervisor_payload["supervisor_agent"]["lifecycle"].is_string());
    assert!(supervisor_payload["supervisor_agent"]["events"].is_array());

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
