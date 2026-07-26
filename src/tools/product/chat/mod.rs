use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{JsonObject, Timestamp};
use crate::process::supervisor::errors::RefineResult;
use crate::tools::product::project_state::GoalSummaryProjection;

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

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

fn is_false(value: &bool) -> bool {
    !*value
}

mod operation_support;
mod prompt_context;
mod provider_turns;
mod queue;
mod queue_messages;
mod service_adapter;
mod session_foundation;
mod sessions;
mod standalone;
mod standalone_naming;
#[cfg(test)]
mod tests;
mod transcript;

use operation_support::{
    chat_operation_log, chat_process_metadata, chat_session_id_from_operation,
};
use queue_messages::{combined_queued_message, is_internal_queued_message};
use session_foundation::{
    ChatCapacityPermit, ChatSessionLock, default_chat_runtime_root, new_chat_id,
    new_queued_message_id, now_timestamp, reject_retired_supervisor, validate_session_id,
    write_chat_record_atomically,
};
use standalone_naming::derive_standalone_goal_name;
use transcript::{
    chat_event, event_bool, event_text, importable_artifacts_from_output, unread_lines,
    unread_progress,
};
