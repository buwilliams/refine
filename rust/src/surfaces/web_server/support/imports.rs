use serde_json::json;

use crate::core::product::work_items::{BulkGapUpdate, FileWorkItemService};
use crate::core::supervisor::errors::RefineResult;

use super::super::*;

pub(in crate::surfaces::web_server) fn body_text(body: &serde_json::Value) -> &str {
    body.get("text")
        .or_else(|| body.get("csv"))
        .or_else(|| body.get("content"))
        .or_else(|| body.get("input"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

pub(in crate::surfaces::web_server) fn import_destination_feature_id(
    service: &FileWorkItemService,
    body: &serde_json::Value,
) -> RefineResult<Option<crate::core::product::project_state::FeatureSummaryProjection>> {
    if let Some(name) = body
        .get("new_feature_name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return service
            .create_feature_summary(
                name,
                None,
                body.get("new_feature_description")
                    .or_else(|| body.get("feature_description"))
                    .and_then(|value| value.as_str()),
                body.get("feature_reporter")
                    .or_else(|| body.get("reporter"))
                    .and_then(|value| value.as_str()),
            )
            .map(Some);
    }
    if let Some(feature_id) = body
        .get("feature_id")
        .or_else(|| body.get("feature"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|feature_id| !feature_id.is_empty())
    {
        return service.show_feature_summary(feature_id).map(Some);
    }
    Ok(None)
}

pub(in crate::surfaces::web_server) fn feature_import_response(
    feature: &crate::core::product::project_state::FeatureSummaryProjection,
) -> serde_json::Value {
    json!({
        "id": feature.feature.id,
        "name": feature.feature.name,
        "gap_ids": feature.gap_ids,
        "rollup": feature.rollup
    })
}

pub(in crate::surfaces::web_server) fn normalized_dedup_text(values: &[&str]) -> String {
    values
        .iter()
        .flat_map(|value| value.split_whitespace())
        .map(|part| {
            part.chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(in crate::surfaces::web_server) fn gap_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Gap route requires a Gap id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn gap_id_and_action(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/work/gaps/")?;
    let (gap_id, action) = rest.rsplit_once('/')?;
    if gap_id.is_empty() || gap_id.contains('/') || action.is_empty() {
        return None;
    }
    Some((gap_id, action))
}

pub(in crate::surfaces::web_server) fn gap_action_message(action: &str) -> &'static str {
    match action {
        "verify" => "Verified",
        "retry-quality" => "Queued for QA",
        "retry-merge" => "Queued for merge",
        "merge" => "Merged",
        "undo" => "Undone",
        _ => "Gap action completed",
    }
}

pub(in crate::surfaces::web_server) fn feature_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Feature route requires a Feature id"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn invalid_round_body() -> ApiResponse {
    ApiResponse::json(
        400,
        json!({
            "error": {
                "code": "invalid_round",
                "message": "round reporter, actual, and target are required"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn invalid_bulk_body() -> ApiResponse {
    ApiResponse::json(
        400,
        json!({
            "error": {
                "code": "invalid_bulk",
                "message": "bulk request must include selection fields and exactly one update when updating"
            }
        }),
    )
}

pub(in crate::surfaces::web_server) fn parse_bulk_gap_update(
    body: &serde_json::Value,
) -> Option<BulkGapUpdate> {
    let update = body.get("update")?.as_object()?;
    let mut entries = update
        .iter()
        .filter(|(key, _)| matches!(key.as_str(), "priority" | "status" | "reporter"));
    let (field, value) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    let value = value.as_str()?.to_string();
    match field.as_str() {
        "priority" => Some(BulkGapUpdate::Priority(value)),
        "status" => Some(BulkGapUpdate::Status(value)),
        "reporter" => Some(BulkGapUpdate::Reporter(value)),
        _ => None,
    }
}
