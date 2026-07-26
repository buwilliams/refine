use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::model::{JsonObject, Timestamp};
use crate::process::subprocess::FileProcessSupervisor;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::prompts::{PromptEngine, PromptTemplate, render};
use crate::tools::host::agent_providers::{
    HostAgentProviderService, ProviderInvocation, ProviderInvocationResult,
};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::host::project_layout::target_root_for_refine_dir;
use crate::tools::product::project_state::{FileProjectStateStore, GoalSummaryProjection};
use crate::tools::product::work_items::FileWorkItemService;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatAttachment {
    Goal(String),
    Feature(String),
    Supervisor,
    Standalone,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChatSessionRecord {
    pub id: String,
    pub mode: String,
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub attachment: ChatAttachment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<ChatSessionWorktree>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub transcript_events: Vec<JsonObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued_messages: Vec<ChatQueuedMessage>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub queue_dispatching: bool,
    pub importable_artifacts: Vec<JsonObject>,
    pub closed: bool,
    #[serde(default, skip_serializing)]
    pub in_flight: bool,
    #[serde(default, skip_serializing)]
    pub last_turn_started_at: Option<Timestamp>,
    pub interrupted: bool,
    pub interruption_detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatSessionWorktree {
    pub branch: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_goal_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChatQueuedMessage {
    pub id: String,
    pub text: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "is_false")]
    pub internal: bool,
}

impl ChatSessionRecord {
    pub fn visible_queued_messages(&self) -> Vec<ChatQueuedMessage> {
        self.queued_messages
            .iter()
            .filter(|message| !is_internal_queued_message(&self.attachment, message))
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ChatReadResult {
    pub alive: bool,
    pub session_id: String,
    pub lines: Vec<String>,
    pub progress_lines: Vec<String>,
    pub queued_messages: Vec<ChatQueuedMessage>,
    pub importable_artifacts: Vec<JsonObject>,
    pub closed_reason: Option<String>,
    pub in_flight: bool,
    pub provider_session_id: Option<String>,
    pub worktree: Option<ChatSessionWorktree>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StandaloneReadyMergeRequest {
    pub name: Option<String>,
    pub reporter: String,
    pub prompt: String,
    pub priority: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StandaloneReadyMergeResult {
    pub goal: GoalSummaryProjection,
    pub worktree: ChatSessionWorktree,
}

pub trait ChatService {
    fn start(&self, attachment: ChatAttachment) -> RefineResult<ChatSessionRecord>;
    fn resume(&self, session_id: &str) -> RefineResult<ChatSessionRecord>;
    fn append_user_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> RefineResult<ChatSessionRecord>;
    fn interrupt(&self, session_id: &str, detail: &str) -> RefineResult<ChatSessionRecord>;
}

#[derive(Clone, Debug)]
pub struct FileChatService {
    pub refine_dir: PathBuf,
    pub runtime_root: PathBuf,
}

impl FileChatService {}

struct ChatSessionLock {
    path: PathBuf,
}

struct ChatCapacityPermit;

impl Drop for ChatSessionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn unread_lines(record: &ChatSessionRecord) -> Vec<String> {
    record
        .transcript_events
        .iter()
        .filter(|event| !event_bool(event, "delivered"))
        .filter(|event| !event_bool(event, "progress"))
        .filter_map(event_text)
        .collect()
}

fn unread_progress(record: &ChatSessionRecord) -> Vec<String> {
    record
        .transcript_events
        .iter()
        .filter(|event| !event_bool(event, "delivered"))
        .filter(|event| event_bool(event, "progress"))
        .filter_map(event_text)
        .collect()
}

impl FileChatService {
    fn project_root(&self) -> PathBuf {
        self.refine_dir
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.refine_dir.clone())
    }

    fn chat_cwd(&self, record: &ChatSessionRecord) -> PathBuf {
        match &record.attachment {
            ChatAttachment::Standalone => record
                .worktree
                .as_ref()
                .map(|worktree| PathBuf::from(&worktree.path))
                .unwrap_or_else(|| self.project_root()),
            _ => self.project_root(),
        }
    }

    fn provider_path_override(&self) -> Option<String> {
        let mut paths = Vec::new();
        paths.push(self.refine_dir.join("provider-bin"));
        paths.push(self.project_root().join("node_modules/.bin"));
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            paths.push(home.join(".local/bin"));
            paths.push(home.join(".npm-global/bin"));
            paths.push(home.join(".cargo/bin"));
        }
        let joined = std::env::join_paths(paths).ok()?;
        Some(joined.to_string_lossy().to_string())
    }

    fn chat_prompt(&self, record: &ChatSessionRecord, message: &str) -> String {
        let attachment = match &record.attachment {
            ChatAttachment::Goal(id) => format!("Goal {id}"),
            ChatAttachment::Feature(id) => format!("Feature {id}"),
            ChatAttachment::Supervisor => "supervisor agent".to_string(),
            ChatAttachment::Standalone => "standalone chat".to_string(),
        };
        let instructions = chat_mode_instructions(record);
        let context = self
            .attached_product_context(record)
            .unwrap_or_else(|error| {
                format!("Attachment context could not be rebuilt from refine records: {error}")
            });
        render(
            PromptTemplate::Chat,
            &[
                ("mode", &record.mode),
                ("attachment", &attachment),
                ("instructions", instructions),
                ("context", &context),
                ("message", message),
            ],
        )
    }

    fn attached_product_context(&self, record: &ChatSessionRecord) -> RefineResult<String> {
        let store = FileProjectStateStore::with_runtime_root(&self.refine_dir, &self.runtime_root);
        let snapshot = store.load_or_refresh_projection(&self.runtime_root.join("cache"))?;
        match &record.attachment {
            ChatAttachment::Goal(id) => {
                let Some(goal) = snapshot.goals.get(id) else {
                    return Err(RefineError::NotFound(format!("Goal {id} was not found")));
                };
                serde_json::to_string_pretty(&json!({
                    "type": "goal",
                    "id": &goal.goal.id,
                    "name": &goal.goal.name,
                    "status": &goal.goal.status,
                    "priority": &goal.goal.priority,
                    "reporter": &goal.goal.reporter,
                    "round_count": goal.goal.round_count,
                    "feature_id": &goal.goal.feature_id,
                    "node_id": &goal.goal.node_id,
                    "updated": &goal.goal.updated
                }))
            }
            ChatAttachment::Feature(id) => {
                let Some(feature) = snapshot.features.get(id) else {
                    return Err(RefineError::NotFound(format!("Feature {id} was not found")));
                };
                serde_json::to_string_pretty(&json!({
                    "type": "feature",
                    "id": &feature.feature.id,
                    "name": &feature.feature.name,
                    "status": &feature.status,
                    "goal_ids": &feature.goal_ids,
                    "rollup": &feature.rollup,
                    "updated": &feature.feature.updated
                }))
            }
            ChatAttachment::Supervisor => {
                return Err(RefineError::Conflict(
                    "Supervisor Agent sessions are retired".to_string(),
                ));
            }
            ChatAttachment::Standalone => {
                let mut context = json!({
                    "type": "standalone",
                    "description": "standalone chat; no attached product record"
                });
                if let Some(worktree) = &record.worktree {
                    context["worktree"] = json!(worktree);
                }
                serde_json::to_string_pretty(&context)
            }
        }
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode chat attachment context: {error}"))
        })
    }
}

fn chat_mode_instructions(record: &ChatSessionRecord) -> &'static str {
    if record.mode.eq_ignore_ascii_case("plan") {
        return PromptEngine::load(PromptTemplate::ChatPlan);
    }
    PromptEngine::load(match &record.attachment {
        ChatAttachment::Goal(_) => PromptTemplate::ChatGoal,
        ChatAttachment::Feature(_) => PromptTemplate::ChatFeature,
        ChatAttachment::Supervisor => PromptTemplate::ChatAgent,
        ChatAttachment::Standalone => PromptTemplate::ChatStandalone,
    })
}

