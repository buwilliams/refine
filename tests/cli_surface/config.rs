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
