use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::RefineError;
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::logs::FileLogService;
use crate::tools::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::tools::product::chat::FileChatService;
use crate::tools::product::imports::{
    FileImportService, ImportDraft, ImportExtractionResult, import_drafts_from_value,
    import_extraction_prompt, order_feature_dependency_drafts, parse_provider_import_result,
    parse_structured_import_result,
};
use crate::tools::product::merging::FileMergerService;
use crate::tools::product::project_state::{
    ActivityProjectionQuery, ChangeProjectionQuery, FeatureProjectionQuery, GoalProjectionQuery,
    PROJECTION_SNAPSHOT_FILE, PageRequest, ProjectionQuery,
};
use crate::tools::product::work_items::{
    BulkFeatureSelection, BulkFeatureUpdate, BulkGoalSelection, FileWorkItemService,
};
use crate::workflow::WorkflowEngine;

use super::support::*;
use super::*;

fn derive_goal_name(prompt: &str) -> Option<String> {
    let source = prompt.trim();
    if source.is_empty() {
        return None;
    }
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
    prompt: &str,
) -> Result<Option<Value>, RefineError> {
    if prompt.is_empty() {
        return Ok(None);
    }
    for goal in service.list_goal_summaries()? {
        if goal.goal.round_count == 0 {
            continue;
        }
        let detail = service.show_goal_detail(&goal.goal.id)?;
        let Some(round) = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| rounds.last())
        else {
            continue;
        };
        let round_prompt = round
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if round_prompt == prompt {
            return Ok(Some(json!({
                "id": goal.goal.id,
                "name": goal.goal.name,
                "status": goal.goal.status,
                "node_id": goal.goal.node_id,
                "node_display_name": goal.node_display_name,
                "prompt": round_prompt
            })));
        }
    }
    Ok(None)
}

