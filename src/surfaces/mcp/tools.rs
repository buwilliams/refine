//! The MCP tool catalog.
//!
//! Each tool maps a stable, agent-facing name onto a shared daemon API call so
//! MCP clients reuse the exact behavior of the CLI, browser, and HTTP API. New
//! tools should bind to an existing capability route rather than introduce
//! parallel logic here.

use serde_json::{Value, json};

/// A resolved daemon request produced from a tool call's arguments.
pub struct RequestParts {
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
}

/// How a tool turns its arguments into a daemon request.
enum ToolBinding {
    /// A fixed capability route. `{param}` segments in the path are filled from
    /// string arguments named in `path_params`.
    Api {
        method: &'static str,
        path: &'static str,
        path_params: &'static [&'static str],
    },
    /// Escape hatch: method, path, and body are taken from the arguments so an
    /// agent can reach any daemon route, including writes.
    Passthrough,
}

/// A capability the MCP surface exposes to clients.
pub struct McpTool {
    pub name: &'static str,
    pub description: &'static str,
    input_schema: fn() -> Value,
    binding: ToolBinding,
}

impl McpTool {
    /// The MCP `tools/list` descriptor for this tool.
    pub fn describe(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": (self.input_schema)(),
        })
    }

    /// Resolve a tool call's arguments into a concrete daemon request. Returns
    /// an error string (surfaced to the client as a tool error) when required
    /// arguments are missing or invalid.
    pub fn build_request(&self, arguments: &Value) -> Result<RequestParts, String> {
        match &self.binding {
            ToolBinding::Api {
                method,
                path,
                path_params,
            } => {
                let mut resolved = path.to_string();
                for param in *path_params {
                    let value = arguments
                        .get(param)
                        .and_then(Value::as_str)
                        .ok_or_else(|| format!("missing required string argument: {param}"))?;
                    if value.is_empty() || value.contains('/') {
                        return Err(format!("invalid value for argument '{param}'"));
                    }
                    resolved = resolved.replace(&format!("{{{param}}}"), value);
                }
                Ok(RequestParts {
                    method: method.to_string(),
                    path: resolved,
                    body: None,
                })
            }
            ToolBinding::Passthrough => {
                let path = arguments
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "refine_request requires a string 'path'".to_string())?;
                if !path.starts_with('/') {
                    return Err("'path' must start with '/'".to_string());
                }
                let method = arguments
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET")
                    .to_uppercase();
                let body = arguments
                    .get("body")
                    .cloned()
                    .filter(|body| !body.is_null());
                Ok(RequestParts {
                    method,
                    path: path.to_string(),
                    body,
                })
            }
        }
    }
}

/// The tools advertised to MCP clients. Reads are first-class; the
/// `refine_request` escape hatch covers everything else, including writes.
pub fn tool_catalog() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "refine_system_status",
            description: "Read the local Refine daemon status: health, worker state, active operations, and target app state.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/system/status",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_api_groups",
            description: "List the daemon API capability groups and what each one covers. Use this to discover routes for the refine_request tool.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/system/api-groups",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_dashboard",
            description: "Read the active project dashboard projection and diagnostic summary.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/dashboard",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_list_gaps",
            description: "List Gaps (units of product feedback and work) for the active app.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/work/gaps",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_show_gap",
            description: "Show a single Gap by id, including its current workflow state.",
            input_schema: gap_id_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/work/gaps/{gap_id}",
                path_params: &["gap_id"],
            },
        },
        McpTool {
            name: "refine_list_features",
            description: "List Features (grouped Gaps) for the active app.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/work/features",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_next",
            description: "Recommend the next operations for the active project and its fleet, each with the exact CLI command to run. Start here when deciding what to do: it reports unprovisioned or failed nodes, work that should be distributed, and reviewable work waiting to converge.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/guidance/next",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_request",
            description: "Escape hatch: call any local daemon API route directly. Provide 'method' (default GET), 'path' (e.g. /work/gaps), and an optional JSON 'body'. Discover routes with the refine_api_groups tool.",
            input_schema: request_schema,
            binding: ToolBinding::Passthrough,
        },
    ]
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    })
}

fn gap_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "gap_id": {
                "type": "string",
                "description": "Gap identifier, e.g. GAP1",
            },
        },
        "required": ["gap_id"],
        "additionalProperties": false,
    })
}

fn request_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "method": {
                "type": "string",
                "description": "HTTP method (default GET)",
            },
            "path": {
                "type": "string",
                "description": "Daemon API path, e.g. /work/gaps",
            },
            "body": {
                "description": "Optional JSON request body for writes",
            },
        },
        "required": ["path"],
        "additionalProperties": false,
    })
}
