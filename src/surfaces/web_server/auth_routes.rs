use serde_json::json;

use crate::core::product::scheduling::FileSchedulingService;
use crate::core::product::work_items::{BulkGapFilter, BulkGapSelection, BulkGapUpdate};

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn handle_workflow_schedule(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "schedule work items");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("persist scheduler state");
        };
        let scheduler = FileSchedulingService::with_durable_root(runtime_root, durable_root);
        match scheduler.schedule_and_dispatch() {
            Ok(body) => ApiResponse::json(200, json!(body)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_workflow_restore(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "restore workflow state");
        let service = self.work_item_service(durable_root);
        match service.bulk_update_gaps(
            BulkGapSelection {
                filter: BulkGapFilter::default(),
                selected_ids: None,
                exclude_ids: Vec::new(),
            },
            BulkGapUpdate::Status("__last_workflow_state".to_string()),
        ) {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_workflow_enforce(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "enforce workflow state");
        match self
            .work_item_service(durable_root)
            .workflow_enforcement_summary()
        {
            Ok(summary) => ApiResponse::json(200, json!(summary)),
            Err(error) => error_response(error),
        }
    }
}
