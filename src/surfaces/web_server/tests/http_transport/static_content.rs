use super::*;

#[test]
fn local_http_daemon_serves_website_and_markdown_from_repo_root() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let daemon = LocalHttpDaemon::new(server_with_projection(), Some(repo_root));

    let index = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(index.status, 200);
    assert_eq!(index.content_type, "text/html; charset=utf-8");
    assert!(
        String::from_utf8(index.body)
            .unwrap()
            .contains("Agentic Software Delivery")
    );

    let docs_home = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/docs".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(docs_home.status, 200);
    assert_eq!(docs_home.content_type, "text/html; charset=utf-8");
    let docs_home = String::from_utf8(docs_home.body).unwrap();
    assert!(docs_home.contains("<h1>From your first Goal<br>to a fleet of agents.</h1>"));
    assert!(docs_home.contains("What you need to know"));
    assert!(docs_home.contains("/hub/sites/refine/product/get-started.md"));
    assert!(docs_home.contains("/hub/sites/refine/releases/4.3.2.md"));
    assert!(!docs_home.contains("/hub/sites/refine/releases/4.3.0.md"));

    let raw_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/refine-hub/docs/runbooks/install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(raw_doc.status, 200);
    assert_eq!(raw_doc.content_type, "text/markdown; charset=utf-8");
    assert!(
        String::from_utf8(raw_doc.body)
            .unwrap()
            .contains("# Install or Update Refine")
    );

    let compatibility_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/docs/agent-install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(compatibility_doc.status, 200);
    assert!(
        String::from_utf8(compatibility_doc.body)
            .unwrap()
            .contains("runbooks/install.md")
    );

    let rendered_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/read/refine-hub/docs/runbooks/install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(rendered_doc.status, 200);
    assert_eq!(rendered_doc.content_type, "text/html; charset=utf-8");
    let rendered_doc = String::from_utf8(rendered_doc.body).unwrap();
    assert!(rendered_doc.contains("<h1>Install or Update Refine</h1>"));
    assert!(rendered_doc.contains("Refine Hub"));
    assert!(rendered_doc.contains("<base href=\"/hub/sites/refine/docs/runbooks/\">"));
    // Old public documentation URLs still resolve to the same bundled source.
    for path in [
        "/read/docs/runbooks/install.md",
        "/docs/runbooks/install.md",
        "/hub/sites/refine/docs/runbooks/install.md",
    ] {
        let response = daemon.handle_wire_request(HttpRequest {
            method: "GET".into(),
            path: path.into(),
            headers: BTreeMap::new(),
            body: None,
        });
        assert_eq!(response.status, 200, "{path}");
        assert!(
            String::from_utf8(response.body)
                .unwrap()
                .contains("Install or Update Refine")
        );
    }

    let hidden = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/Cargo.toml".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_ne!(hidden.status, 200);
}

#[test]
fn refine_hub_is_available_without_a_target_and_rejects_mutation() {
    let daemon = LocalHttpDaemon::new(server_with_projection(), None);
    for path in [
        "/hub/sites/refine/",
        "/hub/sites/refine/product/get-started.md",
        "/hub/sites/refine/hub.css",
        "/hub/sites/refine/releases/images/4.3/prompts.png",
        "/hub/sites/refine/releases/4.3.2.md",
        "/hub/sites/refine/releases/images/4.3.2/dashboard.png",
        "/hub/sites/refine/releases/images/4.3.2/files.png",
        "/api/hub/sites",
        "/api/hub/sites/refine",
    ] {
        let response = daemon.handle_wire_request(HttpRequest {
            method: "GET".into(),
            path: path.into(),
            headers: BTreeMap::new(),
            body: None,
        });
        assert_eq!(
            response.status,
            200,
            "{path}: {}",
            String::from_utf8_lossy(&response.body)
        );
    }
    for (method, path) in [
        ("DELETE", "/api/hub/sites/refine"),
        ("PUT", "/api/hub/sites/refine/assets"),
        ("POST", "/api/hub/sites/refine/unpublish"),
        ("PUT", "/api/hub/sites/refine/collections/docs"),
    ] {
        let response = daemon.handle_wire_request(HttpRequest {
            method: method.into(),
            path: path.into(),
            headers: BTreeMap::new(),
            body: Some(b"{}".to_vec()),
        });
        assert!(response.status >= 400, "{method} {path}");
        assert!(String::from_utf8_lossy(&response.body).contains("read-only"));
    }
    let first = daemon.handle_wire_request(HttpRequest {
        method: "GET".into(),
        path: "/hub/sites/refine/".into(),
        headers: BTreeMap::new(),
        body: None,
    });
    let etag = first
        .extra_headers
        .iter()
        .find(|(key, _)| key == "ETag")
        .unwrap()
        .1
        .clone();
    let cached = daemon.handle_wire_request(HttpRequest {
        method: "GET".into(),
        path: "/hub/sites/refine/".into(),
        headers: BTreeMap::from([("if-none-match".into(), etag)]),
        body: None,
    });
    assert_eq!(cached.status, 304);
}
