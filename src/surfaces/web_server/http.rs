use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

use serde_json::{Value, json};

use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::product::project_state::ProjectionQuery;
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::lifecycle::DaemonStatus;

use super::support::*;
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireResponse {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: String,
    pub body: Vec<u8>,
    pub extra_headers: Vec<(String, String)>,
}

impl WireResponse {
    pub fn json(response: ApiResponse) -> Self {
        let body = serde_json::to_vec_pretty(&response.body).unwrap_or_else(|_| b"{}".to_vec());
        Self {
            status: response.status,
            reason: reason_phrase(response.status),
            content_type: response.content_type,
            body,
            extra_headers: Vec::new(),
        }
    }

    pub fn bytes(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status,
            reason: reason_phrase(status),
            content_type: content_type.into(),
            body,
            extra_headers: Vec::new(),
        }
    }

    pub fn sse(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: "text/event-stream".to_string(),
            body: body.into().into_bytes(),
            extra_headers: vec![
                ("Cache-Control".to_string(), "no-cache".to_string()),
                ("Connection".to_string(), "close".to_string()),
            ],
        }
    }
}

fn sse_event(event: &str, data: Value) -> RefineResult<String> {
    let encoded = serde_json::to_string(&data).map_err(|error| {
        RefineError::Serialization(format!("failed to encode SSE event: {error}"))
    })?;
    Ok(format!("event: {event}\ndata: {encoded}\n\n"))
}

#[derive(Clone, Debug)]
pub struct LocalHttpDaemon {
    pub server: InProcessWebServer,
    pub static_root: Option<PathBuf>,
}

impl LocalHttpDaemon {
    pub fn recover_runtime_state(&self) -> RefineResult<()> {
        if let Some(durable_root) = &self.server.durable_root {
            self.server
                .chat_service(&durable_root)
                .recover_interrupted_turns(
                    "Daemon restarted before the provider turn completed.",
                )?;
        }
        Ok(())
    }

    pub fn bind_loopback(port: u16) -> RefineResult<TcpListener> {
        TcpListener::bind(("127.0.0.1", port)).map_err(|error| {
            RefineError::Io(format!(
                "failed to bind local daemon web server on 127.0.0.1:{port}: {error}"
            ))
        })
    }

