use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{HostAgentProviderService, ProviderInvocation};
use crate::infrastructure::process::supervisor::operations::OperationState;

use super::{
    ChatQueuedMessage, ChatSessionRecord, FileChatService, chat_event, chat_process_metadata,
    combined_queued_message, event_bool, event_text, is_internal_queued_message,
    new_queued_message_id, now_timestamp,
};

impl FileChatService {
    pub fn queue_internal_message(
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

        let attachment = record.attachment.clone();
        let existing_id = record
            .queued_messages
            .iter()
            .find(|queued| is_internal_queued_message(&attachment, queued))
            .map(|queued| queued.id.clone());
        let now = now_timestamp();
        if let Some(existing_id) = existing_id {
            record.queued_messages.retain(|queued| {
                !is_internal_queued_message(&attachment, queued) || queued.id == existing_id
            });
            if let Some(queued) = record
                .queued_messages
                .iter_mut()
                .find(|queued| queued.id == existing_id)
            {
                queued.text = text.to_string();
                queued.updated_at = now.clone();
                queued.internal = true;
            }
        } else {
            record.queued_messages.push(ChatQueuedMessage {
                id: new_queued_message_id(),
                text: text.to_string(),
                created_at: now.clone(),
                updated_at: now.clone(),
                internal: true,
            });
        }
        record.updated_at = now;
        self.write_record(&record)?;
        self.ensure_queue_dispatch(&mut record)?;
        self.load_record(session_id)
    }

    pub(super) fn ensure_queue_dispatch(&self, record: &mut ChatSessionRecord) -> RefineResult<()> {
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

    pub(super) fn dispatch_queued_messages(&self, session_id: &str) -> RefineResult<()> {
        loop {
            let _capacity = loop {
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
                let visible = queued
                    .iter()
                    .filter(|message| !is_internal_queued_message(&record.attachment, message))
                    .cloned()
                    .collect::<Vec<_>>();
                if !visible.is_empty() {
                    record.transcript_events.push(chat_event(
                        "user",
                        &combined_queued_message(&visible),
                        false,
                        record.provider_session_id.clone(),
                        None,
                    ));
                }
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
                    stall_timeout_seconds: None,
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
        }
    }

    pub(super) fn append_capacity_wait_progress(&self, session_id: &str) -> RefineResult<()> {
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

    pub(super) fn mark_dispatch_failure(&self, session_id: &str, detail: &str) -> RefineResult<()> {
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
