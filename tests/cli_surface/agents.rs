use super::super::*;

pub(crate) fn agent_commands_use_smoke_ai(fixture: &IntegrationFixture) {
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

pub(crate) fn node_ids(payload: &serde_json::Value) -> Vec<String> {
    payload["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_string))
        .collect()
}
