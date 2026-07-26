use super::super::*;

pub(crate) fn log_commands_query_public_activity(fixture: &IntegrationFixture) {
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
