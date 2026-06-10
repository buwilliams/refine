use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::core::host::process_supervision::FileProcessSupervisor;
use crate::core::host::quality::{
    FileQualityService, QualityCheckRequest, QualityJobRunner, QualityService, QualitySettingsPatch,
};
use crate::core::host::target_apps::{FileTargetAppService, TargetAppSnapshot};
use crate::core::product::chat::{ChatAttachment, ChatService, ChatSessionWorktree};
use crate::core::product::work_items::FileWorkItemService;
use crate::core::supervisor::config::{ConfigService, FileSettingsService};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::workflow::GapStatus;

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn quality_timing_setting(&self) -> String {
        self.durable_root
            .as_ref()
            .and_then(|root| FileSettingsService::new(root).load().ok())
            .and_then(|settings| {
                settings
                    .get("quality_timing")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "pre_merge".to_string())
    }

    pub(super) fn with_runtime_settings(&self, mut value: Value) -> Value {
        if let Some(runtime) = self.runtime_settings_value()
            && let Some(object) = value.as_object_mut()
        {
            object.insert("runtime".to_string(), runtime);
        }
        value
    }

    pub(super) fn runtime_settings_value(&self) -> Option<Value> {
        let runtime_root = self.runtime_root.as_ref()?;
        let pause_state = FileProcessSupervisor::new(runtime_root)
            .pause_state()
            .ok()?;
        Some(json!({
            "paused": pause_state.agents_paused || pause_state.background_processes_stopped,
            "agents_paused": pause_state.agents_paused,
            "background_processes_stopped": pause_state.background_processes_stopped,
            "runtime_root": runtime_root.display().to_string()
        }))
    }

    pub(super) fn target_app_service(&self) -> RefineResult<FileTargetAppService> {
        let Some(durable_root) = self.current_durable_root()? else {
            return Err(RefineError::Degraded(
                "durable root is required for target-app operations".to_string(),
            ));
        };
        let Some(runtime_root) = &self.runtime_root else {
            return Err(RefineError::Degraded(
                "runtime root is required for target-app operations".to_string(),
            ));
        };
        let source_root = durable_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(FileTargetAppService::new(
            durable_root,
            runtime_root,
            source_root,
        ))
    }

    pub(super) fn target_app_response(&self, snapshot: TargetAppSnapshot) -> serde_json::Value {
        let settings = self
            .current_durable_root()
            .ok()
            .flatten()
            .and_then(|root| FileSettingsService::new(root).load().ok())
            .unwrap_or_default();
        let get = |key: &str| {
            settings
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string()
        };
        json!({
            "ok": snapshot.ok,
            "state": snapshot.state,
            "health_url": first_non_empty(&get("target_app_http_check_url"), &get("target_app_health_url")),
            "app_url": get("target_app_url"),
            "has_start_command": !get("target_app_start_command").trim().is_empty(),
            "has_stop_command": !get("target_app_stop_command").trim().is_empty(),
            "has_rebuild_command": !get("target_app_rebuild_command").trim().is_empty(),
            "has_status_checks": !get("target_app_status_command").trim().is_empty()
                || !get("target_app_http_check_url").trim().is_empty()
                || !get("target_app_tcp_check_host").trim().is_empty()
                || !get("target_app_process_check_command").trim().is_empty(),
            "has_start_instructions": !get("target_app_start_command").trim().is_empty()
                || !get("target_app_start_instructions").trim().is_empty(),
            "has_stop_instructions": !get("target_app_stop_command").trim().is_empty()
                || !get("target_app_stop_instructions").trim().is_empty(),
            "last_check_at": snapshot.last_check_at,
            "last_check_ok": snapshot.last_check_ok,
            "last_check_message": snapshot.last_check_message,
            "last_health_at": snapshot.last_health_at,
            "last_health_ok": snapshot.last_health_ok,
            "last_health_message": snapshot.last_health_message,
            "last_error": snapshot.last_error,
            "last_operation_id": snapshot.last_operation_id,
            "last_operation": snapshot.last_operation,
            "process_id": snapshot.process_id,
            "pid": snapshot.pid,
            "auto_rebuild": get("target_app_auto_rebuild"),
            "auto_rebuild_hour_utc": get("target_app_auto_rebuild_hour_utc"),
            "auto_rebuild_last_started_at": "",
            "auto_rebuild_last_finished_at": "",
            "auto_rebuild_last_ok": false,
            "auto_rebuild_last_message": "",
            "legacy_config_present": !get("target_app_start_instructions").trim().is_empty()
                || !get("target_app_stop_instructions").trim().is_empty()
                || !get("target_app_health_url").trim().is_empty(),
            "message": snapshot.message
        })
    }

    pub(super) fn handle_quality_get(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "load quality settings");
        match FileQualityService::new(durable_root).load_settings() {
            Ok(settings) => ApiResponse::json(200, json!(settings)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_save(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "save quality settings");
        let patch = match serde_json::from_value::<QualitySettingsPatch>(
            request.body.unwrap_or_else(|| json!({})),
        ) {
            Ok(patch) => patch,
            Err(error) => {
                return ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_input",
                            "message": format!("invalid quality settings body: {error}")
                        }
                    }),
                );
            }
        };
        match FileQualityService::new(&durable_root).save_settings(patch) {
            Ok(settings) => {
                append_quality_activity(&durable_root, "Quality settings updated".to_string());
                ApiResponse::json(200, json!(settings))
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_checks(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "run quality checks");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("run quality checks");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let owner_id = body
            .get("owner_id")
            .or_else(|| body.get("gap_id"))
            .or_else(|| body.get("feature_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("app")
            .to_string();
        let command = body
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let browser_required = body
            .get("browser_required")
            .or_else(|| body.get("browser"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        match QualityJobRunner::new(&durable_root, runtime_root).run_checks(QualityCheckRequest {
            owner_id,
            command,
            browser_required,
        }) {
            Ok(job_result) => {
                append_quality_activity(
                    &durable_root,
                    format!(
                        "Quality checks completed for {}",
                        job_result.result.owner_id
                    ),
                );
                ApiResponse::json(
                    200,
                    json!({
                        "ok": job_result.result.ok,
                        "result": job_result.result,
                        "job": job_response(job_result.job)
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_screenshots(&self, raw_path: &str) -> ApiResponse {
        let durable_root = require_durable_root!(self, "list quality screenshots");
        let owner_id = query_param(raw_path, "owner_id").unwrap_or_else(|| "app".to_string());
        match FileQualityService::new(durable_root).screenshots(&owner_id) {
            Ok(screenshots) => {
                let screenshot_count = screenshots.len();
                ApiResponse::json(
                    200,
                    json!({
                        "ok": true,
                        "owner_id": owner_id,
                        "screenshots": screenshots,
                        "screenshot_count": screenshot_count
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_regression_create(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "create quality regressions");
        let body = request.body.unwrap_or_else(|| json!({}));
        let title = body
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let prompt = body
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let description = body
            .get("description")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileQualityService::new(&durable_root).create_regression(title, description, prompt) {
            Ok(regression) => {
                append_quality_activity(
                    &durable_root,
                    format!("Regression created: {}", regression.title),
                );
                ApiResponse::json(201, json!({"ok": true, "regression": regression}))
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_regression_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update quality regressions");
        let Some(regression_id) = request
            .path
            .strip_prefix("/quality/regressions/")
            .filter(|regression_id| !regression_id.is_empty() && !regression_id.contains('/'))
        else {
            return regression_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileQualityService::new(durable_root).update_regression(regression_id, &body) {
            Ok(regression) => ApiResponse::json(200, json!({"ok": true, "regression": regression})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_regression_delete(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "delete quality regressions");
        let Some(regression_id) = request
            .path
            .strip_prefix("/quality/regressions/")
            .filter(|regression_id| !regression_id.is_empty() && !regression_id.contains('/'))
        else {
            return regression_id_required();
        };
        match FileQualityService::new(durable_root).delete_regression(regression_id) {
            Ok(()) => ApiResponse::json(200, json!({"ok": true})),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_regression_run(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "run quality regressions");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("run quality regressions");
        };
        match FileQualityService::with_runtime_root(durable_root, runtime_root)
            .run_regressions(true)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_start(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "start chat sessions");
        let body = request.body.unwrap_or_else(|| json!({}));
        let attachment = if let Some(gap_id) = body.get("gap_id").and_then(|value| value.as_str()) {
            ChatAttachment::Gap(gap_id.to_string())
        } else if let Some(feature_id) = body.get("feature_id").and_then(|value| value.as_str()) {
            ChatAttachment::Feature(feature_id.to_string())
        } else {
            ChatAttachment::Standalone
        };
        let mode = body
            .get("purpose")
            .or_else(|| body.get("mode"))
            .and_then(|value| value.as_str());
        let configured_provider = || {
            FileSettingsService::new(&durable_root)
                .load()
                .ok()
                .and_then(|settings| {
                    settings
                        .get("agent_cli")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
        };
        let requested_provider = body
            .get("provider")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let provider = requested_provider
            .clone()
            .or_else(|| effective_chat_provider(configured_provider()));
        let service = self.chat_service(&durable_root);
        match service.start_with_options(attachment.clone(), provider.as_deref(), mode) {
            Ok(mut session) => {
                if matches!(attachment, ChatAttachment::Standalone) {
                    let Some(runtime_root) = &self.runtime_root else {
                        let _ = service.interrupt(&session.id, "standalone worktree setup failed");
                        return runtime_root_unavailable("start standalone chat sessions");
                    };
                    match create_standalone_chat_worktree(&durable_root, &runtime_root, &session.id)
                        .and_then(|worktree| service.attach_worktree(&session.id, worktree))
                    {
                        Ok(updated) => session = updated,
                        Err(error) => {
                            let _ =
                                service.interrupt(&session.id, "standalone worktree setup failed");
                            return error_response(error);
                        }
                    }
                }
                ApiResponse::json(
                    201,
                    json!({
                        "ok": true,
                        "session_id": session.id,
                        "provider": session.provider,
                        "mode": session.mode,
                        "worktree": session.worktree
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_input(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "send chat input");
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/input"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let Some(text) = request
            .body
            .as_ref()
            .and_then(|body| body.get("text"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "bad_request",
                        "message": "text is required"
                    }
                }),
            );
        };
        let service = self.chat_service(&durable_root);
        match service.append_user_message(session_id, text) {
            Ok(session) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "queued_messages": session.queued_messages
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_queue_update(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "update queued chat input");
        let Some((session_id, message_id)) = chat_queue_path_parts(&request.path) else {
            return chat_session_id_required();
        };
        let Some(text) = request
            .body
            .as_ref()
            .and_then(|body| body.get("text"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "bad_request",
                        "message": "text is required"
                    }
                }),
            );
        };
        let service = self.chat_service(&durable_root);
        match service.update_queued_message(session_id, message_id, text) {
            Ok(session) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "queued_messages": session.queued_messages
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_queue_delete(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "remove queued chat input");
        let Some((session_id, message_id)) = chat_queue_path_parts(&request.path) else {
            return chat_session_id_required();
        };
        let service = self.chat_service(&durable_root);
        match service.remove_queued_message(session_id, message_id) {
            Ok(session) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "queued_messages": session.queued_messages
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_read(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "read chat sessions");
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/read"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let service = self.chat_service(&durable_root);
        match service.read(session_id) {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_stop(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "stop chat sessions");
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/stop"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let service = self.chat_service(&durable_root);
        let existing = match service.resume(session_id) {
            Ok(session) => session,
            Err(error) => return error_response(error),
        };
        if matches!(existing.attachment, ChatAttachment::Standalone)
            && existing
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.submitted_gap_id.as_deref())
                .is_none()
            && let Some(worktree) = existing.worktree.as_ref()
            && let Err(error) = cleanup_standalone_chat_worktree(&durable_root, worktree)
        {
            return error_response(error);
        }
        match service.stop(session_id) {
            Ok(session) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "session_id": session.id,
                    "alive": !session.closed,
                    "closed_reason": session.interruption_detail
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_submit_ready_merge(&self, request: ApiRequest) -> ApiResponse {
        let durable_root = require_durable_root!(self, "submit standalone chat for merge");
        let runtime_root = match &self.runtime_root {
            Some(root) => root.clone(),
            None => return runtime_root_unavailable("submit standalone chat for merge"),
        };
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/submit-ready-merge"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let service = self.chat_service(&durable_root);
        let session = match service.resume(session_id) {
            Ok(session) => session,
            Err(error) => return error_response(error),
        };
        if !matches!(session.attachment, ChatAttachment::Standalone) {
            return error_response(RefineError::InvalidInput(
                "only standalone chat sessions can be submitted for merge".to_string(),
            ));
        }
        if session.closed {
            return error_response(RefineError::Conflict(format!(
                "Chat session {session_id} is closed"
            )));
        }
        let read_state = match service.read(session_id) {
            Ok(read) => read,
            Err(error) => return error_response(error),
        };
        if session.in_flight
            || session.queue_dispatching
            || !session.queued_messages.is_empty()
            || read_state.in_flight
            || !read_state.queued_messages.is_empty()
        {
            return error_response(RefineError::Conflict(
                "wait for the standalone chat to finish before submitting for merge".to_string(),
            ));
        }
        let Some(worktree) = session.worktree.clone() else {
            return error_response(RefineError::Conflict(format!(
                "Chat session {session_id} has no standalone worktree"
            )));
        };
        if worktree.submitted_gap_id.is_some() {
            return error_response(RefineError::Conflict(format!(
                "Chat session {session_id} was already submitted"
            )));
        }

        let actual = body
            .get("actual")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let target = body
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let reporter = body
            .get("reporter")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let priority = body
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("low")
            .trim();
        let Some(name) = body
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| derive_standalone_gap_name(actual, target))
        else {
            return error_response(RefineError::InvalidInput(
                "body.name, body.actual, or body.target is required".to_string(),
            ));
        };
        if reporter.is_empty() || actual.is_empty() || target.is_empty() {
            return error_response(RefineError::InvalidInput(
                "reporter, actual, and target are required".to_string(),
            ));
        }
        if !matches!(priority, "low" | "medium" | "high") {
            return error_response(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }

        let settings = match FileSettingsService::new(&durable_root).load() {
            Ok(settings) => settings,
            Err(error) => return error_response(error),
        };
        let target_branch = settings
            .get("merge_target_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("main");
        let worktree_git = FileGitWorktreeService::with_runtime_root(&worktree.path, &runtime_root);
        let work_items =
            FileWorkItemService::with_projection_cache(&durable_root, runtime_root.join("cache"));
        let gap = match work_items.create_gap_summary(&name, None) {
            Ok(gap) => gap,
            Err(error) => return error_response(error),
        };
        let gap_id = gap.gap.id.clone();
        let submit_result = (|| -> RefineResult<_> {
            work_items.append_gap_round_summary(&gap_id, reporter, actual, target)?;
            if priority != "low" {
                work_items.update_gap_metadata_summary(&gap_id, None, Some(priority), None)?;
            }
            match worktree_git.commit(&format!("Submit {gap_id} from standalone chat"), &[]) {
                Ok(_) => {}
                Err(error) => {
                    if !worktree_git.has_commits_since(target_branch)? {
                        return Err(error);
                    }
                }
            }
            work_items.set_gap_branch_name(&gap_id, &worktree.branch)?;
            work_items.transition_gap_status(&gap_id, GapStatus::Todo)?;
            work_items.advance_automated_gap_status(&gap_id, GapStatus::InProgress)?;
            work_items.advance_automated_gap_status(&gap_id, GapStatus::Qa)?;
            let gap = work_items.advance_automated_gap_status(&gap_id, GapStatus::ReadyMerge)?;
            service.mark_worktree_submitted(session_id, &gap_id)?;
            service.interrupt(session_id, "submitted for ready-merge")?;
            Ok(gap)
        })();
        match submit_result {
            Ok(gap) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(
                    201,
                    json!({
                        "ok": true,
                        "gap": gap.gap,
                        "session_id": session_id,
                        "worktree": worktree
                    }),
                ),
                Err(error) => error_response(error),
            },
            Err(error) => {
                let _ = work_items.delete_gap_record(&gap_id);
                error_response(error)
            }
        }
    }
}

fn create_standalone_chat_worktree(
    durable_root: &Path,
    runtime_root: &Path,
    session_id: &str,
) -> RefineResult<ChatSessionWorktree> {
    let source_root = durable_root.parent().ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "durable root {} has no source repository parent",
            durable_root.display()
        ))
    })?;
    let branch = format!("refine/standalone/{session_id}");
    let git = FileGitWorktreeService::with_runtime_root(source_root, runtime_root);
    let target = git
        .git_path("refine-standalone-worktrees")?
        .join(session_id);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create standalone worktree directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let path = git.ensure_worktree(&branch, &target)?;
    Ok(ChatSessionWorktree {
        branch,
        path,
        submitted_gap_id: None,
    })
}

fn cleanup_standalone_chat_worktree(
    durable_root: &Path,
    worktree: &ChatSessionWorktree,
) -> RefineResult<()> {
    let source_root = durable_root.parent().ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "durable root {} has no source repository parent",
            durable_root.display()
        ))
    })?;
    let git = FileGitWorktreeService::new(source_root);
    let path = PathBuf::from(&worktree.path);
    if path.exists() {
        git.remove_worktree(&path, true)?;
    }
    let _ = git.delete_branch(&worktree.branch, true);
    Ok(())
}

fn derive_standalone_gap_name(actual: &str, target: &str) -> Option<String> {
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

fn effective_chat_provider(configured: Option<String>) -> Option<String> {
    let configured = configured
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let status = provider_status_value().ok()?;
    let providers = status.get("providers").and_then(Value::as_array);
    let configured_installed = configured.as_deref().is_some_and(|configured| {
        providers.into_iter().flatten().any(|provider| {
            provider.get("name").and_then(Value::as_str) == Some(configured)
                && provider
                    .get("installed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
    });
    if configured_installed {
        return configured;
    }
    status
        .get("selected_provider")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(configured)
}

fn chat_queue_path_parts(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/chat/")?;
    let (session_id, rest) = rest.split_once("/queue/")?;
    let message_id = rest.trim();
    if session_id.is_empty()
        || session_id.contains('/')
        || message_id.is_empty()
        || message_id.contains('/')
    {
        return None;
    }
    Some((session_id, message_id))
}
