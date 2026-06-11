use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::process::subprocess::{FileProcessSupervisor, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::jobs::{FileJobRegistry, JobRegistry};
use crate::tools::product::chat::ChatSessionRecord;

use super::super::http::HttpRequest;
use super::super::*;
use super::*;

pub(in crate::surfaces::web_server) fn normalize_api_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let mut normalized = if let Some(rest) = path.strip_prefix("/api/gaps") {
        format!("/work/gaps{rest}")
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
    } else if let Some(rest) = path.strip_prefix("/api/jobs") {
        format!("/jobs{rest}")
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
    } else if let Some(rest) = path.strip_prefix("/api/cluster") {
        format!("/cluster{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/agents") {
        format!("/agents{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/settings") {
        format!("/settings{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/workflow") {
        format!("/workflow{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/system") {
        format!("/system{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/upgrade") {
        format!("/upgrade{rest}")
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

pub(in crate::surfaces::web_server) fn local_origin_allowed(request: &HttpRequest) -> bool {
    let Some(origin) = request
        .headers
        .get("origin")
        .or_else(|| request.headers.get("referer"))
    else {
        return true;
    };
    origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("http://localhost:")
        || origin.starts_with("tauri://")
        || origin.starts_with("https://tauri.localhost/")
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

pub(in crate::surfaces::web_server) fn recent_api_mutation_events(
    runtime_root: &Path,
    limit: usize,
) -> RefineResult<Vec<ApiMutationEvent>> {
    let path = runtime_root.join(API_EVENTS_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read API event log {}: {error}",
            path.display()
        ))
    })?;
    let mut events = text
        .lines()
        .rev()
        .take(limit)
        .filter_map(|line| serde_json::from_str::<ApiMutationEvent>(line).ok())
        .collect::<Vec<_>>();
    events.reverse();
    Ok(events)
}

pub(in crate::surfaces::web_server) fn recent_job_sse_events(
    runtime_root: &Path,
    limit: usize,
) -> RefineResult<Vec<Value>> {
    let registry = FileJobRegistry::new(runtime_root);
    let jobs = registry.recover()?;
    let mut events = Vec::new();
    for job in jobs.into_iter().rev().take(limit) {
        let (logs, _, _) = registry.page_logs(&job.id, 5, 0)?;
        let latest_log = logs.last().cloned();
        events.push(json!({
            "job": job_response(job),
            "logs": logs,
            "latest_log": latest_log,
            "timestamp": now_timestamp_web()
        }));
    }
    events.reverse();
    Ok(events)
}

pub(in crate::surfaces::web_server) fn recent_process_sse_events(
    runtime_root: &Path,
    limit: usize,
) -> RefineResult<Vec<Value>> {
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let mut events = Vec::new();
    for process in supervisor.list()?.into_iter().rev().take(limit) {
        let (output, truncated) = if process.stdout_path.is_some() || process.stderr_path.is_some()
        {
            let full_output = supervisor.stream(&process.id)?;
            let truncated = full_output.chars().count() > 4000;
            (tail_text(full_output, 4000), truncated)
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
    events.reverse();
    Ok(events)
}

pub(in crate::surfaces::web_server) fn recent_chat_sse_events(
    durable_root: &Path,
    limit: usize,
) -> RefineResult<Vec<Value>> {
    let sessions_dir = durable_root.join("chat/sessions");
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
                "failed to inspect chat session entry {}: {error}",
                sessions_dir.display()
            ))
        })?;
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
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