fn chat_event(
    role: &str,
    text: &str,
    progress: bool,
    provider_session_id: Option<String>,
    extra: Option<Value>,
) -> JsonObject {
    let mut value = json!({
        "id": new_event_id(),
        "role": role,
        "text": text,
        "progress": progress,
        "delivered": false,
        "created_at": now_timestamp(),
        "provider_session_id": provider_session_id
    });
    if let Some(extra) = extra {
        value["extra"] = extra;
    }
    value.as_object().cloned().unwrap_or_default()
}

fn chat_process_metadata(record: &ChatSessionRecord) -> JsonObject {
    let mut metadata = JsonObject::new();
    metadata.insert("kind".to_string(), json!("chat"));
    metadata.insert("session_id".to_string(), json!(record.id));
    metadata.insert("mode".to_string(), json!(record.mode));
    if matches!(record.attachment, ChatAttachment::Supervisor) {
        metadata.insert("agent_role".to_string(), json!("supervisor"));
    }
    metadata
}

fn event_text(event: &JsonObject) -> Option<String> {
    let role = event
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let text = event.get("text").and_then(|value| value.as_str())?;
    match role {
        "user" => Some(format!("> {text}")),
        "assistant" | "system" => Some(text.to_string()),
        _ => Some(text.to_string()),
    }
}