    pub fn local_addr(listener: &TcpListener) -> RefineResult<SocketAddr> {
        listener.local_addr().map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect daemon listener address: {error}"
            ))
        })
    }

    pub fn serve_next(&self, listener: &TcpListener) -> RefineResult<()> {
        let (stream, _) = listener.accept().map_err(|error| {
            RefineError::Io(format!("failed to accept daemon HTTP request: {error}"))
        })?;
        self.handle_stream(stream)
    }

    pub fn serve_next_concurrent(
        &self,
        listener: &TcpListener,
    ) -> RefineResult<JoinHandle<RefineResult<()>>> {
        let (stream, _) = listener.accept().map_err(|error| {
            RefineError::Io(format!("failed to accept daemon HTTP request: {error}"))
        })?;
        let daemon = self.clone();
        Ok(thread::spawn(move || daemon.handle_stream(stream)))
    }

    pub fn handle_stream(&self, mut stream: TcpStream) -> RefineResult<()> {
        let request = read_http_request(&mut stream)?;
        let response = self.handle_wire_request(request);
        write_http_response(&mut stream, response)
    }

    pub fn handle_wire_request(&self, request: HttpRequest) -> WireResponse {
        if request.path == "/events" || request.path == "/api/sse" {
            return match self.server_sent_events("events") {
                Ok(events) => WireResponse::sse(events),
                Err(error) => WireResponse::json(error_response(error)),
            };
        }

        if request.method != "GET" {
            if !local_origin_allowed(&request) {
                return WireResponse::json(ApiResponse::json(
                    403,
                    json!({
                        "error": {
                            "code": "forbidden_origin",
                            "message": "mutation request origin must be local"
                        }
                    }),
                ));
            }
            if let Some(version) = request.headers.get("x-refine-api-version")
                && version != API_CONTRACT_VERSION
            {
                return WireResponse::json(ApiResponse::json(
                    426,
                    json!({
                        "error": {
                            "code": "api_version_mismatch",
                            "message": "unsupported Refine API contract version"
                        },
                        "api_contract_version": API_CONTRACT_VERSION,
                        "supported_api_contract_versions": [API_CONTRACT_VERSION]
                    }),
                ));
            }
            if let Some(key) = request.headers.get("idempotency-key")
                && !valid_idempotency_key(key)
            {
                return WireResponse::json(ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_idempotency_key",
                            "message": "idempotency key must be 1-128 ASCII letters, digits, '.', '_', '-', or ':'"
                        }
                    }),
                ));
            }
        }

        if let Some(response) = self.try_static_response(&request.path) {
            return response;
        }

        let idempotency_key = request.headers.get("idempotency-key").cloned();
        let fingerprint = idempotency_key.as_ref().map(|_| {
            idempotency_fingerprint(
                &request.method,
                &normalize_api_path(&request.path),
                request.body.as_deref().unwrap_or(&[]),
            )
        });
        if let (Some(runtime_root), Some(key), Some(fingerprint)) = (
            self.server.runtime_root.as_ref(),
            idempotency_key.as_deref(),
            fingerprint.as_deref(),
        ) {
            match load_idempotency_record(runtime_root, key) {
                Ok(Some(record)) if record.fingerprint == fingerprint => {
                    return WireResponse::json(record.response);
                }
                Ok(Some(_)) => {
                    return WireResponse::json(ApiResponse::json(
                        409,
                        json!({
                            "error": {
                                "code": "idempotency_conflict",
                                "message": "idempotency key was already used for a different request"
                            }
                        }),
                    ));
                }
                Ok(None) => {}
                Err(error) => return WireResponse::json(error_response(error)),
            }
        }

        let method = request.method.clone();
        let path = request.path.clone();
        let response = self.server.handle(ApiRequest {
            method: request.method,
            path: request.path,
            body: request
                .body
                .and_then(|body| serde_json::from_slice(&body).ok()),
        });
        if method != "GET"
            && response.status < 400
            && let Some(runtime_root) = self.server.runtime_root.as_ref()
            && let Err(error) =
                append_api_mutation_event(runtime_root, &method, &path, response.status)
        {
            return WireResponse::json(error_response(error));
        }
        if method != "GET"
            && response.status < 400
            && let Err(error) = self.server.refresh_projection_cache_after_mutation()
        {
            return WireResponse::json(error_response(error));
        }
        if let (Some(runtime_root), Some(key), Some(fingerprint)) = (
            self.server.runtime_root.as_ref(),
            idempotency_key.as_deref(),
            fingerprint.as_deref(),
        ) {
            if let Err(error) = save_idempotency_record(runtime_root, key, fingerprint, &response) {
                return WireResponse::json(error_response(error));
            }
        }
        WireResponse::json(response)
    }

    fn try_static_response(&self, path: &str) -> Option<WireResponse> {
        let static_root = self.static_root.as_ref()?;
        let relative = match path {
            "/" => PathBuf::from("index.html"),
            path => {
                let trimmed = path.trim_start_matches('/');
                let trimmed = trimmed.strip_prefix("static/").unwrap_or(trimmed);
                if trimmed.contains("..") || trimmed.starts_with('/') {
                    return Some(WireResponse::json(ApiResponse::json(
                        400,
                        json!({
                            "error": {
                                "code": "invalid_path",
                                "message": "static asset path is invalid"
                            }
                        }),
                    )));
                }
                PathBuf::from(trimmed)
            }
        };
        let full_path = static_root.join(relative);
        if !is_within(static_root, &full_path) || !full_path.is_file() {
            return None;
        }
        let bytes = match fs::read(&full_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Some(WireResponse::json(ApiResponse::json(
                    500,
                    json!({
                        "error": {
                            "code": "static_read_failed",
                            "message": format!("failed to read static asset: {error}")
                        }
                    }),
                )));
            }
        };
        Some(WireResponse::bytes(
            200,
            content_type_for_path(&full_path),
            bytes,
        ))
    }
}

