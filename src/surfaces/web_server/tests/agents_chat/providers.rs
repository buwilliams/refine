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
