use super::*;

#[test]
fn configuration_routes_report_detached_target_app() {
    let server = server_with_projection();
    for path in [
        "/api/settings",
        "/api/quality",
        "/api/governance",
        "/api/guidance",
    ] {
        let response = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            body: None,
        });
        assert_eq!(response.status, 503, "unexpected response for {path}");
        assert_eq!(response.body["error"]["code"], "target_root_unavailable");
    }
}

#[test]
fn web_server_manages_governance_guidance_and_reporters() {
    let temp_root = unique_temp_dir("http-project-config");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let governance = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/governance".to_string(),
        body: Some(json!({
            "product": "Refine",
            "constitution": "Be useful",
            "rules": [{"text": "No regressions"}]
        })),
    });
    assert_eq!(governance.status, 200);
    assert_eq!(governance.body["configured"], true);
    assert_eq!(governance.body["rules"].as_array().unwrap().len(), 1);

    let generated = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/governance/generate-rules".to_string(),
        body: Some(json!({"product": "Refine", "constitution": "Be useful"})),
    });
    assert_eq!(generated.status, 200);
    assert_eq!(generated.body["ok"], true);
    assert!(generated.body["rules"].as_array().unwrap().len() >= 2);

    let guidance = server.handle(ApiRequest {
        method: "PUT".to_string(),
        path: "/api/guidance".to_string(),
        body: Some(json!({"guidance": [{
            "name": "Accessibility",
            "rule": "When UI changes",
            "instructions": "Check keyboard behavior",
            "enabled": true
        }]})),
    });
    assert_eq!(guidance.status, 200);
    assert_eq!(guidance.body["guidance"].as_array().unwrap().len(), 1);
    assert_eq!(guidance.body["revision"], 1);
    let guidance_id = guidance.body["guidance"][0]["id"].as_str().unwrap();
    let added = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/guidance".to_string(),
        body: Some(json!({
            "revision": 1,
            "name": "Cohesion",
            "rule": "When code changes",
            "instructions": "Keep files focused"
        })),
    });
    assert_eq!(added.status, 200);
    assert_eq!(added.body["revision"], 2);
    assert_eq!(added.body["guidance"].as_array().unwrap().len(), 2);
    let stale = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/guidance/{guidance_id}"),
        body: Some(json!({"revision": 1, "enabled": false})),
    });
    assert_eq!(stale.status, 409);
    let stale_list = server.handle(ApiRequest {
        method: "PUT".to_string(),
        path: "/api/guidance".to_string(),
        body: Some(json!({
            "revision": 1,
            "guidance": [{
                "id": guidance_id,
                "name": "Accessibility",
                "rule": "When UI changes",
                "instructions": "Overwrite concurrently added entry",
                "enabled": true
            }]
        })),
    });
    assert_eq!(stale_list.status, 409);
    let after_stale = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/guidance".to_string(),
        body: None,
    });
    assert_eq!(after_stale.body["guidance"].as_array().unwrap().len(), 2);
    let missing = server.handle(ApiRequest {
        method: "DELETE".to_string(),
        path: "/api/guidance/missing".to_string(),
        body: Some(json!({"revision": 2})),
    });
    assert_eq!(missing.status, 404);

    let stale_governance = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: "/api/governance".to_string(),
        body: Some(json!({"rules": [{"text": "Stale"}], "rules_revision": 0})),
    });
    assert_eq!(stale_governance.status, 409);

    let reporter_one = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/reporters".to_string(),
        body: Some(json!({"name": "Buddy"})),
    });
    assert_eq!(reporter_one.status, 201);
    let reporter_one_id = reporter_one.body["reporter"]["id"].as_u64().unwrap();
    let reporter_two = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/reporters".to_string(),
        body: Some(json!({"name": "Alex"})),
    });
    let reporter_two_id = reporter_two.body["reporter"]["id"].as_u64().unwrap();

    let renamed = server.handle(ApiRequest {
        method: "PATCH".to_string(),
        path: format!("/api/reporters/{reporter_one_id}"),
        body: Some(json!({"name": "Buddy Williams"})),
    });
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.body["new"], "Buddy Williams");

    let merged = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/reporters/{reporter_one_id}/merge"),
        body: Some(json!({"target_id": reporter_two_id})),
    });
    assert_eq!(merged.status, 200);
    assert_eq!(merged.body["ok"], true);

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/reporters".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["reporters"].as_array().unwrap().len(), 1);
    assert!(refine_dir.join("governance.json").exists());
    assert!(refine_dir.join("guidance.json").exists());
    assert!(refine_dir.join("reporters.json").exists());

    remove_temp_dir(&temp_root);
}
