use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::host::agent_providers::{
    HostAgentProviderService, ProviderInvocation, ProviderInvocationResult,
};
use crate::core::product::project_state::FileProjectStateStore;
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::jobs::{FileJobRegistry, JobHandle, JobRegistry, JobState};
use crate::model::log::LogEntry;
use crate::model::{JsonObject, Timestamp};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatAttachment {
    Gap(String),
    Feature(String),
    Standalone,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChatSessionRecord {
    pub id: String,
    pub mode: String,
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub attachment: ChatAttachment,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub transcript_events: Vec<JsonObject>,
    pub importable_artifacts: Vec<JsonObject>,
    pub closed: bool,
    #[serde(default, skip_serializing)]
    pub in_flight: bool,
    #[serde(default, skip_serializing)]
    pub last_turn_started_at: Option<Timestamp>,
    pub interrupted: bool,
    pub interruption_detail: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ChatReadResult {
    pub alive: bool,
    pub session_id: String,
    pub lines: Vec<String>,
    pub progress_lines: Vec<String>,
    pub importable_artifacts: Vec<JsonObject>,
    pub closed_reason: Option<String>,
    pub in_flight: bool,
    pub provider_session_id: Option<String>,
}

pub trait ChatService {
    fn start(&self, attachment: ChatAttachment) -> RefineResult<ChatSessionRecord>;
    fn resume(&self, session_id: &str) -> RefineResult<ChatSessionRecord>;
    fn append_user_message(&self, session_id: &str, message: &str) -> RefineResult<()>;
    fn interrupt(&self, session_id: &str, detail: &str) -> RefineResult<ChatSessionRecord>;
}

#[derive(Clone, Debug)]
pub struct FileChatService {
    pub durable_root: PathBuf,
    pub runtime_root: PathBuf,
}

impl FileChatService {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        let durable_root = durable_root.into();
        let runtime_root = default_chat_runtime_root(&durable_root);
        Self {
            durable_root,
            runtime_root,
        }
    }

    pub fn with_runtime_root(
        durable_root: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn start_with_options(
        &self,
        attachment: ChatAttachment,
        provider: Option<&str>,
        mode: Option<&str>,
    ) -> RefineResult<ChatSessionRecord> {
        let now = now_timestamp();
        let attachment_mode = match &attachment {
            ChatAttachment::Gap(_) => "gap",
            ChatAttachment::Feature(_) => "feature",
            ChatAttachment::Standalone => "standalone",
        };
        let record = ChatSessionRecord {
            id: new_chat_id(),
            mode: mode.unwrap_or(attachment_mode).trim().to_string(),
            provider: provider.unwrap_or("claude").trim().to_string(),
            provider_session_id: None,
            attachment,
            created_at: now.clone(),
            updated_at: now,
            transcript_events: Vec::new(),
            importable_artifacts: Vec::new(),
            closed: false,
            in_flight: false,
            last_turn_started_at: None,
            interrupted: false,
            interruption_detail: None,
        };
        self.write_record(&record)?;
        Ok(record)
    }

    pub fn read(&self, session_id: &str) -> RefineResult<ChatReadResult> {
        let mut record = self.load_record(session_id)?;
        let lines = unread_lines(&record);
        let progress_lines = unread_progress(&record);
        if !lines.is_empty() || !progress_lines.is_empty() {
            for event in &mut record.transcript_events {
                event.insert("delivered".to_string(), Value::Bool(true));
            }
            self.write_record(&record)?;
        }
        let active_job = self.session_has_active_job(&record.id)?;
        Ok(ChatReadResult {
            alive: !record.closed,
            session_id: record.id.clone(),
            lines,
            progress_lines,
            importable_artifacts: record.importable_artifacts.clone(),
            closed_reason: record.interruption_detail.clone(),
            in_flight: record.in_flight || active_job,
            provider_session_id: record.provider_session_id.clone(),
        })
    }

    pub fn stop(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        self.interrupt(session_id, "stopped")
    }

    fn load_record(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        validate_session_id(session_id)?;
        let path = self.session_path(session_id);
        let bytes = fs::read_to_string(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                return RefineError::NotFound(format!("Chat session {session_id} was not found"));
            }
            RefineError::Io(format!(
                "failed to read chat session {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_str(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse chat session {}: {error}",
                path.display()
            ))
        })
    }

    fn write_record(&self, record: &ChatSessionRecord) -> RefineResult<()> {
        fs::create_dir_all(self.sessions_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create chat sessions directory {}: {error}",
                self.sessions_dir().display()
            ))
        })?;
        let path = self.session_path(&record.id);
        let encoded = serde_json::to_string_pretty(record).map_err(|error| {
            RefineError::Serialization(format!("failed to encode chat session: {error}"))
        })?;
        fs::write(&path, format!("{encoded}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to write chat session {}: {error}",
                path.display()
            ))
        })
    }

    fn sessions_dir(&self) -> PathBuf {
        self.durable_root.join("chat/sessions")
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{session_id}.json"))
    }

    pub fn resume_provider_turn(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        let mut record = self.load_record(session_id)?;
        if record.closed {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} is closed"
            )));
        }
        let Some(provider_session_id) = record.provider_session_id.clone() else {
            return Err(RefineError::InvalidInput(format!(
                "Chat session {session_id} does not have a provider session id"
            )));
        };
        record.in_flight = true;
        record.last_turn_started_at = Some(now_timestamp());
        record.updated_at = now_timestamp();
        record.transcript_events.push(chat_event(
            "progress",
            "Resuming provider session.",
            true,
            Some(provider_session_id.clone()),
            None,
        ));
        self.write_record(&record)?;

        let job = self.register_provider_job(&record, "resume")?;
        let provider = HostAgentProviderService {
            path_override: self.provider_path_override(),
            runtime_root: Some(self.runtime_root.clone()),
        };
        match provider.resume_detailed(&record.provider, &provider_session_id) {
            Ok(result) => {
                self.apply_provider_success(&mut record, result, "Provider session resumed.");
                self.finish_provider_job(&job.id, JobState::Succeeded, "Provider session resumed")?;
            }
            Err(error) => {
                let detail =
                    format!("Provider session resume failed; transcript preserved: {error}");
                self.apply_provider_failure(&mut record, detail);
                self.finish_provider_job(
                    &job.id,
                    JobState::Failed,
                    "Provider session resume failed",
                )?;
            }
        }
        record.updated_at = now_timestamp();
        self.write_record(&record)?;
        Ok(record)
    }

    pub fn recover_interrupted_turns(&self, detail: &str) -> RefineResult<Vec<ChatSessionRecord>> {
        let message = detail.trim();
        let registry = self.job_registry();
        let mut recovered_session_ids = Vec::new();
        for job in registry.recover()? {
            let Some(session_id) = chat_session_id_from_job(&job) else {
                continue;
            };
            if !matches!(
                job.state,
                JobState::Pending
                    | JobState::Running
                    | JobState::Cancelling
                    | JobState::Interrupted
            ) {
                continue;
            }
            let mut record = self.load_record(session_id)?;
            if record.interrupted && record.interruption_detail.as_deref() == Some(message) {
                continue;
            }
            self.mark_record_interrupted(&mut record, message);
            self.write_record(&record)?;
            if !matches!(job.state, JobState::Interrupted) {
                registry.finish(&job.id, JobState::Interrupted)?;
            }
            recovered_session_ids.push(record.id);
        }

        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut recovered = Vec::new();
        for entry in fs::read_dir(&sessions_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to read chat sessions directory {}: {error}",
                sessions_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read chat session entry {}: {error}",
                    sessions_dir.display()
                ))
            })?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read_to_string(entry.path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read chat session {}: {error}",
                    entry.path().display()
                ))
            })?;
            let mut record: ChatSessionRecord = serde_json::from_str(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse chat session {}: {error}",
                    entry.path().display()
                ))
            })?;
            if !record.in_flight {
                continue;
            }
            self.mark_record_interrupted(&mut record, message);
            self.write_record(&record)?;
            if !recovered_session_ids.contains(&record.id) {
                recovered_session_ids.push(record.id.clone());
                recovered.push(record);
            }
        }
        for session_id in recovered_session_ids {
            if recovered.iter().any(|record| record.id == session_id) {
                continue;
            }
            recovered.push(self.load_record(&session_id)?);
        }
        Ok(recovered)
    }

    fn apply_provider_success(
        &self,
        record: &mut ChatSessionRecord,
        result: ProviderInvocationResult,
        progress_message: &str,
    ) {
        if let Some(provider_session_id) = result.provider_session_id {
            record.provider_session_id = Some(provider_session_id);
        }
        let artifacts = importable_artifacts_from_output(&result.output);
        if !artifacts.is_empty() {
            record.importable_artifacts.extend(artifacts.clone());
            record.transcript_events.push(chat_event(
                "system",
                &format!("Detected {} importable artifact(s).", artifacts.len()),
                true,
                record.provider_session_id.clone(),
                Some(json!({"importable_artifacts": artifacts})),
            ));
        }
        record.transcript_events.push(chat_event(
            "assistant",
            nonempty_or(&result.output, "(provider returned no output)"),
            false,
            record.provider_session_id.clone(),
            None,
        ));
        record.transcript_events.push(chat_event(
            "progress",
            progress_message,
            true,
            record.provider_session_id.clone(),
            None,
        ));
        record.in_flight = false;
        record.last_turn_started_at = None;
        record.interrupted = false;
        record.interruption_detail = None;
    }

    fn apply_provider_failure(&self, record: &mut ChatSessionRecord, detail: String) {
        record.transcript_events.push(chat_event(
            "system",
            &detail,
            false,
            record.provider_session_id.clone(),
            None,
        ));
        record.in_flight = false;
        record.last_turn_started_at = None;
        record.interrupted = true;
        record.interruption_detail = Some(detail);
    }

    fn mark_record_interrupted(&self, record: &mut ChatSessionRecord, detail: &str) {
        record.in_flight = false;
        record.last_turn_started_at = None;
        record.interrupted = true;
        record.interruption_detail = Some(detail.to_string());
        record.updated_at = now_timestamp();
        record.transcript_events.push(chat_event(
            "system",
            detail,
            false,
            record.provider_session_id.clone(),
            None,
        ));
    }

    fn job_registry(&self) -> FileJobRegistry {
        FileJobRegistry::new(&self.runtime_root)
    }

    fn register_provider_job(
        &self,
        record: &ChatSessionRecord,
        operation: &str,
    ) -> RefineResult<JobHandle> {
        let registry = self.job_registry();
        let job = registry.register(&format!("chat:{}", record.id))?;
        let mut details = JsonObject::new();
        details.insert("session_id".to_string(), json!(record.id));
        details.insert("provider".to_string(), json!(record.provider));
        details.insert("mode".to_string(), json!(record.mode));
        details.insert("operation".to_string(), json!(operation));
        registry.append_log(
            &job.id,
            chat_job_log("info", "Chat provider job started", Some(details)),
        )?;
        Ok(job)
    }

    fn finish_provider_job(
        &self,
        job_id: &str,
        state: JobState,
        message: &str,
    ) -> RefineResult<JobHandle> {
        let registry = self.job_registry();
        registry.append_log(job_id, chat_job_log("info", message, None))?;
        registry.finish(job_id, state)
    }

    fn session_has_active_job(&self, session_id: &str) -> RefineResult<bool> {
        Ok(self.job_registry().recover()?.into_iter().any(|job| {
            chat_session_id_from_job(&job) == Some(session_id)
                && matches!(
                    job.state,
                    JobState::Pending | JobState::Running | JobState::Cancelling
                )
        }))
    }
}

