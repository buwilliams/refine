use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::agent_sessions::find_goal_agent_session;
use crate::process::runner::FileRunnerWorkerService;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::RefineError;
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::prompts::{PromptEngine, PromptTemplate};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::deployed_update::active_refine_paths;
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::logs::FileLogService;
use crate::tools::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::tools::product::chat::FileChatService;
use crate::tools::product::goal_exports::FileGoalExportService;
use crate::tools::product::imports::{
    FileImportService, ImportDraft, ImportExtractionResult, import_drafts_from_value,
    import_extraction_prompt, order_feature_dependency_drafts, parse_provider_import_result,
    parse_structured_import_result, validate_import_extraction_result,
};
use crate::tools::product::merging::FileMergerService;
use crate::tools::product::project_state::{
    ActivityProjectionQuery, ChangeProjectionQuery, FeatureProjectionQuery, GoalProjectionQuery,
    PROJECTION_SNAPSHOT_FILE, PageRequest, ProjectionQuery,
};
use crate::tools::product::work_items::{
    BulkFeatureSelection, BulkFeatureUpdate, BulkGoalSelection, FeatureGoalAuthoringRequest,
    FileWorkItemService, GoalAuthoringRequest,
};
use crate::workflow::WorkflowEngine;
use crate::workflow::promotion::BacklogPromotionService;

use super::support::*;
use super::*;

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

pub(super) fn terminal_profile_prompt(
    server: &InProcessWebServer,
    profile: &str,
    goal_id: Option<&str>,
    feature_id: Option<&str>,
    supplemental_prompt: Option<&str>,
) -> Result<String, RefineError> {
    let template = match profile {
        "agent" => PromptTemplate::ChatAgent,
        "plan" => PromptTemplate::ChatPlan,
        "goal" => PromptTemplate::ChatGoal,
        "standalone" => PromptTemplate::ChatStandalone,
        _ => {
            return Err(RefineError::InvalidInput(format!(
                "terminal profile {profile} does not launch an agent"
            )));
        }
    };
    let mut sections = vec![PromptEngine::load(template).trim().to_string()];
    if profile == "agent" {
        let (executable, checkout) = active_refine_paths()?;
        sections.push(format!(
            "Active Refine executable: `{}`. Resolved Refine source checkout: `{}`. If `refine` is absent from PATH, run the checkout-local `./r` from that checkout.",
            executable.display(),
            checkout.display(),
        ));
    }
    let projection = (goal_id.is_some() || feature_id.is_some())
        .then(|| server.current_projection())
        .transpose()?;
    if let Some(goal_id) = goal_id {
        let goal = projection
            .as_ref()
            .expect("projection loaded for attached Goal")
            .goals
            .get(goal_id)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} was not found")))?;
        let context = serde_json::to_string_pretty(&json!({
            "id": goal.goal.id,
            "name": goal.goal.name,
            "status": goal.goal.status,
            "priority": goal.goal.priority,
            "reporter": goal.goal.reporter,
            "assignee": goal.goal.assignee,
            "round_count": goal.goal.round_count,
            "feature_id": goal.goal.feature_id,
            "node_id": goal.goal.node_id,
            "updated": goal.goal.updated,
        }))
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode Goal context: {error}"))
        })?;
        sections.push(format!("Attached Refine Goal context:\n{context}"));
    }
    if let Some(feature_id) = feature_id {
        let feature = projection
            .as_ref()
            .expect("projection loaded for attached Feature")
            .features
            .get(feature_id)
            .ok_or_else(|| RefineError::NotFound(format!("Feature {feature_id} was not found")))?;
        let context = serde_json::to_string_pretty(&json!({
            "id": feature.feature.id,
            "name": feature.feature.name,
            "description": feature.feature.description,
            "status": feature.status,
            "goal_ids": feature.goal_ids,
            "updated": feature.feature.updated,
        }))
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode Feature context: {error}"))
        })?;
        sections.push(format!("Attached Refine Feature context:\n{context}"));
    }
    if profile == "plan" {
        sections.push(
            "Use Refine's CLI when the user asks you to persist the resulting Feature or Goals."
                .to_string(),
        );
    } else if profile == "goal" {
        sections.push(
            "Use Refine's CLI and repository evidence to inspect and advance the attached Goal."
                .to_string(),
        );
    }
    if let Some(prompt) = supplemental_prompt {
        sections.push(format!("User-provided starting context:\n{prompt}"));
    }
    Ok(sections.join("\n\n"))
}

