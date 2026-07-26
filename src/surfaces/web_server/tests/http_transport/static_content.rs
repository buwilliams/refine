use super::*;

#[test]
fn local_http_daemon_serves_website_and_markdown_from_repo_root() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: Some(repo_root),
    };

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
    assert!(docs_home.contains("<h1 id=\"docs-home-title\">How Refine works.</h1>"));
    assert!(docs_home.contains("Browser Details"));
    assert!(docs_home.contains(r#"href="/read/docs/intent/04-surfaces/05-agent.md""#));

    let raw_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/docs/runbooks/install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(raw_doc.status, 200);
    assert_eq!(raw_doc.content_type, "text/markdown; charset=utf-8");
    assert!(
        String::from_utf8(raw_doc.body)
            .unwrap()
            .contains("# Install Refine")
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
            .contains("docs/runbooks/install.md")
    );

    let rendered_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/read/docs/runbooks/install.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(rendered_doc.status, 200);
    assert_eq!(rendered_doc.content_type, "text/html; charset=utf-8");
    let rendered_doc = String::from_utf8(rendered_doc.body).unwrap();
    assert!(rendered_doc.contains("<h1>Install Refine</h1>"));
    assert!(rendered_doc.contains("Raw Markdown"));
    assert!(
        rendered_doc.contains(r#"<div class="menu-docs" aria-label="Documentation sections">"#)
    );
    assert!(!rendered_doc.contains(r#"class="reader-nav""#));
    assert_eq!(rendered_doc.matches(r#"class="doc-pager""#).count(), 2);
    assert!(rendered_doc.contains(r#">Docs home</a>"#));
    assert!(rendered_doc.contains(r#"href="/docs""#));
    assert!(rendered_doc.contains("/read/docs/intent/02-foundation/01-node.md"));

    let design_doc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/read/docs/intent/01-design.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(design_doc.status, 200);
    let design_doc = String::from_utf8(design_doc.body).unwrap();
    assert_eq!(design_doc.matches(r#"class="doc-pager""#).count(), 2);
    assert!(design_doc.contains(
        r#"<a class="doc-pager-link" href="/read/docs/intent/README.md"><span>Previous</span><strong>Design Intent</strong></a>"#
    ));
    assert!(
        design_doc.contains(r#"<a class="doc-pager-link" href="/read/docs/intent/02-foundation/01-node.md"><span>Next</span><strong>Node</strong></a>"#)
    );

    let intent_toc = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/read/docs/intent/README.md".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(intent_toc.status, 200);
    let intent_toc = String::from_utf8(intent_toc.body).unwrap();
    assert!(intent_toc.contains("<h1>Design Intent</h1>"));
    assert!(!intent_toc.contains("<h1>Table of Contents</h1>"));
    assert!(intent_toc.contains(r#"href="/read/docs/intent/01-design.md""#));
    assert!(
        intent_toc
            .contains(r#"href="/read/docs/intent/03-capabilities/03-workflow/00-overview.md""#)
    );

    let hidden = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/Cargo.toml".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_ne!(hidden.status, 200);
}
