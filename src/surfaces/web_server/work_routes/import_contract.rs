use std::path::Path;

use serde_json::{Value, json};

use crate::application::chat::FileChatService;
use crate::application::imports::{
    ImportExtractionResult, ImportPersistObserver, ImportPersistProgress, ImportRollbackEvidence,
};
use crate::error::RefineError;
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};

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

pub(super) struct WebImportPersistObserver<'a> {
    pub(super) registry: &'a FileOperationRegistry,
    pub(super) operation_id: &'a str,
}

impl ImportPersistObserver for WebImportPersistObserver<'_> {
    fn is_cancelled(&self) -> bool {
        self.registry
            .status(self.operation_id)
            .map(|operation| {
                matches!(
                    operation.state,
                    OperationState::Cancelling | OperationState::Cancelled
                )
            })
            .unwrap_or(false)
    }

    fn on_progress(&mut self, progress: ImportPersistProgress) {
        let _ = self.registry.update_progress(
            self.operation_id,
            json!({
                "message": "Saving import",
                "completed": progress.completed,
                "total": progress.total
            }),
        );
        // Keep background batches cooperative so cancellation can be observed
        // between durable draft transactions.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    fn on_rollback_start(&mut self, rollback: &ImportRollbackEvidence) {
        let _ = self.registry.update_progress(
            self.operation_id,
            json!({
                "message": "Rolling back import",
                "completed": 0,
                "total": rollback.created_goal_ids.len()
            }),
        );
    }
}