fn event_bool(event: &JsonObject, key: &str) -> bool {
    event
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn derive_standalone_goal_name(prompt: &str) -> Option<String> {
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

fn importable_artifacts_from_output(output: &str) -> Vec<JsonObject> {
    let mut artifacts = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(output.trim()) {
        collect_importable_artifacts(&value, &mut artifacts);
    }
    for line in output.lines() {
        let Some(raw) = line
            .trim()
            .strip_prefix("REFINE_ARTIFACT:")
            .or_else(|| line.trim().strip_prefix("refine_artifact:"))
        else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(raw.trim()) {
            collect_importable_artifacts(&value, &mut artifacts);
        }
    }
    artifacts
}

fn collect_importable_artifacts(value: &Value, artifacts: &mut Vec<JsonObject>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_importable_artifacts(value, artifacts);
            }
        }
        Value::Object(object) => {
            if let Some(values) = object
                .get("importable_artifacts")
                .and_then(|value| value.as_array())
            {
                for value in values {
                    collect_importable_artifacts(value, artifacts);
                }
            }
            if recognized_artifact(object) {
                artifacts.push(object.clone());
                return;
            }
            for (key, artifact_type) in [
                ("round", "round"),
                ("goal", "goal"),
                ("feature_plan", "feature_plan"),
            ] {
                if let Some(Value::Object(payload)) = object.get(key) {
                    let mut artifact = JsonObject::new();
                    artifact.insert("type".to_string(), Value::String(artifact_type.to_string()));
                    artifact.insert(key.to_string(), Value::Object(payload.clone()));
                    artifacts.push(artifact);
                }
            }
            if let Some(Value::Array(goals)) = object.get("goals") {
                let mut artifact = JsonObject::new();
                artifact.insert("type".to_string(), Value::String("goals".to_string()));
                artifact.insert("goals".to_string(), Value::Array(goals.clone()));
                artifacts.push(artifact);
            }
        }
        _ => {}
    }
}

fn recognized_artifact(object: &JsonObject) -> bool {
    matches!(
        object.get("type").and_then(|value| value.as_str()),
        Some("round" | "goal" | "goals" | "feature_plan")
    )
}

fn combined_queued_message(messages: &[ChatQueuedMessage]) -> String {
    if messages.len() == 1 {
        return messages[0].text.clone();
    }
    messages
        .iter()
        .enumerate()
        .map(|(idx, message)| format!("Message {}:\n{}", idx + 1, message.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn is_internal_queued_message(attachment: &ChatAttachment, message: &ChatQueuedMessage) -> bool {
    message.internal
        || (matches!(attachment, ChatAttachment::Supervisor)
            && message.text.starts_with(
                "Supervise until the queue is idle using the evidence and Refine's tools.",
            ))
}

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn reject_retired_supervisor(record: &ChatSessionRecord) -> RefineResult<()> {
    if matches!(record.attachment, ChatAttachment::Supervisor) {
        return Err(RefineError::Conflict(
            "Supervisor Agent sessions are retired; open an independent general Agent".to_string(),
        ));
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> RefineResult<()> {
    if !session_id.is_empty()
        && session_id.len() <= 64
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(
            "chat session id is invalid".to_string(),
        ))
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn new_chat_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{:x}{:x}{:x}",
        now.as_millis(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn new_event_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "evt-{}-{}",
        now.as_millis(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn new_queued_message_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "qm-{:x}{:x}{:x}",
        now.as_millis(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn default_chat_runtime_root(refine_dir: &Path) -> PathBuf {
    refine_dir
        .parent()
        .map(|root| root.join("run/chat"))
        .unwrap_or_else(|| refine_dir.join("run/chat"))
}

fn write_chat_record_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp_path = path.with_extension(format!("json.{}.tmp", new_event_id()));
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)
}

fn chat_session_id_from_operation(operation: &OperationHandle) -> Option<&str> {
    operation.owner.strip_prefix("chat:")
}

fn chat_operation_log(severity: &str, message: &str, details: Option<JsonObject>) -> LogEntry {
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

mod provider_turns;
mod queue;
mod service_adapter;
mod sessions;
mod standalone;
#[cfg(test)]
mod tests;
