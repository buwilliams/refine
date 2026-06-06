use crate::core::supervisor::config::{
    FileGovernanceService, FileGuidanceService, FileReporterService, FileSettingsService,
};
use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::core::host::cluster::{ClusterNodeUpdate, ClusterService, FileClusterRegistryService};
use crate::core::host::process_supervision::FileProcessSupervisor;
use crate::core::product::nodes::{FileNodeRegistryService, NodeUpdate, detached_nodes_response};
use crate::core::product::project_registry::{
    FileProjectRegistryService, ProjectRegistryService, registry_apps_array,
};
use crate::core::product::work_items::BulkGapSelection;
use crate::core::supervisor::errors::{RefineError, RefineResult};

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn handle_dashboard(&self) -> ApiResponse {
        let attached = match self.current_durable_root() {
            Ok(value) => value.is_some(),
            Err(error) => return error_response(error),
        };
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
        let durable_root = require_durable_root!(self, "create node");
        let body = request.body.unwrap_or_else(|| json!({}));
        if let Some(node_id) = body.get("id").and_then(|value| value.as_str()) {
            let node_id = node_id.trim();
            if node_id.is_empty() {
                return error_response(RefineError::InvalidInput(
                    "node id is required".to_string(),
                ));
            }
            return match FileNodeRegistryService::new(&durable_root).create(node_id) {
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
        match FileNodeRegistryService::new(&durable_root).create_with_display_name(display_name) {
            Ok(_) => self.handle_nodes(),
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
        match FileNodeRegistryService::new(durable_root).activate(node_id) {
            Ok(_) => self.handle_nodes(),
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
        let body = request.body.unwrap_or_else(|| json!({}));
        let update = NodeUpdate {
            display_name: body
                .get("display_name")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            archived: body.get("archived").and_then(|value| value.as_bool()),
        };
        match FileNodeRegistryService::new(durable_root).update(node_id, update) {
            Ok(_) => self.handle_nodes(),
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
        if let Err(error) =
            FileNodeRegistryService::new(&durable_root).ensure_transfer_target(target_node_id)
        {
            return error_response(error);
        }
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(durable_root)
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
        match FileClusterRegistryService::new(durable_root).list_response() {
            Ok(value) => ApiResponse::json(200, value),
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
        let update = ClusterNodeUpdate {
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
        match FileClusterRegistryService::new(durable_root).upsert_node(id, update) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cluster_node_delete(&self, node_id: Option<String>) -> ApiResponse {
        let durable_root = require_durable_root!(self, "remove cluster node");
        let Some(node_id) = node_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "cluster node id is required".to_string(),
            ));
        };
        match FileClusterRegistryService::new(durable_root).remove_node(node_id) {
            Ok(value) => ApiResponse::json(200, value),
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
        let body = request.body.unwrap_or_else(|| json!({}));
        let dry_run = body
            .get("dry_run")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        match FileClusterRegistryService::new(durable_root)
            .bootstrap_node_response(node_id, dry_run)
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cluster_node_run(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "run cluster command");
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
            return error_response(RefineError::InvalidInput(
                "cluster node id is required".to_string(),
            ));
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let command = body
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileClusterRegistryService::with_runtime_root(durable_root, runtime_root)
            .run_remote_response(node_id, command)
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cluster_node_transfer(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "transfer cluster item");
        let Some(node_id) = request
            .path
            .strip_prefix("/cluster/nodes/")
            .and_then(|path| path.strip_suffix("/transfer"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "cluster node id is required".to_string(),
            ));
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
        if let Err(error) =
            FileClusterRegistryService::new(&durable_root).transfer(item_id, node_id)
        {
            return error_response(error);
        }
        let selection = BulkGapSelection {
            selected_ids: Some(vec![item_id.to_string()]),
            ..BulkGapSelection::default()
        };
        match self
            .work_item_service(durable_root)
            .bulk_transfer_gaps_to_node(node_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn nodes_response(&self) -> RefineResult<serde_json::Value> {
        let Some(durable_root) = self.current_durable_root()? else {
            return Ok(detached_nodes_response(BTreeMap::new()));
        };
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
        FileNodeRegistryService::new(durable_root).list_with_counts_response(counts)
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
            .map(str::trim);
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone())
            .register_path(name, path, false)
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

    pub(super) fn handle_project_clone(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
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
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).clone_app(
            source,
            destination,
            name,
            make_current,
        ) {
            Ok(status) => ApiResponse::json(201, project_status_value(status)),
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
        match self
            .work_item_service(durable_root)
            .create_gap_summary(name, None)
        {
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
