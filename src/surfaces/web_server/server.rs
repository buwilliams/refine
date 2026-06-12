use serde_json::json;

use crate::process::supervisor::lifecycle::{current_launch_executable, current_launch_mode};

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub fn handle(&self, request: ApiRequest) -> ApiResponse {
        let method = request.method.clone();
        let path = request.path.clone();
        let response = self.handle_inner(request);
        if method != "GET"
            && response.status < 400
            && should_refresh_projection_after_mutation(&path)
            && let Err(error) = self.refresh_projection_cache_after_mutation()
        {
            return error_response(error);
        }
        response
    }

    fn handle_inner(&self, mut request: ApiRequest) -> ApiResponse {
        let raw_path = request.path.clone();
        request.path = normalize_api_path(&request.path);

        if request.method == "GET" && request.path == "/system/version" {
            return ApiResponse::json(
                200,
                json!({
                    "product": "refine",
                    "version": env!("CARGO_PKG_VERSION"),
                    "launch_mode": current_launch_mode(),
                    "executable_path": current_launch_executable(),
                    "api_contract_version": API_CONTRACT_VERSION,
                    "supported_api_contract_versions": [API_CONTRACT_VERSION]
                }),
            );
        }

        if request.method == "GET" && request.path == "/activity" {
            return self.handle_activity_list(&raw_path);
        }

        if request.method == "POST" && request.path == "/activity/cleanup" {
            return self.handle_activity_cleanup(request);
        }

        if request.method == "POST" && request.path == "/activity/ui-error" {
            return self.handle_activity_ui_error(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/operations/")
            && request.path.ends_with("/logs")
        {
            return self.handle_operation_logs(request, &raw_path);
        }

        if request.method == "GET" && request.path.starts_with("/operations/") {
            return self.handle_operation_status(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/workflow/executions/")
            && request.path.ends_with("/retry")
        {
            return self.handle_workflow_execution_retry(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/workflow/executions/")
            && request.path.ends_with("/cancel")
        {
            return self.handle_workflow_execution_cancel(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/operations/")
            && request.path.ends_with("/cancel")
        {
            return self.handle_operation_cancel(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/processes/")
            && request.path.ends_with("/stream")
        {
            return self.handle_process_stream(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/processes/")
            && request.path.ends_with("/stop")
        {
            return self.handle_process_stop(request);
        }

        if request.method == "GET" && request.path == "/processes" {
            return self.handle_processes(&raw_path);
        }

        if request.method == "GET" && request.path == "/system/install" {
            return self.handle_install_status();
        }

        if request.method == "POST" && request.path == "/system/install" {
            return self.handle_install(request);
        }

        if request.method == "POST" && request.path == "/system/repair" {
            return self.handle_install_repair();
        }

        if request.method == "POST" && request.path == "/system/update" {
            return self.handle_install_update(request);
        }

        if request.method == "POST" && request.path == "/system/rollback" {
            return self.handle_install_rollback();
        }

        if request.method == "POST" && request.path == "/system/uninstall" {
            return self.handle_install_uninstall();
        }

        if request.method == "POST" && request.path == "/processes/background" {
            return self.handle_processes_background(request);
        }

        if request.method == "POST" && request.path == "/processes/agents" {
            return self.handle_processes_agents(request);
        }

        if request.method == "POST" && request.path == "/workflow/pause" {
            return self.handle_workflow_pause(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/agents/")
            && request.path.ends_with("/diagnostics")
        {
            return self.handle_agent_diagnostics(request);
        }

        if request.method == "GET" && request.path == "/agents/secrets" {
            return self.handle_agent_secrets_list();
        }

        if request.method == "GET" && request.path == "/agents/secrets/status" {
            return self.handle_agent_secrets_status();
        }

        if request.path.starts_with("/agents/secrets/") {
            return self.handle_agent_secret(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/agents/")
            && request.path.ends_with("/configure")
        {
            return self.handle_agent_configure(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/agents/")
            && request.path.ends_with("/invoke")
        {
            return self.handle_agent_invoke(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/agents/")
            && request.path.ends_with("/resume")
        {
            return self.handle_agent_resume(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/agents/")
            && (request.path.ends_with("/auth") || request.path.ends_with("/authenticate"))
        {
            return self.handle_agent_authenticate(request);
        }

        if request.method == "GET" && request.path == "/agents" {
            return self.handle_agents();
        }

        if request.method == "GET" && request.path == "/diagnostics" {
            return self.handle_diagnostics();
        }

        if request.method == "POST" && request.path == "/diagnostics/support-bundle" {
            return self.handle_support_bundle(request);
        }

        if request.method == "GET" && request.path == "/dashboard" {
            return self.handle_dashboard(&raw_path);
        }

        if request.method == "GET" && request.path == "/nodes" {
            return self.handle_nodes();
        }

        if request.method == "POST" && request.path == "/nodes" {
            return self.handle_node_create(request);
        }

        if request.method == "POST" && request.path == "/nodes/activate" {
            return self.handle_node_activate(request);
        }

        if request.method == "POST" && request.path == "/nodes/transfer-gaps" {
            return self.handle_node_transfer_gaps(request);
        }

        if request.method == "POST" && request.path == "/nodes/transfer-features" {
            return self.handle_node_transfer_features(request);
        }

        if request.method == "POST" && request.path == "/nodes/copy-settings" {
            return self.handle_node_copy_settings(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/nodes/") {
            return self.handle_node_update(request);
        }

        if request.method == "GET" && request.path == "/cluster" {
            return self.handle_cluster();
        }

        if request.method == "POST" && request.path == "/cluster/nodes" {
            return self.handle_remote_node_upsert(request, None);
        }

        if request.method == "PATCH" && request.path.starts_with("/cluster/nodes/") {
            let node_id = node_id_from_cluster_path(&request.path);
            return self.handle_remote_node_upsert(request, node_id);
        }

        if request.method == "DELETE" && request.path.starts_with("/cluster/nodes/") {
            let node_id = node_id_from_cluster_path(&request.path);
            return self.handle_remote_node_delete(node_id);
        }

        if request.method == "POST"
            && request.path.starts_with("/cluster/nodes/")
            && request.path.ends_with("/bootstrap")
        {
            return self.handle_remote_node_bootstrap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/cluster/nodes/")
            && request.path.ends_with("/run")
        {
            return self.handle_remote_node_run(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/cluster/nodes/")
            && request.path.ends_with("/transfer")
        {
            return self.handle_remote_node_transfer(request);
        }

        if request.method == "GET" && request.path == "/target-app/status" {
            return self.handle_target_app_status();
        }

        if (request.method == "GET" || request.method == "POST")
            && request.path == "/target-app/health"
        {
            return self.handle_target_app_health();
        }

        if request.method == "POST"
            && matches!(
                request.path.as_str(),
                "/target-app/start" | "/target-app/stop" | "/target-app/build"
            )
        {
            return self.handle_target_app_action(request);
        }

        if request.method == "POST" && request.path == "/target-app/generate-instructions" {
            return self.handle_target_app_generate_instructions(request);
        }

        if request.method == "POST" && request.path == "/runner-workers/target-app-builder/build" {
            return self.handle_target_app_build_queue();
        }

        if request.method == "POST" && request.path == "/runner-workers/merger/hard-reset-worktree"
        {
            return self.handle_merger_hard_reset_worktree();
        }

        if request.method == "GET" && request.path == "/changes" {
            return self.handle_changes_list(&raw_path);
        }

        if request.method == "POST" && request.path == "/changes/undo" {
            return self.handle_changes_undo(request);
        }

        if request.method == "POST" && request.path == "/cache/rebuild" {
            return self.handle_cache_rebuild();
        }

        if request.method == "GET" && request.path == "/performance" {
            return self.handle_performance_list(&raw_path);
        }

        if request.method == "POST" && request.path == "/performance/cleanup" {
            return self.handle_performance_cleanup(request);
        }

        if request.method == "POST" && request.path == "/terminal/session" {
            return self.handle_terminal_session_start(request);
        }

        if request.method == "GET"
            && let Some(_) = terminal_session_route(&request.path, "/events")
        {
            return self.handle_terminal_events_snapshot(&raw_path);
        }

        if request.method == "POST"
            && let Some(session_id) = terminal_session_route(&request.path, "/input")
        {
            return self.handle_terminal_input(request, &session_id);
        }

        if request.method == "POST"
            && let Some(session_id) = terminal_session_route(&request.path, "/resize")
        {
            return self.handle_terminal_resize(request, &session_id);
        }

        if request.method == "POST"
            && let Some(session_id) = terminal_session_route(&request.path, "/stop")
        {
            return self.handle_terminal_stop(&session_id);
        }

        if request.method == "GET" && request.path == "/files/tree" {
            return self.handle_files_tree(&raw_path);
        }

        if request.method == "GET" && request.path == "/files/read" {
            return self.handle_files_read(&raw_path);
        }

        if request.method == "GET" && request.path == "/files/search" {
            return self.handle_files_search(&raw_path);
        }

        if request.method == "POST" && request.path == "/import/extract" {
            return self.handle_import_extract(request);
        }

        if request.method == "POST" && request.path == "/import/csv/parse" {
            return self.handle_import_csv_parse(request);
        }

        if request.method == "POST" && request.path == "/import/dedup" {
            return self.handle_import_dedup(request);
        }

        if request.method == "POST" && request.path == "/import/persist" {
            return self.handle_import_persist(request);
        }

        if request.method == "GET"
            && (request.path == "/project/status" || request.path == "/apps/status")
        {
            return self.handle_project_status();
        }

        if request.method == "GET" && request.path == "/project/path" {
            return self.handle_project_path(&raw_path);
        }

        if request.method == "GET" && request.path == "/project/directories" {
            return self.handle_project_directories(&raw_path);
        }

        if request.method == "GET" && (request.path == "/projects" || request.path == "/apps") {
            return self.handle_project_list();
        }

        if request.method == "DELETE" && (request.path == "/projects" || request.path == "/apps") {
            return self.handle_project_remove(request);
        }

        if request.method == "POST"
            && (request.path == "/project/attach" || request.path == "/apps/attach")
        {
            return self.handle_project_attach(request);
        }

        if request.method == "POST" && request.path == "/project/migrate" {
            return self.handle_project_migrate();
        }

        if request.method == "POST" && request.path == "/apps/register" {
            return self.handle_project_register(request);
        }

        if request.method == "POST" && request.path == "/apps/clone" {
            return self.handle_project_clone(request);
        }

        if request.method == "POST" && request.path == "/apps/switch" {
            return self.handle_project_switch(request);
        }

        if request.method == "POST"
            && (request.path == "/project/detach" || request.path == "/apps/detach")
        {
            return self.handle_project_detach();
        }

        if request.method == "POST" && request.path == "/project/sync" {
            return self.handle_project_sync();
        }

        if request.method == "GET" && request.path == "/settings" {
            return self.handle_settings_get();
        }

        if request.method == "PATCH" && request.path == "/settings" {
            return self.handle_settings_update(request);
        }

        if request.method == "POST" && request.path == "/settings/recheck-auth" {
            return self.handle_recheck_auth();
        }

        if request.method == "GET" && request.path == "/upgrade" {
            return self.handle_upgrade_status();
        }

        if request.method == "GET" && request.path == "/governance" {
            return self.handle_governance_get();
        }

        if request.method == "PATCH" && request.path == "/governance" {
            return self.handle_governance_save(request);
        }

        if request.method == "POST" && request.path == "/governance/generate-rules" {
            return self.handle_governance_generate_rules(request);
        }

        if request.method == "GET" && request.path == "/guidance" {
            return self.handle_guidance_list();
        }

        if request.method == "PUT" && request.path == "/guidance" {
            return self.handle_guidance_update(request);
        }

        if request.method == "GET" && request.path == "/reporters" {
            return self.handle_reporters_list();
        }

        if request.method == "POST" && request.path == "/reporters" {
            return self.handle_reporter_create(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/reporters/") {
            return self.handle_reporter_rename(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/reporters/")
            && request.path.ends_with("/merge")
        {
            return self.handle_reporter_merge(request);
        }

        if request.method == "DELETE" && request.path.starts_with("/reporters/") {
            return self.handle_reporter_delete(request);
        }

        if request.method == "GET" && request.path == "/quality" {
            return self.handle_quality_get();
        }

        if request.method == "PATCH" && request.path == "/quality" {
            return self.handle_quality_save(request);
        }

        if request.method == "POST" && request.path == "/quality/checks" {
            return self.handle_quality_checks(request);
        }

        if request.method == "GET" && request.path == "/quality/screenshots" {
            return self.handle_quality_screenshots(&raw_path);
        }

        if request.method == "POST" && request.path == "/chat/start" {
            return self.handle_chat_start(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/chat/")
            && request.path.ends_with("/input")
        {
            return self.handle_chat_input(request);
        }

        if request.method == "PATCH"
            && request.path.starts_with("/chat/")
            && request.path.contains("/queue/")
        {
            return self.handle_chat_queue_update(request);
        }

        if request.method == "DELETE"
            && request.path.starts_with("/chat/")
            && request.path.contains("/queue/")
        {
            return self.handle_chat_queue_delete(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/chat/")
            && request.path.ends_with("/read")
        {
            return self.handle_chat_read(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/chat/")
            && request.path.ends_with("/submit-ready-merge")
        {
            return self.handle_chat_submit_ready_merge(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/chat/")
            && request.path.ends_with("/stop")
        {
            return self.handle_chat_stop(request);
        }

        if request.method == "POST" && request.path == "/work/gaps" {
            return self.handle_gap_create(request);
        }

        if request.method == "POST" && request.path == "/work/gaps/bulk" {
            return self.handle_gap_bulk_update(request);
        }

        if request.method == "POST" && request.path == "/work/gaps/bulk/delete" {
            return self.handle_gap_bulk_delete(request);
        }

        if request.method == "POST" && request.path == "/work/features" {
            return self.handle_feature_create(request);
        }

        if request.method == "POST" && request.path == "/work/features/bulk" {
            return self.handle_feature_bulk_update(request);
        }

        if request.method == "POST" && request.path == "/work/features/bulk/delete" {
            return self.handle_feature_bulk_delete(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/gaps/bulk")
        {
            return self.handle_feature_bulk_assign_gaps(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/gaps")
        {
            return self.handle_feature_add_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/order")
        {
            return self.handle_feature_order_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/unorder")
        {
            return self.handle_feature_unorder_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.contains("/gaps/")
            && !request.path.ends_with("/reorder")
        {
            return self.handle_feature_add_gap_path(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/reorder")
        {
            return self.handle_feature_reorder_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/move")
        {
            return self.handle_feature_move(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/transfer")
        {
            return self.handle_feature_transfer(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/cancel")
        {
            return self.handle_feature_cancel(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/work/features/") {
            return self.handle_feature_update(request);
        }

        if request.method == "DELETE"
            && request.path.starts_with("/work/features/")
            && request.path.contains("/gaps/")
        {
            return self.handle_feature_remove_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/notes")
        {
            return self.handle_gap_note(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/rounds")
        {
            return self.handle_gap_round_append(request);
        }

        if request.method == "PATCH"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/rounds/latest")
        {
            return self.handle_gap_round_edit_latest(request);
        }

        if request.method == "PATCH"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/rounds/latest/evaluation")
        {
            return self.handle_gap_round_evaluation_update(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.contains("/rounds/")
            && request.path.ends_with("/logs")
        {
            return self.handle_gap_round_log_append(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/logs")
        {
            return self.handle_gap_logs(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/transition")
        {
            return self.handle_gap_transition(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && (request.path.ends_with("/start")
                || request.path.ends_with("/verify")
                || request.path.ends_with("/retry-quality")
                || request.path.ends_with("/retry-merge")
                || request.path.ends_with("/submit-merge")
                || request.path.ends_with("/merge")
                || request.path.ends_with("/undo"))
        {
            return self.handle_gap_action(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/cancel")
        {
            return self.handle_gap_cancel(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/work/gaps/") {
            return self.handle_gap_update(request);
        }

        if request.method == "DELETE" && request.path.starts_with("/work/gaps/") {
            return self.handle_gap_delete(request);
        }

        if request.method == "GET" && request.path.starts_with("/work/gaps/") {
            return self.handle_gap_show(request);
        }

        if request.method == "GET" && request.path.starts_with("/work/features/") {
            return self.handle_feature_show(request);
        }

        if request.method == "DELETE" && request.path.starts_with("/work/features/") {
            return self.handle_feature_delete(request);
        }

        if request.method == "GET" && request.path == "/work/gaps" {
            return self.handle_gaps_list(&raw_path);
        }

        if request.method == "GET" && request.path == "/work/features" {
            return self.handle_features_list(&raw_path);
        }

        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/system/status") => ApiResponse::json(200, json!(self.status)),
            ("GET", "/system/api-groups") => {
                let groups: Vec<_> = API_GROUPS
                    .iter()
                    .map(|group| json!({"prefix": group.prefix, "capability": group.capability}))
                    .collect();
                ApiResponse::json(200, json!({"groups": groups}))
            }
            ("GET", "/events") => ApiResponse::json(
                200,
                json!({
                    "stream": "sse",
                    "events": ["activity", "process", "operation", "chat"]
                }),
            ),
            _ => ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": format!("no route for {} {}", request.method, request.path)
                    }
                }),
            ),
        }
    }
}

fn terminal_session_route(path: &str, suffix: &str) -> Option<String> {
    let rest = path.strip_prefix("/terminal/")?;
    let session_id = rest.strip_suffix(suffix)?;
    if session_id.is_empty() || session_id.contains('/') {
        return None;
    }
    Some(session_id.to_string())
}

fn should_refresh_projection_after_mutation(path: &str) -> bool {
    let path = normalize_api_path(path);
    !path.starts_with("/terminal/") && path != "/project/sync"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_mutations_do_not_refresh_projection_cache() {
        assert!(!should_refresh_projection_after_mutation(
            "/api/terminal/session"
        ));
        assert!(!should_refresh_projection_after_mutation(
            "/api/terminal/session-1/input"
        ));
        assert!(!should_refresh_projection_after_mutation(
            "/terminal/session-1/resize"
        ));
        assert!(!should_refresh_projection_after_mutation(
            "/api/project/sync"
        ));
        assert!(should_refresh_projection_after_mutation(
            "/api/gaps/GAP1/start"
        ));
    }
}

fn node_id_from_cluster_path(path: &str) -> Option<String> {
    path.strip_prefix("/cluster/nodes/")
        .and_then(|rest| rest.split('/').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