fn create_terminal_standalone_worktree(
    target_root: &Path,
    runtime_root: &Path,
) -> Result<Value, RefineError> {
    let worktree_id = Uuid::new_v4().to_string();
    let branch = format!("refine/standalone/{worktree_id}");
    let git = FileGitWorktreeService::with_runtime_root(target_root, runtime_root);
    let target = git
        .git_path("refine-standalone-worktrees")?
        .join(&worktree_id);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create standalone worktree directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let path = with_repository_git_lock(target_root, || git.ensure_worktree(&branch, &target))?;
    Ok(json!({"branch": branch, "path": path}))
}

fn resume_terminal_standalone_worktree(
    target_root: &Path,
    runtime_root: &Path,
    worktree: &Value,
) -> Result<Value, RefineError> {
    let branch = worktree
        .get("branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("refine/standalone/"))
        .ok_or_else(|| {
            RefineError::InvalidInput(
                "standalone worktree branch must be owned by Refine".to_string(),
            )
        })?;
    let requested = worktree
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            RefineError::InvalidInput("standalone worktree path is required".to_string())
        })?;
    let git = FileGitWorktreeService::with_runtime_root(target_root, runtime_root);
    let allowed_root = git.git_path("refine-standalone-worktrees")?;
    let canonical = requested.canonicalize().map_err(|error| {
        RefineError::NotFound(format!(
            "standalone worktree {} is not available: {error}",
            requested.display()
        ))
    })?;
    let canonical_allowed_root = allowed_root.canonicalize().map_err(|error| {
        RefineError::NotFound(format!(
            "standalone worktree root {} is not available: {error}",
            allowed_root.display()
        ))
    })?;
    if !canonical.starts_with(&canonical_allowed_root) {
        return Err(RefineError::InvalidInput(format!(
            "standalone worktree {} is outside Refine's worktree root",
            canonical.display()
        )));
    }
    let status = git.inspect(canonical.to_str().ok_or_else(|| {
        RefineError::InvalidInput("standalone worktree path is not valid UTF-8".to_string())
    })?)?;
    let inspected_root = PathBuf::from(&status.root)
        .canonicalize()
        .map_err(|error| {
            RefineError::NotFound(format!(
                "standalone worktree root {} is not available: {error}",
                status.root
            ))
        })?;
    if inspected_root != canonical {
        return Err(RefineError::InvalidInput(format!(
            "standalone worktree path {} is not the worktree root",
            canonical.display()
        )));
    }
    if status.branch.as_deref() != Some(branch) {
        return Err(RefineError::InvalidInput(format!(
            "standalone worktree {} is checked out on {}, not {branch}",
            canonical.display(),
            status.branch.as_deref().unwrap_or("a detached HEAD")
        )));
    }
    Ok(json!({"branch": branch, "path": canonical.display().to_string()}))
}

fn cleanup_failed_terminal_worktree(target_root: &Path, worktree: &Value) {
    let Some(path) = worktree.get("path").and_then(Value::as_str) else {
        return;
    };
    let Some(branch) = worktree.get("branch").and_then(Value::as_str) else {
        return;
    };
    let git = FileGitWorktreeService::new(target_root);
    let path = PathBuf::from(path);
    let _ = with_repository_git_lock(target_root, || {
        if path.exists() {
            git.remove_worktree(&path, true)?;
        }
        let _ = git.delete_branch(branch, true);
        Ok(())
    });
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
        && let Some(duplicate) = service.latest_round_duplicate(draft.prompt.trim())?
    {
        let duplicate_id = duplicate.id;
        match decision {
            "duplicate" => return Ok(None),
            "move_original_to_backlog" => {
                if duplicate.status == GoalStatus::Backlog || duplicate_id.is_empty() {
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

fn feature_detail_response_from_goals(
    feature: &crate::tools::product::project_state::FeatureSummaryProjection,
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
}

mod activity_routes;
mod feature_routes;
mod file_terminal_routes;
mod goal_routes;
mod import_routes;
#[cfg(test)]
mod tests;
