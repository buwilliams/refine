use crate::process::supervisor::config::{
    ConfigService, FileGovernanceService, FileGuidanceService, FileReporterService,
    FileSettingsService,
};
use std::collections::BTreeMap;
use std::thread;

use chrono::Utc;
use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{FileProcessSupervisor, ProcessOwner, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{current_launch_executable, current_launch_mode};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::cluster::{ClusterService, FileClusterService, NodeRemoteUpdate};
use crate::tools::host::git_sync::{FileGitSyncService, GitSyncResult};
use crate::tools::host::target_apps::TargetAppGeneratedConfig;
use crate::tools::product::next_actions::FileNextActionsService;
use crate::tools::product::nodes::{FileNodeRegistryService, NodeUpdate, detached_nodes_response};
use crate::tools::product::project_registry::{ProjectRegistryService, registry_apps_array};
use crate::tools::product::project_state::{DashboardProjectionQuery, ProjectionQuery};
use crate::tools::product::work_items::BulkGoalSelection;
use crate::workflow::WorkflowEngine;

use super::support::*;
use super::*;

fn configured_provider_from_settings(
    refine_dir: &std::path::Path,
    active_root: Option<&std::path::Path>,
    body: &Value,
) -> String {
    body.get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let service = match active_root {
                Some(active_root) => FileSettingsService::with_active_root(refine_dir, active_root),
                None => FileSettingsService::new(refine_dir),
            };
            service.load().ok().and_then(|settings| {
                settings
                    .get("agent_cli")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|provider| !provider.is_empty())
                    .map(str::to_string)
            })
        })
        .or_else(|| {
            provider_status_value().ok().and_then(|status| {
                status
                    .get("selected_provider")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|provider| !provider.is_empty())
                    .map(str::to_string)
            })
        })
        .unwrap_or_else(|| "claude".to_string())
}

fn dashboard_attention_items(indicators: &[String], runner_reachable: bool) -> Vec<Value> {
    let mut items = indicators
        .iter()
        .map(|message| {
            json!({
                "kind": "filter",
                "severity": "warn",
                "message": message
            })
        })
        .collect::<Vec<_>>();
    if !runner_reachable {
        items.push(json!({
            "kind": "banner",
            "severity": "error",
            "message": "Refine cannot reach the runtime worker. Re-check auth after restoring provider access."
        }));
    }
    items
}

