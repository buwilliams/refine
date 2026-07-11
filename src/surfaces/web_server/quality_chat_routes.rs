use serde_json::{Value, json};

use crate::process::subprocess::FileProcessSupervisor;
use crate::process::supervisor::config::ConfigService;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::quality::{
    FileQualityService, QualityCheckRequest, QualityOperationRunner, QualityService,
    QualitySettingsPatch,
};
use crate::tools::host::target_apps::{FileTargetAppService, TargetAppSnapshot};
use crate::tools::product::chat::{ChatAttachment, ChatService, StandaloneReadyMergeRequest};

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn quality_timing_setting(&self) -> String {
        self.current_refine_dir()
            .ok()
            .flatten()
            .and_then(|refine_dir| self.settings_service(refine_dir).load().ok())
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
        let Some(refine_dir) = self.current_refine_dir()? else {
            return Err(RefineError::Degraded(
                "target root is required for target-app operations".to_string(),
            ));
        };
        let Some(target_root) = self.current_target_root()? else {
            return Err(RefineError::Degraded(
                "target root is required for target-app operations".to_string(),
            ));
        };
        let Some(runtime_root) = &self.runtime_root else {
            return Err(RefineError::Degraded(
                "runtime root is required for target-app operations".to_string(),
            ));
        };
        Ok(FileTargetAppService::new(
            refine_dir,
            runtime_root,
            target_root,
        ))
    }

    pub(super) fn target_app_response(&self, snapshot: TargetAppSnapshot) -> serde_json::Value {
        let settings = self
            .current_refine_dir()
            .ok()
            .flatten()
            .and_then(|root| self.settings_service(root).load().ok())
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
            "has_build_command": !get("target_app_build_command").trim().is_empty(),
            "has_start_instructions": !get("target_app_start_instructions").trim().is_empty(),
            "has_stop_instructions": !get("target_app_stop_instructions").trim().is_empty(),
            "has_build_instructions": !get("target_app_build_instructions").trim().is_empty(),
            "has_test_command": !get("target_app_test_command").trim().is_empty(),
            "has_status_checks": !get("target_app_status_command").trim().is_empty()
                || !get("target_app_http_check_url").trim().is_empty()
                || !get("target_app_tcp_check_host").trim().is_empty()
                || !get("target_app_process_check_command").trim().is_empty(),
            "has_start_action": !get("target_app_start_instructions").trim().is_empty()
                || !get("target_app_start_command").trim().is_empty(),
            "has_stop_action": !get("target_app_stop_instructions").trim().is_empty()
                || !get("target_app_stop_command").trim().is_empty(),
            "has_build_action": !get("target_app_build_instructions").trim().is_empty()
                || !get("target_app_build_command").trim().is_empty(),
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
            "auto_build": get("target_app_auto_build"),
            "auto_build_hour_utc": get("target_app_auto_build_hour_utc"),
            "auto_build_last_started_at": "",
            "auto_build_last_finished_at": "",
            "auto_build_last_ok": false,
            "auto_build_last_message": "",
            "legacy_config_present": !get("target_app_start_command").trim().is_empty()
                || !get("target_app_stop_command").trim().is_empty()
                || !get("target_app_build_command").trim().is_empty()
                || !get("target_app_health_url").trim().is_empty(),
            "message": snapshot.message
        })
    }

    pub(super) fn handle_quality_get(&self) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "load quality settings");
        match FileQualityService::new(refine_dir).load_settings() {
            Ok(settings) => ApiResponse::json(200, json!(settings)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_save(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "save quality settings");
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
        match FileQualityService::new(&refine_dir).save_settings(patch) {
            Ok(settings) => {
                append_quality_activity(&refine_dir, "Quality settings updated".to_string());
                ApiResponse::json(200, json!(settings))
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_checks(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "run quality checks");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("run quality checks");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let owner_id = body
            .get("owner_id")
            .or_else(|| body.get("goal_id"))
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
        match QualityOperationRunner::new(&refine_dir, runtime_root).run_checks(
            QualityCheckRequest {
                owner_id,
                command,
                process_metadata: Default::default(),
            },
        ) {
            Ok(operation_result) => {
                append_quality_activity(
                    &refine_dir,
                    format!(
                        "Quality checks completed for {}",
                        operation_result.result.owner_id
                    ),
                );
                ApiResponse::json(
                    200,
                    json!({
                        "ok": operation_result.result.ok,
                        "result": operation_result.result,
                        "operation": operation_response(operation_result.operation)
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_quality_screenshots(&self, raw_path: &str) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "list quality screenshots");
        let owner_id = query_param(raw_path, "owner_id").unwrap_or_else(|| "app".to_string());
        match FileQualityService::new(refine_dir).screenshots(&owner_id) {
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

    pub(super) fn handle_chat_start(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "start chat sessions");
        let body = request.body.unwrap_or_else(|| json!({}));
        let attachment = if let Some(goal_id) = body.get("goal_id").and_then(|value| value.as_str())
        {
            ChatAttachment::Goal(goal_id.to_string())
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
            self.settings_service(&refine_dir)
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
        let service = self.chat_service(&refine_dir);
        if matches!(attachment, ChatAttachment::Standalone) && self.runtime_root.is_none() {
            return runtime_root_unavailable("start standalone chat sessions");
        }
        let result = service.start_with_options(attachment.clone(), provider.as_deref(), mode);
        match result {
            Ok(session) => ApiResponse::json(
                201,
                json!({
                    "ok": true,
                    "session_id": session.id,
                    "provider": session.provider,
                    "mode": session.mode,
                    "worktree": session.worktree
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_input(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "send chat input");
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
        let service = self.chat_service(&refine_dir);
        match service.append_user_message(session_id, text) {
            Ok(session) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "in_flight": session.in_flight || session.queue_dispatching,
                    "queued_messages": session.queued_messages
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_queue_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update queued chat input");
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
        let service = self.chat_service(&refine_dir);
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
        let refine_dir = require_refine_dir!(self, "remove queued chat input");
        let Some((session_id, message_id)) = chat_queue_path_parts(&request.path) else {
            return chat_session_id_required();
        };
        let service = self.chat_service(&refine_dir);
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
        let refine_dir = require_refine_dir!(self, "read chat sessions");
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/read"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let service = self.chat_service(&refine_dir);
        match service.read(session_id) {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_chat_stop(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "stop chat sessions");
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/stop"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let service = self.chat_service(&refine_dir);
        match service.stop_with_standalone_cleanup(session_id) {
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
        let refine_dir = require_refine_dir!(self, "submit standalone chat for merge");
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("submit standalone chat for merge");
        }
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/submit-ready-merge"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let service = self.chat_service(&refine_dir);
        let submit = service.submit_standalone_ready_merge(
            session_id,
            StandaloneReadyMergeRequest {
                name: body.get("name").and_then(Value::as_str).map(str::to_string),
                reporter: body
                    .get("reporter")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                prompt: body
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                priority: body
                    .get("priority")
                    .and_then(Value::as_str)
                    .unwrap_or("low")
                    .to_string(),
            },
        );
        match submit {
            Ok(result) => match self.refresh_projection_cache_after_mutation() {
                Ok(()) => ApiResponse::json(
                    201,
                    json!({
                        "ok": true,
                        "goal": result.goal.goal,
                        "session_id": session_id,
                        "worktree": result.worktree
                    }),
                ),
                Err(error) => error_response(error),
            },
            Err(error) => error_response(error),
        }
    }
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
