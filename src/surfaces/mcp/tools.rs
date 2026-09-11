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
    /// A fixed capability route whose JSON body is assembled from tool arguments.
    BodyApi {
        method: &'static str,
        path: &'static str,
        required: &'static [&'static str],
        optional: &'static [&'static str],
        fixed: &'static [(&'static str, &'static str)],
    },
    /// Escape hatch: method, path, and body are taken from the arguments so an
    /// agent can reach any daemon route, including writes.
    Passthrough,
    SkillTrigger,
    WorkflowControl,
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
            ToolBinding::WorkflowControl => {
                let id = arguments["goal_id"].as_str().ok_or("goal_id is required")?;
                if id.is_empty()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    return Err("invalid goal_id".into());
                }
                let action = arguments["action"].as_str().unwrap_or("move");
                if !["move", "integrate"].contains(&action) {
                    return Err("action must be move or integrate".into());
                }
                let body = arguments
                    .get("decision")
                    .filter(|v| v.is_object())
                    .ok_or("decision object is required")?
                    .clone();
                Ok(RequestParts {
                    method: "POST".into(),
                    path: format!("/workflow/goals/{id}/{action}"),
                    body: Some(body),
                })
            }
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
            ToolBinding::BodyApi {
                method,
                path,
                required,
                optional,
                fixed,
            } => {
                let mut body = serde_json::Map::new();
                for name in *required {
                    let value = arguments
                        .get(name)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| format!("missing required string argument: {name}"))?;
                    body.insert((*name).to_string(), json!(value));
                }
                for name in *optional {
                    if let Some(value) = arguments
                        .get(name)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        body.insert((*name).to_string(), json!(value));
                    }
                }
                for (name, value) in *fixed {
                    body.insert((*name).to_string(), json!(value));
                }
                Ok(RequestParts {
                    method: method.to_string(),
                    path: path.to_string(),
                    body: Some(Value::Object(body)),
                })
            }
            ToolBinding::SkillTrigger => {
                let id = arguments
                    .get("skill_id")
                    .and_then(Value::as_str)
                    .filter(|id| crate::model::automation::valid_id(id))
                    .ok_or_else(|| "a valid skill_id is required".to_string())?;
                let mut body = arguments
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "arguments must be an object".to_string())?;
                body.remove("skill_id");
                Ok(RequestParts {
                    method: "POST".into(),
                    path: format!("/skills/{id}/trigger"),
                    body: Some(Value::Object(body)),
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
            name: "refine_list_skills",
            description: "Read reusable Skill instructions, parameters and configuration revisions.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/skills",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_skill_triggers",
            description: "List the automatic and Custom trigger types available to Skills.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/skills/catalog",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_trigger_skill",
            description: "Run one Skill with a Custom trigger on the active node, independently of Goals. Discover required inputs at /skills/{skill_id}/inputs. Reuse request_id to retry transport without duplicating work.",
            input_schema: || json!({"type":"object","properties":{"skill_id":{"type":"string"},"node_id":{"type":"string"},"request_id":{"type":"string"},"parameters":{"type":"object"}},"required":["skill_id"],"additionalProperties":false}),
            binding: ToolBinding::SkillTrigger,
        },
        McpTool {
            name: "refine_workflow_control",
            description: "Move or explicitly integrate a Goal through shared outcome controls. Read the current workflow_revision first. No Skill is required; forced actions retain override evidence.",
            input_schema: || json!({"type":"object","properties":{"goal_id":{"type":"string"},"action":{"enum":["move","integrate"]},"decision":{"type":"object","properties":{"to":{"type":"string"},"reason":{"type":"string"},"expected_revision":{"type":"integer"},"request_id":{"type":"string"},"force":{"type":"boolean"},"context":{"type":"string"},"actor":{"type":"string"},"invocation_id":{"type":"string"}},"required":["to","reason","expected_revision","request_id"]}},"required":["goal_id","decision"]}),
            binding: ToolBinding::WorkflowControl,
        },
        McpTool {
            name: "refine_hub_sites",
            description: "List Knowledge Hub sites. Manage assets, collections, records, indexes, queries and publication through /api/hub routes with refine_request.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/api/hub/sites",
                path_params: &[],
            },
        },
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
            name: "refine_list_goals",
            description: "List Goals (units of product feedback and work) for the active app.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/work/goals",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_show_goal",
            description: "Show authoritative Goal detail by id, including Round implementation plans, evidence, and current workflow state.",
            input_schema: goal_id_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/work/goals/{goal_id}",
                path_params: &["goal_id"],
            },
        },
        McpTool {
            name: "refine_stop_process",
            description: "Stop one managed agent or other process through the shared process-control capability. Linked Goals are cancelled only after exact process exit is confirmed.",
            input_schema: process_id_schema,
            binding: ToolBinding::Api {
                method: "POST",
                path: "/processes/{process_id}/stop",
                path_params: &["process_id"],
            },
        },
        McpTool {
            name: "refine_export_goal_jira",
            description: "Export a Goal's SOC 2 delivery evidence as Jira-importable CSV, including implementation reports and commits.",
            input_schema: goal_id_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/work/goals/{goal_id}/export/jira",
                path_params: &["goal_id"],
            },
        },
        McpTool {
            name: "refine_draft_goal",
            description: "Draft exactly one implementation-ready Goal from a Plan transcript for review. This does not persist the Goal.",
            input_schema: plan_goal_draft_schema,
            binding: ToolBinding::BodyApi {
                method: "POST",
                path: "/import/extract",
                required: &["text"],
                optional: &["reporter", "provider"],
                fixed: &[("purpose", "plan_goal")],
            },
        },
        McpTool {
            name: "refine_list_features",
            description: "List Features (grouped Goals) for the active app.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/work/features",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_next",
            description: "Recommend the next operations for the active project and its fleet, each with the exact CLI command to run. Start here when deciding what to do: it reports failed nodes, work that should be distributed, and reviewable work waiting to converge.",
            input_schema: empty_schema,
            binding: ToolBinding::Api {
                method: "GET",
                path: "/next",
                path_params: &[],
            },
        },
        McpTool {
            name: "refine_request",
            description: "Escape hatch: call any local daemon API route directly. Provide 'method' (default GET), 'path' (e.g. /work/goals), and an optional JSON 'body'. Discover routes with the refine_api_groups tool.",
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

fn goal_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "goal_id": {
                "type": "string",
                "description": "Goal identifier, e.g. GOAL1",
            },
        },
        "required": ["goal_id"],
        "additionalProperties": false,
    })
}

fn process_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "process_id": {
                "type": "string",
                "description": "Managed or synthetic process identifier",
            },
        },
        "required": ["process_id"],
        "additionalProperties": false,
    })
}

fn plan_goal_draft_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "text": {
                "type": "string",
                "description": "Plan transcript to turn into one reviewable Goal draft",
            },
            "reporter": {
                "type": "string",
                "description": "Reporter to include in the drafted Goal",
            },
            "provider": {
                "type": "string",
                "description": "Configured AI provider to use for extraction",
            },
        },
        "required": ["text"],
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
                "description": "Daemon API path, e.g. /work/goals",
            },
            "body": {
                "description": "Optional JSON request body for writes",
            },
        },
        "required": ["path"],
        "additionalProperties": false,
    })
}
