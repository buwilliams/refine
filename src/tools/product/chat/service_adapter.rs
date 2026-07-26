use super::*;

impl ChatService for FileChatService {
    fn start(&self, attachment: ChatAttachment) -> RefineResult<ChatSessionRecord> {
        self.start_with_options(attachment, None, None)
    }

    fn resume(&self, session_id: &str) -> RefineResult<ChatSessionRecord> {
        let record = self.load_record(session_id)?;
        reject_retired_supervisor(&record)?;
        Ok(record)
    }

    fn append_user_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> RefineResult<ChatSessionRecord> {
        let _guard = self.acquire_session_lock(session_id)?;
        let mut record = self.load_record(session_id)?;
        reject_retired_supervisor(&record)?;
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
            internal: false,
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
