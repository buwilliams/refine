use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::core::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::logs::FileLogService;
use crate::core::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::core::product::imports::{FileImportService, ImportDraft, import_drafts_from_value};
use crate::core::product::nodes::FileNodeRegistryService;
use crate::core::product::project_state::{
    ActivityProjectionQuery, ChangeProjectionQuery, FeatureProjectionQuery, GapProjectionQuery,
    PROJECTION_SNAPSHOT_FILE, PageRequest, ProjectionQuery,
};
use crate::core::product::work_items::{BulkGapSelection, FileWorkItemService};
use crate::core::supervisor::config::{ConfigService, FileSettingsService};
use crate::core::supervisor::errors::RefineError;
use crate::core::supervisor::jobs::{FileJobRegistry, JobRegistry, JobState};
use crate::model::log::LogEntry;
use crate::model::workflow::GapStatus;

use super::support::*;
use super::*;

fn derive_gap_name(actual: &str, target: &str) -> Option<String> {
    let source = [target.trim(), actual.trim()]
        .into_iter()
        .find(|value| !value.is_empty())?;
    let collapsed = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name = collapsed.chars().take(80).collect::<String>();
    if collapsed.chars().count() > 80 {
        name = name
            .trim_end_matches(|ch: char| !ch.is_alphanumeric())
            .to_string();
    }
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

fn latest_round_duplicate_match(
    service: &FileWorkItemService,
    actual: &str,
    target: &str,
) -> Result<Option<Value>, RefineError> {
    if actual.is_empty() || target.is_empty() {
        return Ok(None);
    }
    for gap in service.list_gap_summaries()? {
        if gap.gap.round_count == 0 {
            continue;
        }
        let detail = service.show_gap_detail(&gap.gap.id)?;
        let Some(round) = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.last())
        else {
            continue;
        };
        let round_actual = round
            .get("actual")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let round_target = round
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if round_actual == actual && round_target == target {
            return Ok(Some(json!({
                "id": gap.gap.id,
                "name": gap.gap.name,
                "status": gap.gap.status,
                "node_id": gap.gap.node_id,
                "node_display_name": gap.node_display_name,
                "actual": round_actual,
                "target": round_target
            })));
        }
    }
    Ok(None)
}

#[derive(Default)]
struct ImportDuplicateActions {
    moved_to_backlog: usize,
    move_noop: usize,
    updated_original: usize,
}

