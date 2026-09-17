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
        fixture.json_stdout(&catalog)["sources"],
        json!(
            std::iter::once(refine::model::automation::CUSTOM_EVENT_ID.to_string())
                .chain(refine::model::automation::system_catalog())
                .collect::<Vec<_>>()
        )
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = fixture.artifact_root.join("Custom Agent");
        fs::write(&executable, "#!/usr/bin/python3\nimport json,sys,os\nprint(json.dumps({'argv':sys.argv[1:], 'cwd':os.getcwd()}))\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let mut catalog = fixture.json_stdout(&saved)["catalog"].clone();
        let custom = catalog["providers"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap();
        custom["executable"] = json!(executable);
        custom["automated"]["args"] = json!(["first parameter", "{{context}}"]);
        let context = "Quotes '\"\nUnicode λ $(echo literal) {{cwd}} {{context}}";
        for parameter in ["first parameter", "edited parameter"] {
            catalog["providers"]
                .as_array_mut()
                .unwrap()
                .last_mut()
                .unwrap()["automated"]["args"][0] = json!(parameter);
            let saved = fixture.run_refine(&[
                "config",
                "providers",
                "save",
                "--json",
                &catalog.to_string(),
            ]);
            fixture.assert_success("save executable and updated arguments", &saved);
            catalog = fixture.json_stdout(&saved)["catalog"].clone();
            let invoked = fixture.run_refine(&[
                "agent",
                "invoke",
                context,
                "--cwd",
                fixture.app_root.to_str().unwrap(),
            ]);
            fixture.assert_success("invoke configured provider through daemon", &invoked);
            let response = fixture.json_stdout(&invoked);
            let captured: serde_json::Value =
                serde_json::from_str(response["output"].as_str().unwrap()).unwrap();
            assert_eq!(captured["argv"].as_array().unwrap().len(), 2);
            assert_eq!(captured["argv"][0], parameter);
            assert!(captured["argv"][1].as_str().unwrap().contains(context));
            assert_eq!(captured["cwd"], fixture.app_root.to_str().unwrap());
        }
    }
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