impl LocalDaemonWebServer for LocalHttpDaemon {
    fn serve(&self, port: u16) -> RefineResult<DaemonStatus> {
        self.recover_runtime_state()?;
        let listener = Self::bind_loopback(port)?;
        loop {
            self.serve_next_concurrent(&listener)?;
        }
    }

    fn server_sent_events(&self, stream: &str) -> RefineResult<String> {
        let mut events = String::new();
        events.push_str("retry: 3000\n");
        events.push_str(&sse_event(
            "ready",
            json!({
                "stream": stream,
                "backend": "native",
                "timestamp": now_timestamp_web()
            }),
        )?);
        events.push_str(&sse_event(
            "project_updated",
            json!({
                "gap_count": self.server.projection.gaps.len(),
                "feature_count": self.server.projection.features.len(),
                "status_counts": self.server.projection.status_counts(),
                "dashboard": self.server.projection.dashboard
            }),
        )?);
        events.push_str(&sse_event(
            "status_change",
            json!({
                "status_counts": self.server.projection.status_counts(),
                "attention": self.server.projection.dashboard.attention_indicators
            }),
        )?);
        if let Some(durable_root) = &self.server.durable_root {
            if let Some(entry) = FileActivityService::new(durable_root)
                .recent(1)?
                .into_iter()
                .next()
            {
                events.push_str(&sse_event("activity_added", json!(entry))?);
            }
            for payload in recent_chat_sse_events(durable_root, 10)? {
                events.push_str(&sse_event("chat_event", payload)?);
            }
        }
        if let Some(runtime_root) = &self.server.runtime_root {
            let process = process_summary_response(runtime_root).body;
            events.push_str(&sse_event(
                "system_operation",
                json!({
                    "message": "Process snapshot refreshed",
                    "status": "info",
                    "category": "process",
                    "timestamp": now_timestamp_web(),
                    "details": process
                }),
            )?);
            for payload in recent_process_sse_events(runtime_root, 10)? {
                events.push_str(&sse_event("process_output", payload)?);
            }
            for payload in recent_job_sse_events(runtime_root, 10)? {
                events.push_str(&sse_event("job_progress", payload)?);
            }
            for event in recent_api_mutation_events(runtime_root, 25)? {
                events.push_str(&sse_event(
                    "api_mutation",
                    json!({
                        "method": event.method,
                        "path": event.path,
                        "status": event.status,
                        "timestamp": event.created_at
                    }),
                )?);
            }
        }
        Ok(events)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
}

fn read_http_request(stream: &mut TcpStream) -> RefineResult<HttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| RefineError::Io(format!("failed to read HTTP request: {error}")))?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err(RefineError::InvalidInput(
                "HTTP request headers exceed 1 MiB".to_string(),
            ));
        }
    }

    let header_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| RefineError::InvalidInput("malformed HTTP request".to_string()))?
        + 4;
    let header_text = std::str::from_utf8(&buffer[..header_end]).map_err(|error| {
        RefineError::InvalidInput(format!("HTTP headers are not valid UTF-8: {error}"))
    })?;
    let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());
    let request_line = lines
        .next()
        .ok_or_else(|| RefineError::InvalidInput("missing HTTP request line".to_string()))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| RefineError::InvalidInput("missing HTTP method".to_string()))?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| RefineError::InvalidInput("missing HTTP path".to_string()))?
        .to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| RefineError::Io(format!("failed to read HTTP body: {error}")))?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = if content_length > 0 && buffer.len() >= header_end + content_length {
        Some(buffer[header_end..header_end + content_length].to_vec())
    } else {
        None
    };

    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_http_response(stream: &mut TcpStream, response: WireResponse) -> RefineResult<()> {
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: http://127.0.0.1\r\n",
        response.status,
        response.reason,
        response.content_type,
        response.body.len()
    );
    for (name, value) in response.extra_headers {
        headers.push_str(&format!("{name}: {value}\r\n"));
    }
    headers.push_str("\r\n");
    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(&response.body))
        .and_then(|_| stream.flush())
        .map_err(|error| RefineError::Io(format!("failed to write HTTP response: {error}")))
}

fn is_within(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root)
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        426 => "Upgrade Required",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "OK",
    }
}
