use std::path::PathBuf;

use serde_json::json;

use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::logs::FileLogService;
use crate::core::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::core::product::imports::{FileImportService, import_drafts_from_value};
use crate::core::product::project_state::{
    ActivityProjectionQuery, ChangeProjectionQuery, FeatureProjectionQuery, FileProjectStateStore,
    GapProjectionQuery, PROJECTION_SNAPSHOT_FILE, PageRequest, ProjectStateStore, ProjectionQuery,
};
use crate::core::product::work_items::BulkGapSelection;
use crate::core::supervisor::errors::RefineError;
use crate::model::log::LogEntry;
use crate::model::workflow::GapStatus;

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn handle_gap_transition(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "mutate work items");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/transition"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Gap transition route requires a Gap id"
                    }
                }),
            );
        };
        let Some(status) = request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
            .and_then(GapStatus::parse_wire)
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_status",
                        "message": "body.status must be a valid Gap status"
                    }
                }),
            );
        };

        match self
            .work_item_service(durable_root)
            .transition_gap_status(gap_id, status)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_action(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "mutate work items");
        let Some((gap_id, action)) = gap_id_and_action(&request.path) else {
            return gap_id_required();
        };
        let service = self.work_item_service(durable_root);
        let result = match action {
            "start" => service.start_gap_workflow(gap_id),
            "verify" => service.verify_gap_summary(gap_id),
            "retry-quality" => service.retry_gap_quality_summary(gap_id),
            "retry-merge" => service.retry_gap_merge_summary(gap_id),
            "merge" => service.merge_gap_summary(gap_id),
            "undo" => service.undo_gap_summary(gap_id),
            _ => {
                return ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": "unknown Gap action"
                        }
                    }),
                );
            }
        };
        match result {
            Ok(gap) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "message": gap_action_message(action),
                    "gap": gap.gap
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_create(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "create work items");
        let Some(name) = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name is required"
                    }
                }),
            );
        };
        let id = request
            .body
            .as_ref()
            .and_then(|body| body.get("id"))
            .and_then(|id| id.as_str());

        match self
            .work_item_service(durable_root)
            .create_gap_summary(name, id)
        {
            Ok(gap) => ApiResponse::json(201, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_bulk_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "bulk update work items");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some(update) = parse_bulk_gap_update(body) else {
            return invalid_bulk_body();
        };
        match self
            .work_item_service(durable_root)
            .bulk_update_gaps(selection, update)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_bulk_delete(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "bulk delete work items");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(durable_root)
            .bulk_delete_gaps(selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_create(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "create features");
        let Some(name) = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name is required"
                    }
                }),
            );
        };
        let id = request
            .body
            .as_ref()
            .and_then(|body| body.get("id"))
            .and_then(|id| id.as_str());
        let description = request
            .body
            .as_ref()
            .and_then(|body| body.get("description"))
            .and_then(|description| description.as_str());
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|reporter| reporter.as_str());
        match self.work_item_service(durable_root).create_feature_summary(
            name,
            id,
            description,
            reporter,
        ) {
            Ok(feature) => ApiResponse::json(
                201,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match self
            .work_item_service(durable_root)
            .update_feature_metadata_summary(
                feature_id,
                body.get("name").and_then(|value| value.as_str()),
                body.get("description").and_then(|value| value.as_str()),
                body.get("reporter").and_then(|value| value.as_str()),
            ) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_bulk_assign_gaps(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "bulk assign Gaps to Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/gaps/bulk"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(durable_root)
            .bulk_assign_gaps_to_feature(feature_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update work items");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str());
        let priority = request
            .body
            .as_ref()
            .and_then(|body| body.get("priority"))
            .and_then(|priority| priority.as_str());
        match self
            .work_item_service(durable_root)
            .update_gap_metadata_summary(gap_id, name, priority)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_note(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "edit work items");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/notes"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return gap_id_required();
        };
        let Some(body) = request
            .body
            .as_ref()
            .and_then(|body| body.get("body"))
            .and_then(|body| body.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_note",
                        "message": "body.body is required"
                    }
                }),
            );
        };
        let author = request
            .body
            .as_ref()
            .and_then(|body| body.get("author"))
            .and_then(|author| author.as_str())
            .unwrap_or("");
        match self
            .work_item_service(durable_root)
            .add_gap_note_summary(gap_id, author, body)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_round_append(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "append Gap rounds");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/rounds"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return gap_id_required();
        };
        let Some(reporter) = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let Some(actual) = request
            .body
            .as_ref()
            .and_then(|body| body.get("actual"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let Some(target) = request
            .body
            .as_ref()
            .and_then(|body| body.get("target"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        match self
            .work_item_service(durable_root)
            .append_gap_round_summary(gap_id, reporter, actual, target)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_round_edit_latest(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "edit latest Gap round");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/rounds/latest"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return gap_id_required();
        };
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str());
        let actual = request
            .body
            .as_ref()
            .and_then(|body| body.get("actual"))
            .and_then(|value| value.as_str());
        let target = request
            .body
            .as_ref()
            .and_then(|body| body.get("target"))
            .and_then(|value| value.as_str());
        match self
            .work_item_service(durable_root)
            .edit_latest_gap_round_summary(gap_id, reporter, actual, target)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_round_log_append(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "append Gap round logs");
        let Some(rest) = request.path.strip_prefix("/work/gaps/") else {
            return gap_id_required();
        };
        let Some((gap_id, round_part)) = rest.split_once("/rounds/") else {
            return gap_id_required();
        };
        let Some(round_idx) = round_part
            .strip_suffix("/logs")
            .and_then(|value| value.parse::<usize>().ok())
        else {
            return ApiResponse::json(
                400,
                json!({"error": {"code": "invalid_round", "message": "round index is required"}}),
            );
        };
        let gap = match self
            .work_item_service(&durable_root)
            .show_gap_summary(gap_id)
        {
            Ok(gap) => gap,
            Err(error) => return error_response(error),
        };
        if round_idx >= gap.gap.round_count {
            return ApiResponse::json(
                404,
                json!({"error": {"code": "not_found", "message": "Round not found"}}),
            );
        }
        let body = request.body.unwrap_or_else(|| json!({}));
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return ApiResponse::json(
                400,
                json!({"error": {"code": "invalid_log", "message": "log message is required"}}),
            );
        }
        let entry = LogEntry {
            datetime: body
                .get("datetime")
                .and_then(|datetime| datetime.as_str())
                .unwrap_or("")
                .to_string(),
            severity: body
                .get("severity")
                .and_then(|severity| severity.as_str())
                .unwrap_or("info")
                .to_string(),
            category: body
                .get("category")
                .and_then(|category| category.as_str())
                .unwrap_or("state")
                .to_string(),
            message: message.to_string(),
            details: body
                .get("details")
                .and_then(|details| details.as_object())
                .cloned(),
            actions: Vec::new(),
            actor: body
                .get("actor")
                .and_then(|actor| actor.as_str())
                .map(str::to_string),
            gap_id: Some(gap_id.to_string()),
        };
        match FileLogService::new(durable_root).append_round_log(gap_id, round_idx, entry) {
            Ok(log) => ApiResponse::json(
                200,
                json!({"log": log, "gap_id": gap_id, "round_idx": round_idx}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_logs(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "read Gap round logs");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        let gap = match self
            .work_item_service(&durable_root)
            .show_gap_summary(gap_id)
        {
            Ok(gap) => gap,
            Err(error) => return error_response(error),
        };
        if gap.gap.round_count == 0 {
            return ApiResponse::json(
                404,
                json!({"error": {"code": "not_found", "message": "Round not found"}}),
            );
        }
        let round_idx = 0;
        match FileLogService::new(durable_root).page_round_logs(gap_id, round_idx, 50, 0) {
            Ok((logs, has_more, total)) => ApiResponse::json(
                200,
                json!({
                    "gap_id": gap_id,
                    "round_idx": round_idx,
                    "logs": logs,
                    "pagination": {
                        "limit": 50,
                        "offset": 0,
                        "total": total,
                        "has_more": has_more
                    },
                    "round_log_count": total,
                    "activity_count": 0
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_delete(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "delete work items");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        match self
            .work_item_service(durable_root)
            .delete_gap_record(gap_id)
        {
            Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": gap_id})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_cancel(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "cancel work items");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        match self
            .work_item_service(durable_root)
            .cancel_gap_summary(gap_id)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_show(&self, request: ApiRequest) -> ApiResponse {
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        match self.current_projection() {
            Ok(projection) => match projection.features.get(feature_id) {
                Some(feature) => ApiResponse::json(
                    200,
                    json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
                ),
                None => ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": format!("Feature {feature_id} was not found")
                        }
                    }),
                ),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_add_gap(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "assign Gaps to Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/gaps"))
            .filter(|feature_id| !feature_id.is_empty())
        else {
            return feature_id_required();
        };
        let Some(gap_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("gap_id"))
            .and_then(|gap_id| gap_id.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_gap_id",
                        "message": "body.gap_id is required"
                    }
                }),
            );
        };
        match self
            .work_item_service(durable_root)
            .assign_gap_to_feature(feature_id, gap_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_add_gap_path(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "assign Gaps to Features");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, gap_part)) = rest.split_once("/gaps/") else {
            return feature_id_required();
        };
        let gap_id = gap_part;
        if feature_id.is_empty() || gap_id.is_empty() || gap_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(durable_root)
            .assign_gap_to_feature(feature_id, gap_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_remove_gap(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "remove Gaps from Features");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, gap_part)) = rest.split_once("/gaps/") else {
            return feature_id_required();
        };
        let gap_id = gap_part;
        if feature_id.is_empty() || gap_id.is_empty() || gap_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(durable_root)
            .remove_gap_from_feature(feature_id, gap_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_reorder_gap(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "reorder Feature Gaps");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, gap_part)) = rest.split_once("/gaps/") else {
            return feature_id_required();
        };
        let Some(gap_id) = gap_part.strip_suffix("/reorder") else {
            return gap_id_required();
        };
        let Some(order) = request
            .body
            .as_ref()
            .and_then(|body| body.get("order"))
            .and_then(|order| order.as_i64())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_order",
                        "message": "body.order is required"
                    }
                }),
            );
        };
        match self
            .work_item_service(durable_root)
            .reorder_gap_in_feature(feature_id, gap_id, order)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_move(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "move Feature workflow");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/move"))
            .filter(|feature_id| !feature_id.is_empty())
        else {
            return feature_id_required();
        };
        let Some(target) = request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
            .and_then(GapStatus::parse_wire)
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_status",
                        "message": "body.status must be backlog or todo"
                    }
                }),
            );
        };
        match self
            .work_item_service(durable_root)
            .move_feature_workflow(feature_id, target)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_cancel(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "cancel Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let gap_ids = match self.current_projection() {
            Ok(projection) => projection
                .features
                .get(feature_id)
                .map(|feature| feature.gap_ids.clone())
                .unwrap_or_default(),
            Err(error) => return error_response(error),
        };
        let runtime_reconciled = match self.reconcile_feature_runtime_work(feature_id, &gap_ids) {
            Ok(summary) => summary,
            Err(error) => return error_response(error),
        };
        match self
            .work_item_service(durable_root)
            .cancel_feature_summary(feature_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup,
                    "runtime_reconciled": {
                        "processes": runtime_reconciled.processes,
                        "jobs": runtime_reconciled.jobs
                    }
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_delete(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "delete Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        match self
            .work_item_service(durable_root)
            .delete_feature_record(feature_id)
        {
            Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": feature_id})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gap_show(&self, request: ApiRequest) -> ApiResponse {
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Gap route requires a Gap id"
                    }
                }),
            );
        };
        match self.current_projection() {
            Ok(projection) => match projection.gaps.get(gap_id) {
                Some(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
                None => ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": format!("Gap {gap_id} was not found")
                        }
                    }),
                ),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_gaps_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let page = bounded_query_usize(raw_path, "page", 1, usize::MAX).max(1);
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| (page - 1).saturating_mul(limit));
        let query = GapProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "updated".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            status: query_param(raw_path, "status").and_then(|value| GapStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            node: query_param(raw_path, "node"),
            current_node_id: Some("default".to_string()),
            feature: query_param(raw_path, "feature"),
            rounds_gte: query_param(raw_path, "rounds_gte")
                .and_then(|value| value.parse::<usize>().ok()),
            rounds_lte: query_param(raw_path, "rounds_lte")
                .and_then(|value| value.parse::<usize>().ok()),
            severity: query_param(raw_path, "severity"),
            category: query_param(raw_path, "category"),
            actor: query_param(raw_path, "actor"),
        };
        let result = projection.list_gaps(query);
        ApiResponse::json(
            200,
            json!({
                "gaps": result.gaps,
                "counts": projection.status_counts(),
                "filtered_counts": result.filtered_status_counts,
                "matching_ids": result.matching_ids,
                "projection_version": projection.version,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "page": page,
                    "total": result.total,
                    "has_more": offset + limit < result.total
                }
            }),
        )
    }

    pub(super) fn handle_features_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let page = bounded_query_usize(raw_path, "page", 1, usize::MAX).max(1);
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| (page - 1).saturating_mul(limit));
        let query = FeatureProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "updated".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            status: query_param(raw_path, "status").and_then(|value| GapStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            node: query_param(raw_path, "node"),
            current_node_id: Some("default".to_string()),
        };
        let result = projection.list_features(query);
        let features: Vec<_> = result
            .features
            .into_iter()
            .map(|feature| {
                json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                })
            })
            .collect();
        ApiResponse::json(
            200,
            json!({
                "features": features,
                "matching_ids": result.matching_ids,
                "projection_version": projection.version,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "page": page,
                    "total": result.total,
                    "has_more": offset + limit < result.total
                }
            }),
        )
    }

    pub(super) fn handle_activity_list(&self, raw_path: &str) -> ApiResponse {
        let Some(_) = (match self.current_durable_root() {
            Ok(durable_root) => durable_root,
            Err(error) => return error_response(error),
        }) else {
            return durable_root_unavailable("read activity");
        };
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        let result = projection.list_activity(ActivityProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "datetime".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            gap_id: query_param(raw_path, "gap_id"),
            severity: query_param(raw_path, "severity"),
            category: query_param(raw_path, "category"),
            actor: query_param(raw_path, "actor"),
            q: query_param(raw_path, "q"),
        });
        ApiResponse::json(
            200,
            json!({
                "activity": result.activity,
                "facets": result.facets,
                "matching_ids": result.matching_ids,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "has_more": offset + limit < result.total,
                    "total": result.total
                }
            }),
        )
    }

    pub(super) fn handle_activity_ui_error(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "record UI activity");
        let body = request.body.unwrap_or_else(|| json!({}));
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("UI error")
            .trim();
        let service = FileActivityService::new(durable_root);
        let mut entry = service.new_entry(
            if message.is_empty() {
                "UI error"
            } else {
                message
            },
            "error",
            "ui",
            body.get("gap_id")
                .and_then(|gap_id| gap_id.as_str())
                .map(str::to_string),
            Some("browser".to_string()),
        );
        if let Some(details) = body.as_object() {
            entry.details = Some(details.clone());
        }
        match service.append(entry.clone()) {
            Ok(()) => ApiResponse::json(200, json!({"recorded": true, "entry": entry})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_activity_cleanup(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "clean up activity");
        let body = request.body.unwrap_or_else(|| json!({}));
        let days = body
            .get("days")
            .and_then(|value| value.as_i64())
            .unwrap_or(7);
        let clear = body
            .get("clear")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || days == 0;
        let service = FileActivityService::new(durable_root);
        match service.cleanup(days, clear) {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "deleted": result.deleted,
                    "retained": result.retained,
                    "cleared": result.cleared,
                    "retention_days": days
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_changes_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        let result = projection.list_changes(ChangeProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "committed".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            gap_id: query_param(raw_path, "gap_id"),
            status: query_param(raw_path, "status")
                .and_then(|status| GapStatus::parse_wire(&status)),
            priority: query_param(raw_path, "priority"),
            branch: query_param(raw_path, "branch"),
        });
        let branch = result
            .changes
            .iter()
            .find_map(|change| change.branch.clone())
            .unwrap_or_else(|| "main".to_string());
        let changes = result
            .changes
            .iter()
            .map(|change| {
                json!({
                    "commit": change.commit,
                    "gap_id": change.gap_id,
                    "name": change.gap_name,
                    "status": change.gap_status,
                    "priority": change.gap_priority,
                    "committed": change.committed_time,
                    "subject": change.subject,
                    "branch": change.branch
                })
            })
            .collect::<Vec<_>>();
        ApiResponse::json(
            200,
            json!({
                "branch": branch,
                "changes": changes,
                "matching_ids": result.matching_ids,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "has_more": offset + limit < result.total,
                    "total": result.total
                }
            }),
        )
    }

    pub(super) fn handle_changes_undo(&self, request: ApiRequest) -> ApiResponse {
        let commit = request
            .body
            .as_ref()
            .and_then(|body| body.get("commit"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if commit.is_empty() {
            return ApiResponse::json(
                400,
                json!({
                    "ok": false,
                    "error": {
                        "code": "invalid_input",
                        "message": "body.commit is required"
                    }
                }),
            );
        }
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("undo Git changes");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("undo Git changes");
        };
        match FileGitWorktreeService::with_runtime_root(source_root, runtime_root)
            .revert_commit(commit)
        {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "pushed": false,
                    "commit": commit,
                    "conflicts": result.conflicts,
                    "message": result.message.unwrap_or_default()
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cache_rebuild(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "rebuild projection cache");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("rebuild projection cache");
        };
        let store = FileProjectStateStore::new(durable_root);
        let projection = match store.rebuild_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let cache_dir = runtime_root.join("cache");
        if let Err(error) = store.persist_projection_snapshot(&cache_dir, &projection) {
            return error_response(error);
        }
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "mode": "rebuilt",
                "gaps": projection.gaps.len(),
                "features": projection.features.len(),
                "projection_version": projection.version,
                "cache": cache_dir.join(PROJECTION_SNAPSHOT_FILE).display().to_string()
            }),
        )
    }

    pub(super) fn handle_performance_list(&self, raw_path: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read performance metrics");
        };
        let query = PerformanceQuery {
            limit: bounded_query_usize(raw_path, "limit", 50, 1000),
            offset: bounded_query_usize(raw_path, "offset", 0, usize::MAX),
            operation: query_param(raw_path, "operation").filter(|value| !value.is_empty()),
            success: query_param(raw_path, "success").and_then(|value| match value.as_str() {
                "1" | "true" | "True" | "TRUE" => Some(true),
                "0" | "false" | "False" | "FALSE" => Some(false),
                _ => None,
            }),
        };
        match performance_report_value(runtime_root, query) {
            Ok(value) => {
                let response = ApiResponse::json(200, value.clone());
                if let Some(performance) = value.as_object().cloned() {
                    let _ = self.persist_runtime_projection_override(|runtime| {
                        runtime.performance = Some(performance);
                    });
                }
                response
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_performance_cleanup(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("clean up performance metrics");
        };
        let clear = request
            .body
            .as_ref()
            .and_then(|body| body.get("clear"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let service = FileMetricsService::new(runtime_root);
        match service.cleanup(clear) {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "deleted": result.deleted,
                    "retained": result.retained,
                    "cleared": result.cleared,
                    "retention_days": service.retention_days
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_files_tree(&self, raw_path: &str) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("read source files");
        };
        let path = query_param(raw_path, "path").unwrap_or_default();
        let recursive = query_param(raw_path, "recursive")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let max_depth = query_param(raw_path, "max_depth")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .min(8);
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(200)
            .clamp(1, 1000);
        match files_tree_response(&source_root, &path, recursive, max_depth, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_files_read(&self, raw_path: &str) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("read source file");
        };
        let path = query_param(raw_path, "path").unwrap_or_default();
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param(raw_path, "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(128_000)
            .clamp(1, 512_000);
        match files_read_response(&source_root, &path, offset, limit) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_files_search(&self, raw_path: &str) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("search source files");
        };
        let query = query_param(raw_path, "q").unwrap_or_default();
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20)
            .clamp(1, 200);
        match files_search_response(&source_root, &query, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_merger_hard_reset_worktree(&self) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("hard-reset Git worktree");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("hard-reset Git worktree");
        };
        match FileGitWorktreeService::with_runtime_root(source_root, runtime_root).hard_reset() {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "conflicts": result.conflicts,
                    "message": result.message.unwrap_or_default()
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_import_extract(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let text = body_text(&body);
        match FileImportService::new(PathBuf::new())
            .parse_text(text, body.get("reporter").and_then(|value| value.as_str()))
        {
            Ok(drafts) => ApiResponse::json(200, json!({"drafts": drafts})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_import_csv_parse(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileImportService::new(PathBuf::new()).parse_csv(
            body_text(&body),
            body.get("reporter").and_then(|value| value.as_str()),
        ) {
            Ok(drafts) => ApiResponse::json(200, json!({"drafts": drafts})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_import_dedup(&self, request: ApiRequest) -> ApiResponse {
        let Some(body) = request.body.as_ref() else {
            return error_response(RefineError::InvalidInput(
                "body.drafts must be an array".to_string(),
            ));
        };
        let drafts = match import_drafts_from_value(body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let mut matches = Vec::new();
        for (index, draft) in drafts.iter().enumerate() {
            let needle = normalized_dedup_text(&[
                draft.name.as_str(),
                draft.actual.as_str(),
                draft.target.as_str(),
            ]);
            if needle.is_empty() {
                continue;
            }
            if let Some(existing) = projection.gaps.values().find(|gap| {
                let haystack = normalized_dedup_text(&[
                    gap.gap.name.as_str(),
                    gap.searchable_text.as_str(),
                    gap.gap.id.as_str(),
                ]);
                haystack == needle || (!haystack.is_empty() && haystack.contains(&needle))
            }) {
                matches.push(json!({
                    "index": index + 1,
                    "score": 1.0,
                    "match": {
                        "id": existing.gap.id,
                        "name": existing.gap.name,
                        "status": existing.gap.status,
                        "priority": existing.gap.priority,
                        "reporter": existing.gap.reporter
                    }
                }));
            }
        }
        ApiResponse::json(
            200,
            json!({
                "matches": matches,
                "threshold": 1.0,
                "algorithm": "normalized_exact"
            }),
        )
    }

    pub(super) fn handle_import_persist(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "persist imported Gaps");
        let body = request.body.unwrap_or_else(|| json!({}));
        let drafts = match import_drafts_from_value(&body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let service = self.work_item_service(&durable_root);
        let mut failures = Vec::new();
        let mut feature_response = serde_json::Value::Null;
        let feature_id = match import_destination_feature_id(&service, &body) {
            Ok(feature) => {
                feature_response = feature
                    .as_ref()
                    .map(feature_import_response)
                    .unwrap_or(serde_json::Value::Null);
                feature.map(|feature| feature.feature.id)
            }
            Err(error) => {
                failures.push(json!({
                    "index": 0,
                    "name": "feature",
                    "message": error.to_string()
                }));
                None
            }
        };
        let import_result = if failures.is_empty() {
            match FileImportService::new(durable_root).persist(drafts, feature_id.as_deref()) {
                Ok(result) => Some(result),
                Err(error) => {
                    failures.push(json!({
                        "index": 0,
                        "name": "import",
                        "message": error.to_string()
                    }));
                    None
                }
            }
        } else {
            None
        };
        let created = import_result
            .as_ref()
            .map(|result| {
                result
                    .gap_ids
                    .iter()
                    .filter_map(|gap_id| service.show_gap_summary(gap_id).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(feature_id) = feature_id.as_deref() {
            if let Ok(feature) = service.show_feature_summary(feature_id) {
                feature_response = feature_import_response(&feature);
            }
        }

        ApiResponse::json(
            if failures.is_empty() { 201 } else { 207 },
            json!({
                "ok": failures.is_empty(),
                "count": created.len(),
                "created": created,
                "gaps": created.iter().map(|gap| &gap.gap).collect::<Vec<_>>(),
                "failures": failures,
                "duplicate_actions": {},
                "feature": feature_response
            }),
        )
    }
}
