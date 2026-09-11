use super::super::*;

pub(crate) fn hub_and_workflow_controls_share_the_daemon_surface(fixture: &IntegrationFixture) {
    let run = |args: &[&str]| {
        let output = fixture.run_refine(args);
        fixture.assert_success(&args.join(" "), &output);
        fixture.json_stdout(&output)
    };
    let input = fixture.artifact_root.join("hub-input.json");
    let input_path = input.to_str().unwrap();
    fs::write(&input, r#"{"name":"Usage reports"}"#).unwrap();
    let site = run(&["hub", "save", "usage", "--file", input_path]);
    assert_eq!(site["item"]["name"], "Usage reports");
    fs::write(&input, r#"{"indexes":{"fields":{"value":"number"}}}"#).unwrap();
    run(&["hub", "collection", "usage", "events", "--file", input_path]);
    fs::write(&input, r#"{"data":{"value":42},"request_id":"cli-event"}"#).unwrap();
    run(&["hub", "put", "usage", "events", "one", "--file", input_path]);
    assert_eq!(
        run(&["hub", "get", "usage", "events", "one"])["item"]["data"]["value"],
        42
    );
    fs::write(&input, r#"{"version":1,"limit":10}"#).unwrap();
    assert_eq!(
        run(&["hub", "query", "usage", "events", "--file", input_path])["total"],
        1
    );
    let export = fixture.artifact_root.join("hub-export.jsonl");
    run(&[
        "hub",
        "export",
        "usage",
        "events",
        "--file",
        export.to_str().unwrap(),
    ]);
    let exported: serde_json::Value =
        serde_json::from_str(fs::read_to_string(export).unwrap().trim()).unwrap();
    assert!(exported["revision"].is_string());
    assert!(exported["request_id"].is_null());
    let assets = fixture.artifact_root.join("hub-assets");
    fs::create_dir_all(&assets).unwrap();
    let binary = vec![137_u8; 3 * 1024 * 1024];
    fs::write(assets.join("large.bin"), &binary).unwrap();
    fs::write(assets.join("index.html"), "<h1>Usage report</h1>").unwrap();
    run(&["hub", "upload", "usage", assets.to_str().unwrap()]);
    let download = fixture.artifact_root.join("hub-download");
    run(&["hub", "download", "usage", download.to_str().unwrap()]);
    assert_eq!(fs::read(download.join("large.bin")).unwrap(), binary);
    let current = run(&["hub", "show", "usage"]);
    run(&[
        "hub",
        "publish",
        "usage",
        "--revision",
        current["revision"].as_str().unwrap(),
        "--collection",
        "events",
    ]);
    let status = run(&["hub", "status", "usage"]);
    assert!(status.is_object());
    let id = fixture.create_goal("Explicit workflow control");
    let goal = run(&["workflow", "show", &id]);
    let revision = goal["workflow_revision"].as_u64().unwrap_or(0).to_string();
    let args = [
        "workflow",
        "move",
        &id,
        "--to",
        "done",
        "--force",
        "--reason",
        "Status-only completion",
        "--expected-revision",
        &revision,
        "--request-id",
        "cli-decision",
    ];
    let receipt = run(&args);
    assert_eq!(receipt["integration_performed"], false);
    assert_eq!(run(&args), receipt);
    assert_eq!(run(&["workflow", "show", &id])["status"], "done");
    let mcp = fixture.api_json("POST", "/api/mcp", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"refine_hub_sites","arguments":{}}}));
    assert!(mcp["error"].is_null(), "{mcp}");
    assert_ne!(mcp["result"]["isError"], true, "{mcp}");
}
