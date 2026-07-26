use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;

use crate::process::supervisor::errors::{RefineError, RefineResult};

use super::{ChatAttachment, ChatSessionRecord};

pub(super) struct ChatSessionLock {
    pub(super) path: PathBuf,
}

pub(super) struct ChatCapacityPermit;

impl Drop for ChatSessionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn reject_retired_supervisor(record: &ChatSessionRecord) -> RefineResult<()> {
    if matches!(record.attachment, ChatAttachment::Supervisor) {
        return Err(RefineError::Conflict(
            "Supervisor Agent sessions are retired; open an independent general Agent".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_session_id(session_id: &str) -> RefineResult<()> {
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

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(super) fn new_chat_id() -> String {
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

pub(super) fn new_event_id() -> String {
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

pub(super) fn new_queued_message_id() -> String {
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

pub(super) fn default_chat_runtime_root(refine_dir: &Path) -> PathBuf {
    refine_dir
        .parent()
        .map(|root| root.join("run/chat"))
        .unwrap_or_else(|| refine_dir.join("run/chat"))
}

pub(super) fn write_chat_record_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp_path = path.with_extension(format!("json.{}.tmp", new_event_id()));
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)
}
