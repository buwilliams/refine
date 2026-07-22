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
use crate::workflow::WorkflowEngine;
use crate::workflow::capacity::{AgentCapacityRequest, AgentCapacityService};

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

impl FileChatService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        let refine_dir = refine_dir.into();
        let runtime_root = default_chat_runtime_root(&refine_dir);
        Self {
            refine_dir,
            runtime_root,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn start_with_options(
        &self,
        attachment: ChatAttachment,
        provider: Option<&str>,
        mode: Option<&str>,
    ) -> RefineResult<ChatSessionRecord> {
        if matches!(attachment, ChatAttachment::Standalone) {
            return self.start_standalone_with_options(provider, mode);
        }
        self.start_record_with_options(attachment, provider, mode)
    }

    fn start_record_with_options(
        &self,
        attachment: ChatAttachment,
        provider: Option<&str>,
        mode: Option<&str>,
    ) -> RefineResult<ChatSessionRecord> {
        let now = now_timestamp();
        let attachment_mode = match &attachment {
            ChatAttachment::Goal(_) => "goal",
            ChatAttachment::Feature(_) => "feature",
            ChatAttachment::Supervisor => "supervisor",
            ChatAttachment::Standalone => "standalone",
        };
        let record = ChatSessionRecord {
            id: new_chat_id(),
            mode: mode.unwrap_or(attachment_mode).trim().to_string(),
            provider: provider.unwrap_or("claude").trim().to_string(),
            provider_session_id: None,
            attachment,
            worktree: None,
            created_at: now.clone(),
            updated_at: now,
            transcript_events: Vec::new(),
            queued_messages: Vec::new(),
            queue_dispatching: false,
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
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        let lines = unread_lines(&record);
        let progress_lines = unread_progress(&record);
        if !lines.is_empty() || !progress_lines.is_empty() {
            for event in &mut record.transcript_events {
                event.insert("delivered".to_string(), Value::Bool(true));
            }
            self.write_record(&record)?;
        }
        let active_operation = self.session_has_active_operation(&record.id)?;
        Ok(ChatReadResult {
            alive: !record.closed,
            session_id: record.id.clone(),
            lines,
            progress_lines,
            queued_messages: record.queued_messages.clone(),
            importable_artifacts: record.importable_artifacts.clone(),
            closed_reason: record.interruption_detail.clone(),
            in_flight: record.in_flight || record.queue_dispatching || active_operation,
            provider_session_id: record.provider_session_id.clone(),
            worktree: record.worktree.clone(),
        })
    }

    pub fn attach_worktree(
        &self,
        session_id: &str,
        worktree: ChatSessionWorktree,
    ) -> RefineResult<ChatSessionRecord> {
        let mut record = self.load_record(session_id)?;
        record.worktree = Some(worktree);
        record.updated_at = now_timestamp();
        self.write_record(&record)?;
        Ok(record)
    }

    pub fn mark_worktree_submitted(
        &self,
        session_id: &str,
        goal_id: &str,
    ) -> RefineResult<ChatSessionRecord> {
        let mut record = self.load_record(session_id)?;
        let Some(worktree) = record.worktree.as_mut() else {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} has no standalone worktree"
            )));
        };
        worktree.submitted_goal_id = Some(goal_id.to_string());
        record.updated_at = now_timestamp();
        self.write_record(&record)?;
        Ok(record)
    }

    pub fn stop(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        self.interrupt(session_id, "stopped")
    }

    pub fn migrate_supervisor_provider(
        &self,
        session_id: &str,
        configured_provider: &str,
    ) -> RefineResult<Option<ChatSessionRecord>> {
        let configured_provider = configured_provider.trim();
        if configured_provider.is_empty() {
            return Err(RefineError::InvalidInput(
                "configured agent_cli provider is required".to_string(),
            ));
        }
        let existing = self.load_record(session_id)?;
        if !matches!(existing.attachment, ChatAttachment::Supervisor) {
            return Err(RefineError::InvalidInput(
                "only Supervisor sessions can migrate configured providers".to_string(),
            ));
        }
        if existing.provider == configured_provider {
            return Ok(Some(existing));
        }
        if existing.in_flight
            || existing.queue_dispatching
            || self.session_has_active_operation(session_id)?
            || self.session_has_managed_process(session_id)?
        {
            self.interrupt(
                session_id,
                &format!(
                    "closed because configured agent_cli changed from {} to {configured_provider}",
                    existing.provider
                ),
            )?;
            return Ok(None);
        }

        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        let previous = std::mem::replace(&mut record.provider, configured_provider.to_string());
        record.provider_session_id = None;
        record.interrupted = false;
        record.interruption_detail = None;
        record.updated_at = now_timestamp();
        record.transcript_events.push(chat_event(
            "system",
            &format!(
                "Supervisor provider migrated from {previous} to {configured_provider}; provider-session resume state was reset."
            ),
            false,
            None,
            Some(json!({"source": "configured_agent_cli"})),
        ));
        self.write_record(&record)?;
        Ok(Some(record))
    }

    pub fn start_standalone_with_options(
        &self,
        provider: Option<&str>,
        mode: Option<&str>,
    ) -> RefineResult<ChatSessionRecord> {
        let mut session =
            self.start_record_with_options(ChatAttachment::Standalone, provider, mode)?;
        match self
            .create_standalone_worktree(&session.id)
            .and_then(|worktree| self.attach_worktree(&session.id, worktree))
        {
            Ok(updated) => {
                session = updated;
                Ok(session)
            }
            Err(error) => {
                let _ = self.interrupt(&session.id, "standalone worktree setup failed");
                Err(error)
            }
        }
    }

    pub fn stop_with_standalone_cleanup(
        &self,
        session_id: &str,
    ) -> RefineResult<ChatSessionRecord> {
        let existing = self.load_record(session_id)?;
        if matches!(existing.attachment, ChatAttachment::Standalone)
            && existing
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.submitted_goal_id.as_deref())
                .is_none()
            && let Some(worktree) = existing.worktree.as_ref()
        {
            self.cleanup_standalone_worktree(worktree)?;
        }
        self.stop(session_id)
    }

    pub fn submit_standalone_ready_merge(
        &self,
        session_id: &str,
        request: StandaloneReadyMergeRequest,
    ) -> RefineResult<StandaloneReadyMergeResult> {
        let session = self.load_record(session_id)?;
        if !matches!(session.attachment, ChatAttachment::Standalone) {
            return Err(RefineError::InvalidInput(
                "only standalone chat sessions can be submitted for merge".to_string(),
            ));
        }
        if session.closed {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} is closed"
            )));
        }
        let read_state = self.read(session_id)?;
        if session.in_flight
            || session.queue_dispatching
            || !session.queued_messages.is_empty()
            || read_state.in_flight
            || !read_state.queued_messages.is_empty()
        {
            return Err(RefineError::Conflict(
                "wait for the standalone chat to finish before submitting for merge".to_string(),
            ));
        }
        let Some(worktree) = session.worktree.clone() else {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} has no standalone worktree"
            )));
        };
        if worktree.submitted_goal_id.is_some() {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} was already submitted"
            )));
        }

        let prompt = request.prompt.trim();
        let reporter = request.reporter.trim();
        let priority = request.priority.trim();
        let name = request
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| derive_standalone_goal_name(prompt))
            .ok_or_else(|| {
                RefineError::InvalidInput("body.name or body.prompt is required".to_string())
            })?;
        if reporter.is_empty() || prompt.is_empty() {
            return Err(RefineError::InvalidInput(
                "reporter and prompt are required".to_string(),
            ));
        }
        if !matches!(priority, "low" | "medium" | "high") {
            return Err(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }

        let settings =
            FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root).load()?;
        let target_branch = settings
            .get("merge_target_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("main");
        let worktree_git =
            FileGitWorktreeService::with_runtime_root(&worktree.path, &self.runtime_root);
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            self.runtime_root.join("cache"),
        );
        let goal = work_items.create_goal_summary(&name, None)?;
        let goal_id = goal.goal.id.clone();
        let submit_result = (|| -> RefineResult<GoalSummaryProjection> {
            work_items.append_goal_round_summary(&goal_id, reporter, prompt)?;
            if priority != "low" {
                work_items.update_goal_metadata_summary(
                    &goal_id,
                    None,
                    Some(priority),
                    None,
                    None,
                )?;
            }
            with_repository_git_lock(&target_root, || {
                match worktree_git.commit(&format!("Submit {goal_id} from standalone chat"), &[]) {
                    Ok(_) => {}
                    Err(error) => {
                        if !worktree_git.has_commits_since(target_branch)? {
                            return Err(error);
                        }
                    }
                }
                Ok(())
            })?;
            work_items.set_goal_branch_name(&goal_id, &worktree.branch)?;
            work_items.transition_goal_status(&goal_id, GoalStatus::Todo)?;
            work_items.advance_automated_goal_status(&goal_id, GoalStatus::InProgress)?;
            let goal =
                work_items.advance_automated_goal_status(&goal_id, GoalStatus::ReadyMerge)?;
            self.mark_worktree_submitted(session_id, &goal_id)?;
            self.interrupt(session_id, "submitted for ready-merge")?;
            Ok(goal)
        })();
        match submit_result {
            Ok(goal) => Ok(StandaloneReadyMergeResult { goal, worktree }),
            Err(error) => {
                let _ = work_items.delete_goal_record(&goal_id);
                Err(error)
            }
        }
    }

    pub fn list_sessions(&self) -> RefineResult<Vec<ChatSessionRecord>> {
        let sessions_dir = self.sessions_dir();
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
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
            let session = serde_json::from_str::<ChatSessionRecord>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse chat session {}: {error}",
                    entry.path().display()
                ))
            })?;
            sessions.push(session);
        }
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(sessions)
    }

    pub fn transcript_text(&self, session_id: &str) -> RefineResult<String> {
        let record = self.load_record(session_id)?;
        Ok(record
            .transcript_events
            .iter()
            .filter(|event| !event_bool(event, "progress"))
            .filter_map(event_text)
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n"))
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
        write_chat_record_atomically(&path, format!("{encoded}\n").as_bytes()).map_err(|error| {
            RefineError::Io(format!(
                "failed to write chat session {}: {error}",
                path.display()
            ))
        })
    }

    fn sessions_dir(&self) -> PathBuf {
        self.refine_dir.join("chat/sessions")
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{session_id}.json"))
    }

    fn acquire_session_lock(&self, session_id: &str) -> RefineResult<ChatSessionLock> {
        validate_session_id(session_id)?;
        fs::create_dir_all(self.sessions_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create chat sessions directory {}: {error}",
                self.sessions_dir().display()
            ))
        })?;
        let path = self.sessions_dir().join(format!(".{session_id}.lock"));
        for _ in 0..500 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(ChatSessionLock { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(30));
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to lock chat session {session_id}: {error}"
                    )));
                }
            }
        }
        Err(RefineError::Conflict(format!(
            "Chat session {session_id} is busy; retry shortly"
        )))
    }

    fn create_standalone_worktree(&self, session_id: &str) -> RefineResult<ChatSessionWorktree> {
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let branch = format!("refine/standalone/{session_id}");
        let git = FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
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
        let path =
            with_repository_git_lock(&target_root, || git.ensure_worktree(&branch, &target))?;
        Ok(ChatSessionWorktree {
            branch,
            path,
            submitted_goal_id: None,
        })
    }

    fn cleanup_standalone_worktree(&self, worktree: &ChatSessionWorktree) -> RefineResult<()> {
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let git = FileGitWorktreeService::new(&target_root);
        let path = PathBuf::from(&worktree.path);
        with_repository_git_lock(&target_root, || {
            if path.exists() {
                git.remove_worktree(&path, true)?;
            }
            let _ = git.delete_branch(&worktree.branch, true);
            Ok(())
        })
    }

    pub fn resume_provider_turn(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        let capacity = self
            .try_turn_capacity(&self.load_record(session_id)?)?
            .ok_or_else(|| {
                RefineError::Conflict("automation concurrency limit reached".to_string())
            })?;
        let (record, provider_session_id) = {
            let _guard = self.acquire_session_lock(session_id)?;
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
            (record, provider_session_id)
        };

        let operation = self.register_provider_operation(&record, "resume")?;
        let provider = HostAgentProviderService {
            path_override: self.provider_path_override(),
            runtime_root: Some(self.runtime_root.join("agents")),
        };
        let provider_name = record.provider.clone();
        let result = provider.resume_detailed_with_output_and_metadata(
            &provider_name,
            &provider_session_id,
            chat_process_metadata(&record),
            |line| {
                let _ = self.append_provider_activity_progress(session_id, &line);
            },
        );
        let _guard = self.acquire_session_lock(session_id)?;
        let mut latest = self.load_record(session_id)?;
        let mut provider_failure = None;
        if latest.closed {
            latest.in_flight = false;
            latest.queue_dispatching = false;
            latest.last_turn_started_at = None;
            latest.transcript_events.push(chat_event(
                "progress",
                "Managed provider process exited after cancellation.",
                true,
                latest.provider_session_id.clone(),
                Some(json!({"source": "process_supervisor"})),
            ));
            self.finish_provider_operation(
                &operation.id,
                OperationState::Cancelled,
                "Provider session resume cancelled after managed process exit",
            )?;
        } else {
            match result {
                Ok(result) => {
                    self.apply_provider_success(&mut latest, result, "Provider session resumed.");
                    self.finish_provider_operation(
                        &operation.id,
                        OperationState::Succeeded,
                        "Provider session resumed",
                    )?;
                }
                Err(error) => {
                    let detail =
                        format!("Provider session resume failed; transcript preserved: {error}");
                    provider_failure = Some(detail.clone());
                    self.apply_provider_failure(&mut latest, detail);
                    self.finish_provider_operation(
                        &operation.id,
                        OperationState::Failed,
                        "Provider session resume failed",
                    )?;
                }
            }
        }
        latest.updated_at = now_timestamp();
        self.write_record(&latest)?;
        drop(_guard);
        self.record_supervisor_provider_outcome(&latest, provider_failure.as_deref());
        drop(capacity);
        Ok(latest)
    }

    pub fn recover_interrupted_turns(&self, detail: &str) -> RefineResult<Vec<ChatSessionRecord>> {
        let message = detail.trim();
        let registry = self.operation_registry();
        let mut recovered_session_ids = Vec::new();
        for operation in registry.recover()? {
            let Some(session_id) = chat_session_id_from_operation(&operation) else {
                continue;
            };
            if !matches!(
                operation.state,
                OperationState::Pending
                    | OperationState::Running
                    | OperationState::Cancelling
                    | OperationState::Interrupted
            ) {
                continue;
            }
            let mut record = self.load_record(session_id)?;
            if record.interrupted && record.interruption_detail.as_deref() == Some(message) {
                continue;
            }
            self.mark_record_interrupted(&mut record, message);
            self.write_record(&record)?;
            if !matches!(operation.state, OperationState::Interrupted) {
                registry.finish(&operation.id, OperationState::Interrupted)?;
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
            if !record.in_flight && !record.queue_dispatching {
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
        record.interruption_detail = Some(detail.clone());
    }

    fn record_supervisor_provider_outcome(
        &self,
        record: &ChatSessionRecord,
        failure: Option<&str>,
    ) {
        if !matches!(record.attachment, ChatAttachment::Supervisor) {
            return;
        }
        let service = crate::tools::product::supervisor_agent::FileSupervisorAgentService::new(
            &self.refine_dir,
            &self.runtime_root,
        );
        if let Some(detail) = failure {
            let _ = service.record_failure(
                "provider",
                format!("Supervisor provider failure: {detail}"),
                true,
            );
        } else {
            let _ = service.record_recovery(
                "provider",
                "Supervisor provider turn",
                "completed and preserved in the shared transcript",
                true,
            );
        }
    }

    fn append_provider_activity_progress(&self, session_id: &str, line: &str) -> RefineResult<()> {
        let text = line.trim();
        if text.is_empty() {
            return Ok(());
        }
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        let duplicate = record.transcript_events.iter().rev().take(20).any(|event| {
            event_bool(event, "progress") && event_text(event).as_deref() == Some(text)
        });
        if duplicate {
            return Ok(());
        }
        record.transcript_events.push(chat_event(
            "progress",
            text,
            true,
            record.provider_session_id.clone(),
            Some(json!({"source": "provider_output"})),
        ));
        record.updated_at = now_timestamp();
        self.write_record(&record)
    }

    fn mark_record_interrupted(&self, record: &mut ChatSessionRecord, detail: &str) {
        record.in_flight = false;
        record.queue_dispatching = false;
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

    fn operation_registry(&self) -> FileOperationRegistry {
        FileOperationRegistry::new(&self.runtime_root)
    }

    fn try_turn_capacity(
        &self,
        record: &ChatSessionRecord,
    ) -> RefineResult<Option<ChatCapacityPermit>> {
        if !matches!(record.attachment, ChatAttachment::Supervisor) {
            return Ok(Some(ChatCapacityPermit::unlimited()));
        }
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let policy = WorkflowEngine::with_target_root(&self.runtime_root, &target_root).policy()?;
        if record.provider != policy.provider {
            return Err(RefineError::Conflict(format!(
                "Supervisor session provider {} does not match configured agent_cli {}; migrate the session before dispatch",
                record.provider, policy.provider
            )));
        }
        if self.other_supervisor_process_is_running(&record.id)? {
            return Ok(None);
        }
        let service = AgentCapacityService::new(&self.runtime_root);
        let owner_id = format!("supervisor:{}", record.id);
        let acquired = service.try_acquire(
            &policy,
            AgentCapacityRequest {
                owner_id: owner_id.clone(),
                role: "supervisor".to_string(),
                node_id: policy.active_node_id.clone(),
                provider: policy.provider.clone(),
                target_app_id: policy.target_app_id.clone(),
            },
        )?;
        Ok(acquired.then(|| ChatCapacityPermit {
            service: Some(service),
            owner_id,
        }))
    }

    fn register_provider_operation(
        &self,
        record: &ChatSessionRecord,
        operation_kind: &str,
    ) -> RefineResult<OperationHandle> {
        let registry = self.operation_registry();
        let operation = registry.register(&format!("chat:{}", record.id))?;
        let mut details = JsonObject::new();
        details.insert("session_id".to_string(), json!(record.id));
        details.insert("provider".to_string(), json!(record.provider));
        details.insert("mode".to_string(), json!(record.mode));
        details.insert("operation".to_string(), json!(operation_kind));
        registry.append_log(
            &operation.id,
            chat_operation_log("info", "Chat provider operation started", Some(details)),
        )?;
        Ok(operation)
    }

    fn finish_provider_operation(
        &self,
        operation_id: &str,
        state: OperationState,
        message: &str,
    ) -> RefineResult<OperationHandle> {
        let registry = self.operation_registry();
        registry.append_log(operation_id, chat_operation_log("info", message, None))?;
        registry.finish(operation_id, state)
    }

    fn session_has_active_operation(&self, session_id: &str) -> RefineResult<bool> {
        Ok(self
            .operation_registry()
            .recover()?
            .into_iter()
            .any(|operation| {
                chat_session_id_from_operation(&operation) == Some(session_id)
                    && matches!(
                        operation.state,
                        OperationState::Pending
                            | OperationState::Running
                            | OperationState::Cancelling
                    )
            }))
    }

    fn managed_process_roots(&self) -> [PathBuf; 2] {
        [self.runtime_root.join("agents"), self.runtime_root.clone()]
    }

    fn session_managed_processes(&self, session_id: &str) -> RefineResult<Vec<(PathBuf, String)>> {
        let mut matches = Vec::new();
        for root in self.managed_process_roots() {
            for process in FileProcessSupervisor::new(&root).list()? {
                let belongs_to_session = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                    .and_then(|details| {
                        details
                            .get("session_id")
                            .and_then(Value::as_str)
                            .map(|value| value == session_id)
                    })
                    .unwrap_or(false);
                if belongs_to_session {
                    matches.push((root.clone(), process.id));
                }
            }
        }
        Ok(matches)
    }

    fn session_has_managed_process(&self, session_id: &str) -> RefineResult<bool> {
        Ok(!self.session_managed_processes(session_id)?.is_empty())
    }

    fn other_supervisor_process_is_running(&self, session_id: &str) -> RefineResult<bool> {
        for root in self.managed_process_roots() {
            for process in FileProcessSupervisor::new(root).list()? {
                let details = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok());
                let is_supervisor = details.as_ref().is_some_and(|details| {
                    details.get("agent_role").and_then(Value::as_str) == Some("supervisor")
                        || details.get("mode").and_then(Value::as_str) == Some("supervisor")
                });
                let is_current = details
                    .as_ref()
                    .and_then(|details| details.get("session_id"))
                    .and_then(Value::as_str)
                    == Some(session_id);
                if is_supervisor && !is_current {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn request_session_process_termination(&self, session_id: &str) -> RefineResult<usize> {
        let processes = self.session_managed_processes(session_id)?;
        for (root, process_id) in &processes {
            match FileProcessSupervisor::new(root).request_termination(process_id, "terminate") {
                Ok(_) | Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(processes.len())
    }

    fn ensure_queue_dispatch(&self, record: &mut ChatSessionRecord) -> RefineResult<()> {
        if record.closed || record.queued_messages.is_empty() || record.queue_dispatching {
            return Ok(());
        }
        record.queue_dispatching = true;
        record.in_flight = true;
        record.last_turn_started_at = Some(now_timestamp());
        record.updated_at = now_timestamp();
        self.write_record(record)?;
        let service = self.clone();
        let session_id = record.id.clone();
        thread::spawn(move || {
            if let Err(error) = service.dispatch_queued_messages(&session_id) {
                let _ = service.mark_dispatch_failure(&session_id, &format!("{error}"));
            }
        });
        Ok(())
    }

    fn dispatch_queued_messages(&self, session_id: &str) -> RefineResult<()> {
        loop {
            let capacity = loop {
                let record = {
                    let _guard = self.acquire_session_lock(session_id)?;
                    let mut record = self.load_record(session_id)?;
                    if record.closed || record.queued_messages.is_empty() {
                        record.queue_dispatching = false;
                        record.in_flight = false;
                        record.last_turn_started_at = None;
                        record.updated_at = now_timestamp();
                        self.write_record(&record)?;
                        return Ok(());
                    }
                    record
                };
                if let Some(permit) = self.try_turn_capacity(&record)? {
                    break permit;
                }
                self.append_capacity_wait_progress(session_id)?;
                thread::sleep(Duration::from_millis(100));
            };
            let (record, message) = {
                let _guard = self.acquire_session_lock(session_id)?;
                let mut record = self.load_record(session_id)?;
                if record.closed || record.queued_messages.is_empty() {
                    record.queue_dispatching = false;
                    record.in_flight = false;
                    record.last_turn_started_at = None;
                    record.updated_at = now_timestamp();
                    self.write_record(&record)?;
                    return Ok(());
                }
                let queued = std::mem::take(&mut record.queued_messages);
                let message = combined_queued_message(&queued);
                record.transcript_events.push(chat_event(
                    "user",
                    &message,
                    false,
                    record.provider_session_id.clone(),
                    None,
                ));
                record.transcript_events.push(chat_event(
                    "progress",
                    &format!(
                        "Sent {} queued message{} to the provider.",
                        queued.len(),
                        if queued.len() == 1 { "" } else { "s" }
                    ),
                    true,
                    record.provider_session_id.clone(),
                    None,
                ));
                record.in_flight = true;
                record.last_turn_started_at = Some(now_timestamp());
                record.updated_at = now_timestamp();
                self.write_record(&record)?;
                (record, message)
            };

            let operation = self.register_provider_operation(&record, "invoke")?;
            let provider = HostAgentProviderService {
                path_override: self.provider_path_override(),
                runtime_root: Some(self.runtime_root.join("agents")),
            };
            let result = provider.invoke_detailed_with_output(
                ProviderInvocation {
                    provider: record.provider.clone(),
                    prompt: self.chat_prompt(&record, &message),
                    session_id: record.provider_session_id.clone(),
                    cwd: Some(self.chat_cwd(&record).display().to_string()),
                    process_metadata: chat_process_metadata(&record),
                },
                |line| {
                    let _ = self.append_provider_activity_progress(session_id, &line);
                },
            );
            let _guard = self.acquire_session_lock(session_id)?;
            let mut latest = self.load_record(session_id)?;
            let mut provider_failure = None;
            if latest.closed {
                latest.in_flight = false;
                latest.queue_dispatching = false;
                latest.last_turn_started_at = None;
                latest.transcript_events.push(chat_event(
                    "progress",
                    "Managed provider process exited after cancellation.",
                    true,
                    latest.provider_session_id.clone(),
                    Some(json!({"source": "process_supervisor"})),
                ));
                self.finish_provider_operation(
                    &operation.id,
                    OperationState::Cancelled,
                    "Provider turn cancelled after managed process exit",
                )?;
            } else {
                match result {
                    Ok(result) => {
                        self.apply_provider_success(
                            &mut latest,
                            result,
                            "Provider turn completed.",
                        );
                        self.finish_provider_operation(
                            &operation.id,
                            OperationState::Succeeded,
                            "Provider turn completed",
                        )?;
                    }
                    Err(error) => {
                        let detail = format!("Provider turn failed: {error}");
                        provider_failure = Some(detail.clone());
                        self.apply_provider_failure(&mut latest, detail);
                        self.finish_provider_operation(
                            &operation.id,
                            OperationState::Failed,
                            "Provider turn failed",
                        )?;
                    }
                }
            }
            latest.updated_at = now_timestamp();
            self.write_record(&latest)?;
            drop(_guard);
            self.record_supervisor_provider_outcome(&latest, provider_failure.as_deref());
            drop(capacity);
        }
    }

    fn append_capacity_wait_progress(&self, session_id: &str) -> RefineResult<()> {
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        let message = "Queued; waiting for shared agent capacity.";
        let already_reported = record.transcript_events.iter().rev().take(10).any(|event| {
            event_bool(event, "progress") && event_text(event).as_deref() == Some(message)
        });
        if !already_reported {
            record.transcript_events.push(chat_event(
                "progress",
                message,
                true,
                record.provider_session_id.clone(),
                Some(json!({"source": "agent_capacity"})),
            ));
            record.updated_at = now_timestamp();
            self.write_record(&record)?;
        }
        Ok(())
    }

    fn mark_dispatch_failure(&self, session_id: &str, detail: &str) -> RefineResult<()> {
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        record.queue_dispatching = false;
        record.in_flight = false;
        record.last_turn_started_at = None;
        record.interrupted = true;
        record.interruption_detail = Some(detail.to_string());
        record.updated_at = now_timestamp();
        record
            .transcript_events
            .push(chat_event("system", detail, false, None, None));
        self.write_record(&record)
    }

    pub fn update_queued_message(
        &self,
        session_id: &str,
        message_id: &str,
        text: &str,
    ) -> RefineResult<ChatSessionRecord> {
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(RefineError::InvalidInput("text is required".to_string()));
        }
        let Some(message) = record
            .queued_messages
            .iter_mut()
            .find(|message| message.id == message_id)
        else {
            return Err(RefineError::NotFound(format!(
                "Queued chat message {message_id} was not found"
            )));
        };
        message.text = text.to_string();
        message.updated_at = now_timestamp();
        record.updated_at = now_timestamp();
        self.write_record(&record)?;
        Ok(record)
    }

    pub fn remove_queued_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> RefineResult<ChatSessionRecord> {
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        let before = record.queued_messages.len();
        record
            .queued_messages
            .retain(|message| message.id != message_id);
        if record.queued_messages.len() == before {
            return Err(RefineError::NotFound(format!(
                "Queued chat message {message_id} was not found"
            )));
        }
        record.updated_at = now_timestamp();
        self.write_record(&record)?;
        Ok(record)
    }
}

struct ChatSessionLock {
    path: PathBuf,
}

struct ChatCapacityPermit {
    service: Option<AgentCapacityService>,
    owner_id: String,
}

impl ChatCapacityPermit {
    fn unlimited() -> Self {
        Self {
            service: None,
            owner_id: String::new(),
        }
    }
}

impl Drop for ChatCapacityPermit {
    fn drop(&mut self) {
        if let Some(service) = &self.service {
            let _ = service.release(&self.owner_id);
        }
    }
}

impl Drop for ChatSessionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl ChatService for FileChatService {
    fn start(&self, attachment: ChatAttachment) -> RefineResult<ChatSessionRecord> {
        self.start_with_options(attachment, None, None)
    }

    fn resume(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        self.load_record(session_id)
    }

    fn append_user_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> RefineResult<ChatSessionRecord> {
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        if record.closed {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} is closed"
            )));
        }
        let text = message.trim();
        if text.is_empty() {
            return Err(RefineError::InvalidInput("text is required".to_string()));
        }
        let now = now_timestamp();
        record.queued_messages.push(ChatQueuedMessage {
            id: new_queued_message_id(),
            text: text.to_string(),
            created_at: now.clone(),
            updated_at: now,
        });
        record.updated_at = now_timestamp();
        self.write_record(&record)?;
        self.ensure_queue_dispatch(&mut record)?;
        self.load_record(session_id)
    }

    fn interrupt(&self, session_id: &str, detail: &str) -> RefineResult<ChatSessionRecord> {
        self.request_session_process_termination(session_id)?;
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        record.closed = true;
        record.interrupted = true;
        record.interruption_detail = Some(detail.trim().to_string());
        record.queue_dispatching = false;
        record.queued_messages.clear();
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
                let supervisor =
                    crate::tools::product::supervisor_agent::FileSupervisorAgentService::new(
                        &self.refine_dir,
                        &self.runtime_root,
                    )
                    .snapshot()?;
                serde_json::to_string_pretty(&json!({
                    "type": "supervisor",
                    "supervisor_agent": supervisor,
                    "workflow": snapshot.runtime,
                }))
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
        ChatAttachment::Supervisor => PromptTemplate::ChatSupervisor,
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

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

fn is_false(value: &bool) -> bool {
    !*value
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

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::tools::product::work_items::FileWorkItemService;
    use crate::workflow::{WorkflowAutomation, WorkflowClaimState};

    #[test]
    fn file_chat_service_persists_session_transcript_and_stop() {
        let temp_root = unique_temp_dir("chat");
        let refine_dir = temp_root.join(".refine");
        write_fake_provider(&refine_dir, "smoke-ai", 0, "provider says hello");
        let service = FileChatService::new(&refine_dir);

        let session = service
            .start_with_options(
                ChatAttachment::Goal("GOAL1".to_string()),
                Some("smoke-ai"),
                Some("goal"),
            )
            .unwrap();
        assert_eq!(session.mode, "goal");
        assert_eq!(session.provider, "smoke-ai");

        service
            .append_user_message(&session.id, "What should I test?")
            .unwrap();
        let queued = service.read(&session.id).unwrap();
        assert!(queued.in_flight || !queued.queued_messages.is_empty());
        let streamed = wait_for_chat_line(&service, &session.id, "provider says hello");
        assert!(streamed.alive);
        assert!(
            streamed
                .lines
                .iter()
                .any(|line| line.contains("provider says hello"))
                || streamed
                    .progress_lines
                    .iter()
                    .any(|line| line.contains("provider says hello"))
        );
        let record = wait_for_chat_record(&service, &session.id, |record| {
            record.transcript_events.iter().any(|event| {
                event
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|line| line.contains("Provider turn completed"))
            })
        });
        let read = service.read(&session.id).unwrap();
        assert!(read.alive);
        assert!(record.transcript_events.iter().any(|event| {
            event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|line| line.contains("What should I test?"))
        }));
        assert!(record.transcript_events.iter().any(|event| {
            event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|line| line.contains("Provider turn completed"))
        }));
        let stopped = service.stop(&session.id).unwrap();
        assert!(stopped.closed);
        assert!(!service.read(&session.id).unwrap().alive);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_streams_provider_output_into_progress() {
        let temp_root = unique_temp_dir("chat-provider-stream");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        write_fake_provider_script(
            &refine_dir,
            "claude",
            "#!/bin/sh\nprintf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"streamed activity line\"}}'\nsleep 1\nprintf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"final response line\"}}'\n",
        );
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("claude"), Some("chat"))
            .unwrap();

        service.append_user_message(&session.id, "hello").unwrap();
        let streamed = wait_for_chat_read(&service, &session.id, |read| {
            read.in_flight
                && read
                    .progress_lines
                    .iter()
                    .any(|line| line.contains("streamed activity line"))
        });
        assert!(
            streamed
                .progress_lines
                .iter()
                .any(|line| line.contains("streamed activity line"))
        );
        let completed = wait_for_chat_read(&service, &session.id, |read| !read.in_flight);
        assert!(!completed.in_flight);
        let record = wait_for_chat_record(&service, &session.id, |record| {
            record.transcript_events.iter().any(|event| {
                event
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|line| line.contains("final response line"))
            })
        });
        assert!(record.transcript_events.iter().any(|event| {
            event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|line| line.contains("final response line"))
        }));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_rebuilds_attached_goal_context_from_refine_records() {
        let temp_root = unique_temp_dir("chat-goal-context");
        let refine_dir = temp_root.join(".refine");
        FileWorkItemService::new(&refine_dir)
            .create_goal_summary("Checkout fails", Some("GOAL1"))
            .unwrap();
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(
                ChatAttachment::Goal("GOAL1".to_string()),
                Some("smoke-ai"),
                Some("goal"),
            )
            .unwrap();

        let prompt = service.chat_prompt(&session, "What changed?");
        assert!(prompt.contains("Current refine context"));
        assert!(prompt.contains("\"id\": \"GOAL1\""));
        assert!(prompt.contains("\"name\": \"Checkout fails\""));
        assert!(prompt.contains("What changed?"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_plan_prompt_drafts_software_specs() {
        let temp_root = unique_temp_dir("chat-plan-prompt");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("plan"))
            .unwrap();

        let prompt = service.chat_prompt(&session, "Plan authentication cleanup.");
        assert!(prompt.contains("Plan Mode drafts the whole picture of the software"));
        assert!(prompt.contains("product spec"));
        assert!(prompt.contains("software purpose"));
        assert!(prompt.contains("audience"));
        assert!(prompt.contains("success criteria"));
        assert!(prompt.contains("constraints"));
        assert!(prompt.contains("surfaces"));
        assert!(prompt.contains("technical work"));
        assert!(prompt.contains("architecture lenses"));
        assert!(prompt.contains("durable state"));
        assert!(prompt.contains("logic and code organization"));
        assert!(prompt.contains("do not force a fixed checklist"));
        assert!(prompt.contains("natural build order"));
        assert!(prompt.contains("test or verification work"));
        assert!(prompt.contains("Draft Feature or Draft Goal action"));
        assert!(prompt.contains("implementation-ready work"));
        assert!(prompt.contains("Do not reduce the answer to generic strategy"));
        assert!(!prompt.contains("highest-leverage"));
        assert!(prompt.contains("Plan authentication cleanup."));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_starts_plan_mode_for_unborn_project_repo() {
        let temp_root = unique_temp_dir("chat-plan-unborn");
        init_unborn_git_app(&temp_root);
        fs::write(temp_root.join("draft.txt"), "local draft\n").unwrap();
        let refine_dir = temp_root.join(".refine");
        let service = FileChatService::new(&refine_dir);

        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("plan"))
            .unwrap();
        let worktree = PathBuf::from(session.worktree.as_ref().unwrap().path.clone());

        assert!(worktree.join(".git").exists());
        assert_eq!(
            git_stdout(&worktree, &["branch", "--show-current"]),
            session.worktree.as_ref().unwrap().branch
        );
        assert_eq!(
            git_stdout(&temp_root, &["log", "--pretty=%s", "-1"]),
            "Initialize Refine workspace"
        );
        assert!(!worktree.join("draft.txt").exists());
        assert!(temp_root.join("draft.txt").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_persists_importable_artifacts_from_provider_output() {
        let temp_root = unique_temp_dir("chat-artifacts");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        write_fake_provider(
            &refine_dir,
            "smoke-ai",
            0,
            r#"{"importable_artifacts":[{"type":"round","round":{"reporter":"QA","prompt": "Fixed"}},{"type":"goal","goal":{"name":"Imported goal","prompt": "B"}}]}"#,
        );
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();

        service
            .append_user_message(&session.id, "draft follow-up")
            .unwrap();
        let resumed = wait_for_chat_record(&service, &session.id, |record| {
            record.importable_artifacts.len() == 2
        });
        assert_eq!(resumed.importable_artifacts.len(), 2);
        assert_eq!(resumed.importable_artifacts[0]["type"], "round");
        assert_eq!(resumed.importable_artifacts[1]["type"], "goal");
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
    fn file_chat_service_runs_standalone_provider_turns_in_attached_worktree() {
        let temp_root = unique_temp_dir("chat-standalone-worktree-cwd");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        write_cwd_provider(&refine_dir, "smoke-ai");
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();
        let worktree = PathBuf::from(session.worktree.as_ref().unwrap().path.clone());

        service
            .append_user_message(&session.id, "write cwd marker")
            .unwrap();
        wait_for_chat_line(&service, &session.id, "cwd provider response");
        assert_eq!(
            fs::read_to_string(worktree.join("provider-cwd.txt")).unwrap(),
            format!("{}\n", worktree.display())
        );
        assert!(!temp_root.join("provider-cwd.txt").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_persists_provider_failure() {
        let temp_root = unique_temp_dir("chat-failure");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        write_fake_provider(&refine_dir, "smoke-ai", 2, "provider failed");
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();

        service.append_user_message(&session.id, "hello").unwrap();
        let resumed = wait_for_chat_record(&service, &session.id, |record| record.interrupted);
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
    fn supervisor_chat_provider_failure_is_shared_with_supervisor_state() {
        let temp_root = unique_temp_dir("supervisor-chat-failure");
        init_git_app(&temp_root);
        let refine_dir =
            crate::tools::host::project_layout::prepare_refine_dir(&temp_root).unwrap();
        write_fake_provider(&refine_dir, "smoke-ai", 2, "provider auth failed");
        let service = FileChatService::new(&refine_dir);
        FileSettingsService::with_active_root(&refine_dir, &service.runtime_root)
            .update(&json!({"agent_cli": "smoke-ai"}))
            .unwrap();
        let session = service
            .start_with_options(
                ChatAttachment::Supervisor,
                Some("smoke-ai"),
                Some("supervisor"),
            )
            .unwrap();

        service
            .append_user_message(&session.id, "investigate")
            .unwrap();
        wait_for_chat_record(&service, &session.id, |record| record.interrupted);
        let state = crate::tools::product::supervisor_agent::FileSupervisorAgentService::new(
            &refine_dir,
            &service.runtime_root,
        )
        .snapshot()
        .unwrap();
        assert_eq!(state.health, "degraded");
        assert!(state.events.iter().any(|event| {
            event.kind == "failure"
                && event.message.contains("provider auth failed")
                && event.actionable
        }));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn supervisor_and_workflow_share_capacity_and_preserve_the_queue() {
        let temp_root = unique_temp_dir("supervisor-workflow-capacity");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        write_fake_provider(&refine_dir, "smoke-ai", 0, "shared capacity response");
        FileSettingsService::with_active_root(&refine_dir, &runtime_root)
            .update(&json!({
                "agent_cli": "smoke-ai",
                "parallel_run_cap": "1",
                "parallel_per_node_cap": "1",
                "parallel_per_provider_cap": "1",
                "parallel_per_target_app_cap": "1"
            }))
            .unwrap();

        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Capacity one", Some("GOAL1"))
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        let workflow = WorkflowEngine::with_target_root(&runtime_root, &temp_root);
        let claim = workflow.claim("GOAL1").unwrap();
        let execution = workflow.start_claim(&claim).unwrap();

        let chat = FileChatService::with_runtime_root(&refine_dir, &runtime_root);
        let supervisor = chat
            .start_with_options(
                ChatAttachment::Supervisor,
                Some("smoke-ai"),
                Some("supervisor"),
            )
            .unwrap();
        chat.append_user_message(&supervisor.id, "wait durably")
            .unwrap();
        let waiting = wait_for_chat_record(&chat, &supervisor.id, |record| {
            !record.queued_messages.is_empty()
                && record.transcript_events.iter().any(|event| {
                    event
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains("waiting for shared agent capacity"))
                })
        });
        assert_eq!(waiting.queued_messages.len(), 1);
        assert!(
            AgentCapacityService::new(&runtime_root)
                .snapshot()
                .unwrap()
                .leases
                .iter()
                .all(|lease| lease.role == "workflow")
        );

        workflow.cancel(&execution).unwrap();
        let completed = wait_for_chat_record(&chat, &supervisor.id, |record| {
            record.queued_messages.is_empty()
                && !record.queue_dispatching
                && record.transcript_events.iter().any(|event| {
                    event
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains("shared capacity response"))
                })
        });
        assert!(completed.queued_messages.is_empty());

        FileSettingsService::with_active_root(&refine_dir, &runtime_root)
            .update(&json!({
                "parallel_run_cap": "2",
                "parallel_per_node_cap": "2",
                "parallel_per_provider_cap": "2",
                "parallel_per_target_app_cap": "2"
            }))
            .unwrap();
        work_items
            .create_goal_summary("Capacity two", Some("GOAL2"))
            .unwrap();
        work_items
            .transition_goal_status("GOAL2", GoalStatus::Todo)
            .unwrap();
        let claim = workflow.claim("GOAL2").unwrap();
        let execution = workflow.start_claim(&claim).unwrap();
        chat.append_user_message(&supervisor.id, "run concurrently")
            .unwrap();
        let concurrent = wait_for_chat_record(&chat, &supervisor.id, |record| {
            record.transcript_events.iter().any(|event| {
                event
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.contains("shared capacity response"))
            }) && record.queued_messages.is_empty()
                && !record.queue_dispatching
        });
        assert!(concurrent.queued_messages.is_empty());
        assert_eq!(
            workflow
                .load_state()
                .unwrap()
                .claims
                .iter()
                .find(|claim| claim.goal_id == "GOAL2")
                .unwrap()
                .state,
            WorkflowClaimState::Running
        );
        workflow.cancel(&execution).unwrap();
        assert!(
            AgentCapacityService::new(&runtime_root)
                .snapshot()
                .unwrap()
                .leases
                .is_empty()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_persists_provider_session_id_and_in_flight_lifecycle() {
        let temp_root = unique_temp_dir("chat-provider-session");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        write_fake_provider(
            &refine_dir,
            "smoke-ai",
            0,
            r#"{"session_id":"prov-1","item":{"type":"agent_message","text":"provider says hello"}}"#,
        );
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();

        service.append_user_message(&session.id, "hello").unwrap();
        let resumed = wait_for_chat_record(&service, &session.id, |record| {
            record.provider_session_id.as_deref() == Some("prov-1")
        });
        assert_eq!(resumed.provider_session_id.as_deref(), Some("prov-1"));
        assert!(!resumed.in_flight);
        assert_eq!(resumed.last_turn_started_at, None);
        assert!(!resumed.interrupted);
        let persisted: Value = serde_json::from_str(
            &fs::read_to_string(refine_dir.join(format!("chat/sessions/{}.json", session.id)))
                .unwrap(),
        )
        .unwrap();
        assert!(persisted.get("in_flight").is_none());
        assert!(persisted.get("last_turn_started_at").is_none());
        let operations = FileOperationRegistry::new(&service.runtime_root)
            .recover()
            .unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].owner, format!("chat:{}", session.id));
        assert_eq!(operations[0].state, OperationState::Succeeded);

        let read = service.read(&session.id).unwrap();
        assert!(!read.in_flight);
        assert_eq!(read.provider_session_id.as_deref(), Some("prov-1"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_edits_removes_and_batches_queued_messages() {
        let temp_root = unique_temp_dir("chat-queue");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        write_fake_provider(&refine_dir, "smoke-ai", 0, "queued provider response");
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();
        let mut busy = service.resume(&session.id).unwrap();
        busy.queue_dispatching = true;
        service.write_record(&busy).unwrap();

        let queued = service.append_user_message(&session.id, "first").unwrap();
        let first_id = queued.queued_messages[0].id.clone();
        let queued = service.append_user_message(&session.id, "second").unwrap();
        let second_id = queued.queued_messages[1].id.clone();
        service
            .update_queued_message(&session.id, &first_id, "first edited")
            .unwrap();
        service
            .remove_queued_message(&session.id, &second_id)
            .unwrap();
        service.append_user_message(&session.id, "third").unwrap();

        let mut ready = service.resume(&session.id).unwrap();
        assert_eq!(ready.queued_messages.len(), 2);
        ready.queue_dispatching = false;
        service.write_record(&ready).unwrap();
        service.ensure_queue_dispatch(&mut ready).unwrap();
        wait_for_chat_line(&service, &session.id, "queued provider response");
        let record = service.resume(&session.id).unwrap();
        let user_events = record
            .transcript_events
            .iter()
            .filter(|event| event.get("role").and_then(|value| value.as_str()) == Some("user"))
            .collect::<Vec<_>>();
        assert_eq!(user_events.len(), 1);
        let user_text = user_events[0]
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(user_text.contains("first edited"));
        assert!(user_text.contains("third"));
        assert!(!user_text.contains("second"));
        assert!(record.queued_messages.is_empty());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_chat_service_recovers_stale_in_flight_turns() {
        let temp_root = unique_temp_dir("chat-recovery");
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        let service = FileChatService::new(&refine_dir);
        let session = service
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();
        let registry = FileOperationRegistry::new(&service.runtime_root);
        let operation = registry.register(&format!("chat:{}", session.id)).unwrap();

        let recovered = service
            .recover_interrupted_turns("daemon restarted during provider turn")
            .unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            registry.status(&operation.id).unwrap().state,
            OperationState::Interrupted
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
        init_git_app(&temp_root);
        let refine_dir = temp_root.join(".refine");
        write_fake_provider(
            &refine_dir,
            "claude",
            0,
            r#"{"session_id":"prov-2","item":{"type":"agent_message","text":"resumed ok"}}"#,
        );
        let service = FileChatService::new(&refine_dir);
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
        let temp_root = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        temp_root.join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    fn init_git_app(repo: &Path) {
        fs::create_dir_all(repo.join(".refine")).unwrap();
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        git(repo, &["config", "user.name", "Test User"]);
        fs::write(repo.join("app.txt"), "base\n").unwrap();
        git(repo, &["add", "app.txt"]);
        git(repo, &["commit", "-m", "initial"]);
    }

    fn init_unborn_git_app(repo: &Path) {
        fs::create_dir_all(repo.join(".refine")).unwrap();
        git(repo, &["init", "-b", "main"]);
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn wait_for_chat_line(
        service: &FileChatService,
        session_id: &str,
        needle: &str,
    ) -> ChatReadResult {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let read = service.read(session_id).unwrap();
            if read.lines.iter().any(|line| line.contains(needle))
                || read.progress_lines.iter().any(|line| line.contains(needle))
            {
                return read;
            }
            if Instant::now() >= deadline {
                return read;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_chat_read<F>(
        service: &FileChatService,
        session_id: &str,
        predicate: F,
    ) -> ChatReadResult
    where
        F: Fn(&ChatReadResult) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let read = service.read(session_id).unwrap();
            if predicate(&read) {
                return read;
            }
            if Instant::now() >= deadline {
                return read;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_chat_record<F>(
        service: &FileChatService,
        session_id: &str,
        predicate: F,
    ) -> ChatSessionRecord
    where
        F: Fn(&ChatSessionRecord) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let record = service.resume(session_id).unwrap();
            if predicate(&record) {
                return record;
            }
            if Instant::now() >= deadline {
                return record;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn write_fake_provider_script(refine_dir: &Path, name: &str, script: &str) {
        let bin_dir = refine_dir.join("provider-bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        fs::write(&path, script).unwrap();
        make_provider_executable(&path);
    }

    fn write_fake_provider(refine_dir: &Path, name: &str, exit_code: i32, output: &str) {
        let bin_dir = refine_dir.join("provider-bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' {output:?}\nexit {exit_code}"
        )
        .unwrap();
        make_provider_executable(&path);
    }

    fn make_provider_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    fn write_cwd_provider(refine_dir: &Path, name: &str) {
        let bin_dir = refine_dir.join("provider-bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "#!/bin/sh\npwd > provider-cwd.txt\nprintf '%s\\n' 'cwd provider response'"
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
