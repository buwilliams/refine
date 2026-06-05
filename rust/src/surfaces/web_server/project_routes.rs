use crate::core::supervisor::config::{
    FileGovernanceService, FileGuidanceService, FileReporterService, FileSettingsService,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::core::host::cluster::{ClusterBootstrapRequest, bootstrap_remote_node};
use crate::core::host::process_supervision::FileProcessSupervisor;
use crate::core::product::project_registry::{
    FileProjectRegistryService, ProjectRegistryService, registry_apps_array,
};
use crate::core::product::work_items::{BulkGapSelection, FileWorkItemService};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::cluster::{valid_cluster_node_id, valid_ssh_host};
use crate::model::project::RegisteredApp;

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn handle_dashboard(&self) -> ApiResponse {
        let projection = match self.current_projection_with_runtime() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let activity = projection
            .dashboard
            .recent_activity_ids
            .iter()
            .filter_map(|activity_id| projection.activity.get(activity_id))
            .map(|activity| activity.entry.clone())
            .collect::<Vec<_>>();
        let process = runtime_process_summary_value(&projection.runtime);
        let preflight = projection
            .runtime
            .preflight
            .clone()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"ok": false, "providers": []}));
        ApiResponse::json(
            200,
            json!({
                "counts": projection.dashboard.current_node_status_counts,
                "all_node_counts": projection.dashboard.all_node_status_counts,
                "running": [],
                "merger": null,
                "governance": null,
                "preflight": preflight,
                "activity": activity,
                "runner_reachable": process.get("runner_reachable").and_then(|value| value.as_bool()).unwrap_or(false),
                "reporter_stats": projection.dashboard.reporter_stats,
                "node_scope": "current",
                "node_filter": "current",
                "quality_timing": self.quality_timing_setting(),
                "active_node_id": "default",
                "active_node_display_name": "Default",
                "needs_attention": projection.dashboard.attention_indicators,
                "attached": self.durable_root.is_some()
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
        let durable_root = require_durable_root!(self, "create node");
        let body = request.body.unwrap_or_else(|| json!({}));
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
        let mut registry = match load_node_registry(&durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let id = unique_node_id(&registry, display_name);
        let now = now_timestamp_web();
        registry.nodes.push(json!({
            "id": id,
            "display_name": display_name,
            "archived": false,
            "active": false,
            "created_at": now,
            "updated_at": now
        }));
        match save_node_registry(&durable_root, &registry) {
            Ok(()) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_activate(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "activate node");
        let body = request.body.unwrap_or_else(|| json!({}));
        let node_id = body
            .get("node_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        let registry = match load_node_registry(&durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        if !registry
            .nodes
            .iter()
            .any(|node| node_id_value(node) == node_id && !node_archived(node))
        {
            return error_response(RefineError::NotFound(format!(
                "node {node_id} was not found or is archived"
            )));
        }
        match save_active_node_id(&durable_root, node_id) {
            Ok(()) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update node");
        let Some(node_id) = request
            .path
            .strip_prefix("/nodes/")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        let mut registry = match load_node_registry(&durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let active_node_id = match load_active_node_id(&durable_root) {
            Ok(active) => active,
            Err(error) => return error_response(error),
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(node) = registry
            .nodes
            .iter_mut()
            .find(|node| node_id_value(node) == node_id)
        else {
            return error_response(RefineError::NotFound(format!(
                "node {node_id} was not found"
            )));
        };
        if let Some(display_name) = body.get("display_name").and_then(|value| value.as_str()) {
            let display_name = display_name.trim();
            if display_name.is_empty() {
                return error_response(RefineError::InvalidInput(
                    "display_name cannot be empty".to_string(),
                ));
            }
            node["display_name"] = json!(display_name);
        }
        if let Some(archived) = body.get("archived").and_then(|value| value.as_bool()) {
            if archived && node_id == active_node_id {
                return error_response(RefineError::Conflict(
                    "active node cannot be archived".to_string(),
                ));
            }
            node["archived"] = json!(archived);
        }
        node["updated_at"] = json!(now_timestamp_web());
        match save_node_registry(&durable_root, &registry) {
            Ok(()) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_transfer_gaps(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "transfer Gaps to node");
        let body = request.body.unwrap_or_else(|| json!({}));
        let target_node_id = body
            .get("target_node_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        let registry = match load_node_registry(&durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        if !registry
            .nodes
            .iter()
            .any(|node| node_id_value(node) == target_node_id && !node_archived(node))
        {
            return error_response(RefineError::NotFound(format!(
                "node {target_node_id} was not found or is archived"
            )));
        }
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match FileWorkItemService::new(durable_root)
            .bulk_transfer_gaps_to_node(target_node_id, selection)
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
        let durable_root = match self.current_durable_root() {
            Ok(Some(path)) => path,
            Ok(None) => {
                return ApiResponse::json(
                    200,
                    json!({
                        "nodes": [],
                        "maintenance": null,
                        "enabled": false,
                        "message": "No cluster nodes configured."
                    }),
                );
            }
            Err(error) => return error_response(error),
        };
        match load_cluster_registry(&durable_root) {
            Ok(registry) => ApiResponse::json(200, cluster_response(registry)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cluster_node_upsert(
        &self,
        request: ApiRequest,
        path_id: Option<String>,
    ) -> ApiResponse {
        let durable_root = require_durable_root!(self, "configure cluster node");
        let body = request.body.unwrap_or_else(|| json!({}));
        let id = path_id
            .or_else(|| {
                body.get("id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let id = id.trim();
        if !valid_cluster_node_id(id) {
            return error_response(RefineError::InvalidInput(
                "cluster node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            ));
        }
        let mut registry = match load_cluster_registry(&durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let existing_index = registry
            .nodes
            .iter()
            .position(|node| cluster_node_id_value(node) == id);
        let mut node = existing_index
            .and_then(|index| registry.nodes.get(index).cloned())
            .unwrap_or_else(|| default_cluster_node(id));
        if let Some(display_name) = body.get("display_name").and_then(|value| value.as_str()) {
            node["display_name"] = json!(display_name.trim());
        }
        if let Some(ssh_host) = body.get("ssh_host").and_then(|value| value.as_str()) {
            let ssh_host = ssh_host.trim();
            if !valid_ssh_host(ssh_host) {
                return error_response(RefineError::InvalidInput(
                    "ssh_host must be a host without user@ prefix".to_string(),
                ));
            }
            node["ssh_host"] = json!(ssh_host);
        }
        for (field, default_value) in [("ssh_port", 22_u64), ("refine_port", 8080_u64)] {
            if let Some(port) = body.get(field).and_then(|value| value.as_u64()) {
                node[field] = json!(if port == 0 { default_value } else { port });
            }
        }
        for field in ["refine_checkout", "target_app_path"] {
            if let Some(value) = body.get(field).and_then(|value| value.as_str()) {
                node[field] = json!(value.trim());
            }
        }
        if let Some(enabled) = body.get("enabled").and_then(|value| value.as_bool()) {
            node["enabled"] = json!(enabled);
        }
        node["updated_at"] = json!(now_timestamp_web());
        if let Some(index) = existing_index {
            registry.nodes[index] = node;
        } else {
            registry.nodes.push(node);
        }
        registry.updated_at = now_timestamp_web();
        match save_cluster_registry(&durable_root, &registry) {
            Ok(()) => ApiResponse::json(200, cluster_response(registry)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cluster_node_bootstrap(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "bootstrap cluster node");
        let Some(node_id) = request
            .path
            .strip_prefix("/cluster/nodes/")
            .and_then(|path| path.strip_suffix("/bootstrap"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "cluster node id is required".to_string(),
            ));
        };
        let mut registry = match load_cluster_registry(&durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let Some(index) = registry
            .nodes
            .iter()
            .position(|node| cluster_node_id_value(node) == node_id)
        else {
            return error_response(RefineError::NotFound(format!(
                "cluster node {node_id} was not found"
            )));
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let node = registry.nodes[index].clone();
        let dry_run = body
            .get("dry_run")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let result = match bootstrap_remote_node(ClusterBootstrapRequest {
            node_id: node_id.to_string(),
            ssh_host: node
                .get("ssh_host")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            ssh_port: node
                .get("ssh_port")
                .and_then(|value| value.as_u64())
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(22),
            refine_checkout: node
                .get("refine_checkout")
                .and_then(|value| value.as_str())
                .unwrap_or("~/refine")
                .to_string(),
            target_app_path: node
                .get("target_app_path")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            refine_port: node
                .get("refine_port")
                .and_then(|value| value.as_u64())
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(8080),
            dry_run,
        }) {
            Ok(result) => result,
            Err(error) => return error_response(error),
        };
        registry.nodes[index]["health"] = json!({
            "status": if result.ok { "ready" } else { "failed" },
            "checked_at": now_timestamp_web(),
            "details": {
                "bootstrap": result
            }
        });
        registry.updated_at = now_timestamp_web();
        if let Err(error) = save_cluster_registry(&durable_root, &registry) {
            return error_response(error);
        }
        let result = registry.nodes[index]
            .get("health")
            .and_then(|health| health.get("details"))
            .and_then(|details| details.get("bootstrap"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        ApiResponse::json(
            200,
            json!({
                "ok": result.get("ok").and_then(|value| value.as_bool()).unwrap_or(false),
                "node_id": node_id,
                "dry_run": dry_run,
                "result": result,
                "cluster": cluster_response(registry)
            }),
        )
    }

    pub(super) fn nodes_response(&self) -> RefineResult<serde_json::Value> {
        let Some(durable_root) = self.current_durable_root()? else {
            return Ok(json!({
                "active_node_id": "default",
                "active_node": "Default",
                "nodes": [default_node("default", "Default", true)],
                "counts": {}
            }));
        };
        let registry = load_node_registry(&durable_root)?;
        let active_node_id = load_active_node_id(&durable_root)?;
        let projection = self.current_projection()?;
        let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for gap in projection.gaps.values() {
            let node_id = gap.gap.node_id.as_deref().unwrap_or("default").to_string();
            *counts
                .entry(node_id)
                .or_default()
                .entry(gap.gap.status.as_str().to_string())
                .or_insert(0) += 1;
        }
        let nodes: Vec<_> = registry
            .nodes
            .iter()
            .map(|node| {
                let mut node = node.clone();
                node["active"] = json!(node_id_value(&node) == active_node_id);
                node
            })
            .collect();
        let active_node = nodes
            .iter()
            .find(|node| node_id_value(node) == active_node_id)
            .and_then(|node| node.get("display_name"))
            .and_then(|value| value.as_str())
            .unwrap_or("Default");
        Ok(json!({
            "active_node_id": active_node_id,
            "active_node": active_node,
            "nodes": nodes,
            "counts": counts
        }))
    }

    pub(super) fn handle_target_app_status(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.status())
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
                "rebuild" => service.rebuild(),
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

    pub(super) fn handle_target_app_generate_instructions(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.generate_config())
        {
            Ok(config) => {
                let settings = json!({
                    "target_app_start_command": config.start_command.clone(),
                    "target_app_stop_command": config.stop_command.clone(),
                    "target_app_rebuild_command": config.rebuild_command.clone(),
                    "target_app_status_command": config.status_command.clone(),
                    "target_app_cwd": config.cwd.clone(),
                    "target_app_env_json": serde_json::to_string_pretty(&config.env).unwrap_or_else(|_| "{}".to_string()),
                    "target_app_start_timeout_seconds": config.start_timeout_seconds.to_string(),
                    "target_app_stop_timeout_seconds": config.stop_timeout_seconds.to_string(),
                    "target_app_rebuild_timeout_seconds": config.rebuild_timeout_seconds.to_string(),
                    "target_app_status_timeout_seconds": config.status_timeout_seconds.to_string(),
                    "target_app_log_path": config.log_path.clone(),
                    "target_app_http_check_url": config.http_check_url.clone(),
                    "target_app_tcp_check_host": config.tcp_check_host.clone(),
                    "target_app_tcp_check_port": config.tcp_check_port.clone(),
                    "target_app_process_check_command": config.process_check_command.clone()
                });
                ApiResponse::json(
                    200,
                    json!({
                        "ok": true,
                        "config": config,
                        "settings": settings,
                        "message": "Generated target-app configuration from local project files."
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_target_app_rebuild_queue(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.rebuild())
        {
            Ok(snapshot) => {
                let mut value = self.target_app_response(snapshot);
                value["queued"] =
                    json!(value.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false));
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_status(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read project status");
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).status() {
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
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("list projects");
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone())
            .list_response()
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_attach(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
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
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).attach(path)
        {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_register(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
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
            .map(str::to_string)
            .unwrap_or_else(|| {
                Path::new(path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(path)
                    .to_string()
            });
        let app_path = PathBuf::from(path.trim());
        let app_path = if app_path.is_absolute() {
            app_path
        } else {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(app_path),
                Err(error) => {
                    return error_response(RefineError::Io(format!(
                        "failed to inspect cwd: {error}"
                    )));
                }
            }
        };
        let app = RegisteredApp {
            name,
            path: app_path.display().to_string(),
            added_at: now_timestamp_web(),
            last_used_at: None,
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).register(app)
        {
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

    pub(super) fn handle_project_switch(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
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
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).switch(name)
        {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_detach(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("detach project");
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).detach() {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_project_remove(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
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
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).remove(path)
        {
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
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "message": "Project state projection rebuilt.",
                "projection_version": projection.version,
                "gap_count": projection.gaps.len(),
                "feature_count": projection.features.len()
            }),
        )
    }

    pub(super) fn handle_project_templates(&self) -> ApiResponse {
        ApiResponse::json(200, json!({"templates": []}))
    }

    pub(super) fn handle_project_scaffold(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "create scaffold Gaps");
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("Scaffold target application");
        match FileWorkItemService::new(durable_root).create_gap_summary(name, None) {
            Ok(gap) => ApiResponse::json(201, json!({"ok": true, "gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_settings_get(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "read settings");
        match FileSettingsService::new(durable_root).list_response() {
            Ok(value) => ApiResponse::json(200, self.with_runtime_settings(value)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_settings_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update settings");
        let body = request.body.unwrap_or_else(|| json!({}));
        if let Some(paused) = body.get("paused").map(runtime_bool_setting)
            && let Some(runtime_root) = &self.runtime_root
        {
            let supervisor = FileProcessSupervisor::new(runtime_root);
            if let Err(error) = supervisor.set_agents_paused(paused) {
                return error_response(error);
            }
            if let Err(error) = supervisor.set_background_processes_stopped(paused) {
                return error_response(error);
            }
        }
        match FileSettingsService::new(durable_root).update(&body) {
            Ok(value) => {
                let value = self.with_runtime_settings(value);
                if let Err(error) = self.current_projection_with_runtime() {
                    return error_response(error);
                }
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_upgrade_status(&self) -> ApiResponse {
        let current_version = env!("CARGO_PKG_VERSION");
        let command = std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "refine-native".to_string());
        ApiResponse::json(
            200,
            json!({
                "upgrade": {
                    "available": false,
                    "upgrade_available": false,
                    "current_version": current_version,
                    "latest_version": current_version,
                    "local_development": true,
                    "message": format!("Running native Refine {current_version}; remote release discovery is not configured for this build."),
                    "command": command
                }
            }),
        )
    }

    pub(super) fn handle_governance_get(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "read governance settings");
        match FileGovernanceService::new(durable_root).load() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_governance_save(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "save governance settings");
        match FileGovernanceService::new(durable_root)
            .save(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_governance_generate_rules(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "generate governance rules");
        match FileGovernanceService::new(durable_root)
            .generate_rules(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_guidance_list(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "read guidance");
        match FileGuidanceService::new(durable_root).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_guidance_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update guidance");
        match FileGuidanceService::new(durable_root)
            .update(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporters_list(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "list reporters");
        match FileReporterService::new(durable_root).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_create(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "create reporters");
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(durable_root).create(name) {
            Ok(value) => ApiResponse::json(201, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_rename(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "rename reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(durable_root).rename(id, name) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_merge(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "merge reporters");
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
        match FileReporterService::new(durable_root).merge(id, target_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_reporter_delete(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "delete reporters");
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        match FileReporterService::new(durable_root).delete(id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }
}
