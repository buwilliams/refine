mod support;

use support::integration::IntegrationFixture;

#[test]
#[ignore = "daemon-backed surface test; run through `cargo run --manifest-path xtask/Cargo.toml -- test-cli`"]
fn cli_surface_suite() {
    let fixture = IntegrationFixture::start("cli");

    system_status_reports_healthy_daemon(&fixture);
    project_status_is_attached_to_test_app(&fixture);
    project_doctor_runs(&fixture);
    gap_create_list_show_edit_note_round_delete(&fixture);
    workflow_allowed_and_user_transitions(&fixture);
    feature_create_membership_rollup_and_delete(&fixture);
    node_create_activate_archive(&fixture);
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

fn gap_create_list_show_edit_note_round_delete(fixture: &IntegrationFixture) {
    let gap_id = fixture.create_gap("cli surface gap");
    assert_eq!(fixture.gap_field(&gap_id, "status"), "backlog");
    assert_eq!(fixture.gap_field(&gap_id, "priority"), "low");
    assert_eq!(fixture.gap_field(&gap_id, "name"), "cli surface gap");
    assert_eq!(fixture.gap_field(&gap_id, "node_id"), "default");

    let list = fixture.run_refine(&["gap", "list"]);
    fixture.assert_success("gap list", &list);
    let payload = fixture.json_stdout(&list);
    assert!(payload["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|gap| gap["id"].as_str() == Some(gap_id.as_str())));

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
    assert_eq!(fixture.gap_field(&gap_id, "name"), "cli surface renamed gap");
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
    assert!(String::from_utf8_lossy(&after.stderr)
        .to_lowercase()
        .contains("not found"));
}

fn workflow_allowed_and_user_transitions(fixture: &IntegrationFixture) {
    let allowed = fixture.run_refine(&["workflow", "allowed", "backlog", "todo"]);
    fixture.assert_success("workflow allowed backlog todo", &allowed);
    assert_eq!(fixture.json_stdout(&allowed)["allowed"], true);

    let blocked = fixture.run_refine(&["workflow", "allowed", "backlog", "in-progress"]);
    fixture.assert_success("workflow allowed backlog in-progress", &blocked);
    let payload = fixture.json_stdout(&blocked);
    assert_eq!(payload["allowed"], false);
    assert!(payload["reason"].as_str().unwrap_or_default().len() > 0);

    let gap_id = fixture.create_gap("workflow transition gap");
    let moved = fixture.run_refine(&["workflow", "transition", &gap_id, "todo"]);
    fixture.assert_success("workflow transition", &moved);
    assert_eq!(fixture.gap_field(&gap_id, "status"), "todo");

    let cancelled = fixture.run_refine(&["gap", "cancel", &gap_id]);
    fixture.assert_success("gap cancel", &cancelled);
    assert_eq!(fixture.gap_field(&gap_id, "status"), "cancelled");

    let reopened = fixture.run_refine(&["workflow", "transition", &gap_id, "todo"]);
    fixture.assert_success("workflow reopen", &reopened);
    assert_eq!(fixture.gap_field(&gap_id, "status"), "todo");

    let delete = fixture.run_refine(&["gap", "delete", &gap_id]);
    fixture.assert_success("gap delete workflow", &delete);
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

fn node_ids(payload: &serde_json::Value) -> Vec<String> {
    payload["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_string))
        .collect()
}
