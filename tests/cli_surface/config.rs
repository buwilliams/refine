use super::super::*;

pub(crate) fn config_commands_route_through_the_active_daemon(fixture: &IntegrationFixture) {
    let shown = fixture.run_refine(&["config", "show"]);
    fixture.assert_success("config show", &shown);
    let shown = fixture.json_stdout(&shown);
    for domain in ["settings", "skills"] {
        assert!(shown[domain].is_object(), "missing {domain}: {shown:#}");
    }
    let revision = shown["skills"]["revision"].as_u64().unwrap().to_string();
    let saved = fixture.run_refine(&[
        "skills",
        "save",
        "cli-check",
        "--revision",
        &revision,
        "--json",
        r#"{"name":"CLI check","prompt":"Verify authoritative readback","trigger":{"source":"custom"}}"#,
    ]);
    fixture.assert_success("skills save", &saved);
    let saved = fixture.json_stdout(&saved);
    assert_eq!(saved["item"]["name"], "CLI check");
    let catalog = fixture.run_refine(&["skills", "triggers"]);
    fixture.assert_success("skills triggers", &catalog);
    assert_eq!(
        fixture.json_stdout(&catalog)["sources"]
            .as_array()
            .unwrap()
            .len(),
        42
    );
    for retired in ["quality", "governance", "guidance"] {
        assert!(
            !fixture
                .run_refine(&["config", retired, "show"])
                .status
                .success()
        );
    }
}

pub(crate) fn provider_configuration_commands(fixture: &IntegrationFixture) {
    let shown = fixture.run_refine(&["config", "providers", "show"]);
    fixture.assert_success("config providers show", &shown);
    let shown = fixture.json_stdout(&shown);
    let mut catalog = shown["catalog"].clone();
    catalog["providers"].as_array_mut().unwrap().push(serde_json::json!({
        "id":"CLIConfigured", "name":"CLI configured agent", "executable":"/Not Installed/Agent",
        "automated":{"args":["--prompt","{{context}}"]}, "interactive":{"args":["{{context}}"]}
    }));
    let payload = serde_json::to_string(&catalog).unwrap();
    let saved = fixture.run_refine(&["config", "providers", "save", "--json", &payload]);
    fixture.assert_success("config providers save", &saved);
    assert!(
        !fixture
            .run_refine(&["config", "providers", "save", "--json", &payload])
            .status
            .success()
    );
    let selected = fixture.run_refine(&["config", "providers", "select", "CLIConfigured"]);
    fixture.assert_success("config providers select", &selected);
    assert_eq!(
        fixture.json_stdout(&selected)["providers"]["node_override"],
        "CLIConfigured"
    );
    let diagnosed = fixture.run_refine(&["agent", "diagnose"]);
    fixture.assert_success("diagnose inherited CLI selection", &diagnosed);
    assert_eq!(fixture.json_stdout(&diagnosed)["provider"], "CLIConfigured");
    let clear = fixture.run_refine(&["config", "providers", "select"]);
    fixture.assert_success("config providers clear", &clear);
    assert!(fixture.json_stdout(&clear)["providers"]["node_override"].is_null());
    if let Some(previous) = shown["node_override"].as_str() {
        fixture.assert_success(
            "restore provider selection",
            &fixture.run_refine(&["config", "providers", "select", previous]),
        );
    }
}