impl ImportDuplicateActions {
    fn to_json(&self) -> Value {
        json!({
            "moved_to_backlog": self.moved_to_backlog,
            "move_noop": self.move_noop,
            "updated_original": self.updated_original
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_import_result_sanitizes_feature_metadata_and_reads_feature_gaps() {
        let output = json!({
            "feature": {
                "name": "Personal Budget App — Product Spec",
                "description": "created by Plan Mode",
                "gaps": [
                    {
                        "name": "Track spending by category",
                        "actual": "Users cannot categorize transactions.",
                        "target": "Users can assign each transaction to a budget category.",
                        "priority": "medium"
                    },
                    {
                        "name": "Monthly budget overview",
                        "actual": "Users cannot see monthly budget progress.",
                        "target": "Users can compare month-to-date spending against budget limits.",
                        "priority": "high"
                    }
                ]
            }
        })
        .to_string();

        let result = parse_provider_import_result(&output, Some("Product")).unwrap();
        let feature = result.feature_destination.unwrap();
        assert_eq!(feature.name, "Personal Budget App");
        assert_eq!(feature.description, "");
        assert_eq!(result.drafts.len(), 2);
        assert_eq!(result.drafts[0].name, "Track spending by category");
        assert_eq!(
            result.drafts[0].target,
            "Users can assign each transaction to a budget category."
        );
        assert_eq!(result.drafts[0].reporter, "Product");
        assert_eq!(result.drafts[1].priority, "high");
    }

    #[test]
    fn plan_import_prompt_excludes_refine_from_feature_metadata_contract() {
        let prompt = import_extraction_prompt("Personal Budget App\nTrack expenses.", "plan");
        assert!(prompt.contains("feature"));
        assert!(prompt.contains("Gap drafts must be concrete product behavior"));
        assert!(prompt.contains("do not mention Refine"));
        assert!(prompt.contains("Product Spec"));
    }
}

fn persist_import_draft_with_duplicate_decision(
    service: &FileWorkItemService,
    draft: &ImportDraft,
    feature_id: Option<&str>,
    actions: &mut ImportDuplicateActions,
    created_gap_ids: &mut Vec<String>,
) -> Result<Option<String>, RefineError> {
    let decision = draft.duplicate_decision.trim();
    if !decision.is_empty() && decision != "original" {
        if let Some(duplicate) =
            latest_round_duplicate_match(service, draft.actual.trim(), draft.target.trim())?
        {
            let duplicate_id = duplicate
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match decision {
                "duplicate" => return Ok(None),
                "move_original_to_backlog" => {
                    let from = duplicate
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("backlog");
                    if from == "backlog" || duplicate_id.is_empty() {
                        actions.move_noop += 1;
                    } else if service
                        .transition_gap_status(&duplicate_id, GapStatus::Backlog)
                        .is_ok()
                    {
                        actions.moved_to_backlog += 1;
                    } else {
                        actions.move_noop += 1;
                    }
                    return Ok(None);
                }
                "update_original_actual"
                | "update_original_target"
                | "update_original_reporter"
                | "update_original_priority" => {
                    if !duplicate_id.is_empty() {
                        if decision == "update_original_priority" {
                            service.update_gap_metadata_summary(
                                &duplicate_id,
                                None,
                                Some(&draft.priority),
                            )?;
                        } else {
                            let actual = (decision == "update_original_actual")
                                .then_some(draft.actual.as_str());
                            let target = (decision == "update_original_target")
                                .then_some(draft.target.as_str());
                            let reporter = (decision == "update_original_reporter")
                                .then(|| nonempty_import_option(&draft.reporter))
                                .flatten();
                            service.edit_latest_gap_round_summary(
                                &duplicate_id,
                                reporter,
                                actual,
                                target,
                            )?;
                        }
                        actions.updated_original += 1;
                    }
                    return Ok(None);
                }
                other => {
                    return Err(RefineError::InvalidInput(format!(
                        "unknown duplicate_decision: {other}"
                    )));
                }
            }
        }
    }

    let gap = service.create_gap_summary(&draft.name, None)?;
    created_gap_ids.push(gap.gap.id.clone());
    if !draft.actual.trim().is_empty() || !draft.target.trim().is_empty() {
        service.append_gap_round_summary(
            &gap.gap.id,
            nonempty_or_import_value(&draft.reporter, "Imported"),
            &draft.actual,
            &draft.target,
        )?;
    }
    if gap.gap.priority.as_str() != draft.priority {
        service.update_gap_metadata_summary(&gap.gap.id, None, Some(&draft.priority))?;
    }
    if let Some(feature_id) = feature_id {
        service.assign_gap_to_feature(feature_id, &gap.gap.id)?;
    }
    Ok(Some(gap.gap.id))
}

fn nonempty_import_option(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn import_extraction_prompt(text: &str, purpose: &str) -> String {
    let instruction = match purpose {
        "plan" => {
            "Extract one product Feature and its Gaps from this Plan chat transcript. Return only \
             one JSON object shaped like {\"feature\":{\"name\":\"...\",\"description\":\"...\"},\
             \"drafts\":[{\"name\":\"...\",\"actual\":\"...\",\"target\":\"...\",\"reporter\":\"\",\
             \"priority\":\"low\"}]}. The feature name and description must describe the user's \
             product or capability only; do not mention Refine, Plan Mode, Product Spec, drafts, \
             extraction, or how the plan was created. Gap drafts must be concrete product behavior \
             gaps that are relevant to the spec."
        }
        "round" => {
            "Draft Round data from this Gap chat transcript. Return only one actual => target line."
        }
        "standalone_gap" => {
            "Draft one standalone Gap from this Standalone chat transcript. Return only one \
             JSON object with name, actual, target, reporter, and priority, or one actual => target line."
        }
        _ => {
            "Import these notes into Refine Gap drafts. Return only draft data, one draft per line, \
             using actual => target text or JSON objects with name, actual, target, reporter, and priority."
        }
    };
    format!("{instruction}\n\n{text}")
}

#[derive(Clone, Debug)]
struct ImportExtractionResult {
    drafts: Vec<ImportDraft>,
    feature_destination: Option<PlanFeatureDestination>,
}

#[derive(Clone, Debug)]
struct PlanFeatureDestination {
    name: String,
    description: String,
}

fn import_provider_from_settings(durable_root: &std::path::Path, body: &Value) -> String {
    body.get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
        .or_else(|| {
            FileSettingsService::new(durable_root)
                .load()
                .ok()
                .and_then(|settings| {
                    settings
                        .get("agent_cli")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|provider| !provider.is_empty())
                        .map(str::to_string)
                })
        })
        .or_else(|| {
            provider_status_value().ok().and_then(|status| {
                status
                    .get("selected_provider")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|provider| !provider.is_empty())
                    .map(str::to_string)
            })
        })
        .unwrap_or_else(|| "claude".to_string())
}

fn parse_provider_import_result(
    output: &str,
    reporter: Option<&str>,
) -> crate::core::supervisor::errors::RefineResult<ImportExtractionResult> {
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        let feature_destination = plan_feature_destination_from_value(&value);
        let body = match value {
            Value::Array(items) => json!({ "drafts": items, "reporter": reporter.unwrap_or("") }),
            Value::Object(mut object) => {
                normalize_plan_feature_draft_object(&mut object);
                Value::Object(object)
            }
            other => other,
        };
        if let Ok(drafts) = import_drafts_from_value(&body, reporter) {
            return Ok(ImportExtractionResult {
                drafts,
                feature_destination,
            });
        }
    }

    let json_lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>();
    if let Ok(items) = json_lines
        && !items.is_empty()
    {
        let body = json!({ "drafts": items, "reporter": reporter.unwrap_or("") });
        if let Ok(drafts) = import_drafts_from_value(&body, reporter) {
            return Ok(ImportExtractionResult {
                drafts,
                feature_destination: None,
            });
        }
    }

    FileImportService::new(PathBuf::new())
        .parse_text(output, reporter)
        .map(|drafts| ImportExtractionResult {
            drafts,
            feature_destination: None,
        })
}

fn normalize_plan_feature_draft_object(object: &mut Map<String, Value>) {
    if object.get("drafts").is_some() || object.get("items").is_some() {
        return;
    }
    if let Some(gaps) = object.get("gaps").cloned() {
        object.insert("drafts".to_string(), gaps);
        return;
    }
    if let Some(feature) = object.get("feature").and_then(Value::as_object) {
        if let Some(gaps) = feature
            .get("gaps")
            .or_else(|| feature.get("drafts"))
            .or_else(|| feature.get("items"))
            .cloned()
        {
            object.insert("drafts".to_string(), gaps);
        }
    }
}

fn plan_feature_destination_from_value(value: &Value) -> Option<PlanFeatureDestination> {
    let feature = match value {
        Value::Object(object) => object
            .get("feature")
            .and_then(Value::as_object)
            .or(Some(object)),
        _ => None,
    }?;
    let name = sanitize_plan_feature_name(
        feature
            .get("name")
            .or_else(|| feature.get("feature_name"))
            .or_else(|| feature.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    let description = sanitize_plan_feature_description(
        feature
            .get("description")
            .or_else(|| feature.get("summary"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    if name.is_empty() && description.is_empty() {
        return None;
    }
    Some(PlanFeatureDestination { name, description })
}

fn sanitize_plan_feature_name(raw: &str) -> String {
    let mut value = collapse_ws(raw);
    for suffix in [
        " - Product Spec",
        " – Product Spec",
        " — Product Spec",
        ": Product Spec",
        " Product Spec",
    ] {
        if value.to_lowercase().ends_with(&suffix.to_lowercase()) {
            value.truncate(value.len().saturating_sub(suffix.len()));
            value = collapse_ws(&value);
        }
    }
    for prefix in ["Product Spec:", "Plan:", "Feature:", "Project:"] {
        if value.to_lowercase().starts_with(&prefix.to_lowercase()) {
            value = collapse_ws(&value[prefix.len()..]);
        }
    }
    trim_feature_text(value, 80)
}

fn sanitize_plan_feature_description(raw: &str) -> String {
    let value = collapse_ws(raw);
    let lower = value.to_lowercase();
    if lower.is_empty()
        || lower.contains("created by plan")
        || lower.contains("created from plan")
        || lower.contains("plan mode")
        || lower.contains("refine")
        || lower.contains("product spec")
        || lower.contains("draft")
        || lower.contains("extract")
    {
        return String::new();
    }
    trim_feature_text(value, 500)
}

fn collapse_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_feature_text(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.trim().to_string();
    }
    let mut trimmed = value.chars().take(max_chars).collect::<String>();
    trimmed = trimmed
        .trim_end_matches(|ch: char| !ch.is_alphanumeric())
        .trim()
        .to_string();
    trimmed
}

fn feature_detail_response_from_gaps(
    feature: &crate::core::product::project_state::FeatureSummaryProjection,
    gaps: Vec<crate::model::gap::GapIndexProjection>,
) -> Value {
    let mut value = serde_json::to_value(&feature.feature).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("status".to_string(), json!(feature.rollup.status));
        object.insert("gap_count".to_string(), json!(feature.rollup.gap_count));
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
        object.insert("next_gap".to_string(), json!(feature.rollup.next_gap));
        object.insert("gap_ids".to_string(), json!(feature.gap_ids));
        object.insert("gaps".to_string(), json!(gaps));
        object.insert("rollup".to_string(), json!(feature.rollup));
    }
    value
}

fn feature_reorder_order_from_body(
    body: Option<&Value>,
    projection: &crate::core::product::project_state::ProjectionSnapshot,
    feature_id: &str,
    gap_id: &str,
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
    let mut ordered_gap_ids = feature.gap_ids.clone();
    let Some(source_index) = ordered_gap_ids.iter().position(|id| id == gap_id) else {
        return Err(ApiResponse::json(
            404,
            json!({
                "error": {
                    "code": "not_found",
                    "message": format!("Gap {gap_id} was not found in Feature {feature_id}")
                }
            }),
        ));
    };
    if target_id == gap_id {
        return Ok(source_index as i64 + 1);
    }
    ordered_gap_ids.remove(source_index);
    let Some(target_index) = ordered_gap_ids.iter().position(|id| id == target_id) else {
        return Err(ApiResponse::json(
            400,
            json!({
                "error": {
                    "code": "invalid_order",
                    "message": format!("target Gap {target_id} is not assigned to Feature {feature_id}")
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

enum ImportPersistWorkerError {
    Cancelled,
    Failed(RefineError),
}

fn import_job_cancelled(registry: &FileJobRegistry, job_id: &str) -> bool {
    registry
        .status(job_id)
        .map(|job| matches!(job.state, JobState::Cancelled))
        .unwrap_or(false)
}

fn rollback_import_gaps(service: &FileWorkItemService, gap_ids: &[String]) {
    for gap_id in gap_ids.iter().rev() {
        let _ = service.delete_gap_record(gap_id);
    }
}

fn nonempty_or_import_value<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

impl InProcessWebServer {
    fn active_node_id_for_routes(&self) -> String {
        self.current_durable_root()
            .ok()
            .flatten()
            .and_then(|durable_root| {
                FileNodeRegistryService::new(durable_root)
                    .active_node_id()
                    .ok()
            })
            .filter(|node_id| !node_id.trim().is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    fn node_display_names_for_routes(&self) -> BTreeMap<String, String> {
        self.current_durable_root()
            .ok()
            .flatten()
            .and_then(|durable_root| {
                FileNodeRegistryService::new(durable_root)
                    .list_response()
                    .ok()
            })
            .and_then(|value| {
                value
                    .get("nodes")
                    .and_then(|nodes| nodes.as_array())
                    .cloned()
            })
            .into_iter()
            .flatten()
            .filter_map(|node| {
                let id = node.get("id").and_then(|value| value.as_str())?;
                let display_name = node
                    .get("display_name")
                    .and_then(|value| value.as_str())
                    .unwrap_or(id);
                Some((id.to_string(), display_name.to_string()))
            })
            .collect()
    }

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
            "submit-merge" => service.submit_gap_for_merge_summary(gap_id),
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
        let body = request.body.as_ref();
        let actual = body
            .and_then(|body| body.get("actual"))
            .and_then(|actual| actual.as_str())
            .unwrap_or("")
            .trim();
        let target = body
            .and_then(|body| body.get("target"))
            .and_then(|target| target.as_str())
            .unwrap_or("")
            .trim();
        let Some(name) = body
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| derive_gap_name(actual, target))
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name, body.actual, or body.target is required"
                    }
                }),
            );
        };
        let id = body
            .and_then(|body| body.get("id"))
            .and_then(|id| id.as_str());
        let reporter = body
            .and_then(|body| body.get("reporter"))
            .and_then(|reporter| reporter.as_str())
            .unwrap_or("")
            .trim();
        let priority = body
            .and_then(|body| body.get("priority"))
            .and_then(|priority| priority.as_str())
            .unwrap_or("low")
            .trim();
        let feature_id = body
            .and_then(|body| body.get("feature_id"))
            .and_then(|feature_id| feature_id.as_str())
            .map(str::trim)
            .filter(|feature_id| !feature_id.is_empty());
        let duplicate_decision = body
            .and_then(|body| body.get("duplicate_decision"))
            .and_then(|decision| decision.as_str())
            .unwrap_or("")
            .trim();
        if !matches!(priority, "low" | "medium" | "high") {
            return error_response(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }

        let service = self.work_item_service(durable_root);
        let duplicate = if id.is_none() {
            match latest_round_duplicate_match(&service, actual, target) {
                Ok(duplicate) => duplicate,
                Err(error) => return error_response(error),
            }
        } else {
            None
        };
        if let Some(duplicate) = duplicate {
            let duplicate_id = duplicate
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match duplicate_decision {
                "" => {
                    return ApiResponse::json(
                        409,
                        json!({
                            "error": {
                                "code": "duplicate_gap",
                                "message": "Possible duplicate Gap",
                                "duplicate": {
                                    "match": duplicate
                                }
                            }
                        }),
                    );
                }
                "duplicate" => {
                    return ApiResponse::json(
                        200,
                        json!({
                            "created": false,
                            "duplicate_action": "duplicate",
                            "duplicate": {
                                "match": duplicate
                            }
                        }),
                    );
                }
                "move_original_to_backlog" => {
                    let from = duplicate
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("backlog")
                        .to_string();
                    let mut move_result = json!({
                        "moved": false,
                        "from": from,
                        "to": "backlog",
                        "reason": "already_backlog"
                    });
                    if from != "backlog" && !duplicate_id.is_empty() {
                        match service.transition_gap_status(&duplicate_id, GapStatus::Backlog) {
                            Ok(_) => {
                                move_result = json!({
                                    "moved": true,
                                    "from": from,
                                    "to": "backlog"
                                });
                            }
                            Err(_) => {
                                move_result = json!({
                                    "moved": false,
                                    "from": from,
                                    "to": "backlog",
                                    "reason": "protected_status"
                                });
                            }
                        }
                    }
                    return ApiResponse::json(
                        200,
                        json!({
                            "created": false,
                            "duplicate_action": "move_original_to_backlog",
                            "duplicate": {
                                "match": duplicate
                            },
                            "move": move_result
                        }),
                    );
                }
                "original" => {}
                other => {
                    return error_response(RefineError::InvalidInput(format!(
                        "unknown duplicate_decision: {other}"
                    )));
                }
            }
        }
        let mut gap = match service.create_gap_summary(&name, id) {
            Ok(gap) => gap,
            Err(error) => return error_response(error),
        };
        if priority != "low" {
            match service.update_gap_metadata_summary(&gap.gap.id, None, Some(priority)) {
                Ok(updated) => gap = updated,
                Err(error) => return error_response(error),
            }
        }
        if !reporter.is_empty() && !actual.is_empty() && !target.is_empty() {
            match service.append_gap_round_summary(&gap.gap.id, reporter, actual, target) {
                Ok(updated) => gap = updated,
                Err(error) => return error_response(error),
            }
        }
        if let Some(feature_id) = feature_id {
            if let Err(error) = service.assign_gap_to_feature(feature_id, &gap.gap.id) {
                return error_response(error);
            }
            match service.show_gap_summary(&gap.gap.id) {
                Ok(updated) => gap = updated,
                Err(error) => return error_response(error),
            }
        }

        match self.refresh_projection_cache_after_mutation() {
            Ok(()) => ApiResponse::json(201, json!({"gap": gap.gap})),
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
        let notes = match request.body.as_ref().and_then(|body| body.get("notes")) {
            Some(Value::Array(notes)) => Some(notes.clone()),
            Some(_) => {
                return ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_notes",
                            "message": "body.notes must be an array"
                        }
                    }),
                );
            }
            None => None,
        };
        let status = match request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
        {
            Some(status) => match GapStatus::parse_wire(status) {
                Some(status) => Some(status),
                None => {
                    return ApiResponse::json(
                        400,
                        json!({
                            "error": {
                                "code": "invalid_status",
                                "message": "body.status must be a valid Gap status"
                            }
                        }),
                    );
                }
            },
            None => None,
        };
        let service = self.work_item_service(durable_root);
        let mut gap = match status {
            Some(status) => match service.transition_gap_status(gap_id, status) {
                Ok(gap) => gap,
                Err(error) => return error_response(error),
            },
            None => match service.show_gap_summary(gap_id) {
                Ok(gap) => gap,
                Err(error) => return error_response(error),
            },
        };
        if name.is_some() || priority.is_some() {
            match service.update_gap_metadata_summary(gap_id, name, priority) {
                Ok(updated) => gap = updated,
                Err(error) => return error_response(error),
            }
        }
        if let Some(notes) = notes {
            match service.replace_gap_notes_summary(gap_id, &notes) {
                Ok(updated) => gap = updated,
                Err(error) => return error_response(error),
            }
        }
        ApiResponse::json(200, json!({"gap": gap.gap}))
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

    pub(super) fn handle_gap_round_evaluation_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update latest Gap round evaluation");
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/rounds/latest/evaluation"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return gap_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match self
            .work_item_service(durable_root)
            .update_latest_gap_round_evaluation_summary(gap_id, &body)
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
            Ok(()) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": gap_id})),
                Err(error) => error_response(error),
            },
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
        let durable_root = require_durable_root!(self, "read Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let service = FileWorkItemService::new(durable_root);
        match service.show_feature_summary(feature_id) {
            Ok(feature) => {
                let gaps = feature
                    .gap_ids
                    .iter()
                    .filter_map(|gap_id| service.show_gap_summary(gap_id).ok().map(|gap| gap.gap))
                    .collect::<Vec<_>>();
                let feature_detail = feature_detail_response_from_gaps(&feature, gaps);
                ApiResponse::json(
                    200,
                    json!({
                        "feature": feature_detail,
                        "gap_ids": feature.gap_ids,
                        "rollup": feature.rollup
                    }),
                )
            }
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
            Ok(feature) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(
                    200,
                    json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
                ),
                Err(error) => error_response(error),
            },
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
        let order = match self
            .current_projection()
            .map_err(error_response)
            .and_then(|projection| {
                feature_reorder_order_from_body(
                    request.body.as_ref(),
                    &projection,
                    feature_id,
                    gap_id,
                )
            }) {
            Ok(order) => order,
            Err(response) => return response,
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
            Ok(()) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": feature_id})),
                Err(error) => error_response(error),
            },
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
        let durable_root = require_durable_root!(self, "read Gap detail");
        match self.work_item_service(durable_root).show_gap_detail(gap_id) {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap})),
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
        let current_node_id = self.active_node_id_for_routes();
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
            current_node_id: Some(current_node_id),
            feature: query_param(raw_path, "feature"),
            rounds_gte: query_param(raw_path, "rounds_gte")
                .and_then(|value| value.parse::<usize>().ok()),
            rounds_lte: query_param(raw_path, "rounds_lte")
                .and_then(|value| value.parse::<usize>().ok()),
            severity: query_param(raw_path, "severity"),
            category: query_param(raw_path, "category"),
            actor: query_param(raw_path, "actor"),
        };
        let include_facets = query_param(raw_path, "facets").is_some_and(|value| value == "1");
        let mut facet_query = query.clone();
        facet_query.status = None;
        facet_query.page.offset = 0;
        facet_query.page.limit = 0;
        let facet_status_counts =
            include_facets.then(|| projection.list_gaps(facet_query).filtered_status_counts);
        let activity_facets = include_facets.then(|| {
            projection
                .list_activity(ActivityProjectionQuery::default())
                .facets
        });
        let result = projection.list_gaps(query);
        let node_names = self.node_display_names_for_routes();
        let gaps = result
            .gaps
            .into_iter()
            .map(|gap| {
                let node_display_name = gap
                    .node_id
                    .as_deref()
                    .and_then(|node_id| node_names.get(node_id))
                    .cloned();
                let mut value = json!(gap);
                if let Some(display_name) = node_display_name
                    && let Some(object) = value.as_object_mut()
                {
                    object.insert("node_display_name".to_string(), json!(display_name));
                }
                value
            })
            .collect::<Vec<_>>();
        let mut body = json!({
            "gaps": gaps,
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
        });
        if let Some(status_counts) = facet_status_counts {
            body["facets"] = json!({
                "status_counts": status_counts,
                "categories": activity_facets
                    .as_ref()
                    .map(|facets| facets.categories.clone())
                    .unwrap_or_default(),
                "severities": activity_facets
                    .as_ref()
                    .map(|facets| facets.severities.clone())
                    .unwrap_or_default(),
                "actors": activity_facets
                    .as_ref()
                    .map(|facets| facets.actors.clone())
                    .unwrap_or_default()
            });
        }
        ApiResponse::json(200, body)
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
        let current_node_id = self.active_node_id_for_routes();
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
            current_node_id: Some(current_node_id),
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
            .or_else(|| {
                self.source_root().and_then(|source_root| {
                    FileGitWorktreeService::new(source_root)
                        .inspect("")
                        .ok()
                        .and_then(|status| status.branch)
                })
            });
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
        let durable_root = require_durable_root!(self, "undo Git changes");
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("undo Git changes");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("undo Git changes");
        };
        let linked_gap_id = self.current_projection().ok().and_then(|projection| {
            projection
                .changes
                .values()
                .find(|change| change.commit == commit)
                .and_then(|change| change.gap_id.clone())
        });
        match FileGitWorktreeService::with_runtime_root(source_root, runtime_root)
            .revert_commit(commit)
        {
            Ok(result) => {
                let cancelled_gap = if result.ok {
                    match linked_gap_id.as_deref() {
                        Some(gap_id) => match self
                            .work_item_service(durable_root)
                            .cancel_gap_summary(gap_id)
                        {
                            Ok(gap) => Some(gap.gap.id),
                            Err(error) => return error_response(error),
                        },
                        None => None,
                    }
                } else {
                    None
                };
                let _ = self.rebuild_current_projection_cache();
                ApiResponse::json(
                    200,
                    json!({
                        "ok": result.ok,
                        "pushed": false,
                        "commit": commit,
                        "conflicts": result.conflicts,
                        "message": result.message.unwrap_or_default(),
                        "gap_id": linked_gap_id,
                        "cancelled_gap": cancelled_gap
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_cache_rebuild(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("rebuild projection cache");
        };
        let projection = match self.rebuild_current_projection_cache() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let cache_dir = runtime_root.join("cache");
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
        if query == PerformanceQuery::default()
            && let Ok(runtime) = self.current_runtime_projection()
            && let Some(performance) = runtime.performance
        {
            return ApiResponse::json(200, serde_json::Value::Object(performance));
        }
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
            Ok(result) => {
                let _ = performance_report_value(runtime_root, PerformanceQuery::default())
                    .and_then(|value| {
                        if let Some(performance) = value.as_object().cloned() {
                            self.persist_runtime_projection_override(|runtime| {
                                runtime.performance = Some(performance);
                            })?;
                        }
                        Ok(())
                    });
                ApiResponse::json(
                    200,
                    json!({
                    "ok": result.ok,
                    "deleted": result.deleted,
                    "retained": result.retained,
                    "cleared": result.cleared,
                    "retention_days": service.retention_days
                    }),
                )
            }
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

    pub(super) fn handle_terminal_worktrees(&self) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("list terminal worktrees");
        };
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        match terminal_worktrees_response(&source_root, &projection) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_terminal_run(&self, request: ApiRequest) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("run terminal command");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let worktree_path = body
            .get("worktree_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        let command = body.get("command").and_then(Value::as_str).unwrap_or("");
        match terminal_run_response(&source_root, worktree_path, command) {
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
        let durable_root = require_durable_root!(self, "extract imported Gaps");
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("extract imported Gaps in the background");
            };
            let purpose = body
                .get("purpose")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("import");
            let owner = if purpose == "plan" {
                "import:extract:plan"
            } else {
                "import:extract"
            };
            let registry = FileJobRegistry::new(runtime_root);
            let job = match registry.register(owner) {
                Ok(job) => job,
                Err(error) => return error_response(error),
            };
            let _ = registry.update_progress(
                &job.id,
                json!({
                    "message": if purpose == "plan" {
                        "Extracting Plan Feature and Gaps"
                    } else {
                        "Extracting import drafts"
                    },
                    "completed": 0,
                    "total": 1
                }),
            );
            let job = registry.status(&job.id).unwrap_or(job);
            let server = self.clone();
            let job_id = job.id.clone();
            let runtime_root = runtime_root.clone();
            thread::spawn(move || {
                let registry = FileJobRegistry::new(&runtime_root);
                let response = server.import_extract_response(durable_root, body);
                let mut result = response.body.clone();
                match result.as_object_mut() {
                    Some(object) => {
                        object.insert("http_status".to_string(), json!(response.status));
                    }
                    None => {
                        result = json!({
                            "http_status": response.status,
                            "body": result
                        });
                    }
                }
                if response.status >= 400 {
                    let error = result
                        .get("error")
                        .cloned()
                        .unwrap_or_else(|| result.clone());
                    let _ = registry.fail_with_error(&job_id, error);
                    return;
                }
                let draft_count = result
                    .get("drafts")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let _ = registry.update_progress(
                    &job_id,
                    json!({
                        "message": "Plan Feature and Gap drafts extracted",
                        "completed": 1,
                        "total": 1,
                        "draft_count": draft_count
                    }),
                );
                let _ = registry.finish_with_result(&job_id, JobState::Succeeded, result);
            });
            return ApiResponse::json(202, json!({"job": job_response(job)}));
        }
        self.import_extract_response(durable_root, body)
    }

    fn import_extract_response(&self, durable_root: PathBuf, body: Value) -> ApiResponse {
        let text = body_text(&body);
        let purpose = body
            .get("purpose")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("import");
        let provider = import_provider_from_settings(&durable_root, &body);
        let cwd = self.source_root().map(|path| path.display().to_string());
        let output = match HostAgentProviderService::new().invoke(ProviderInvocation {
            provider: provider.clone(),
            prompt: import_extraction_prompt(text, purpose),
            session_id: None,
            cwd,
        }) {
            Ok(output) => output,
            Err(error) => return error_response(error),
        };
        match parse_provider_import_result(
            &output,
            body.get("reporter").and_then(|value| value.as_str()),
        ) {
            Ok(result) => {
                let mut body = json!({
                    "drafts": result.drafts,
                    "provider": provider,
                    "purpose": purpose,
                    "source": "provider"
                });
                if let Some(feature) = result.feature_destination
                    && let Some(object) = body.as_object_mut()
                {
                    object.insert(
                        "feature_destination".to_string(),
                        json!({
                            "mode": "new",
                            "newName": feature.name,
                            "newDescription": feature.description,
                            "existingId": ""
                        }),
                    );
                }
                ApiResponse::json(200, body)
            }
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
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("persist imported Gaps in the background");
            };
            let registry = FileJobRegistry::new(runtime_root);
            let job = match registry.register("import:persist") {
                Ok(job) => job,
                Err(error) => return error_response(error),
            };
            let draft_total = body
                .get("drafts")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let _ = registry.update_progress(
                &job.id,
                json!({
                    "message": "Saving import",
                    "completed": 0,
                    "total": draft_total
                }),
            );
            let job = registry.status(&job.id).unwrap_or(job);
            let server = self.clone();
            let job_id = job.id.clone();
            let runtime_root = runtime_root.clone();
            thread::spawn(move || {
                let registry = FileJobRegistry::new(&runtime_root);
                let response = server.import_persist_background_response(
                    durable_root,
                    body,
                    &registry,
                    &job_id,
                );
                if response.status == 499 {
                    return;
                }
                let mut result = response.body.clone();
                match result.as_object_mut() {
                    Some(object) => {
                        object.insert("http_status".to_string(), json!(response.status));
                    }
                    None => {
                        result = json!({
                            "http_status": response.status,
                            "body": result
                        });
                    }
                }
                let count = result.get("count").and_then(Value::as_u64).unwrap_or(0);
                let _ = registry.update_progress(
                    &job_id,
                    json!({
                        "message": "Import saved",
                        "completed": count,
                        "total": draft_total
                    }),
                );
                let refresh_result = server.refresh_projection_cache_after_mutation();
                let state = if response.status >= 400 {
                    JobState::Failed
                } else {
                    JobState::Succeeded
                };
                if let Err(error) = refresh_result {
                    let _ = registry.fail_with_error(
                        &job_id,
                        json!({
                            "code": "projection_refresh_failed",
                            "message": error.to_string()
                        }),
                    );
                } else {
                    let _ = registry.finish_with_result(&job_id, state, result);
                }
            });
            return ApiResponse::json(202, json!({ "job": job_response(job) }));
        }
        self.import_persist_response(durable_root, body)
    }

    fn import_persist_background_response(
        &self,
        durable_root: PathBuf,
        body: Value,
        registry: &FileJobRegistry,
        job_id: &str,
    ) -> ApiResponse {
        let drafts = match import_drafts_from_value(&body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let draft_total = drafts.len();
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
        let mut created_gap_ids = Vec::new();
        let mut duplicate_actions = ImportDuplicateActions::default();
        if failures.is_empty() {
            match self.persist_import_drafts_incrementally(
                &service,
                drafts,
                feature_id.as_deref(),
                registry,
                job_id,
                &mut created_gap_ids,
                &mut duplicate_actions,
            ) {
                Ok(()) => {}
                Err(ImportPersistWorkerError::Cancelled) => {
                    rollback_import_gaps(&service, &created_gap_ids);
                    if let Err(error) = self.refresh_projection_cache_after_mutation() {
                        let _ = registry.fail_with_error(
                            job_id,
                            json!({
                                "code": "projection_refresh_failed",
                                "message": error.to_string()
                            }),
                        );
                        return error_response(error);
                    }
                    let _ = registry.update_progress(
                        job_id,
                        json!({
                            "message": "Import cancelled",
                            "completed": 0,
                            "total": draft_total
                        }),
                    );
                    return ApiResponse::json(499, json!({"cancelled": true}));
                }
                Err(ImportPersistWorkerError::Failed(error)) => {
                    failures.push(json!({
                        "index": 0,
                        "name": "import",
                        "message": error.to_string()
                    }));
                }
            }
        }
        let created = created_gap_ids
            .iter()
            .filter_map(|gap_id| service.show_gap_summary(gap_id).ok())
            .collect::<Vec<_>>();
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
                "duplicate_actions": duplicate_actions.to_json(),
                "feature": feature_response
            }),
        )
    }

    fn persist_import_drafts_incrementally(
        &self,
        service: &FileWorkItemService,
        drafts: Vec<ImportDraft>,
        feature_id: Option<&str>,
        registry: &FileJobRegistry,
        job_id: &str,
        created_gap_ids: &mut Vec<String>,
        duplicate_actions: &mut ImportDuplicateActions,
    ) -> Result<(), ImportPersistWorkerError> {
        if let Some(feature_id) = feature_id {
            service
                .show_feature_summary(feature_id)
                .map_err(ImportPersistWorkerError::Failed)?;
        }
        let total = drafts.len();
        for draft in drafts {
            if import_job_cancelled(registry, job_id) {
                return Err(ImportPersistWorkerError::Cancelled);
            }
            if let Some(gap_id) = persist_import_draft_with_duplicate_decision(
                service,
                &draft,
                feature_id,
                duplicate_actions,
                created_gap_ids,
            )
            .map_err(ImportPersistWorkerError::Failed)?
            {
                let _ = gap_id;
            }
            let _ = registry.update_progress(
                job_id,
                json!({
                    "message": "Saving import",
                    "completed": created_gap_ids.len(),
                    "total": total
                }),
            );
            thread::sleep(Duration::from_millis(5));
        }
        if import_job_cancelled(registry, job_id) {
            return Err(ImportPersistWorkerError::Cancelled);
        }
        Ok(())
    }

    fn import_persist_response(&self, durable_root: PathBuf, body: Value) -> ApiResponse {
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
            let mut gap_ids = Vec::new();
            let mut duplicate_actions = ImportDuplicateActions::default();
            let result: Result<crate::core::product::imports::ImportPersistResult, RefineError> =
                (|| {
                    if let Some(feature_id) = feature_id.as_deref() {
                        service.show_feature_summary(feature_id)?;
                    }
                    for draft in drafts {
                        if let Some(gap_id) = persist_import_draft_with_duplicate_decision(
                            &service,
                            &draft,
                            feature_id.as_deref(),
                            &mut duplicate_actions,
                            &mut gap_ids,
                        )? {
                            let _ = gap_id;
                        }
                    }
                    Ok(crate::core::product::imports::ImportPersistResult {
                        created: gap_ids.len(),
                        gap_ids,
                        feature_id: feature_id.clone(),
                    })
                })();
            match result {
                Ok(result) => Some((result, duplicate_actions)),
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
            .map(|(result, _)| {
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
                "duplicate_actions": import_result
                    .as_ref()
                    .map(|(_, actions)| actions.to_json())
                    .unwrap_or_else(|| ImportDuplicateActions::default().to_json()),
                "feature": feature_response
            }),
        )
    }
}