fn import_extraction_text(
    refine_dir: &Path,
    runtime_root: Option<&Path>,
    body: &Value,
) -> Result<String, RefineError> {
    let session_id = body
        .get("chat_session_id")
        .or_else(|| body.get("chatSessionId"))
        .or_else(|| body.get("session_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(session_id) = session_id {
        let chat = if let Some(runtime_root) = runtime_root {
            FileChatService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileChatService::new(refine_dir)
        };
        return chat.transcript_text(session_id);
    }

    Ok(body_text(body).to_string())
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

fn persist_import_draft_with_duplicate_decision(
    service: &FileWorkItemService,
    draft: &ImportDraft,
    feature_id: Option<&str>,
    actions: &mut ImportDuplicateActions,
    created_goal_ids: &mut Vec<String>,
    created_drafts: &mut Vec<(ImportDraft, String)>,
) -> Result<Option<String>, RefineError> {
    let decision = draft.duplicate_decision.trim();
    if !decision.is_empty()
        && decision != "original"
        && let Some(duplicate) = latest_round_duplicate_match(service, draft.prompt.trim())?
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
                    .transition_goal_status(&duplicate_id, GoalStatus::Backlog)
                    .is_ok()
                {
                    actions.moved_to_backlog += 1;
                } else {
                    actions.move_noop += 1;
                }
                return Ok(None);
            }
            "update_original_prompt" | "update_original_reporter" | "update_original_priority" => {
                if !duplicate_id.is_empty() {
                    if decision == "update_original_priority" {
                        service.update_goal_metadata_summary(
                            &duplicate_id,
                            None,
                            Some(&draft.priority),
                            None,
                            None,
                        )?;
                    } else {
                        let prompt =
                            (decision == "update_original_prompt").then_some(draft.prompt.as_str());
                        let reporter = (decision == "update_original_reporter")
                            .then(|| nonempty_import_option(&draft.reporter))
                            .flatten();
                        if let Some(reporter) = reporter {
                            service.update_goal_reporter_summary(&duplicate_id, reporter)?;
                        }
                        if prompt.is_some() {
                            service.edit_latest_goal_round_summary(
                                &duplicate_id,
                                None,
                                None,
                                prompt,
                            )?;
                        }
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

    let goal = service.create_goal_summary(&draft.name, None)?;
    created_goal_ids.push(goal.goal.id.clone());
    if !draft.prompt.trim().is_empty() {
        service.append_goal_round_summary_with_assignee(
            &goal.goal.id,
            nonempty_or_import_value(&draft.reporter, "Imported"),
            draft.assignee.as_deref(),
            &draft.prompt,
        )?;
    }
    if goal.goal.priority.as_str() != draft.priority || !draft.reporter.trim().is_empty() {
        service.update_goal_metadata_summary(
            &goal.goal.id,
            None,
            (goal.goal.priority.as_str() != draft.priority).then_some(draft.priority.as_str()),
            nonempty_import_option(&draft.reporter),
            None,
        )?;
    }
    if let Some(feature_id) = feature_id {
        service.assign_goal_to_feature(feature_id, &goal.goal.id)?;
    }
    created_drafts.push((draft.clone(), goal.goal.id.clone()));
    Ok(Some(goal.goal.id))
}

fn nonempty_import_option(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn import_provider_from_settings(
    refine_dir: &std::path::Path,
    active_root: Option<&std::path::Path>,
    body: &Value,
) -> String {
    body.get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let service = match active_root {
                Some(active_root) => FileSettingsService::with_active_root(refine_dir, active_root),
                None => FileSettingsService::new(refine_dir),
            };
            service.load().ok().and_then(|settings| {
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

fn import_extraction_response(
    result: ImportExtractionResult,
    provider: &str,
    purpose: &str,
    source: &str,
) -> ApiResponse {
    let mut body = json!({
        "drafts": result.drafts,
        "provider": provider,
        "purpose": purpose,
        "source": source
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

fn validate_import_extraction_result(
    result: ImportExtractionResult,
    purpose: &str,
) -> Result<ImportExtractionResult, RefineError> {
    if purpose == "plan" && result.drafts.is_empty() {
        return Err(RefineError::InvalidInput(
            "Plan Draft extraction did not return any Goal drafts".to_string(),
        ));
    }
    Ok(result)
}

fn feature_detail_response_from_goals(
    feature: &crate::tools::product::project_state::FeatureSummaryProjection,
    goals: Vec<crate::model::goal::GoalIndexProjection>,
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

fn feature_reorder_order_from_body(
    body: Option<&Value>,
    projection: &crate::tools::product::project_state::ProjectionSnapshot,
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

enum ImportPersistWorkerError {
    Cancelled,
    Failed(RefineError),
}

struct ImportPersistContext<'a> {
    feature_id: Option<&'a str>,
    registry: &'a FileOperationRegistry,
    operation_id: &'a str,
    created_goal_ids: &'a mut Vec<String>,
    duplicate_actions: &'a mut ImportDuplicateActions,
}

fn import_operation_cancelled(registry: &FileOperationRegistry, operation_id: &str) -> bool {
    registry
        .status(operation_id)
        .map(|operation| matches!(operation.state, OperationState::Cancelled))
        .unwrap_or(false)
}

fn rollback_import_goals(service: &FileWorkItemService, goal_ids: &[String]) {
    for goal_id in goal_ids.iter().rev() {
        let _ = service.delete_goal_record(goal_id);
    }
}

fn nonempty_or_import_value<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

impl InProcessWebServer {
    fn active_node_id_for_routes(&self) -> String {
        self.current_refine_dir()
            .ok()
            .flatten()
            .and_then(|refine_dir| self.node_registry_service(refine_dir).active_node_id().ok())
            .filter(|node_id| !node_id.trim().is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    fn node_display_names_for_routes(&self) -> BTreeMap<String, String> {
        self.current_refine_dir()
            .ok()
            .flatten()
            .and_then(|refine_dir| self.node_registry_service(refine_dir).list_response().ok())
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

    pub(super) fn handle_goal_transition(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "mutate work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/transition"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Goal transition route requires a Goal id"
                    }
                }),
            );
        };
        let Some(status) = request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
            .and_then(GoalStatus::parse_wire)
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_status",
                        "message": "body.status must be a valid Goal status"
                    }
                }),
            );
        };

        match self
            .work_item_service(refine_dir)
            .transition_goal_status(goal_id, status)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_action(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "mutate work items");
        let Some((goal_id, action)) = goal_id_and_action(&request.path) else {
            return goal_id_required();
        };
        let service = self.work_item_service(refine_dir);
        let target_root = match self.current_target_root() {
            Ok(Some(target_root)) => target_root,
            Ok(None) => return target_root_unavailable("approve reviewed Goals"),
            Err(error) => return error_response(error),
        };
        let result = match action {
            "start" => service.start_goal_workflow(goal_id),
            "approve" => {
                let Some(runtime_root) = &self.runtime_root else {
                    return runtime_root_unavailable("approve reviewed Goals");
                };
                FileMergerService::with_target_root(runtime_root, &service.refine_dir, &target_root)
                    .approve_reviewed_goal(goal_id)
            }
            "verify" => match service.show_goal_summary(goal_id) {
                Ok(goal) if goal.goal.status == GoalStatus::Review => {
                    let Some(runtime_root) = &self.runtime_root else {
                        return runtime_root_unavailable("approve reviewed Goals");
                    };
                    FileMergerService::with_target_root(
                        runtime_root,
                        &service.refine_dir,
                        &target_root,
                    )
                    .approve_reviewed_goal(goal_id)
                }
                Ok(_) => service.verify_goal_summary(goal_id),
                Err(error) => Err(error),
            },
            "merge" => {
                let Some(runtime_root) = &self.runtime_root else {
                    return runtime_root_unavailable("approve reviewed Goals");
                };
                FileMergerService::with_target_root(runtime_root, &service.refine_dir, &target_root)
                    .approve_reviewed_goal(goal_id)
            }
            "retry-quality" => service.retry_goal_quality_summary(goal_id),
            "retry-merge" => service.retry_goal_merge_summary(goal_id),
            "submit-merge" => service.submit_goal_for_merge_summary(goal_id),
            "undo" => service.undo_goal_summary(goal_id),
            _ => {
                return ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": "unknown Goal action"
                        }
                    }),
                );
            }
        };
        match result {
            Ok(goal) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "message": goal_action_message(action),
                    "goal": goal.goal
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create work items");
        let body = request.body.as_ref();
        let prompt = body
            .and_then(|body| body.get("prompt"))
            .and_then(|prompt| prompt.as_str())
            .unwrap_or("")
            .trim();
        let Some(name) = body
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| derive_goal_name(prompt))
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name or body.prompt is required"
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
        let assignee = body
            .and_then(|body| body.get("assignee"))
            .and_then(|assignee| assignee.as_str())
            .map(str::trim)
            .filter(|assignee| !assignee.is_empty())
            .unwrap_or(reporter);
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

        let service = self.work_item_service(refine_dir);
        let duplicate = if id.is_none() {
            match latest_round_duplicate_match(&service, prompt) {
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
                                "code": "duplicate_goal",
                                "message": "Possible duplicate Goal",
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
                        match service.transition_goal_status(&duplicate_id, GoalStatus::Backlog) {
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
        let mut goal = match service.create_goal_summary(&name, id) {
            Ok(goal) => goal,
            Err(error) => return error_response(error),
        };
        if priority != "low" || !reporter.is_empty() {
            match service.update_goal_metadata_summary(
                &goal.goal.id,
                None,
                (priority != "low").then_some(priority),
                (!reporter.is_empty()).then_some(reporter),
                None,
            ) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        if !reporter.is_empty() && !prompt.is_empty() {
            match service.append_goal_round_summary_with_assignee(
                &goal.goal.id,
                reporter,
                (!assignee.is_empty()).then_some(assignee),
                prompt,
            ) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        if let Some(feature_id) = feature_id {
            if let Err(error) = service.assign_goal_to_feature(feature_id, &goal.goal.id) {
                return error_response(error);
            }
            match service.show_goal_summary(&goal.goal.id) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }

        match self.refresh_projection_cache_after_mutation() {
            Ok(()) => ApiResponse::json(201, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_bulk_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk update work items");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some(update) = parse_bulk_goal_update(body) else {
            return invalid_bulk_body();
        };
        match self
            .work_item_service(refine_dir)
            .bulk_update_goals(selection, update)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_bulk_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk delete work items");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_delete_goals(selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create features");
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
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|assignee| assignee.as_str());
        match self.work_item_service(refine_dir).create_feature_summary(
            name,
            id,
            description,
            reporter,
            assignee,
        ) {
            Ok(feature) => ApiResponse::json(
                201,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match self
            .work_item_service(refine_dir)
            .update_feature_metadata_summary(
                feature_id,
                body.get("name").and_then(|value| value.as_str()),
                body.get("description").and_then(|value| value.as_str()),
                body.get("reporter").and_then(|value| value.as_str()),
                body.get("assignee").and_then(|value| value.as_str()),
            ) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_bulk_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk update features");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkFeatureSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some((field, value)) =
            body.get("update")
                .and_then(Value::as_object)
                .and_then(|update| {
                    if update.len() == 1 {
                        update.iter().next()
                    } else {
                        None
                    }
                })
        else {
            return invalid_bulk_body();
        };
        let Some(value) = value.as_str() else {
            return invalid_bulk_body();
        };
        let update = match field.as_str() {
            "reporter" => BulkFeatureUpdate::Reporter(value.to_string()),
            "assignee" => BulkFeatureUpdate::Assignee(value.to_string()),
            _ => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_update_features(selection, update)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_bulk_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk delete features");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkFeatureSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_delete_features(selection)
        {
            Ok(result) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!(result)),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_node_transfer_features(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer Features to node");
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkFeatureSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some(target_node_id) = body
            .get("target_node_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_node_id",
                        "message": "body.target_node_id is required"
                    }
                }),
            );
        };
        match self
            .work_item_service(refine_dir)
            .bulk_transfer_features_to_node(target_node_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_bulk_assign_goals(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "bulk assign Goals to Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/goals/bulk"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGoalSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match self
            .work_item_service(refine_dir)
            .bulk_assign_goals_to_feature(feature_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
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
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|assignee| assignee.as_str());
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|reporter| reporter.as_str());
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
            Some(status) => match GoalStatus::parse_wire(status) {
                Some(status) => Some(status),
                None => {
                    return ApiResponse::json(
                        400,
                        json!({
                            "error": {
                                "code": "invalid_status",
                                "message": "body.status must be a valid Goal status"
                            }
                        }),
                    );
                }
            },
            None => None,
        };
        let service = self.work_item_service(refine_dir);
        let mut goal = match status {
            Some(status) => match service.transition_goal_status(goal_id, status) {
                Ok(goal) => goal,
                Err(error) => return error_response(error),
            },
            None => match service.show_goal_summary(goal_id) {
                Ok(goal) => goal,
                Err(error) => return error_response(error),
            },
        };
        if name.is_some() || priority.is_some() || reporter.is_some() {
            match service.update_goal_metadata_summary(goal_id, name, priority, reporter, None) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        if let Some(assignee) = assignee {
            match service.update_goal_assignee_summary(goal_id, assignee) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        if let Some(notes) = notes {
            match service.replace_goal_notes_summary(goal_id, &notes) {
                Ok(updated) => goal = updated,
                Err(error) => return error_response(error),
            }
        }
        ApiResponse::json(200, json!({"goal": goal.goal}))
    }

    pub(super) fn handle_goal_note(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "edit work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/notes"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
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
            .work_item_service(refine_dir)
            .add_goal_note_summary(goal_id, author, body)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_round_append(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "append Goal rounds");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/rounds"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
        };
        let Some(reporter) = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let Some(prompt) = request
            .body
            .as_ref()
            .and_then(|body| body.get("prompt"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|value| value.as_str());
        match self
            .work_item_service(refine_dir)
            .append_goal_round_summary_with_assignee(goal_id, reporter, assignee, prompt)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_round_edit_latest(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "edit latest Goal round");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/rounds/latest"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
        };
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str());
        let prompt = request
            .body
            .as_ref()
            .and_then(|body| body.get("prompt"))
            .and_then(|value| value.as_str());
        let assignee = request
            .body
            .as_ref()
            .and_then(|body| body.get("assignee"))
            .and_then(|value| value.as_str());
        match self
            .work_item_service(refine_dir)
            .edit_latest_goal_round_summary(goal_id, reporter, assignee, prompt)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_round_evaluation_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update latest Goal round evaluation");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/rounds/latest/evaluation"))
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return goal_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match self
            .work_item_service(refine_dir)
            .update_latest_goal_round_evaluation_summary(goal_id, &body)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_round_log_append(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "append Goal round logs");
        let Some(rest) = request.path.strip_prefix("/work/goals/") else {
            return goal_id_required();
        };
        let Some((goal_id, round_part)) = rest.split_once("/rounds/") else {
            return goal_id_required();
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
        let goal = match self
            .work_item_service(&refine_dir)
            .show_goal_summary(goal_id)
        {
            Ok(goal) => goal,
            Err(error) => return error_response(error),
        };
        if round_idx >= goal.goal.round_count {
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
            goal_id: Some(goal_id.to_string()),
        };
        match FileLogService::new(refine_dir).append_round_log(goal_id, round_idx, entry) {
            Ok(log) => ApiResponse::json(
                200,
                json!({"log": log, "goal_id": goal_id, "round_idx": round_idx}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_logs(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read Goal round logs");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        let goal = match self
            .work_item_service(&refine_dir)
            .show_goal_summary(goal_id)
        {
            Ok(goal) => goal,
            Err(error) => return error_response(error),
        };
        if goal.goal.round_count == 0 {
            return ApiResponse::json(
                404,
                json!({"error": {"code": "not_found", "message": "Round not found"}}),
            );
        }
        let round_idx = 0;
        match FileLogService::new(refine_dir).page_round_logs(goal_id, round_idx, 50, 0) {
            Ok((logs, has_more, total)) => ApiResponse::json(
                200,
                json!({
                    "goal_id": goal_id,
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

    pub(super) fn handle_goal_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        match self
            .work_item_service(refine_dir)
            .delete_goal_record(goal_id)
        {
            Ok(()) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": goal_id})),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_cancel(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "cancel work items");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
        else {
            return goal_id_required();
        };
        match self
            .work_item_service(refine_dir)
            .cancel_goal_summary(goal_id)
        {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal.goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_show(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let service = FileWorkItemService::new(refine_dir);
        match service.show_feature_summary(feature_id) {
            Ok(feature) => {
                let goals = feature
                    .goal_ids
                    .iter()
                    .filter_map(|goal_id| {
                        service
                            .show_goal_summary(goal_id)
                            .ok()
                            .map(|goal| goal.goal)
                    })
                    .collect::<Vec<_>>();
                let feature_detail = feature_detail_response_from_goals(&feature, goals);
                ApiResponse::json(
                    200,
                    json!({
                        "feature": feature_detail,
                        "goal_ids": feature.goal_ids,
                        "rollup": feature.rollup
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_add_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "assign Goals to Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/goals"))
            .filter(|feature_id| !feature_id.is_empty())
        else {
            return feature_id_required();
        };
        let Some(goal_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("goal_id"))
            .and_then(|goal_id| goal_id.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_goal_id",
                        "message": "body.goal_id is required"
                    }
                }),
            );
        };
        match self
            .work_item_service(refine_dir)
            .assign_goal_to_feature(feature_id, goal_id)
        {
            Ok(feature) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(
                    200,
                    json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
                ),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_add_goal_path(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "assign Goals to Features");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let goal_id = goal_part;
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .assign_goal_to_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_remove_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "remove Goals from Features");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let goal_id = goal_part;
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .remove_goal_from_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_order_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "order Feature Goals");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let Some(goal_id) = goal_part.strip_suffix("/order") else {
            return goal_id_required();
        };
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .order_goal_in_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_unorder_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "unorder Feature Goals");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let Some(goal_id) = goal_part.strip_suffix("/unorder") else {
            return goal_id_required();
        };
        if feature_id.is_empty() || goal_id.is_empty() || goal_id.contains('/') {
            return feature_id_required();
        }
        match self
            .work_item_service(refine_dir)
            .unorder_goal_in_feature(feature_id, goal_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_reorder_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "reorder Feature Goals");
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, goal_part)) = rest.split_once("/goals/") else {
            return feature_id_required();
        };
        let Some(goal_id) = goal_part.strip_suffix("/reorder") else {
            return goal_id_required();
        };
        let order = match self
            .current_projection()
            .map_err(error_response)
            .and_then(|projection| {
                feature_reorder_order_from_body(
                    request.body.as_ref(),
                    &projection,
                    feature_id,
                    goal_id,
                )
            }) {
            Ok(order) => order,
            Err(response) => return response,
        };
        match self
            .work_item_service(refine_dir)
            .reorder_goal_in_feature(feature_id, goal_id, order)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_move(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "move Feature workflow");
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
            .and_then(GoalStatus::parse_wire)
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
            .work_item_service(refine_dir)
            .move_feature_workflow(feature_id, target)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "goal_ids": feature.goal_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_transfer(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer Feature to node");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/transfer"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let Some(target_node_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("target_node_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_node_id",
                        "message": "body.target_node_id is required"
                    }
                }),
            );
        };
        match self
            .work_item_service(refine_dir)
            .transfer_feature_to_node(target_node_id, feature_id)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_cancel(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "cancel Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let goal_ids = match self.current_projection() {
            Ok(projection) => projection
                .features
                .get(feature_id)
                .map(|feature| feature.goal_ids.clone())
                .unwrap_or_default(),
            Err(error) => return error_response(error),
        };
        let runtime_reconciled = match self.reconcile_feature_runtime_work(feature_id, &goal_ids) {
            Ok(summary) => summary,
            Err(error) => return error_response(error),
        };
        match self
            .work_item_service(refine_dir)
            .cancel_feature_summary(feature_id)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup,
                    "runtime_reconciled": {
                        "processes": runtime_reconciled.processes,
                        "operations": runtime_reconciled.operations
                    }
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_feature_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete Features");
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        match self
            .work_item_service(refine_dir)
            .delete_feature_record(feature_id)
        {
            Ok(()) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": feature_id})),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goal_show(&self, request: ApiRequest) -> ApiResponse {
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .filter(|goal_id| !goal_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Goal route requires a Goal id"
                    }
                }),
            );
        };
        let refine_dir = require_refine_dir!(self, "read Goal detail");
        match self.work_item_service(refine_dir).show_goal_detail(goal_id) {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_goals_list(&self, raw_path: &str) -> ApiResponse {
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
        let query = GoalProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "updated".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            status: query_param(raw_path, "status")
                .and_then(|value| GoalStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            assignee: query_param(raw_path, "assignee"),
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
            include_facets.then(|| projection.list_goals(facet_query).filtered_status_counts);
        let activity_facets = include_facets.then(|| {
            projection
                .list_activity(ActivityProjectionQuery::default())
                .facets
        });
        let result = projection.list_goals(query);
        let node_names = self.node_display_names_for_routes();
        let goals = result
            .goals
            .into_iter()
            .map(|goal| {
                let node_display_name = goal
                    .node_id
                    .as_deref()
                    .and_then(|node_id| node_names.get(node_id))
                    .cloned();
                let mut value = json!(goal);
                if let Some(display_name) = node_display_name
                    && let Some(object) = value.as_object_mut()
                {
                    object.insert("node_display_name".to_string(), json!(display_name));
                }
                value
            })
            .collect::<Vec<_>>();
        let mut body = json!({
            "goals": goals,
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
            status: query_param(raw_path, "status")
                .and_then(|value| GoalStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            assignee: query_param(raw_path, "assignee"),
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
                    "goal_ids": feature.goal_ids,
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
        let Some(_) = (match self.current_refine_dir() {
            Ok(refine_dir) => refine_dir,
            Err(error) => return error_response(error),
        }) else {
            return target_root_unavailable("read activity");
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
            goal_id: query_param(raw_path, "goal_id"),
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
        let refine_dir = require_refine_dir!(self, "record UI activity");
        let body = request.body.unwrap_or_else(|| json!({}));
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("UI error")
            .trim();
        let service = FileActivityService::new(refine_dir);
        let mut entry = service.new_entry(
            if message.is_empty() {
                "UI error"
            } else {
                message
            },
            "error",
            "ui",
            body.get("goal_id")
                .and_then(|goal_id| goal_id.as_str())
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
        let refine_dir = require_refine_dir!(self, "clean up activity");
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
        let service = FileActivityService::new(refine_dir);
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
            goal_id: query_param(raw_path, "goal_id"),
            status: query_param(raw_path, "status")
                .and_then(|status| GoalStatus::parse_wire(&status)),
            priority: query_param(raw_path, "priority"),
            branch: query_param(raw_path, "branch"),
        });
        let branch = result
            .changes
            .iter()
            .find_map(|change| change.branch.clone())
            .or_else(|| {
                self.target_root().and_then(|target_root| {
                    FileGitWorktreeService::new(target_root)
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
                    "goal_id": change.goal_id,
                    "name": change.goal_name,
                    "status": change.goal_status,
                    "priority": change.goal_priority,
                    "assignee": change.goal_assignee,
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
        let refine_dir = require_refine_dir!(self, "undo Git changes");
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("undo Git changes");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("undo Git changes");
        };
        let linked_goal_id = self.current_projection().ok().and_then(|projection| {
            projection
                .changes
                .values()
                .find(|change| change.commit == commit)
                .and_then(|change| change.goal_id.clone())
        });
        match with_repository_git_lock(&target_root, || {
            FileGitWorktreeService::with_runtime_root(&target_root, runtime_root)
                .revert_commit(commit)
        }) {
            Ok(result) => {
                let cancelled_goal = if result.ok {
                    match linked_goal_id.as_deref() {
                        Some(goal_id) => match self
                            .work_item_service(refine_dir)
                            .cancel_goal_summary(goal_id)
                        {
                            Ok(goal) => Some(goal.goal.id),
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
                        "goal_id": linked_goal_id,
                        "cancelled_goal": cancelled_goal
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
                "goals": projection.goals.len(),
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
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("read source files");
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
        match files_tree_response(&target_root, &path, recursive, max_depth, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_files_read(&self, raw_path: &str) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("read source file");
        };
        let path = query_param(raw_path, "path").unwrap_or_default();
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param(raw_path, "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(128_000)
            .clamp(1, 512_000);
        match files_read_response(&target_root, &path, offset, limit) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_files_search(&self, raw_path: &str) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("search source files");
        };
        let query = query_param(raw_path, "q").unwrap_or_default();
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20)
            .clamp(1, 200);
        match files_search_response(&target_root, &query, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_terminal_session_start(&self, request: ApiRequest) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("start terminal session");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let cols = body
            .get("cols")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        let rows = body
            .get("rows")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        match terminal_session_start_response(&target_root, cols, rows) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_terminal_input(
        &self,
        request: ApiRequest,
        session_id: &str,
    ) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let data = body.get("data").and_then(Value::as_str).unwrap_or("");
        match terminal_input_response(session_id, data) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_terminal_resize(
        &self,
        request: ApiRequest,
        session_id: &str,
    ) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let cols = body
            .get("cols")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        let rows = body
            .get("rows")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        match terminal_resize_response(session_id, cols, rows) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_terminal_stop(&self, session_id: &str) -> ApiResponse {
        match terminal_stop_response(session_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_terminal_events_snapshot(&self, raw_path: &str) -> ApiResponse {
        let Some(session_id) = raw_path
            .split('?')
            .next()
            .and_then(|path| path.strip_prefix("/api/terminal/"))
            .and_then(|rest| rest.strip_suffix("/events"))
            .or_else(|| {
                raw_path
                    .split('?')
                    .next()
                    .and_then(|path| path.strip_prefix("/terminal/"))
                    .and_then(|rest| rest.strip_suffix("/events"))
            })
        else {
            return error_response(RefineError::InvalidInput(
                "terminal session id is required".to_string(),
            ));
        };
        let after = query_param(raw_path, "after")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        match terminal_events_since(session_id, after) {
            Ok(events) => ApiResponse::json(200, json!({ "events": events })),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_merger_hard_reset_worktree(&self) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("hard-reset Git worktree");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("hard-reset Git worktree");
        };
        match with_repository_git_lock(&target_root, || {
            FileGitWorktreeService::with_runtime_root(&target_root, runtime_root).hard_reset()
        }) {
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
        let refine_dir = require_refine_dir!(self, "extract imported Goals");
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("extract imported Goals in the background");
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
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register(owner) {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": if purpose == "plan" {
                        "Extracting Plan Feature and Goals"
                    } else {
                        "Extracting import drafts"
                    },
                    "completed": 0,
                    "total": 1
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let server = self.clone();
            let operation_id = operation.id.clone();
            let runtime_root = runtime_root.clone();
            let plan_purpose = purpose == "plan";
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = server.import_extract_response(refine_dir, body);
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
                    let _ = registry.fail_with_error(&operation_id, error);
                    return;
                }
                let draft_count = result
                    .get("drafts")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let _ = registry.update_progress(
                    &operation_id,
                    json!({
                        "message": if plan_purpose {
                            "Plan Feature and Goal drafts extracted"
                        } else {
                            "Import drafts extracted"
                        },
                        "completed": 1,
                        "total": 1,
                        "draft_count": draft_count
                    }),
                );
                let _ =
                    registry.finish_with_result(&operation_id, OperationState::Succeeded, result);
            });
            return ApiResponse::json(202, json!({"operation": operation_response(operation)}));
        }
        self.import_extract_response(refine_dir, body)
    }

    fn import_extract_response(&self, refine_dir: PathBuf, body: Value) -> ApiResponse {
        let text = match import_extraction_text(&refine_dir, self.runtime_root.as_deref(), &body) {
            Ok(text) => text,
            Err(error) => return error_response(error),
        };
        let purpose = body
            .get("purpose")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("import");
        let reporter = body.get("reporter").and_then(|value| value.as_str());
        let provider =
            import_provider_from_settings(&refine_dir, self.runtime_root.as_deref(), &body);
        let force_provider = body
            .get("force_provider")
            .or_else(|| body.get("forceProvider"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if purpose == "plan"
            && !force_provider
            && let Some(result) = parse_structured_import_result(&text, reporter)
        {
            return match validate_import_extraction_result(result, purpose) {
                Ok(result) => import_extraction_response(result, &provider, purpose, "input"),
                Err(error) => error_response(error),
            };
        }
        let cwd = self.target_root().map(|path| path.display().to_string());
        let output = match HostAgentProviderService::new().invoke(ProviderInvocation {
            provider: provider.clone(),
            prompt: import_extraction_prompt(&text, purpose),
            session_id: None,
            cwd,
            process_metadata: Default::default(),
        }) {
            Ok(output) => output,
            Err(error) => return error_response(error),
        };
        match parse_provider_import_result(&output, reporter) {
            Ok(result) => match validate_import_extraction_result(result, purpose) {
                Ok(result) => import_extraction_response(result, &provider, purpose, "provider"),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_import_csv_parse(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("parse CSV import in the background");
            };
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register("import:csv:parse") {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let row_total = body_text(&body)
                .lines()
                .skip(1)
                .filter(|line| !line.trim().is_empty())
                .count();
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": "Preparing CSV import",
                    "completed": 0,
                    "total": row_total
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let runtime_root = runtime_root.clone();
            let body_for_worker = body.clone();
            let operation_id = operation.id.clone();
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = match FileImportService::new(PathBuf::new()).parse_csv(
                    body_text(&body_for_worker),
                    body_for_worker
                        .get("reporter")
                        .and_then(|value| value.as_str()),
                ) {
                    Ok(drafts) => ApiResponse::json(200, json!({"drafts": drafts})),
                    Err(error) => error_response(error),
                };
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
                    let _ = registry.fail_with_error(&operation_id, error);
                    return;
                }
                let draft_count = result
                    .get("drafts")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let _ = registry.update_progress(
                    &operation_id,
                    json!({
                        "message": "CSV import prepared",
                        "completed": draft_count,
                        "total": row_total,
                        "draft_count": draft_count
                    }),
                );
                let _ =
                    registry.finish_with_result(&operation_id, OperationState::Succeeded, result);
            });
            return ApiResponse::json(202, json!({"operation": operation_response(operation)}));
        }
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
            let mut needles = vec![normalized_dedup_text(&[
                draft.name.as_str(),
                draft.prompt.as_str(),
            ])];
            needles.retain(|needle| !needle.is_empty());
            if needles.is_empty() {
                continue;
            }
            if let Some(existing) = projection.goals.values().find(|goal| {
                let haystack = normalized_dedup_text(&[
                    goal.goal.name.as_str(),
                    goal.searchable_text.as_str(),
                    goal.goal.id.as_str(),
                ]);
                needles.iter().any(|needle| {
                    haystack == *needle || (!haystack.is_empty() && haystack.contains(needle))
                })
            }) {
                matches.push(json!({
                    "index": index + 1,
                    "score": 1.0,
                    "match": {
                        "id": existing.goal.id,
                        "name": existing.goal.name,
                        "status": existing.goal.status,
                        "priority": existing.goal.priority,
                        "reporter": existing.goal.reporter
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
        let refine_dir = require_refine_dir!(self, "persist imported Goals");
        let body = request.body.unwrap_or_else(|| json!({}));
        if body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let Some(runtime_root) = &self.runtime_root else {
                return runtime_root_unavailable("persist imported Goals in the background");
            };
            let registry = FileOperationRegistry::new(runtime_root);
            let operation = match registry.register("import:persist") {
                Ok(operation) => operation,
                Err(error) => return error_response(error),
            };
            let draft_total = body
                .get("drafts")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let _ = registry.update_progress(
                &operation.id,
                json!({
                    "message": "Saving import",
                    "completed": 0,
                    "total": draft_total
                }),
            );
            let operation = registry.status(&operation.id).unwrap_or(operation);
            let server = self.clone();
            let operation_id = operation.id.clone();
            let runtime_root = runtime_root.clone();
            thread::spawn(move || {
                let registry = FileOperationRegistry::new(&runtime_root);
                let response = server.import_persist_background_response(
                    refine_dir,
                    body,
                    &registry,
                    &operation_id,
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
                    &operation_id,
                    json!({
                        "message": "Import saved",
                        "completed": count,
                        "total": draft_total
                    }),
                );
                let refresh_result = server.refresh_projection_cache_after_mutation();
                let state = if response.status >= 400 {
                    OperationState::Failed
                } else {
                    OperationState::Succeeded
                };
                if let Err(error) = refresh_result {
                    let _ = registry.fail_with_error(
                        &operation_id,
                        json!({
                            "code": "projection_refresh_failed",
                            "message": error.to_string()
                        }),
                    );
                } else {
                    let _ = registry.finish_with_result(&operation_id, state, result);
                }
            });
            return ApiResponse::json(202, json!({ "operation": operation_response(operation) }));
        }
        self.import_persist_response(refine_dir, body)
    }

    fn import_persist_background_response(
        &self,
        refine_dir: PathBuf,
        body: Value,
        registry: &FileOperationRegistry,
        operation_id: &str,
    ) -> ApiResponse {
        let drafts = match import_drafts_from_value(&body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let draft_total = drafts.len();
        let service = self.work_item_service(&refine_dir);
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
        let mut created_goal_ids = Vec::new();
        let mut duplicate_actions = ImportDuplicateActions::default();
        if failures.is_empty() {
            let mut context = ImportPersistContext {
                feature_id: feature_id.as_deref(),
                registry,
                operation_id,
                created_goal_ids: &mut created_goal_ids,
                duplicate_actions: &mut duplicate_actions,
            };
            match self.persist_import_drafts_incrementally(&service, drafts, &mut context) {
                Ok(()) => {}
                Err(ImportPersistWorkerError::Cancelled) => {
                    rollback_import_goals(&service, &created_goal_ids);
                    if let Err(error) = self.refresh_projection_cache_after_mutation() {
                        let _ = registry.fail_with_error(
                            operation_id,
                            json!({
                                "code": "projection_refresh_failed",
                                "message": error.to_string()
                            }),
                        );
                        return error_response(error);
                    }
                    let _ = registry.update_progress(
                        operation_id,
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
        let mut promoted = 0;
        if failures.is_empty() {
            match self.promote_backlog_after_import() {
                Ok(count) => promoted = count,
                Err(error) => failures.push(json!({
                    "index": 0,
                    "name": "workflow",
                    "message": error.to_string()
                })),
            }
        }
        let created = created_goal_ids
            .iter()
            .filter_map(|goal_id| service.show_goal_summary(goal_id).ok())
            .collect::<Vec<_>>();
        if let Some(feature_id) = feature_id.as_deref()
            && let Ok(feature) = service.show_feature_summary(feature_id)
        {
            feature_response = feature_import_response(&feature);
        }

        ApiResponse::json(
            if failures.is_empty() { 201 } else { 207 },
            json!({
                "ok": failures.is_empty(),
                "count": created.len(),
                "created": created,
                "goals": created.iter().map(|goal| &goal.goal).collect::<Vec<_>>(),
                "promoted": promoted,
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
        context: &mut ImportPersistContext<'_>,
    ) -> Result<(), ImportPersistWorkerError> {
        if let Some(feature_id) = context.feature_id {
            service
                .show_feature_summary(feature_id)
                .map_err(ImportPersistWorkerError::Failed)?;
        }
        let total = drafts.len();
        let mut created_drafts = Vec::new();
        for draft in drafts {
            if import_operation_cancelled(context.registry, context.operation_id) {
                return Err(ImportPersistWorkerError::Cancelled);
            }
            if let Some(goal_id) = persist_import_draft_with_duplicate_decision(
                service,
                &draft,
                context.feature_id,
                context.duplicate_actions,
                context.created_goal_ids,
                &mut created_drafts,
            )
            .map_err(ImportPersistWorkerError::Failed)?
            {
                let _ = goal_id;
            }
            let _ = context.registry.update_progress(
                context.operation_id,
                json!({
                    "message": "Saving import",
                    "completed": context.created_goal_ids.len(),
                    "total": total
                }),
            );
            thread::sleep(Duration::from_millis(5));
        }
        if import_operation_cancelled(context.registry, context.operation_id) {
            return Err(ImportPersistWorkerError::Cancelled);
        }
        if let Some(feature_id) = context.feature_id {
            order_feature_dependency_drafts(service, feature_id, &created_drafts)
                .map_err(ImportPersistWorkerError::Failed)?;
        }
        Ok(())
    }

    fn promote_backlog_after_import(&self) -> Result<usize, RefineError> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(0);
        };
        let Some(target_root) = self.target_root() else {
            return Ok(0);
        };
        WorkflowEngine::with_target_root(runtime_root, target_root).promote_backlog_to_todo()
    }

    fn import_persist_response(&self, refine_dir: PathBuf, body: Value) -> ApiResponse {
        let drafts = match import_drafts_from_value(&body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let service = self.work_item_service(&refine_dir);
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
            let mut goal_ids = Vec::new();
            let mut created_drafts = Vec::new();
            let mut duplicate_actions = ImportDuplicateActions::default();
            let result: Result<crate::tools::product::imports::ImportPersistResult, RefineError> =
                (|| {
                    if let Some(feature_id) = feature_id.as_deref() {
                        service.show_feature_summary(feature_id)?;
                    }
                    for draft in drafts {
                        if let Some(goal_id) = persist_import_draft_with_duplicate_decision(
                            &service,
                            &draft,
                            feature_id.as_deref(),
                            &mut duplicate_actions,
                            &mut goal_ids,
                            &mut created_drafts,
                        )? {
                            let _ = goal_id;
                        }
                    }
                    if let Some(feature_id) = feature_id.as_deref() {
                        order_feature_dependency_drafts(&service, feature_id, &created_drafts)?;
                    }
                    Ok(crate::tools::product::imports::ImportPersistResult {
                        created: goal_ids.len(),
                        goal_ids,
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
        let mut promoted = 0;
        if failures.is_empty() {
            match self.promote_backlog_after_import() {
                Ok(count) => promoted = count,
                Err(error) => failures.push(json!({
                    "index": 0,
                    "name": "workflow",
                    "message": error.to_string()
                })),
            }
        }
        let created = import_result
            .as_ref()
            .map(|(result, _)| {
                result
                    .goal_ids
                    .iter()
                    .filter_map(|goal_id| service.show_goal_summary(goal_id).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(feature_id) = feature_id.as_deref()
            && let Ok(feature) = service.show_feature_summary(feature_id)
        {
            feature_response = feature_import_response(&feature);
        }

        ApiResponse::json(
            if failures.is_empty() { 201 } else { 207 },
            json!({
                "ok": failures.is_empty(),
                "count": created.len(),
                "created": created,
                "goals": created.iter().map(|goal| &goal.goal).collect::<Vec<_>>(),
                "promoted": promoted,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_import_result_sanitizes_feature_metadata_and_reads_feature_goals() {
        let output = json!({
            "feature": {
                "name": "Personal Budget App — Product Spec",
                "description": "created by Plan Mode",
                "goals": [
                    {
                        "name": "Track spending by category",
                        "prompt": "Let users assign each transaction to a budget category.",
                        "priority": "medium"
                    },
                    {
                        "name": "Monthly budget overview",
                        "prompt": "Let users compare month-to-date spending against budget limits.",
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
            result.drafts[0].prompt,
            "Let users assign each transaction to a budget category."
        );
        assert_eq!(result.drafts[0].reporter, "Product");
        assert_eq!(result.drafts[1].priority, "high");
    }

    #[test]
    fn plan_import_result_reads_embedded_pretty_json_before_text_fallback() {
        let output = r#"Provider notes before JSON:
{
  "feature": {
    "name": "Smoke AI Plan Feature",
    "description": "A deterministic product capability planned by the Smoke AI fixture.",
    "goals": [
      {
        "name": "Smoke AI plan goal one",
        "prompt": "smoke-ai plan prompt one",
        "priority": "low"
      }
    ]
  }
}
Provider notes after JSON."#;

        let result = parse_provider_import_result(output, Some("Product")).unwrap();
        let feature = result.feature_destination.unwrap();
        assert_eq!(feature.name, "Smoke AI Plan Feature");
        assert_eq!(result.drafts.len(), 1);
        assert_eq!(result.drafts[0].prompt, "smoke-ai plan prompt one");
    }

    #[test]
    fn plan_import_result_merges_feature_behavior_and_implementation_goal_arrays() {
        let output = json!({
            "feature": {
                "name": "Budget Alerts",
                "description": "Alert users when spending nears limits.",
                "goals": [
                    {
                        "name": "Budget threshold alert",
                        "prompt": "Alert users before a category exceeds its monthly budget.",
                        "priority": "high"
                    }
                ],
                "implementation_goals": [
                    {
                        "name": "Persist alert preferences",
                        "prompt": "Add a refine model that persists threshold preferences and exposes them through the budget settings API.",
                        "priority": "medium"
                    }
                ],
                "technical_goals": [
                    {
                        "name": "Verify alert trigger coverage",
                        "prompt": "Add automated tests for below-threshold, threshold-crossing, and disabled-alert cases.",
                        "priority": "medium"
                    }
                ]
            }
        })
        .to_string();

        let result = parse_provider_import_result(&output, Some("Product")).unwrap();
        assert_eq!(result.drafts.len(), 3);
        assert_eq!(result.drafts[0].name, "Budget threshold alert");
        assert_eq!(result.drafts[1].name, "Persist alert preferences");
        assert_eq!(result.drafts[2].name, "Verify alert trigger coverage");
        assert!(result.drafts[1].prompt.contains("refine model"));
        assert!(result.drafts[2].prompt.contains("automated tests"));
    }

    #[test]
    fn plan_import_prompt_excludes_refine_from_feature_metadata_contract() {
        let prompt = import_extraction_prompt("Personal Budget App\nTrack expenses.", "plan");
        assert!(prompt.contains("feature"));
        assert!(prompt.contains("implementation_goals"));
        assert!(prompt.contains("Draft every concrete implementation goal"));
        assert!(prompt.contains("architecture"));
        assert!(prompt.contains("durable state"));
        assert!(prompt.contains("logic and code organization"));
        assert!(prompt.contains("do not force"));
        assert!(prompt.contains("natural build order"));
        assert!(prompt.contains("verification"));
        assert!(prompt.contains("do not mention Refine"));
        assert!(prompt.contains("Product Spec"));
    }

    #[test]
    fn feature_spec_import_prompt_uses_architecture_lenses() {
        let prompt = import_extraction_prompt("Build a budget app.", "feature import");
        assert!(prompt.contains("Plan or feature-spec source"));
        assert!(prompt.contains("architecture"));
        assert!(prompt.contains("do not force"));
        assert!(prompt.contains("natural build order"));
    }
}
