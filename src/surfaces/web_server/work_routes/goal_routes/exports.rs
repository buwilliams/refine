use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_goal_jira_export(&self, request: ApiRequest) -> ApiResponse {
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/export/jira"))
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        let refine_dir = require_refine_dir!(self, "export Goal evidence for Jira");
        let target_root = match self.current_target_root() {
            Ok(Some(target_root)) => target_root,
            Ok(None) => return target_root_unavailable("export Goal evidence for Jira"),
            Err(error) => return error_response(error),
        };
        let service = match &self.runtime_root {
            Some(runtime_root) => {
                FileGoalExportService::with_runtime_root(refine_dir, target_root, runtime_root)
            }
            None => FileGoalExportService::new(refine_dir, target_root),
        };
        match service.export_jira_csv(goal_id) {
            Ok(export) => ApiResponse::json(200, json!({"export": export})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goals_jira_export(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "export selected Goal evidence for Jira");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let target_root = match self.current_target_root() {
            Ok(Some(target_root)) => target_root,
            Ok(None) => return target_root_unavailable("export selected Goal evidence for Jira"),
            Err(error) => return error_response(error),
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("export selected Goal evidence for Jira");
        };
        match FileRunnerWorkerService::new(runtime_root).queue_jira_export(
            &refine_dir,
            &target_root,
            selection,
        ) {
            Ok(operation) => {
                ApiResponse::json(202, json!({"operation": operation_response(operation)}))
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goals_jira_export_retry(&self, request: ApiRequest) -> ApiResponse {
        let Some(operation_id) = request
            .path
            .strip_prefix("/work/goals/export/jira/")
            .and_then(|path| path.strip_suffix("/retry"))
            .filter(|operation_id| !operation_id.is_empty() && !operation_id.contains('/'))
        else {
            return operation_id_required();
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("recover a Jira export");
        };
        match FileRunnerWorkerService::new(runtime_root).retry_jira_export(operation_id) {
            Ok(operation) => {
                ApiResponse::json(202, json!({"operation": operation_response(operation)}))
            }
            Err(error) => error_response(error),
        }
    }
}
