use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_nodes(&self) -> ApiResponse {
        match self.nodes_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_node_create(&self, request: ApiRequest) -> ApiResponse {
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

    pub(crate) fn handle_node_activate(&self, request: ApiRequest) -> ApiResponse {
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

    pub(crate) fn handle_node_update(&self, request: ApiRequest) -> ApiResponse {
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

    pub(crate) fn handle_node_transfer_goals(&self, request: ApiRequest) -> ApiResponse {
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

    pub(crate) fn handle_node_copy_settings(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let refine_dir = match self.current_refine_dir() {
            Ok(Some(path)) => path,
            Ok(None) => return ApiResponse::json(404, json!({"error": "no active project"})),
            Err(error) => return error_response(error),
        };
        let source_node_id = body
            .get("source_node_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let section = body.get("section").and_then(Value::as_str).unwrap_or("");
        match self
            .settings_service(refine_dir)
            .copy_from_node(source_node_id, section)
        {
            Ok(result) => ApiResponse::json(200, result),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_fleet(&self) -> ApiResponse {
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
        match FileFleetService::new(refine_dir).list_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_remote_node_upsert(
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
        let service = FileFleetService::new(refine_dir);
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

    pub(crate) fn handle_remote_node_delete(&self, node_id: Option<String>) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "remove node");
        let Some(node_id) = node_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        match FileFleetService::new(refine_dir).remove_node(node_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_remote_node_bootstrap(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bootstrap node");
        let Some(node_id) = request
            .path
            .strip_prefix("/fleet/nodes/")
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
            FileFleetService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileFleetService::new(refine_dir)
        };
        match service.bootstrap_node_response(node_id, dry_run) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_remote_node_run(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "run fleet command");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("run fleet command");
        };
        let Some(node_id) = request
            .path
            .strip_prefix("/fleet/nodes/")
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
        match FileFleetService::with_runtime_root(refine_dir, runtime_root)
            .run_remote_response(node_id, command)
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_remote_node_transfer(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer fleet item");
        let Some(node_id) = request
            .path
            .strip_prefix("/fleet/nodes/")
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
        if let Err(error) = FileFleetService::new(&refine_dir).transfer(item_id, node_id) {
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

    /// Ask every other fleet node's daemon to synchronize and report what
    /// each one answered. This node's own state sync stays `POST /sync`; this
    /// route is the fan-out around it, and a node still on the previous build
    /// is reported as that node's pending upgrade rather than failing the
    /// fleet — nodes upgrade one at a time and the rest keep converging.
    pub(crate) fn handle_fleet_sync(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "sync the fleet");
        let service = if let Some(runtime_root) = &self.runtime_root {
            FileFleetService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileFleetService::new(refine_dir)
        };
        match service.sync_nodes() {
            Ok(report) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "nodes": report.nodes,
                    "pending_upgrade": report.pending_upgrade
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_fleet_distribute(&self, request: ApiRequest) -> ApiResponse {
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
            FileFleetService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileFleetService::new(refine_dir)
        };
        match service.distribute_response(to, converge, dry_run) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn nodes_response(&self) -> RefineResult<serde_json::Value> {
        let Some(refine_dir) = self.current_refine_dir()? else {
            return Ok(detached_nodes_response(BTreeMap::new()));
        };
        let projection = self.current_projection_shared()?;
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
        let mut response = self
            .node_registry_service(&refine_dir)
            .list_with_counts_response(counts)?;
        let active_node_id = response
            .get("active_node_id")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string();
        let local_health = self.current_state_sync_health()?;
        if let Some(nodes) = response.get_mut("nodes").and_then(Value::as_array_mut) {
            for node in nodes {
                let node_id = node.get("id").and_then(Value::as_str).unwrap_or("");
                let health = if node_id == active_node_id {
                    local_health
                        .as_ref()
                        .map(|health| json!(health))
                        .unwrap_or_else(|| {
                            json!({
                                "status": "unknown",
                                "reason": "runtime_health_unavailable"
                            })
                        })
                } else {
                    json!({
                        "status": "unknown",
                        "reason": "node_local_evidence_unavailable"
                    })
                };
                if let Some(node) = node.as_object_mut() {
                    node.insert("state_sync_health".to_string(), health);
                }
            }
        }
        Ok(response)
    }
}
