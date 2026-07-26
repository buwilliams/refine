use std::path::Path;

use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::RefineError;
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::product::chat::FileChatService;
use crate::tools::product::imports::{ImportDraft, ImportExtractionResult};
use crate::tools::product::work_items::FileWorkItemService;

use super::{ApiResponse, body_text, provider_status_value};

pub(super) fn import_extraction_text(
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
pub(super) struct ImportDuplicateActions {
    moved_to_backlog: usize,
    move_noop: usize,
    updated_original: usize,
}

impl ImportDuplicateActions {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "moved_to_backlog": self.moved_to_backlog,
            "move_noop": self.move_noop,
            "updated_original": self.updated_original
        })
    }
}

pub(super) fn persist_import_draft_with_duplicate_decision(
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

pub(super) fn import_provider_from_settings(
    refine_dir: &Path,
    active_root: Option<&Path>,
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

pub(super) fn import_extraction_response(
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

pub(super) enum ImportPersistWorkerError {
    Cancelled,
    Failed(RefineError),
}

pub(super) struct ImportPersistContext<'a> {
    pub(super) feature_id: Option<&'a str>,
    pub(super) registry: &'a FileOperationRegistry,
    pub(super) operation_id: &'a str,
    pub(super) created_goal_ids: &'a mut Vec<String>,
    pub(super) duplicate_actions: &'a mut ImportDuplicateActions,
}

pub(super) fn import_operation_cancelled(
    registry: &FileOperationRegistry,
    operation_id: &str,
) -> bool {
    registry
        .status(operation_id)
        .map(|operation| matches!(operation.state, OperationState::Cancelled))
        .unwrap_or(false)
}

pub(super) fn rollback_import_goals(service: &FileWorkItemService, goal_ids: &[String]) {
    for goal_id in goal_ids.iter().rev() {
        let _ = service.delete_goal_record(goal_id);
    }
}

fn nonempty_or_import_value<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}
