use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::process::subprocess::{FileProcessSupervisor, ProcessOutputObservation};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::product::chat::ChatSessionRecord;

use super::super::http::HttpRequest;
use super::super::*;
use super::*;

pub(in crate::surfaces::web_server) fn normalize_api_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let mut normalized = if let Some(rest) = path.strip_prefix("/api/goals") {
        format!("/work/goals{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/features") {
        format!("/work/features{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/activity") {
        format!("/activity{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/import") {
        format!("/import{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/changes") {
        format!("/changes{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/cache") {
        format!("/cache{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/performance") {
        format!("/performance{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/terminal") {
        format!("/terminal{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/files") {
        format!("/files{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/operations") {
        format!("/operations{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/processes") {
        format!("/processes{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/quality") {
        format!("/quality{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/chat") {
        format!("/chat{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/project") {
        format!("/project{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/projects") {
        format!("/projects{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/apps") {
        format!("/apps{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/governance") {
        format!("/governance{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/guidance") {
        format!("/guidance{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/reporters") {
        format!("/reporters{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/todos") {
        format!("/todos{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/target-app") {
        format!("/target-app{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/runner-workers") {
        format!("/runner-workers{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/dashboard") {
        format!("/dashboard{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/diagnostics") {
        format!("/diagnostics{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/nodes") {
        format!("/nodes{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/fleet") {
        format!("/fleet{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/agents") {
        format!("/agents{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/settings") {
        format!("/settings{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/workflow") {
        format!("/workflow{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/sync") {
        format!("/sync{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/system") {
        format!("/system{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/upgrade") {
        format!("/upgrade{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/mcp") {
        format!("/mcp{rest}")
    } else {
        path.to_string()
    };
    if normalized.starts_with("/work/features/") && normalized.ends_with("/workflow") {
        normalized = normalized
            .strip_suffix("/workflow")
            .map(|prefix| format!("{prefix}/move"))
            .unwrap_or(normalized);
    }
    normalized
}

pub(in crate::surfaces::web_server) fn mutation_origin_allowed(request: &HttpRequest) -> bool {
    let Some(origin_or_referer) = request
        .headers
        .get("origin")
        .or_else(|| request.headers.get("referer"))
    else {
        return true;
    };

    let Ok(uri) = origin_or_referer.parse::<axum::http::Uri>() else {
        return false;
    };
    let Some(scheme) = uri.scheme_str() else {
        return false;
    };
    let Some(origin_authority) = uri.authority() else {
        return false;
    };

    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }

    let Some(host) = request.headers.get("host") else {
        return false;
    };
    let Ok(request_authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };

    let origin_has_port = origin_authority.port().is_some();
    let request_has_port = request_authority.port().is_some();
    let origin_port = origin_authority.port_u16();
    let request_port = request_authority.port_u16();

    origin_authority
        .host()
        .eq_ignore_ascii_case(request_authority.host())
        && origin_has_port == request_has_port
        && (!origin_has_port || (origin_port.is_some() && origin_port == request_port))
}

pub(in crate::surfaces::web_server) fn valid_idempotency_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

pub(in crate::surfaces::web_server) fn idempotency_fingerprint(
    method: &str,
    path: &str,
    body: &[u8],
) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in method
        .as_bytes()
        .iter()
        .chain([0].iter())
        .chain(path.as_bytes())
        .chain([0].iter())
        .chain(body)
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(in crate::surfaces::web_server) fn idempotency_path(runtime_root: &Path, key: &str) -> PathBuf {
    runtime_root
        .join(IDEMPOTENCY_DIR)
        .join(format!("{}.json", key.replace(':', "_")))
}

pub(in crate::surfaces::web_server) fn load_idempotency_record(
    runtime_root: &Path,
    key: &str,
) -> RefineResult<Option<IdempotencyRecord>> {
    let path = idempotency_path(runtime_root, key);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read idempotency record {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice::<IdempotencyRecord>(&bytes)
        .map(Some)
        .map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse idempotency record {}: {error}",
                path.display()
            ))
        })
}

pub(in crate::surfaces::web_server) fn save_idempotency_record(
    runtime_root: &Path,
    key: &str,
    fingerprint: &str,
    response: &ApiResponse,
) -> RefineResult<()> {
    let dir = runtime_root.join(IDEMPOTENCY_DIR);
    fs::create_dir_all(&dir).map_err(|error| {
        RefineError::Io(format!(
            "failed to create idempotency directory {}: {error}",
            dir.display()
        ))
    })?;
    let record = IdempotencyRecord {
        key: key.to_string(),
        fingerprint: fingerprint.to_string(),
        response: response.clone(),
        created_at: now_timestamp_web(),
    };
    let encoded = serde_json::to_vec_pretty(&record).map_err(|error| {
        RefineError::Serialization(format!("failed to encode idempotency record: {error}"))
    })?;
    let path = idempotency_path(runtime_root, key);
    fs::write(&path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write idempotency record {}: {error}",
            path.display()
        ))
    })
}

pub(in crate::surfaces::web_server) fn append_api_mutation_event(
    runtime_root: &Path,
    method: &str,
    path: &str,
    status: u16,
) -> RefineResult<()> {
    fs::create_dir_all(runtime_root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create runtime root {}: {error}",
            runtime_root.display()
        ))
    })?;
    let event = ApiMutationEvent {
        method: method.to_string(),
        path: normalize_api_path(path),
        status,
        created_at: now_timestamp_web(),
    };
    let line = serde_json::to_string(&event).map_err(|error| {
        RefineError::Serialization(format!("failed to encode API mutation event: {error}"))
    })?;
    let path = runtime_root.join(API_EVENTS_FILE);
    // One rotation generation keeps disk usage bounded; nothing reads more
    // than the recent tail, but the log previously grew forever.
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > API_EVENTS_ROTATE_BYTES) {
        let _ = fs::rename(&path, runtime_root.join(format!("{API_EVENTS_FILE}.1")));
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open API event log {}: {error}",
                path.display()
            ))
        })?;
    writeln!(file, "{line}").map_err(|error| {
        RefineError::Io(format!(
            "failed to write API event log {}: {error}",
            path.display()
        ))
    })
}

const API_EVENTS_ROTATE_BYTES: u64 = 1_000_000;
/// How much of the API event log's tail is read when recent events are
/// requested; consumers only want the last few entries.
const API_EVENTS_TAIL_BYTES: u64 = 64 * 1024;

pub(in crate::surfaces::web_server) fn recent_api_mutation_events(
    runtime_root: &Path,
    limit: usize,
) -> RefineResult<Vec<ApiMutationEvent>> {
    let path = runtime_root.join(API_EVENTS_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    // Read only the tail: this runs on every SSE frame build, and reading the
    // whole log made the poll cost grow with the log's lifetime. The first
    // line after a mid-line seek is discarded as partial.
    let mut file = fs::File::open(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read API event log {}: {error}",
            path.display()
        ))
    })?;
    let len = file
        .metadata()
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to stat API event log {}: {error}",
                path.display()
            ))
        })?
        .len();
    let seek_to = len.saturating_sub(API_EVENTS_TAIL_BYTES);
    if seek_to > 0 {
        use std::io::Seek;
        file.seek(std::io::SeekFrom::Start(seek_to))
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to seek API event log {}: {error}",
                    path.display()
                ))
            })?;
    }
    let mut text = String::new();
    use std::io::Read;
    file.read_to_string(&mut text).map_err(|error| {
        RefineError::Io(format!(
            "failed to read API event log {}: {error}",
            path.display()
        ))
    })?;
    let mut events = text
        .lines()
        .skip(if seek_to > 0 { 1 } else { 0 })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(limit)
        .filter_map(|line| serde_json::from_str::<ApiMutationEvent>(line).ok())
        .collect::<Vec<_>>();
    events.reverse();
    Ok(events)
}

pub(in crate::surfaces::web_server) fn recent_operation_sse_events(
    runtime_root: &Path,
    limit: usize,
) -> RefineResult<Vec<Value>> {
    let registry = FileOperationRegistry::new(runtime_root);
    let operations = registry.recover()?;
    let mut selected = operations
        .iter()
        .filter(|operation| {
            matches!(
                operation.state,
                OperationState::Pending | OperationState::Running | OperationState::Cancelling
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut recent_terminal = operations
        .into_iter()
        .rev()
        .filter(|operation| {
            matches!(
                operation.state,
                OperationState::Succeeded
                    | OperationState::Failed
                    | OperationState::Cancelled
                    | OperationState::Interrupted
            )
        })
        .take(limit)
        .collect::<Vec<_>>();
    selected.append(&mut recent_terminal);
    selected.sort_by(|a, b| a.id.cmp(&b.id));

    let mut events = Vec::new();
    for operation in selected {
        let (logs, _, _) = registry.page_logs(&operation.id, 5, 0)?;
        let latest_log = logs.last().cloned();
        events.push(json!({
            "operation": operation_response(operation),
            "logs": logs,
            "latest_log": latest_log,
            "timestamp": now_timestamp_web()
        }));
    }
    Ok(events)
}

pub(in crate::surfaces::web_server) fn recent_process_sse_events(
    runtime_root: &Path,
    limit: usize,
) -> RefineResult<Vec<Value>> {
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let mut events = Vec::new();
    for observation in supervisor.recent_output_observations(limit)? {
        let ProcessOutputObservation::Observed { process, output } = observation else {
            continue;
        };
        if !process.is_runtime_projection_visible() {
            continue;
        }
        let (output, truncated) = if process.stdout_path.is_some() || process.stderr_path.is_some()
        {
            let truncated = output.chars().count() > 4000;
            (tail_text(output, 4000), truncated)
        } else {
            (String::new(), false)
        };
        events.push(json!({
            "process_id": process.id,
            "process": process.api_json(),
            "output": output,
            "truncated": truncated,
            "timestamp": now_timestamp_web()
        }));
    }
    Ok(events)
}

/// How many trailing transcript events per session the SSE cache retains;
/// callers never request more than the newest few.
const CHAT_SSE_EVENT_WINDOW: usize = 25;

struct ChatSessionSseCacheEntry {
    len: u64,
    modified: Option<std::time::SystemTime>,
    session: ChatSessionRecord,
}

static CHAT_SSE_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeMap<PathBuf, ChatSessionSseCacheEntry>>,
> = std::sync::OnceLock::new();

pub(in crate::surfaces::web_server) fn recent_chat_sse_events(
    refine_dir: &Path,
    limit: usize,
) -> RefineResult<Vec<Value>> {
    let sessions_dir = refine_dir.join("chat/sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    // The SSE producer calls this twice a second; parsing every transcript in
    // full each time cost the whole chat history per tick. Unchanged files are
    // served from a memo of their trailing events, keyed by size and mtime.
    let cache = CHAT_SSE_CACHE.get_or_init(|| std::sync::Mutex::new(Default::default()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut seen = std::collections::BTreeSet::new();
    let mut sessions = Vec::new();
    for entry in fs::read_dir(&sessions_dir).map_err(|error| {
        RefineError::Io(format!(
            "failed to read chat sessions directory {}: {error}",
            sessions_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect chat session entry {}: {error}",
                sessions_dir.display()
            ))
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        seen.insert(path.clone());
        let metadata = entry.metadata().ok();
        let len = metadata
            .as_ref()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let modified = metadata.and_then(|metadata| metadata.modified().ok());
        if let Some(cached) = cache.get(&path)
            && cached.len == len
            && cached.modified == modified
        {
            sessions.push(cached.session.clone());
            continue;
        }
        let bytes = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read chat session {}: {error}",
                path.display()
            ))
        })?;
        let mut session = serde_json::from_str::<ChatSessionRecord>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse chat session {}: {error}",
                path.display()
            ))
        })?;
        let keep_from = session
            .transcript_events
            .len()
            .saturating_sub(CHAT_SSE_EVENT_WINDOW);
        session.transcript_events.drain(..keep_from);
        cache.insert(
            path,
            ChatSessionSseCacheEntry {
                len,
                modified,
                session: session.clone(),
            },
        );
        sessions.push(session);
    }
    cache.retain(|path, _| seen.contains(path));
    drop(cache);
    sessions.sort_by(|a, b| {
        a.updated_at
            .cmp(&b.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut events = Vec::new();
    for session in sessions.into_iter().rev() {
        for event in session.transcript_events.iter().rev() {
            events.push(json!({
                "session_id": session.id,
                "mode": session.mode,
                "provider": session.provider,
                "attachment": &session.attachment,
                "in_flight": session.in_flight,
                "closed": session.closed,
                "event": event,
                "timestamp": event.get("created_at").and_then(|value| value.as_str()).unwrap_or(&session.updated_at)
            }));
            if events.len() >= limit {
                events.reverse();
                return Ok(events);
            }
        }
    }
    events.reverse();
    Ok(events)
}

pub(in crate::surfaces::web_server) fn tail_text(text: String, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text;
    }
    text.chars().skip(count - max_chars).collect()
}
