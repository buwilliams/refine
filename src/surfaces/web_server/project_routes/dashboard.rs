use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_dashboard(&self, raw_path: &str) -> ApiResponse {
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
        let current_refine_dir = match self.current_refine_dir() {
            Ok(value) => value,
            Err(error) => return error_response(error),
        };
        let active_node_identity = match current_refine_dir {
            Some(refine_dir) => {
                let service = self.node_registry_service(refine_dir);
                match dashboard_active_node(&service) {
                    Ok(active_node) => active_node,
                    Err(error) => return error_response(error),
                }
            }
            None => crate::tools::product::nodes::ActiveNodeIdentity {
                id: "default".to_string(),
                display_name: "Default".to_string(),
                diagnostics: Vec::new(),
            },
        };
        let active_node_id = active_node_identity.id.clone();
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
        let preparation_failures = match (&self.runtime_root, &current_target_root) {
            (Some(runtime_root), Some(target_root)) => {
                match WorkflowEngine::with_target_root(runtime_root, target_root)
                    .preparation_failures_needing_attention()
                {
                    Ok(failures) => failures
                        .into_iter()
                        .filter(|claim| node_filter == "all" || claim.node_id == active_node_id)
                        .collect::<Vec<_>>(),
                    Err(error) => return error_response(error),
                }
            }
            _ => Vec::new(),
        };
        let retry_delays = match (&self.runtime_root, &current_target_root) {
            (Some(runtime_root), Some(target_root)) => {
                match WorkflowEngine::with_target_root(runtime_root, target_root)
                    .retry_delays_needing_attention()
                {
                    Ok(delays) => delays
                        .into_iter()
                        .filter(|delay| node_filter == "all" || delay.node_id == active_node_id)
                        .collect::<Vec<_>>(),
                    Err(error) => return error_response(error),
                }
            }
            _ => Vec::new(),
        };
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
                "active_node_display_name": active_node_identity.display_name,
                "active_node_diagnostics": active_node_identity.diagnostics,
                "needs_attention": dashboard_attention_items(
                    &dashboard.attention_indicators,
                    &preparation_failures,
                    &retry_delays,
                    runner_reachable
                ),
                "attached": attached
            }),
        )
    }

    pub(crate) fn handle_guidance_next(&self) -> ApiResponse {
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
}