impl ChatService for FileChatService {
    fn start(&self, attachment: ChatAttachment) -> RefineResult<ChatSessionRecord> {
        self.start_with_options(attachment, None, None)
    }

    fn resume(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        self.load_record(session_id)
    }

    fn append_user_message(&self, session_id: &str, message: &str) -> RefineResult<()> {
        let mut record = self.load_record(session_id)?;
        if record.closed {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} is closed"
            )));
        }
        record
            .transcript_events
            .push(chat_event("user", message, false, None, None));
        record.transcript_events.push(chat_event(
            "progress",
            "Message persisted; starting provider turn.",
            true,
            None,
            None,
        ));
        record.in_flight = true;
        record.last_turn_started_at = Some(now_timestamp());
        record.updated_at = now_timestamp();
        self.write_record(&record)?;

        let job = self.register_provider_job(&record, "invoke")?;
        let provider = HostAgentProviderService {
            path_override: self.provider_path_override(),
            runtime_root: Some(self.runtime_root.clone()),
        };
        match provider.invoke_detailed(ProviderInvocation {
            provider: record.provider.clone(),
            prompt: self.chat_prompt(&record, message),
            session_id: record.provider_session_id.clone(),
            cwd: Some(self.project_root().display().to_string()),
        }) {
            Ok(result) => {
                self.apply_provider_success(&mut record, result, "Provider turn completed.");
                self.finish_provider_job(&job.id, JobState::Succeeded, "Provider turn completed")?;
            }
            Err(error) => {
                self.apply_provider_failure(&mut record, format!("Provider turn failed: {error}"));
                self.finish_provider_job(&job.id, JobState::Failed, "Provider turn failed")?;
            }
        }
        record.updated_at = now_timestamp();
        self.write_record(&record)
    }

    fn interrupt(&self, session_id: &str, detail: &str) -> RefineResult<ChatSessionRecord> {
        let mut record = self.load_record(session_id)?;
        record.closed = true;
        record.interrupted = true;
        record.interruption_detail = Some(detail.trim().to_string());
        record.updated_at = now_timestamp();
        record
            .transcript_events
            .push(chat_event("system", detail, false, None, None));
        self.write_record(&record)?;
        Ok(record)
    }
}

