use super::super::*;

pub(crate) fn config_commands_route_through_the_active_daemon(fixture: &IntegrationFixture) {
    let shown = fixture.run_refine(&["config", "show"]);
    fixture.assert_success("config show", &shown);
    let shown = fixture.json_stdout(&shown);
    for domain in ["settings", "quality", "governance", "guidance"] {
        assert!(shown[domain].is_object(), "missing {domain}: {shown:#}");
    }

    let settings = fixture.run_refine(&[
        "config",
        "settings",
        "set",
        "--set",
        "file_browser_ignore_patterns=target",
    ]);
    fixture.assert_success("config settings set", &settings);
    assert_eq!(
        fixture.json_stdout(&settings)["settings"]["file_browser_ignore_patterns"],
        "target"
    );

    let quality = fixture.run_refine(&[
        "config",
        "quality",
        "set",
        "--business-requirements",
        "Keep daemon-routed configuration scriptable.",
    ]);
    fixture.assert_success("config quality set", &quality);
    assert_eq!(
        fixture.json_stdout(&quality)["business_requirements"],
        "Keep daemon-routed configuration scriptable."
    );

    let governance = fixture.run_refine(&[
        "config",
        "governance",
        "set",
        "--product",
        "Refine CLI fixture",
        "--constitution",
        "Preserve authoritative configuration readback.",
        "--max-automatic-round-retries",
        "2",
        "--rule",
        "Keep unrelated fields",
    ]);
    fixture.assert_success("config governance set", &governance);
    let governance = fixture.json_stdout(&governance);
    assert_eq!(governance["rules_revision"], 1);
    assert_eq!(governance["rules"][0]["text"], "Keep unrelated fields");

    let generated = fixture.run_refine(&[
        "config",
        "governance",
        "generate-rules",
        "--product",
        "Refine CLI fixture",
        "--constitution",
        "Preserve authoritative configuration readback.",
    ]);
    fixture.assert_success("config governance generate-rules", &generated);
    let generated = fixture.json_stdout(&generated);
    assert!(generated["generation"]["rules"].is_array(), "{generated:#}");
    assert_eq!(generated["governance"]["rules_revision"], 2);

    let added = fixture.run_refine(&[
        "config",
        "guidance",
        "add",
        "--name",
        "CLI fixture",
        "--rule",
        "When config changes",
        "--instructions",
        "Verify authoritative readback",
    ]);
    fixture.assert_success("config guidance add", &added);
    let added = fixture.json_stdout(&added);
    let id = added["guidance"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "CLI fixture")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let edited = fixture.run_refine(&[
        "config",
        "guidance",
        "edit",
        &id,
        "--name",
        "CLI fixture edited",
    ]);
    fixture.assert_success("config guidance edit", &edited);
    assert!(
        fixture.json_stdout(&edited)["guidance"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == id && entry["name"] == "CLI fixture edited")
    );

    for (action, expected) in [("disable", false), ("enable", true)] {
        let output = fixture.run_refine(&["config", "guidance", action, &id]);
        fixture.assert_success(&format!("config guidance {action}"), &output);
        assert!(
            fixture.json_stdout(&output)["guidance"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == id && entry["enabled"] == expected)
        );
    }

    let removed = fixture.run_refine(&["config", "guidance", "remove", &id]);
    fixture.assert_success("config guidance remove", &removed);
    assert!(
        !fixture.json_stdout(&removed)["guidance"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == id)
    );
}
