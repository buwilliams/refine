use super::*;

#[test]
fn existing_web_server_hosts_hub_sites_and_keeps_management_routes() {
    let temp = unique_temp_dir("http-hub-sites");
    fs::create_dir_all(temp.join(".refine")).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp.clone());
    server.runtime_root = Some(temp.join("runtime"));
    let hub = server.hub_service().unwrap();
    let site = hub
        .save_site("reports", &json!({"name":"Reports"}))
        .unwrap();
    hub.save_asset(
        "reports",
        "index.html",
        b"<script src='./app.js'></script>",
        None,
        false,
    )
    .unwrap();
    let assets = hub.assets("reports").unwrap();
    hub.save_asset(
        "reports",
        "app.js",
        b"window.reportReady = true;",
        assets["revision"].as_str(),
        false,
    )
    .unwrap();
    let assets = hub.assets("reports").unwrap();
    hub.save_asset(
        "reports",
        "report + data.json",
        b"{}",
        assets["revision"].as_str(),
        false,
    )
    .unwrap();
    let assets = hub.assets("reports").unwrap();
    hub.save_asset(
        "reports",
        "history/index.html",
        b"Historical report",
        assets["revision"].as_str(),
        false,
    )
    .unwrap();
    let daemon = LocalHttpDaemon::new(server, None);
    let request = |path: &str| HttpRequest {
        method: "GET".into(),
        path: path.into(),
        headers: BTreeMap::new(),
        body: None,
    };
    assert_eq!(
        daemon
            .handle_wire_request(request("/hub/sites/reports/"))
            .status,
        404
    );
    assert_eq!(
        daemon
            .handle_wire_request(request("/hub/preview/reports/"))
            .status,
        200
    );
    hub.publish("reports", site["revision"].as_str().unwrap(), &[], true)
        .unwrap();
    let management = daemon.handle_wire_request(request("/api/hub/sites/reports"));
    assert_eq!(management.status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&management.body).unwrap()["item"]["id"],
        "reports"
    );
    let redirect = daemon.handle_wire_request(request("/hub/sites/reports"));
    assert_eq!(redirect.status, 308);
    assert!(
        redirect
            .extra_headers
            .contains(&("Location".into(), "/hub/sites/reports/".into()))
    );
    let script = daemon.handle_wire_request(request("/hub/sites/reports/app.js"));
    assert_eq!(
        daemon
            .handle_wire_request(request("/hub/sites/reports/history/"))
            .body,
        b"Historical report"
    );
    assert_eq!(script.status, 200);
    assert_eq!(script.content_type, "text/javascript; charset=utf-8");
    assert_eq!(
        daemon
            .handle_wire_request(request("/hub/sites/reports/report%20%2B%20data.json"))
            .status,
        200
    );
    let page = daemon.handle_wire_request(request("/hub/sites/reports/"));
    assert_eq!(page.status, 200);
    assert_eq!(page.content_type, "text/html; charset=utf-8");
    assert_eq!(
        daemon
            .handle_wire_request(request("/system/version"))
            .status,
        200
    );
    assert_ne!(
        daemon
            .handle_wire_request(request("/hub/sites/reports/%2e%2e/secret"))
            .status,
        200
    );
    let etag = page
        .extra_headers
        .iter()
        .find(|(key, _)| key == "ETag")
        .unwrap()
        .1
        .clone();
    let mut cached = request("/hub/sites/reports/");
    cached.headers.insert("if-none-match".into(), etag);
    assert_eq!(daemon.handle_wire_request(cached).status, 304);
    let current = hub.show("reports").unwrap();
    hub.publish("reports", current["revision"].as_str().unwrap(), &[], false)
        .unwrap();
    assert_eq!(
        daemon
            .handle_wire_request(request("/hub/sites/reports/"))
            .status,
        404
    );
    remove_temp_dir(temp);
}

#[test]
fn hub_uploads_cross_the_real_http_boundary_without_json_byte_arrays() {
    use base64::Engine as _;
    let temp = unique_temp_dir("http-hub-upload");
    fs::create_dir_all(temp.join(".refine")).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp.clone());
    server.runtime_root = Some(temp.join("runtime"));
    let hub = server.hub_service().unwrap();
    hub.save_site("reports", &json!({"name":"Reports"}))
        .unwrap();
    let bytes = vec![137_u8; 3 * 1024 * 1024];
    let body = serde_json::to_vec(&json!({"path":"large.bin","bytes_base64":base64::engine::general_purpose::STANDARD.encode(&bytes)})).unwrap();
    let daemon = LocalHttpDaemon::new(server, None);
    let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
    let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
    let handle = thread::spawn(move || daemon.serve_once(listener).unwrap());
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let header = format!(
        "PUT /api/hub/sites/reports/assets HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nX-Refine-API-Version: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        crate::application::protocol::API_CONTRACT_VERSION,
        body.len()
    );
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(&body).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    handle.join().unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(hub.asset("reports", "large.bin", false).unwrap().0, bytes);
    remove_temp_dir(temp);
}