fn unread_lines(record: &ChatSessionRecord) -> Vec<String> {
    record
        .transcript_events
        .iter()
        .filter(|event| !event_bool(event, "delivered"))
        .filter(|event| !event_bool(event, "progress"))
        .filter_map(|event| event_text(event))
        .collect()
}

fn unread_progress(record: &ChatSessionRecord) -> Vec<String> {
    record
        .transcript_events
        .iter()
        .filter(|event| !event_bool(event, "delivered"))
        .filter(|event| event_bool(event, "progress"))
        .filter_map(|event| event_text(event))
        .collect()
}

impl FileChatService {
    fn project_root(&self) -> PathBuf {
        self.durable_root
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.durable_root.clone())
    }

    fn provider_path_override(&self) -> Option<String> {
        let mut paths = Vec::new();
        paths.push(self.durable_root.join("provider-bin"));
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
            ChatAttachment::Gap(id) => format!("Gap {id}"),
            ChatAttachment::Feature(id) => format!("Feature {id}"),
            ChatAttachment::Standalone => "standalone chat".to_string(),
        };
        let context = self
            .attached_product_context(record)
            .unwrap_or_else(|error| {
                format!("Attachment context could not be rebuilt from durable records: {error}")
            });
        format!(
            "Refine {mode} chat attached to {attachment}.\n\nCurrent durable context:\n{context}\n\nUser message:\n{message}",
            mode = record.mode
        )
    }

    fn attached_product_context(&self, record: &ChatSessionRecord) -> RefineResult<String> {
        let store = FileProjectStateStore::new(&self.durable_root);
        let snapshot = store.load_or_refresh_projection(&self.runtime_root.join("cache"))?;
        match &record.attachment {
            ChatAttachment::Gap(id) => {
                let Some(gap) = snapshot.gaps.get(id) else {
                    return Err(RefineError::NotFound(format!("Gap {id} was not found")));
                };
                serde_json::to_string_pretty(&json!({
                    "type": "gap",
                    "id": &gap.gap.id,
                    "name": &gap.gap.name,
                    "status": &gap.gap.status,
                    "priority": &gap.gap.priority,
                    "reporter": &gap.gap.reporter,
                    "round_count": gap.gap.round_count,
                    "feature_id": &gap.gap.feature_id,
                    "node_id": &gap.gap.node_id,
                    "updated": &gap.gap.updated
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
                    "gap_ids": &feature.gap_ids,
                    "rollup": &feature.rollup,
                    "updated": &feature.feature.updated
                }))
            }
            ChatAttachment::Standalone => {
                Ok("standalone chat; no attached product record".to_string())
            }
        }
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode chat attachment context: {error}"))
        })
    }
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
                ("gap", "gap"),
                ("feature_plan", "feature_plan"),
            ] {
                if let Some(Value::Object(payload)) = object.get(key) {
                    let mut artifact = JsonObject::new();
                    artifact.insert("type".to_string(), Value::String(artifact_type.to_string()));
                    artifact.insert(key.to_string(), Value::Object(payload.clone()));
                    artifacts.push(artifact);
                }
            }
            if let Some(Value::Array(gaps)) = object.get("gaps") {
                let mut artifact = JsonObject::new();
                artifact.insert("type".to_string(), Value::String("gaps".to_string()));
                artifact.insert("gaps".to_string(), Value::Array(gaps.clone()));
                artifacts.push(artifact);
            }
        }
        _ => {}
    }
}

