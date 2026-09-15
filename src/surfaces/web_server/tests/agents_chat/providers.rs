use super::*;

#[test]
fn web_server_force_provider_plan_extraction_skips_structured_input_parse() {
    let temp_root = unique_temp_dir("http-import-plan-force-provider");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write_fake_provider(
        &refine_dir,
        "smoke-ai",
        0,
        &json!({
            "feature": {
                "name": "Provider Extracted Feature",
                "goals": [{
                    "name": "Provider extracted goal",
                    "prompt": "The provider extracts implementation-ready drafts.",
                    "priority": "medium"
                }]
            }
        })
        .to_string(),
    );
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var(
            "REFINE_SMOKE_AI_PATH",
            refine_dir.join("provider-bin/smoke-ai").to_str().unwrap(),
        );
    }
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(refine_dir.join("runtime/8080"));

    let extracted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/extract".to_string(),
        body: Some(json!({
            "purpose": "plan",
            "provider": "smoke-ai",
            "force_provider": true,
            "text": "[]"
        })),
    });
    assert_eq!(extracted.status, 200);
    assert_eq!(extracted.body["source"], "provider");
    assert_eq!(
        extracted.body["feature_destination"]["newName"],
        "Provider Extracted Feature"
    );
    assert_eq!(extracted.body["drafts"].as_array().unwrap().len(), 1);
    assert_eq!(
        extracted.body["drafts"][0]["name"],
        "Provider extracted goal"
    );

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    remove_temp_dir(&temp_root);
}

#[test]
fn provider_catalog_api_round_trip_selection_and_stale_edits() {
    let target = unique_temp_dir("provider-catalog-api");
    init_git_app(&target);
    let mut server = server_with_projection();
    server.target_root = Some(target.clone());
    server.runtime_root = Some(target.join("run/8082"));
    let call = |method: &str, path: &str, body| {
        server.handle(ApiRequest {
            method: method.into(),
            path: path.into(),
            body,
        })
    };
    let initial = call("GET", "/api/providers", None);
    assert_eq!(initial.status, 200, "{:?}", initial.body);
    assert!(initial.body["node_override"].is_null());
    let mut catalog = initial.body["catalog"].clone();
    catalog["providers"].as_array_mut().unwrap().push(json!({
        "id":"CustomAgent", "name":"Custom Agent", "executable":"/Some Path/MyAgent",
        "automated":{"args":["--prompt","{{context}}"]},
        "interactive":{"args":["{{context}}"]}
    }));
    catalog["default_provider"] = json!("CustomAgent");
    let saved = call("PUT", "/api/providers", Some(catalog.clone()));
    assert_eq!(saved.status, 200, "{:?}", saved.body);
    assert_eq!(saved.body["effective_provider"], "CustomAgent");
    let diagnostics = call("GET", "/api/agents/CustomAgent/diagnostics", None);
    assert_eq!(diagnostics.status, 200, "{:?}", diagnostics.body);
    assert!(
        diagnostics.body["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("Custom Agent")
    );
    assert_eq!(
        saved.body["catalog"]["providers"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["automated"]["args"],
        json!(["--prompt", "{{context}}"])
    );
    assert_eq!(call("PUT", "/api/providers", Some(catalog)).status, 409);
    let selected = call(
        "PATCH",
        "/api/settings",
        Some(json!({"agent_cli":"gemini"})),
    );
    assert_eq!(selected.status, 200, "{:?}", selected.body);
    assert_eq!(selected.body["providers"]["node_override"], "gemini");
    let clear = call("PATCH", "/api/settings", Some(json!({"agent_cli":null})));
    assert_eq!(clear.status, 200, "{:?}", clear.body);
    assert_eq!(clear.body["providers"]["selection_source"], "system");
    assert_eq!(clear.body["settings"]["agent_cli"], "CustomAgent");
    assert_eq!(
        call(
            "PATCH",
            "/api/settings",
            Some(json!({"agent_cli":"missing"}))
        )
        .status,
        400
    );
    fs::remove_dir_all(target).unwrap();
}
