use super::*;

#[test]
fn configuration_routes_report_detached_target_app() {
    let server = server_with_projection();
    for path in ["/api/settings", "/api/event-definitions", "/api/skills"] {
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
fn web_server_manages_events_skills_and_reporters() {
    let temp_root = unique_temp_dir("http-project-config");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let catalog = server.handle(ApiRequest {
        method: "GET".into(),
        path: "/api/event-definitions/catalog".into(),
        body: None,
    });
    assert_eq!(catalog.status, 200);
    assert_eq!(catalog.body["sources"].as_array().unwrap().len(), 21);
    let skills = server.handle(ApiRequest {
        method: "GET".into(),
        path: "/api/skills".into(),
        body: None,
    });
    assert_eq!(skills.status, 200);
    let revision = skills.body["revision"].clone();
    let saved = server.handle(ApiRequest { method: "PUT".into(), path: "/api/skills/accessibility".into(), body: Some(json!({"revision": revision,"item":{"name":"Accessibility", "prompt":"Check keyboard behavior", "role":"task"}})) });
    assert_eq!(saved.status, 200);
    let stale = server.handle(ApiRequest {
        method: "PUT".into(),
        path: "/api/skills/accessibility".into(),
        body: Some(
            json!({"revision":revision,"item":{"name":"Stale", "prompt":"Stale", "role":"task"}}),
        ),
    });
    assert_eq!(stale.status, 409);
    let event = server.handle(ApiRequest { method:"PUT".into(), path:"/api/event-definitions/check-ui".into(), body:Some(json!({"revision":saved.body["revision"],"item":{"name":"Check UI", "kind":"custom", "bindings":[{"id":"check", "skill_id":"accessibility"}]}})) });
    assert_eq!(event.status, 200);
    let referenced = server.handle(ApiRequest {
        method: "DELETE".into(),
        path: "/api/skills/accessibility".into(),
        body: Some(json!({"revision":event.body["revision"]})),
    });
    assert_eq!(referenced.status, 409);
    for retired in ["/api/quality", "/api/governance", "/api/guidance"] {
        assert_eq!(
            server
                .handle(ApiRequest {
                    method: "GET".into(),
                    path: retired.into(),
                    body: None
                })
                .status,
            404
        );
    }

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
    assert!(refine_dir.join("automation/config.json").exists());
    assert!(refine_dir.join("reporters.json").exists());

    remove_temp_dir(&temp_root);
}
