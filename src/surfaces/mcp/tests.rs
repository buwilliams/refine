use super::*;
use serde_json::json;

/// Records the daemon request a tool resolves to and echoes it back so tests
/// can assert how MCP arguments map onto capability routes.
struct EchoDispatcher;

impl McpToolDispatcher for EchoDispatcher {
    fn dispatch_tool(&self, method: &str, path: &str, body: Option<Value>) -> McpDispatchResult {
        McpDispatchResult {
            status: 200,
            body: json!({"method": method, "path": path, "body": body}),
        }
    }
}

/// Dispatcher that mimics a daemon error response so error mapping is covered.
struct FailingDispatcher;

impl McpToolDispatcher for FailingDispatcher {
    fn dispatch_tool(&self, _method: &str, _path: &str, _body: Option<Value>) -> McpDispatchResult {
        McpDispatchResult {
            status: 404,
            body: json!({"error": {"code": "not_found"}}),
        }
    }
}

fn call(message: Value) -> Value {
    handle_payload(&message, &EchoDispatcher).expect("request should produce a response")
}

#[test]
fn initialize_reports_protocol_version_and_server_identity() {
    let response = call(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}));
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
    assert_eq!(result["serverInfo"]["name"], MCP_SERVER_NAME);
    assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(result["capabilities"]["tools"].is_object());
}

#[test]
fn tools_list_exposes_the_catalog() {
    let response = call(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools = response["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"refine_system_status"));
    assert!(names.contains(&"refine_list_goals"));
    assert!(names.contains(&"refine_request"));
    // Every advertised tool must carry an input schema.
    assert!(tools.iter().all(|tool| tool["inputSchema"].is_object()));
}

#[test]
fn tool_call_maps_a_read_tool_onto_its_capability_route() {
    let response = call(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "refine_system_status", "arguments": {}},
    }));
    let result = &response["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["method"], "GET");
    assert_eq!(result["structuredContent"]["path"], "/system/status");
    assert!(result["content"][0]["text"].is_string());
}

#[test]
fn tool_call_substitutes_path_parameters() {
    let response = call(json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {"name": "refine_show_goal", "arguments": {"goal_id": "GOAL1"}},
    }));
    assert_eq!(
        response["result"]["structuredContent"]["path"],
        "/work/goals/GOAL1"
    );
}

#[test]
fn passthrough_tool_forwards_method_path_and_body() {
    let response = call(json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "refine_request",
            "arguments": {"method": "post", "path": "/work/goals", "body": {"name": "x"}},
        },
    }));
    let echoed = &response["result"]["structuredContent"];
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["path"], "/work/goals");
    assert_eq!(echoed["body"]["name"], "x");
}

#[test]
fn missing_path_parameter_is_a_tool_error_not_a_protocol_error() {
    let response = call(json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {"name": "refine_show_goal", "arguments": {}},
    }));
    assert!(response.get("error").is_none());
    assert_eq!(response["result"]["isError"], true);
}

#[test]
fn daemon_error_status_marks_the_tool_result_as_an_error() {
    let response = handle_payload(
        &json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {"name": "refine_system_status", "arguments": {}},
        }),
        &FailingDispatcher,
    )
    .unwrap();
    assert_eq!(response["result"]["isError"], true);
}

#[test]
fn unknown_tool_is_reported_as_an_invalid_params_error() {
    let response = call(json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {"name": "does_not_exist", "arguments": {}},
    }));
    assert_eq!(response["error"]["code"], -32602);
}

#[test]
fn unknown_method_returns_method_not_found() {
    let response = call(json!({"jsonrpc": "2.0", "id": 9, "method": "no/such/method"}));
    assert_eq!(response["error"]["code"], -32601);
}

#[test]
fn notifications_receive_no_reply() {
    let outcome = handle_payload(
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        &EchoDispatcher,
    );
    assert!(outcome.is_none());
}

#[test]
fn batch_payloads_answer_each_request() {
    let outcome = handle_payload(
        &json!([
            {"jsonrpc": "2.0", "id": 1, "method": "ping"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        ]),
        &EchoDispatcher,
    )
    .unwrap();
    let responses = outcome.as_array().unwrap();
    // The notification produces no reply, so only the two requests are answered.
    assert_eq!(responses.len(), 2);
}
