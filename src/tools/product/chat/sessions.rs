use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::process::supervisor::errors::{RefineError, RefineResult};

use super::{
    ChatAttachment, ChatReadResult, ChatService, ChatSessionLock, ChatSessionRecord,
    ChatSessionWorktree, FileChatService, default_chat_runtime_root, event_bool, event_text,
    new_chat_id, now_timestamp, reject_retired_supervisor, unread_lines, unread_progress,
    validate_session_id, write_chat_record_atomically,
};

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
        if matches!(attachment, ChatAttachment::Supervisor) {
            return Err(RefineError::Conflict(
                "Supervisor Agent sessions are retired; open an independent general Agent"
                    .to_string(),
            ));
        }
        if matches!(attachment, ChatAttachment::Standalone) {
            return self.start_standalone_with_options(provider, mode);
        }
        self.start_record_with_options(attachment, provider, mode)
    }

    pub(super) fn start_record_with_options(
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
        reject_retired_supervisor(&record)?;
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
            queued_messages: record.visible_queued_messages(),
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

    /// Delete retired Supervisor conversations after their provider processes have been stopped.
    pub fn purge_supervisor_sessions(&self) -> RefineResult<usize> {
        let sessions = self
            .list_sessions()?
            .into_iter()
            .filter(|session| matches!(session.attachment, ChatAttachment::Supervisor))
            .collect::<Vec<_>>();
        for session in &sessions {
            for path in [
                self.session_path(&session.id),
                self.sessions_dir().join(format!(".{}.lock", session.id)),
            ] {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(RefineError::Io(format!(
                            "failed to purge retired Supervisor session {}: {error}",
                            path.display()
                        )));
                    }
                }
            }
        }
        Ok(sessions.len())
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

    pub(super) fn load_record(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
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

    pub(super) fn write_record(&self, record: &ChatSessionRecord) -> RefineResult<()> {
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

    pub(super) fn sessions_dir(&self) -> PathBuf {
        self.refine_dir.join("chat/sessions")
    }

    pub(super) fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{session_id}.json"))
    }

    pub(super) fn acquire_session_lock(&self, session_id: &str) -> RefineResult<ChatSessionLock> {
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
}