fn recognized_artifact(object: &JsonObject) -> bool {
    matches!(
        object.get("type").and_then(|value| value.as_str()),
        Some("round" | "gap" | "gaps" | "feature_plan")
    )
}

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
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

fn default_chat_runtime_root(durable_root: &Path) -> PathBuf {
    durable_root
        .parent()
        .map(|root| root.join("run/chat"))
        .unwrap_or_else(|| durable_root.join("run/chat"))
}

fn chat_session_id_from_job(job: &JobHandle) -> Option<&str> {
    job.owner.strip_prefix("chat:")
}

fn chat_job_log(severity: &str, message: &str, details: Option<JsonObject>) -> LogEntry {
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "chat".to_string(),
        message: message.to_string(),
        details,
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        gap_id: None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::core::product::work_items::FileWorkItemService;

    #[test]
    fn file_chat_service_persists_session_transcript_and_stop() {
        let temp_root = unique_temp_dir("chat");
        let durable_root = temp_root.join(".refine");
        write_fake_provider(&durable_root, "smoke-ai", 0, "provider says hello");
        let service = FileChatService::new(&durable_root);

        let session = service
            .start_with_options(
                ChatAttachment::Gap("GAP1".to_string()),
                Some("smoke-ai"),
                Some("gap"),
            )
            .unwrap();
        assert_eq!(session.mode, "gap");
        assert_eq!(session.provider, "smoke-ai");

        service
            .append_user_message(&session.id, "What should I test?")
            .unwrap();
        let read = service.read(&session.id).unwrap();
        assert!(read.alive);
        assert!(
            read.lines
                .iter()
                .any(|line| line.contains("What should I test?"))
        );
        assert!(
            read.progress_lines
                .iter()
                .any(|line| line.contains("Provider turn completed"))
        );
        assert!(
            read.lines
                .iter()
                .any(|line| line.contains("provider says hello"))
        );

        let stopped = service.stop(&session.id).unwrap();
        assert!(stopped.closed);
        assert_eq!(service.read(&session.id).unwrap().alive, false);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_rebuilds_attached_gap_context_from_durable_records() {
        let temp_root = unique_temp_dir("chat-gap-context");
        let durable_root = temp_root.join(".refine");
        FileWorkItemService::new(&durable_root)
            .create_gap_summary("Checkout fails", Some("GAP1"))
            .unwrap();
        let service = FileChatService::new(&durable_root);
        let session = service
            .start_with_options(
                ChatAttachment::Gap("GAP1".to_string()),
                Some("smoke-ai"),
                Some("gap"),
            )
            .unwrap();

        let prompt = service.chat_prompt(&session, "What changed?");
        assert!(prompt.contains("Current durable context"));
        assert!(prompt.contains("\"id\": \"GAP1\""));
        assert!(prompt.contains("\"name\": \"Checkout fails\""));
        assert!(prompt.contains("What changed?"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_persists_importable_artifacts_from_provider_output() {
        let temp_root = unique_temp_dir("chat-artifacts");
        let durable_root = temp_root.join(".refine");
        write_fake_provider(
            &durable_root,
            "smoke-ai",
            0,
            r#"{"importable_artifacts":[{"type":"round","round":{"reporter":"QA","actual":"Broken","target":"Fixed"}},{"type":"gap","gap":{"name":"Imported gap","actual":"A","target":"B"}}]}"#,
        );
        let service = FileChatService::new(&durable_root);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();

        service
            .append_user_message(&session.id, "draft follow-up")
            .unwrap();
        let resumed = service.resume(&session.id).unwrap();
        assert_eq!(resumed.importable_artifacts.len(), 2);
        assert_eq!(resumed.importable_artifacts[0]["type"], "round");
        assert_eq!(resumed.importable_artifacts[1]["type"], "gap");
        assert!(resumed.transcript_events.iter().any(|event| {
            event
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .contains("Detected 2 importable artifact")
        }));
        let read = service.read(&session.id).unwrap();
        assert_eq!(read.importable_artifacts.len(), 2);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_persists_provider_failure() {
        let temp_root = unique_temp_dir("chat-failure");
        let durable_root = temp_root.join(".refine");
        write_fake_provider(&durable_root, "smoke-ai", 2, "provider failed");
        let service = FileChatService::new(&durable_root);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();

        service.append_user_message(&session.id, "hello").unwrap();
        let resumed = service.resume(&session.id).unwrap();
        assert!(resumed.interrupted);
        assert!(
            resumed
                .interruption_detail
                .as_deref()
                .unwrap_or("")
                .contains("provider failed")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_persists_provider_session_id_and_in_flight_lifecycle() {
        let temp_root = unique_temp_dir("chat-provider-session");
        let durable_root = temp_root.join(".refine");
        write_fake_provider(
            &durable_root,
            "smoke-ai",
            0,
            r#"{"session_id":"prov-1","item":{"type":"agent_message","text":"provider says hello"}}"#,
        );
        let service = FileChatService::new(&durable_root);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();

        service.append_user_message(&session.id, "hello").unwrap();
        let resumed = service.resume(&session.id).unwrap();
        assert_eq!(resumed.provider_session_id.as_deref(), Some("prov-1"));
        assert!(!resumed.in_flight);
        assert_eq!(resumed.last_turn_started_at, None);
        assert!(!resumed.interrupted);
        let persisted: Value = serde_json::from_str(
            &fs::read_to_string(durable_root.join(format!("chat/sessions/{}.json", session.id)))
                .unwrap(),
        )
        .unwrap();
        assert!(persisted.get("in_flight").is_none());
        assert!(persisted.get("last_turn_started_at").is_none());
        let jobs = FileJobRegistry::new(&service.runtime_root)
            .recover()
            .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].owner, format!("chat:{}", session.id));
        assert_eq!(jobs[0].state, JobState::Succeeded);

        let read = service.read(&session.id).unwrap();
        assert!(!read.in_flight);
        assert_eq!(read.provider_session_id.as_deref(), Some("prov-1"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_recovers_stale_in_flight_turns() {
        let temp_root = unique_temp_dir("chat-recovery");
        let durable_root = temp_root.join(".refine");
        let service = FileChatService::new(&durable_root);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();
        let registry = FileJobRegistry::new(&service.runtime_root);
        let job = registry.register(&format!("chat:{}", session.id)).unwrap();

        let recovered = service
            .recover_interrupted_turns("daemon restarted during provider turn")
            .unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            registry.status(&job.id).unwrap().state,
            JobState::Interrupted
        );
        let resumed = service.resume(&session.id).unwrap();
        assert!(!resumed.in_flight);
        assert_eq!(resumed.last_turn_started_at, None);
        assert!(resumed.interrupted);
        assert_eq!(
            resumed.interruption_detail.as_deref(),
            Some("daemon restarted during provider turn")
        );
        assert!(resumed.transcript_events.iter().any(|event| {
            event_text(event)
                .as_deref()
                .unwrap_or("")
                .contains("daemon restarted")
        }));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_resumes_provider_session_when_supported() {
        let temp_root = unique_temp_dir("chat-provider-resume");
        let durable_root = temp_root.join(".refine");
        write_fake_provider(
            &durable_root,
            "claude",
            0,
            r#"{"session_id":"prov-2","item":{"type":"agent_message","text":"resumed ok"}}"#,
        );
        let service = FileChatService::new(&durable_root);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("claude"), Some("chat"))
            .unwrap();
        let mut record = service.load_record(&session.id).unwrap();
        record.provider_session_id = Some("prov-1".to_string());
        record.interrupted = true;
        record.interruption_detail = Some("daemon restarted".to_string());
        service.write_record(&record).unwrap();

        let resumed = service.resume_provider_turn(&session.id).unwrap();
        assert_eq!(resumed.provider_session_id.as_deref(), Some("prov-2"));
        assert!(!resumed.in_flight);
        assert!(!resumed.interrupted);
        assert!(
            resumed
                .transcript_events
                .iter()
                .any(|event| event_text(event).as_deref() == Some("resumed ok"))
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    fn write_fake_provider(durable_root: &PathBuf, name: &str, exit_code: i32, output: &str) {
        let bin_dir = durable_root.join("provider-bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' {output:?}\nexit {exit_code}"
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }
}
