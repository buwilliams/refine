use super::*;

pub(super) fn operation_terminal(state: &OperationState) -> bool {
    matches!(
        state,
        OperationState::Succeeded
            | OperationState::Failed
            | OperationState::Cancelled
            | OperationState::Interrupted
    )
}

pub(super) fn operation_active(state: &OperationState) -> bool {
    matches!(
        state,
        OperationState::Pending | OperationState::Running | OperationState::Cancelling
    )
}

pub(super) fn cancellation_terminal_is_deferred(operation: &OperationHandle) -> bool {
    operation
        .request
        .get("defer_cancellation_terminal")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && operation
            .request
            .get("cancellation_requested")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

#[cfg(not(windows))]
pub(super) fn replace_file(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
pub(super) fn replace_file(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn terminal_recovery_state_is_authoritative(
    current: &OperationState,
    next: &OperationState,
) -> bool {
    matches!(
        current,
        OperationState::Cancelling | OperationState::Cancelled | OperationState::Interrupted
    ) && current != next
}

pub(super) fn process_operation_id(process: &ManagedProcess) -> Option<String> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| {
            details
                .get("operation_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

pub(super) fn new_operation_id() -> String {
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

pub(super) fn empty_object() -> Value {
    json!({})
}

pub(super) fn operation_log_entry(
    operation: &OperationHandle,
    severity: &str,
    message: &str,
    details: Option<crate::model::JsonObject>,
) -> LogEntry {
    let mut details = details.unwrap_or_default();
    details.insert("operation_id".to_string(), json!(operation.id));
    details.insert("owner".to_string(), json!(operation.owner));
    details.insert("state".to_string(), json!(operation.state));
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "operation".to_string(),
        message: message.to_string(),
        details: Some(details),
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        goal_id: operation
            .owner
            .strip_prefix("goal:")
            .map(|goal_id| goal_id.to_string()),
    }
}

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