fn dashboard_active_node(service: &FileNodeRegistryService) -> RefineResult<(String, String)> {
    let active_node_id = service.active_node_id()?;
    if active_node_id == "default" {
        return Ok((active_node_id, "Default".to_string()));
    }
    let active_node_display_name = service
        .show(&active_node_id)
        .ok()
        .and_then(|value| {
            value
                .get("node")
                .and_then(|node| node.get("display_name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| active_node_id.clone());
    Ok((active_node_id, active_node_display_name))
}

fn governance_generation_prompt(product: &str, constitution: &str) -> String {
    format!(
        "Generate governance rules for this project. Return only concise rules, one per line, \
         or JSON with a rules array.\n\nProduct:\n{product}\n\nConstitution:\n{constitution}"
    )
}

fn target_app_generation_prompt(target_root: &std::path::Path) -> String {
    format!(
        "Analyze this target app codebase and generate agent instructions for Refine target-app lifecycle work. \
         Return only JSON with kind=target-app and fields start_instructions, stop_instructions, \
         build_instructions, test_command, status_command, cwd, env, \
         start_timeout_seconds, stop_timeout_seconds, build_timeout_seconds, \
         test_timeout_seconds, status_timeout_seconds, log_path, http_check_url, tcp_check_host, tcp_check_port, \
         process_check_command, and notes. The start/stop/build fields are instructions for an agent, not shell commands. \
         Test and status fields may remain commands because Refine uses them as deterministic checks.\n\nProject root: {}",
        target_root.display()
    )
}

fn target_config_string(value: &Value, key: &str, fallback: &str) -> String {
    let legacy_key = match key {
        "build_command" => Some("rebuild_command"),
        _ => None,
    };
    value
        .get(key)
        .or_else(|| legacy_key.and_then(|key| value.get(key)))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .trim()
        .to_string()
}

fn target_config_u64(value: &Value, key: &str, fallback: u64) -> u64 {
    let legacy_key = match key {
        "build_timeout_seconds" => Some("rebuild_timeout_seconds"),
        _ => None,
    };
    value
        .get(key)
        .or_else(|| legacy_key.and_then(|key| value.get(key)))
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .get(key)
                .and_then(Value::as_str)
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
        .unwrap_or(fallback)
}

fn parse_generated_target_app_config(output: &str) -> Option<TargetAppGeneratedConfig> {
    let value = serde_json::from_str::<Value>(output).ok()?;
    let cfg = value.get("config").unwrap_or(&value);
    let env = cfg
        .get("env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let start_instructions = first_non_empty(
        &target_config_string(cfg, "start_instructions", ""),
        &target_config_string(cfg, "start_command", ""),
    );
    let stop_instructions = first_non_empty(
        &target_config_string(cfg, "stop_instructions", ""),
        &target_config_string(cfg, "stop_command", ""),
    );
    let build_instructions = first_non_empty(
        &target_config_string(cfg, "build_instructions", ""),
        &first_non_empty(
            &target_config_string(cfg, "rebuild_instructions", ""),
            &target_config_string(cfg, "build_command", ""),
        ),
    );
    let test_command = target_config_string(cfg, "test_command", "");
    let status_command = target_config_string(cfg, "status_command", "");
    if start_instructions.is_empty()
        && build_instructions.is_empty()
        && test_command.is_empty()
        && status_command.is_empty()
    {
        return None;
    }
    Some(TargetAppGeneratedConfig {
        start_instructions,
        stop_instructions,
        build_instructions,
        start_command: String::new(),
        stop_command: String::new(),
        build_command: String::new(),
        test_command,
        status_command,
        cwd: target_config_string(cfg, "cwd", "."),
        env,
        start_timeout_seconds: target_config_u64(cfg, "start_timeout_seconds", 120),
        stop_timeout_seconds: target_config_u64(cfg, "stop_timeout_seconds", 60),
        build_timeout_seconds: target_config_u64(cfg, "build_timeout_seconds", 300),
        test_timeout_seconds: target_config_u64(cfg, "test_timeout_seconds", 600),
        status_timeout_seconds: target_config_u64(cfg, "status_timeout_seconds", 10),
        log_path: target_config_string(cfg, "log_path", ""),
        http_check_url: target_config_string(cfg, "http_check_url", ""),
        tcp_check_host: target_config_string(cfg, "tcp_check_host", ""),
        tcp_check_port: target_config_string(cfg, "tcp_check_port", ""),
        process_check_command: target_config_string(cfg, "process_check_command", ""),
        notes: target_config_string(cfg, "notes", ""),
    })
}

fn target_app_generated_settings(config: &TargetAppGeneratedConfig) -> Value {
    json!({
        "target_app_start_instructions": config.start_instructions.clone(),
        "target_app_stop_instructions": config.stop_instructions.clone(),
        "target_app_build_instructions": config.build_instructions.clone(),
        "target_app_start_command": config.start_command.clone(),
        "target_app_stop_command": config.stop_command.clone(),
        "target_app_build_command": config.build_command.clone(),
        "target_app_test_command": config.test_command.clone(),
        "target_app_test_commands": if config.test_command.trim().is_empty() {
            String::new()
        } else {
            json!([{"command": config.test_command.clone(), "enabled": true}]).to_string()
        },
        "target_app_status_command": config.status_command.clone(),
        "target_app_cwd": config.cwd.clone(),
        "target_app_env_json": serde_json::to_string_pretty(&config.env).unwrap_or_else(|_| "{}".to_string()),
        "target_app_start_timeout_seconds": config.start_timeout_seconds.to_string(),
        "target_app_stop_timeout_seconds": config.stop_timeout_seconds.to_string(),
        "target_app_build_timeout_seconds": config.build_timeout_seconds.to_string(),
        "target_app_test_timeout_seconds": config.test_timeout_seconds.to_string(),
        "target_app_status_timeout_seconds": config.status_timeout_seconds.to_string(),
        "target_app_log_path": config.log_path.clone(),
        "target_app_http_check_url": config.http_check_url.clone(),
        "target_app_tcp_check_host": config.tcp_check_host.clone(),
        "target_app_tcp_check_port": config.tcp_check_port.clone(),
        "target_app_process_check_command": config.process_check_command.clone()
    })
}

fn generated_governance_rule(text: &str, index: usize) -> Value {
    let timestamp = Utc::now().to_rfc3339();
    json!({
        "id": format!("generated-rule-{}-{index}", Utc::now().timestamp_millis()),
        "text": text.chars().take(500).collect::<String>(),
        "created": timestamp,
        "updated": timestamp,
        "source": "generated"
    })
}

fn parse_generated_governance_rules(output: &str) -> Vec<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        let rules = value
            .get("rules")
            .or_else(|| value.get("items"))
            .unwrap_or(&value);
        if let Some(items) = rules.as_array() {
            let parsed = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let text = item
                        .get("text")
                        .or_else(|| item.get("rule"))
                        .and_then(Value::as_str)
                        .or_else(|| item.as_str())?
                        .trim();
                    (!text.is_empty()).then(|| generated_governance_rule(text, index + 1))
                })
                .collect::<Vec<_>>();
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }

    output
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(|ch: char| {
                    ch == '-' || ch == '*' || ch.is_ascii_digit() || ch == '.'
                })
                .trim()
        })
        .filter(|line| !line.is_empty())
        .enumerate()
        .map(|(index, line)| generated_governance_rule(line, index + 1))
        .collect()
}

