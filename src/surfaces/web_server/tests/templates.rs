use super::*;

#[test]
fn templates_api_previews_defaults_and_revision_fences_edits() {
    let root = unique_temp_dir("templates-api");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(root.clone());
    let request = |method: &str, path: &str, body| {
        server.handle(ApiRequest {
            method: method.into(),
            path: path.into(),
            body,
        })
    };
    let show = request("GET", "/api/templates/workflow", None);
    assert_eq!(show.status, 200, "{show:?}");
    assert_eq!(show.body["item"]["revision"], 0);
    let preview = request(
        "POST",
        "/api/templates/workflow/preview",
        Some(
            json!({"prompt":"{{skill}}", "values":{"skill":{"template":"Run {{refine_executable}}: {{current_round_goal}}"},"refine_executable":"/bin/refine","current_round_goal":"Keep {{skill}}"}}),
        ),
    );
    assert_eq!(preview.body["prompt"], "Run /bin/refine: Keep {{skill}}");
    assert_eq!(
        request(
            "PUT",
            "/api/templates/workflow",
            Some(json!({"revision":0,"prompt":"Only {{skill}}"}))
        )
        .status,
        200
    );
    assert_eq!(
        request(
            "PUT",
            "/api/templates/workflow",
            Some(json!({"revision":0,"prompt":"stale"}))
        )
        .status,
        409
    );
    assert_eq!(
        request(
            "POST",
            "/api/templates/workflow/preview",
            Some(json!({"prompt":"{{unknown}}"}))
        )
        .status,
        400
    );
    assert_eq!(
        request("DELETE", "/api/templates/workflow", None).status,
        400
    );
    assert_eq!(
        request(
            "PUT",
            "/api/templates/new",
            Some(json!({"revision":0,"prompt":"new"}))
        )
        .status,
        404
    );
    assert_eq!(
        request("GET", "/api/templates/workflow", None).body["item"]["prompt"],
        "Only {{skill}}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn template_reset_api_restores_only_selected_templates() {
    let root = unique_temp_dir("template-reset-api");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(root.clone());
    let request = |method: &str, path: &str, body| {
        server.handle(ApiRequest {
            method: method.into(),
            path: path.into(),
            body,
        })
    };
    for id in ["workflow", "planning-agent"] {
        assert_eq!(
            request(
                "PUT",
                &format!("/api/templates/{id}"),
                Some(json!({"revision":0,"prompt":"Custom"}))
            )
            .status,
            200
        );
    }
    assert_eq!(
        request(
            "POST",
            "/api/templates/reset",
            Some(json!({"revisions":{"workflow":0}}))
        )
        .status,
        409
    );
    assert_eq!(
        request(
            "POST",
            "/api/templates/reset",
            Some(json!({"revisions":{"workflow":1}}))
        )
        .status,
        200
    );
    let shown = request("GET", "/api/templates/workflow", None);
    assert_eq!(shown.body["item"]["prompt"], shown.body["default_prompt"]);
    assert_eq!(shown.body["customized"], false);
    assert_eq!(
        request("GET", "/api/templates/planning-agent", None).body["item"]["prompt"],
        "Custom"
    );
    fs::remove_dir_all(root).unwrap();
}
