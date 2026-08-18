use serde_json::{Value, json};

use crate::application::projects::projection::{FeatureSummaryProjection, ProjectionSnapshot};

use super::ApiResponse;

pub(super) fn feature_detail_response_from_goals(
    feature: &FeatureSummaryProjection,
    goals: Vec<Value>,
) -> Value {
    let mut value = serde_json::to_value(&feature.feature).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("status".to_string(), json!(feature.rollup.status));
        object.insert("goal_count".to_string(), json!(feature.rollup.goal_count));
        object.insert("done_count".to_string(), json!(feature.rollup.done_count));
        object.insert(
            "active_count".to_string(),
            json!(feature.rollup.active_count),
        );
        object.insert(
            "failed_count".to_string(),
            json!(feature.rollup.failed_count),
        );
        object.insert(
            "cancelled_count".to_string(),
            json!(feature.rollup.cancelled_count),
        );
        object.insert(
            "blocked_count".to_string(),
            json!(feature.rollup.blocked_count),
        );
        object.insert("next_goal".to_string(), json!(feature.rollup.next_goal));
        object.insert("goal_ids".to_string(), json!(feature.goal_ids));
        object.insert("goals".to_string(), json!(goals));
        object.insert("rollup".to_string(), json!(feature.rollup));
    }
    value
}

pub(super) fn feature_reorder_order_from_body(
    body: Option<&Value>,
    projection: &ProjectionSnapshot,
    feature_id: &str,
    goal_id: &str,
) -> Result<i64, ApiResponse> {
    let Some(body) = body else {
        return Err(ApiResponse::json(
            400,
            json!({
                "error": {
                    "code": "invalid_order",
                    "message": "body.order, body.before, or body.after is required"
                }
            }),
        ));
    };
    if let Some(order) = body.get("order").and_then(|order| order.as_i64()) {
        return Ok(order);
    }
    let before = body.get("before").and_then(|target| target.as_str());
    let after = body.get("after").and_then(|target| target.as_str());
    let Some((target_id, insert_after)) = (match (before, after) {
        (Some(_), Some(_)) => None,
        (Some(target_id), None) => Some((target_id, false)),
        (None, Some(target_id)) => Some((target_id, true)),
        (None, None) => None,
    }) else {
        return Err(ApiResponse::json(
            400,
            json!({
                "error": {
                    "code": "invalid_order",
                    "message": "body.order, body.before, or body.after is required"
                }
            }),
        ));
    };
    let Some(feature) = projection.features.get(feature_id) else {
        return Err(ApiResponse::json(
            404,
            json!({
                "error": {
                    "code": "not_found",
                    "message": format!("Feature {feature_id} was not found")
                }
            }),
        ));
    };
    let mut ordered_goal_ids = feature
        .goal_ids
        .iter()
        .filter(|id| {
            projection
                .goals
                .get(*id)
                .and_then(|goal| goal.goal.feature_order)
                .is_some()
        })
        .cloned()
        .collect::<Vec<_>>();
    let Some(source_index) = ordered_goal_ids.iter().position(|id| id == goal_id) else {
        return Err(ApiResponse::json(
            404,
            json!({
                "error": {
                    "code": "not_found",
                    "message": format!("Goal {goal_id} was not found in Feature {feature_id}")
                }
            }),
        ));
    };
    if target_id == goal_id {
        return Ok(source_index as i64 + 1);
    }
    ordered_goal_ids.remove(source_index);
    let Some(target_index) = ordered_goal_ids.iter().position(|id| id == target_id) else {
        return Err(ApiResponse::json(
            400,
            json!({
                "error": {
                    "code": "invalid_order",
                    "message": format!("target Goal {target_id} is not assigned to Feature {feature_id}")
                }
            }),
        ));
    };
    let insert_index = if insert_after {
        target_index + 1
    } else {
        target_index
    };
    Ok(insert_index as i64 + 1)
}