impl InProcessWebServer {
    pub(super) fn handle_dashboard(&self, raw_path: &str) -> ApiResponse {
        let current_target_root = match self.current_target_root() {
            Ok(value) => value,
            Err(error) => return error_response(error),
        };
        let attached = current_target_root.is_some();
        let projection = match self.current_projection_with_runtime() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let process = runtime_process_summary_value(&projection.runtime);
        let preflight = projection
            .runtime
            .preflight
            .clone()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"ok": false, "providers": []}));
        let node_filter = if query_param(raw_path, "node").as_deref() == Some("all") {
            "all"
        } else {
            "current"
        };
        let (active_node_id, active_node_display_name) = match current_target_root
            .as_ref()
            .map(|target_root| target_root.join(".refine"))
        {
            Some(refine_dir) => {
                let service = self.node_registry_service(refine_dir);
                match dashboard_active_node(&service) {
                    Ok(active_node) => active_node,
                    Err(error) => return error_response(error),
                }
            }
            None => ("default".to_string(), "Default".to_string()),
        };
        let dashboard = projection.dashboard_summary(DashboardProjectionQuery {
            node: Some(node_filter.to_string()),
            current_node_id: Some(active_node_id.clone()),
        });
        let activity = dashboard
            .recent_activity_ids
            .iter()
            .filter_map(|activity_id| projection.activity.get(activity_id))
            .map(|activity| activity.entry.clone())
            .collect::<Vec<_>>();
        let runner_reachable = process
            .get("runner_reachable")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        ApiResponse::json(
            200,
            json!({
                "counts": dashboard.counts,
                "all_node_counts": dashboard.all_node_counts,
                "running": [],
                "merger": null,
                "governance": null,
                "preflight": preflight,
                "activity": activity,
                "runner_reachable": runner_reachable,
                "assignee_stats": assignee_stats_rows(&dashboard.assignee_stats),
                "reporter_stats": assignee_stats_rows(&dashboard.reporter_stats),
                "node_scope": dashboard.node_filter,
                "node_filter": dashboard.node_filter,
                "quality_timing": self.quality_timing_setting(),
                "active_node_id": dashboard.current_node_id,
                "active_node_display_name": active_node_display_name,
                "needs_attention": dashboard_attention_items(
                    &dashboard.attention_indicators,
                    runner_reachable
                ),
                "attached": attached
            }),
        )
    }

    pub(super) fn handle_nodes(&self) -> ApiResponse {
        match self.nodes_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create node");
        let body = request.body.unwrap_or_else(|| json!({}));
        if let Some(node_id) = body.get("id").and_then(|value| value.as_str()) {
            let node_id = node_id.trim();
            if node_id.is_empty() {
                return error_response(RefineError::InvalidInput(
                    "node id is required".to_string(),
                ));
            }
            return match self.node_registry_service(&refine_dir).create(node_id) {
                Ok(_) => self.handle_nodes(),
                Err(error) => error_response(error),
            };
        }
        let display_name = body
            .get("display_name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if display_name.is_empty() {
            return error_response(RefineError::InvalidInput(
                "display_name is required".to_string(),
            ));
        }
        match self
            .node_registry_service(&refine_dir)
            .create_with_display_name(display_name)
        {
            Ok(_) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_activate(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "activate node");
        let body = request.body.unwrap_or_else(|| json!({}));
        let node_id = body
            .get("node_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        match self.node_registry_service(refine_dir).activate(node_id) {
            Ok(_) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update node");
        let Some(node_id) = request
            .path
            .strip_prefix("/nodes/")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let update = NodeUpdate {
            display_name: body
                .get("display_name")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            archived: body.get("archived").and_then(|value| value.as_bool()),
        };
        match self
            .node_registry_service(refine_dir)
            .update(node_id, update)
        {
            Ok(_) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_transfer_goals(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer Goals to node");
        let body = request.body.unwrap_or_else(|| json!({}));
        let target_node_id = body
            .get("target_node_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if let Err(error) = self
            .node_registry_service(&refine_dir)
            .ensure_transfer_target(target_node_id)
        {
            return error_response(error);
        }
        if let Some(item_id) = body
            .get("item_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return match self
                .work_item_service(refine_dir)
                .transfer_item_to_node(target_node_id, item_id)
            {
                Ok(result) => ApiResponse::json(200, json!(result)),
                Err(error) => error_response(error),
            };
        }
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_transfer_goals_to_node(target_node_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_copy_settings(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "source_node_id": body.get("source_node_id").and_then(|value| value.as_str()).unwrap_or(""),
                "section": body.get("section").and_then(|value| value.as_str()).unwrap_or(""),
                "copied_count": 0,
                "message": "node-scoped settings copy has no native per-node settings to copy yet"
            }),
        )
    }

    pub(super) fn handle_cluster(&self) -> ApiResponse {
        let refine_dir = match self.current_refine_dir() {
            Ok(Some(path)) => path,
            Ok(None) => {
                return ApiResponse::json(
                    200,
                    json!({
                        "nodes": [],
                        "maintenance": null,
                        "enabled": false,
                        "message": "No nodes configured."
                    }),
                );
            }
            Err(error) => return error_response(error),
        };
        match FileClusterService::new(refine_dir).list_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_remote_node_upsert(
        &self,
        request: ApiRequest,
        path_id: Option<String>,
    ) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "configure node");
        let body = request.body.unwrap_or_else(|| json!({}));
        let is_create = request.method == "POST" && path_id.is_none();
        let id = path_id
            .or_else(|| {
                body.get("id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let id = id.trim();
        let update = NodeRemoteUpdate {
            display_name: body
                .get("display_name")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            ssh_host: body
                .get("ssh_host")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            ssh_user: body
                .get("ssh_user")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            ssh_identity_path: body
                .get("ssh_identity_path")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            ssh_port: body.get("ssh_port").and_then(|value| value.as_u64()),
            refine_checkout: body
                .get("refine_checkout")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            target_app_path: body
                .get("target_app_path")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            refine_port: body.get("refine_port").and_then(|value| value.as_u64()),
            enabled: body.get("enabled").and_then(|value| value.as_bool()),
        };
        let service = FileClusterService::new(refine_dir);
        let result = if is_create {
            service
                .add_node(id)
                .and_then(|_| service.upsert_node(id, update))
        } else {
            service.upsert_node(id, update)
        };
        match result {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_remote_node_delete(&self, node_id: Option<String>) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "remove node");
        let Some(node_id) = node_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        match FileClusterService::new(refine_dir).remove_node(node_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_remote_node_bootstrap(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bootstrap node");
        let Some(node_id) = request
            .path
            .strip_prefix("/cluster/nodes/")
            .and_then(|path| path.strip_suffix("/bootstrap"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let dry_run = body
            .get("dry_run")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let service = if let Some(runtime_root) = &self.runtime_root {
            FileClusterService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileClusterService::new(refine_dir)
        };
        match service.bootstrap_node_response(node_id, dry_run) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_remote_node_run(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "run cluster command");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("run cluster command");
        };
        let Some(node_id) = request
            .path
            .strip_prefix("/cluster/nodes/")
            .and_then(|path| path.strip_suffix("/run"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let command = body
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileClusterService::with_runtime_root(refine_dir, runtime_root)
            .run_remote_response(node_id, command)
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_remote_node_transfer(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer cluster item");
        let Some(node_id) = request
            .path
            .strip_prefix("/cluster/nodes/")
            .and_then(|path| path.strip_suffix("/transfer"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let item_id = body
            .get("item_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if item_id.is_empty() {
            return error_response(RefineError::InvalidInput("item_id is required".to_string()));
        }
        if let Err(error) = FileClusterService::new(&refine_dir).transfer(item_id, node_id) {
            return error_response(error);
        }
        match self
            .work_item_service(refine_dir)
            .transfer_item_to_node(node_id, item_id)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cluster_distribute(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "distribute work");
        let body = request.body.unwrap_or_else(|| json!({}));
        let to = body
            .get("to")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let converge = body
            .get("converge")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let dry_run = body
            .get("dry_run")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let service = if let Some(runtime_root) = &self.runtime_root {
            FileClusterService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileClusterService::new(refine_dir)
        };
        match service.distribute_response(to, converge, dry_run) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_guidance_next(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "suggest next actions");
        let service = if let Some(runtime_root) = &self.runtime_root {
            FileNextActionsService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileNextActionsService::new(refine_dir)
        };
        match service.next_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn nodes_response(&self) -> RefineResult<serde_json::Value> {
        let Some(refine_dir) = self.current_refine_dir()? else {
            return Ok(detached_nodes_response(BTreeMap::new()));
        };
        let projection = self.current_projection()?;
        let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for goal in projection.goals.values() {
            let node_id = goal
                .goal
                .node_id
                .as_deref()
                .unwrap_or("default")
                .to_string();
            *counts
                .entry(node_id)
                .or_default()
                .entry(goal.goal.status.as_str().to_string())
                .or_insert(0) += 1;
        }
        self.node_registry_service(refine_dir)
            .list_with_counts_response(counts)
    }

    pub(super) fn handle_target_app_status(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.snapshot())
        {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_target_app_health(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.health())
        {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_target_app_action(&self, request: ApiRequest) -> ApiResponse {
        let kind = request
            .path
            .strip_prefix("/target-app/")
            .unwrap_or("status")
            .to_string();
        let result = match self.target_app_service() {
            Ok(service) => match kind.as_str() {
                "start" => service.start(),
                "stop" => service.stop(),
                "build" => service.build(),
                _ => Err(RefineError::InvalidInput(format!(
                    "unknown target-app action {kind}"
                ))),
            },
            Err(error) => Err(error),
        };
        match result {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_target_app_generate_instructions(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("generate target-app config in the background");
            };
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register("target-app:generate") {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": "Generating target-app config with AI"
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let server = self.clone();
            let runtime_root = runtime_root.clone();
            let operation_id = operation.id.clone();
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = server.target_app_generate_response(&body, true);
                let mut result = response.body.clone();
                match result.as_object_mut() {
                    Some(object) => {
                        object.insert("http_status".to_string(), json!(response.status));
                    }
                    None => {
                        result = json!({
                            "http_status": response.status,
                            "body": result
                        });
                    }
                }
                if response.status >= 400 {
                    let error = result.get("error").cloned().unwrap_or_else(|| {
                        json!({
                            "message": "Target-app config generation failed",
                            "details": result
                        })
                    });
                    let _ = registry.fail_with_error(&operation_id, error);
                } else {
                    let _ = registry.update_progress(
                        &operation_id,
                        json!({
                            "message": "Generated target-app config"
                        }),
                    );
                    let _ = registry.finish_with_result(
                        &operation_id,
                        OperationState::Succeeded,
                        result,
                    );
                }
                let _ = server.refresh_projection_cache_after_mutation();
            });
            return ApiResponse::json(202, json!({"operation": operation_response(operation)}));
        }
        self.target_app_generate_response(&body, false)
    }

    fn target_app_generate_response(&self, body: &Value, persist_settings: bool) -> ApiResponse {
        let service = match self.target_app_service() {
            Ok(service) => service,
            Err(error) => return error_response(error),
        };
        let mut provider = String::new();
        let mut source = "local".to_string();
        let mut raw = String::new();
        let config = match self.current_refine_dir() {
            Ok(Some(refine_dir)) => {
                provider = configured_provider_from_settings(
                    &refine_dir,
                    self.runtime_root.as_deref(),
                    body,
                );
                match HostAgentProviderService::new().invoke(ProviderInvocation {
                    provider: provider.clone(),
                    prompt: target_app_generation_prompt(&service.target_root),
                    session_id: None,
                    cwd: Some(service.target_root.display().to_string()),
                    process_metadata: Default::default(),
                }) {
                    Ok(output) => {
                        raw = output.clone();
                        if let Some(config) = parse_generated_target_app_config(&output) {
                            source = "provider".to_string();
                            Ok(config)
                        } else {
                            service.generate_config()
                        }
                    }
                    Err(_) => service.generate_config(),
                }
            }
            Ok(None) => service.generate_config(),
            Err(error) => Err(error),
        };
        match config {
            Ok(config) => {
                let settings = target_app_generated_settings(&config);
                if persist_settings {
                    match self.current_refine_dir() {
                        Ok(Some(refine_dir)) => {
                            if let Err(error) = self.settings_service(&refine_dir).update(&settings)
                            {
                                return error_response(error);
                            }
                            if let Err(error) = self.apply_current_runtime_settings() {
                                return error_response(error);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => return error_response(error),
                    }
                }
                ApiResponse::json(
                    200,
                    json!({
                        "ok": true,
                        "config": config,
                        "settings": settings,
                        "provider": provider,
                        "source": source,
                        "raw": raw,
                        "message": if source == "provider" {
                            "Generated target-app configuration with the configured provider."
                        } else {
                            "Generated target-app configuration from local project files."
                        }
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_target_app_build_queue(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.build())
        {
            Ok(snapshot) => {
                let queued = snapshot.ok
                    && snapshot
                        .last_operation
                        .as_ref()
                        .map(|operation| operation.kind == "build")
                        .unwrap_or(false);
                let mut value = self.target_app_response(snapshot);
                value["queued"] = json!(queued);
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_status(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("read project status");
        };
        match service.status() {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_path(&self, raw_path: &str) -> ApiResponse {
        let path = query_param(raw_path, "path").unwrap_or_default();
        let resolved = resolve_project_utility_path(&path);
        ApiResponse::json(
            200,
            json!({
                "path": resolved.display().to_string(),
                "input": path,
                "exists": resolved.exists(),
                "is_dir": resolved.is_dir(),
                "parent": resolved.parent().map(|path| path.display().to_string())
            }),
        )
    }

    pub(super) fn handle_project_directories(&self, raw_path: &str) -> ApiResponse {
        let path = query_param(raw_path, "path").unwrap_or_default();
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(200)
            .clamp(1, 1000);
        match project_directories_response(&path, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_list(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("list projects");
        };
        match service.list_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_attach(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("attach projects");
        };
        let Some(path) = request
            .body
            .as_ref()
            .and_then(|body| body.get("path"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "path is required"
                    }
                }),
            );
        };
        self.stop_target_app_for_project_change();
        match service.attach_with_migration(path) {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_migrate(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("migrate project");
        };
        match service.migrate_current() {
            Ok(report) => ApiResponse::json(200, serde_json::to_value(report).unwrap()),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_register(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("register projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(path) = body
            .get("path")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput("path is required".to_string()));
        };
        let name = body
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::trim);
        match service.register_path(name, path, false) {
            Ok(registry) => ApiResponse::json(
                201,
                json!({
                    "ok": true,
                    "apps": registry_apps_array(&registry),
                    "current": registry.active_app.unwrap_or_default(),
                    "registry_enabled": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_clone(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("clone projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(source) = body
            .get("source")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput("source is required".to_string()));
        };
        let Some(destination) = body
            .get("destination")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "destination is required".to_string(),
            ));
        };
        let name = body
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty());
        let make_current = body
            .get("make_current")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        match service.clone_app(source, destination, name, make_current) {
            Ok(status) => ApiResponse::json(201, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_switch(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("switch projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(name) = body
            .get("name")
            .or_else(|| body.get("path"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "name or path is required".to_string(),
            ));
        };
        self.stop_target_app_for_project_change();
        match service.switch_with_migration(name) {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_detach(&self) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("detach project");
        };
        self.stop_target_app_for_project_change();
        match service.detach() {
            Ok(status) => {
                if let Err(error) = self.refresh_runtime_projection_cache() {
                    return error_response(error);
                }
                ApiResponse::json(200, project_status_value(status))
            }
            Err(error) => error_response(error),
        }
    }

    fn stop_target_app_for_project_change(&self) {
        if self.current_target_root().ok().flatten().is_some() {
            let _ = self.target_app_service().and_then(|service| service.stop());
        }
        let Some(runtime_root) = &self.runtime_root else {
            return;
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        if let Ok(processes) = supervisor.recover_owner(ProcessOwner::TargetApp) {
            for process in processes
                .into_iter()
                .filter(|process| process.owner == ProcessOwner::TargetApp)
            {
                let _ = supervisor.signal(&process.id, "stop");
            }
        }
    }

    pub(super) fn handle_project_remove(&self, request: ApiRequest) -> ApiResponse {
        let Some(service) = self.project_registry_service() else {
            return runtime_root_unavailable("remove projects");
        };
        let Some(path) = request
            .body
            .as_ref()
            .and_then(|body| body.get("path").or_else(|| body.get("name")))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "path is required"
                    }
                }),
            );
        };
        match service.remove(path) {
            Ok(registry) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "apps": registry_apps_array(&registry),
                    "current": registry.active_app.unwrap_or_default(),
                    "registry_enabled": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_sync(&self) -> ApiResponse {
        let git_sync = match self.current_target_root() {
            Ok(Some(target_root)) => {
                let runtime_root = self
                    .runtime_root
                    .clone()
                    .unwrap_or_else(|| target_root.join(".refine/runtime"));
                match FileGitSyncService::new(target_root, runtime_root).sync() {
                    Ok(result) => result,
                    Err(error) => return error_response(error),
                }
            }
            Ok(None) => GitSyncResult::default(),
            Err(error) => return error_response(error),
        };
        let projection = if self.runtime_root.is_some() {
            match self.rebuild_current_project_projection_cache() {
                Ok(projection) => projection,
                Err(error) => return error_response(error),
            }
        } else {
            match self.current_projection() {
                Ok(projection) => projection,
                Err(error) => return error_response(error),
            }
        };
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "message": "Project state projection rebuilt.",
                "projection_version": projection.version,
                "goal_count": projection.goals.len(),
                "feature_count": projection.features.len(),
                "git_sync": {
                    "attempted": git_sync.attempted,
                    "committed": git_sync.committed,
                    "pulled": git_sync.pulled,
                    "pushed": git_sync.pushed,
                    "branch": git_sync.branch,
                    "commit": git_sync.commit,
                    "detail": git_sync.detail
                }
            }),
        )
    }

    pub(super) fn handle_settings_get(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read settings");
        match self.settings_service(refine_dir).list_response() {
            Ok(value) => ApiResponse::json(200, self.with_runtime_settings(value)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_settings_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update settings");
        let body = request.body.unwrap_or_else(|| json!({}));
        if let Some(paused) = body.get("paused").map(runtime_bool_setting)
            && let Some(runtime_root) = &self.runtime_root
        {
            match self.current_target_root() {
                Ok(Some(target_root)) => {
                    if let Err(error) = WorkflowEngine::with_target_root(runtime_root, target_root)
                        .set_workflow_paused(paused)
                    {
                        return error_response(error);
                    }
                }
                Ok(None) => {
                    let supervisor = FileProcessSupervisor::new(runtime_root);
                    if paused {
                        if let Err(error) = supervisor.set_agents_paused(true) {
                            return error_response(error);
                        }
                        if let Err(error) = supervisor.set_background_processes_stopped(true) {
                            return error_response(error);
                        }
                    } else {
                        if let Err(error) = supervisor.set_background_processes_stopped(false) {
                            return error_response(error);
                        }
                        if let Err(error) = supervisor.set_agents_paused(false) {
                            return error_response(error);
                        }
                    }
                }
                Err(error) => return error_response(error),
            }
        }
        match self.settings_service(&refine_dir).update(&body) {
            Ok(value) => {
                if let Err(error) = self.apply_current_runtime_settings() {
                    return error_response(error);
                }
                let value = self.with_runtime_settings(value);
                if let Err(error) = self.current_projection_with_runtime() {
                    return error_response(error);
                }
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    fn apply_current_runtime_settings(&self) -> RefineResult<()> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(());
        };
        let Some(target_root) = self.current_target_root()? else {
            return Ok(());
        };
        WorkflowEngine::with_target_root(runtime_root, target_root)
            .apply_runtime_settings()
            .map(|_| ())
    }

    pub(super) fn handle_upgrade_status(&self) -> ApiResponse {
        let current_version = env!("CARGO_PKG_VERSION");
        let latest_version = std::env::var("REFINE_TEST_UPGRADE_LATEST_VERSION")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| current_version.to_string());
        let upgrade_available = latest_version != current_version;
        let local_development = !upgrade_available;
        let command = std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "refine".to_string());
        ApiResponse::json(
            200,
            json!({
                "upgrade": {
                    "available": upgrade_available,
                    "upgrade_available": upgrade_available,
                    "current_version": current_version,
                    "latest_version": latest_version,
                    "launch_mode": current_launch_mode(),
                    "executable_path": current_launch_executable(),
                    "local_development": local_development,
                    "message": if upgrade_available {
                        format!("Refine {latest_version} is available; current version is {current_version}.")
                    } else {
                        format!("Running native Refine {current_version}; remote release discovery is not configured for this build.")
                    },
                    "command": command
                }
            }),
        )
    }

    pub(super) fn handle_governance_get(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read governance settings");
        match FileGovernanceService::new(refine_dir).load() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_governance_save(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "save governance settings");
        match FileGovernanceService::new(refine_dir)
            .save(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_governance_generate_rules(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "generate governance rules");
        let body = request.body.unwrap_or_else(|| json!({}));
        let product = body
            .get("product")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let constitution = body
            .get("constitution")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if product.is_empty() || constitution.is_empty() {
            return error_response(RefineError::InvalidInput(
                "product and constitution are required".to_string(),
            ));
        }

        let provider =
            configured_provider_from_settings(&refine_dir, self.runtime_root.as_deref(), &body);
        let cwd = self.target_root().map(|path| path.display().to_string());
        let output = match HostAgentProviderService::new().invoke(ProviderInvocation {
            provider: provider.clone(),
            prompt: governance_generation_prompt(product, constitution),
            session_id: None,
            cwd,
            process_metadata: Default::default(),
        }) {
            Ok(output) => output,
            Err(_) => {
                return match FileGovernanceService::new(&refine_dir).generate_rules(&body) {
                    Ok(mut value) => {
                        if let Some(object) = value.as_object_mut() {
                            object.insert("source".to_string(), json!("static"));
                        }
                        ApiResponse::json(200, value)
                    }
                    Err(error) => error_response(error),
                };
            }
        };
        let mut rules = parse_generated_governance_rules(&output);
        if rules.is_empty() {
            match FileGovernanceService::new(&refine_dir).generate_rules(&body) {
                Ok(value) => {
                    rules = value["rules"].as_array().cloned().unwrap_or_default();
                }
                Err(error) => return error_response(error),
            }
        }
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "provider": provider,
                "source": "provider",
                "rules": rules,
                "raw": output
            }),
        )
    }

    pub(super) fn handle_guidance_list(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read guidance");
        match FileGuidanceService::new(refine_dir).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_guidance_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update guidance");
        match FileGuidanceService::new(refine_dir)
            .update(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporters_list(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "list reporters");
        match FileReporterService::new(refine_dir).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create reporters");
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(refine_dir).create(name) {
            Ok(value) => ApiResponse::json(201, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_rename(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "rename reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(refine_dir).rename(id, name) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_merge(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "merge reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "/merge") else {
            return reporter_id_required();
        };
        let Some(target_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("target_id"))
            .and_then(|value| value.as_u64())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "target_id is required"
                    }
                }),
            );
        };
        match FileReporterService::new(refine_dir).merge(id, target_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        match FileReporterService::new(refine_dir).delete(id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }
}

fn assignee_stats_rows(
    assignee_stats: &BTreeMap<String, BTreeMap<GoalStatus, usize>>,
) -> Vec<Value> {
    assignee_stats
        .iter()
        .filter(|(assignee, _)| assignee.as_str() != "unassigned")
        .map(|(assignee, counts)| {
            let assigned = counts.values().copied().sum::<usize>();
            let done = counts.get(&GoalStatus::Done).copied().unwrap_or_default();
            let cancelled = counts
                .get(&GoalStatus::Cancelled)
                .copied()
                .unwrap_or_default();
            let active = assigned.saturating_sub(done + cancelled);
            let assigned_review = counts.get(&GoalStatus::Review).copied().unwrap_or_default();
            let completion_rate = if assigned == 0 {
                0.0
            } else {
                (done as f64 / assigned as f64) * 100.0
            };
            json!({
                "assignee": assignee,
                "reporter": assignee,
                "active": active,
                "done": done,
                "reported": assigned,
                "assigned": assigned,
                "assigned_review": assigned_review,
                "completion_rate": completion_rate
            })
        })
        .collect()
}
