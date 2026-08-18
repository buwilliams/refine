use serde_json::json;

use crate::infrastructure::process::supervisor::operations::OperationHandle;
use crate::model::JsonObject;
use crate::model::log::LogEntry;

use super::session_foundation::now_timestamp;
use super::{ChatAttachment, ChatSessionRecord};

pub(super) fn chat_process_metadata(record: &ChatSessionRecord) -> JsonObject {
    let mut metadata = JsonObject::new();
    metadata.insert("kind".to_string(), json!("chat"));
    metadata.insert("session_id".to_string(), json!(record.id));
    metadata.insert("mode".to_string(), json!(record.mode));
    if matches!(record.attachment, ChatAttachment::Supervisor) {
        metadata.insert("agent_role".to_string(), json!("supervisor"));
    }
    metadata
}

pub(super) fn chat_session_id_from_operation(operation: &OperationHandle) -> Option<&str> {
    operation.owner.strip_prefix("chat:")
}

pub(super) fn chat_operation_log(
    severity: &str,
    message: &str,
    details: Option<JsonObject>,
) -> LogEntry {
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "chat".to_string(),
        message: message.to_string(),
        details,
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        goal_id: None,
    }
}
