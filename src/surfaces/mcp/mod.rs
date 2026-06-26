//! MCP (Model Context Protocol) surface.
//!
//! Exposes Refine's local daemon capabilities to MCP clients over JSON-RPC 2.0.
//! The surface is always available: the daemon web server mounts it at
//! [`MCP_ROUTE`], so MCP boots with the daemon and shares its lifetime. It does
//! not own any capability of its own — every tool call is dispatched back
//! through the same shared daemon API the CLI, browser, and HTTP surfaces use.

use serde_json::{Value, json};

#[cfg(test)]
mod tests;
mod tools;

pub use tools::{McpTool, RequestParts, tool_catalog};

/// MCP protocol revision this surface implements.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Stable server identity reported to MCP clients.
pub const MCP_SERVER_NAME: &str = "refine";
/// Daemon route the MCP surface is served from. Always mounted, so MCP is
/// available whenever the daemon is running.
pub const MCP_ROUTE: &str = "/mcp";

/// Outcome of dispatching a tool call into the shared daemon API.
pub struct McpDispatchResult {
    pub status: u16,
    pub body: Value,
}

/// Bridge from MCP tool calls to the shared daemon API. The web server
/// implements this so the MCP surface reuses the exact same capability
/// dispatch as every other surface instead of reimplementing behavior.
pub trait McpToolDispatcher {
    fn dispatch_tool(&self, method: &str, path: &str, body: Option<Value>) -> McpDispatchResult;
}

/// Handle one MCP JSON-RPC payload. A payload may be a single message or a
/// batch array. Returns `Some(response)` for requests and `None` when the
/// payload contained only notifications (which take no reply).
pub fn handle_payload(payload: &Value, dispatcher: &dyn McpToolDispatcher) -> Option<Value> {
    match payload {
        Value::Array(messages) => {
            let responses: Vec<Value> = messages
                .iter()
                .filter_map(|message| handle_message(message, dispatcher))
                .collect();
            if responses.is_empty() {
                None
            } else {
                Some(Value::Array(responses))
            }
        }
        message => handle_message(message, dispatcher),
    }
}

/// Handle a single MCP JSON-RPC message. Returns `Some(response)` for requests
/// (messages carrying an `id`) and `None` for notifications.
fn handle_message(message: &Value, dispatcher: &dyn McpToolDispatcher) -> Option<Value> {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id.map(|id| error_response(id, -32600, "invalid MCP request: missing method"));
    };

    match method {
        "initialize" => id.map(|id| success(id, initialize_result())),
        "ping" => id.map(|id| success(id, json!({}))),
        "tools/list" => id.map(|id| success(id, tools_list_result())),
        "tools/call" => id.map(|id| handle_tool_call(id, message.get("params"), dispatcher)),
        // Notifications (no id) such as `notifications/initialized` need no reply.
        _ if id.is_none() => None,
        _ => id.map(|id| error_response(id, -32601, &format!("method not found: {method}"))),
    }
}

fn handle_tool_call(
    id: Value,
    params: Option<&Value>,
    dispatcher: &dyn McpToolDispatcher,
) -> Value {
    let Some(params) = params else {
        return error_response(id, -32602, "tools/call requires params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, -32602, "tools/call requires a tool name");
    };
    let catalog = tool_catalog();
    let Some(tool) = catalog.iter().find(|tool| tool.name == name) else {
        return error_response(id, -32602, &format!("unknown tool: {name}"));
    };

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    // Argument and execution failures are tool errors (reported inside the
    // result), not protocol errors, per the MCP tool-call contract.
    match tool.build_request(&arguments) {
        Ok(request) => {
            let result = dispatcher.dispatch_tool(&request.method, &request.path, request.body);
            success(id, tool_call_result(result))
        }
        Err(message) => success(id, tool_error_result(&message)),
    }
}

fn tool_call_result(result: McpDispatchResult) -> Value {
    let is_error = result.status >= 400;
    let text =
        serde_json::to_string_pretty(&result.body).unwrap_or_else(|_| result.body.to_string());
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": result.body,
        "isError": is_error,
    })
}

fn tool_error_result(message: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": message}],
        "isError": true,
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": MCP_SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn tools_list_result() -> Value {
    let tools: Vec<Value> = tool_catalog().iter().map(McpTool::describe).collect();
    json!({ "tools": tools })
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
