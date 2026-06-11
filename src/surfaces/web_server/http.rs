use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{OriginalUri, State};
use axum::http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_TYPE};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    DaemonLifecycleService, DaemonStatus, FileDaemonLifecycleService,
};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::metrics::FileMetricsService;
use crate::tools::product::project_state::ProjectionQuery;
use crate::workflow::WorkflowEngine;

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

    fn into_axum_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut builder = Response::builder()
            .status(status)
            .header(CONTENT_TYPE, self.content_type)
            .header(ACCESS_CONTROL_ALLOW_ORIGIN, "http://127.0.0.1");
        for (name, value) in self.extra_headers {
            builder = builder.header(name, value);
        }
        builder
            .body(Body::from(self.body))
            .unwrap_or_else(|_| Response::new(Body::from("{}")))
    }
}

impl IntoResponse for WireResponse {
    fn into_response(self) -> Response {
        self.into_axum_response()
    }
}

fn sse_event(event: &str, data: Value) -> RefineResult<String> {
    let encoded = serde_json::to_string(&data).map_err(|error| {
        RefineError::Serialization(format!("failed to encode SSE event: {error}"))
    })?;
    Ok(format!("event: {event}\ndata: {encoded}\n\n"))
}

#[derive(Clone, Debug)]
struct SseEventFrame {
    event: &'static str,
    data: Value,
}

#[derive(Clone, Debug)]
struct StaticAssetCacheEntry {
    modified: Option<SystemTime>,
    content_type: String,
    bytes: Vec<u8>,
}

static STATIC_ASSET_CACHE: OnceLock<Mutex<BTreeMap<String, StaticAssetCacheEntry>>> =
    OnceLock::new();

const AGENT_WORKFLOW_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct LocalHttpDaemon {
    pub server: InProcessWebServer,
    pub static_root: Option<PathBuf>,
}

pub(super) struct AgentWorkflowLoop {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AgentWorkflowLoop {
    #[cfg(test)]
    pub(super) fn stop_for_test(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AgentWorkflowLoop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.take();
    }
}

#[derive(Clone, Debug)]
struct AxumDaemonState {
    daemon: LocalHttpDaemon,
    complete_after_request: Option<Arc<Notify>>,
}

impl LocalHttpDaemon {
    pub fn recover_runtime_state(&self) -> RefineResult<()> {
        self.recover_runtime_state_with_progress(|_| {})
    }

    pub fn recover_runtime_state_with_progress(
        &self,
        mut report: impl FnMut(&str),
    ) -> RefineResult<()> {
        if let Some(durable_root) = self.server.current_durable_root()? {
            report("recovering interrupted chat turns");
            self.server
                .chat_service(&durable_root)
                .recover_interrupted_turns(
                    "Daemon restarted before the provider turn completed.",
                )?;
        }
        report("warming project and runtime caches");
        let _ = self.server.warm_current_projection_cache()?;
        report("warming diagnostics cache");
        self.server.warm_diagnostics_cache()?;
        report("warming static asset cache");
        self.warm_static_cache()?;
        report("startup cache warming complete");
        Ok(())
    }

    fn warm_static_cache(&self) -> RefineResult<()> {
        let Some(static_root) = &self.static_root else {
            return Ok(());
        };
        warm_static_cache_dir(static_root, static_root)
    }

    pub fn bind_loopback(port: u16) -> RefineResult<TcpListener> {
        Self::bind_address(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    pub fn bind_address(bind_address: IpAddr, port: u16) -> RefineResult<TcpListener> {
        TcpListener::bind((bind_address, port)).map_err(|error| {
            RefineError::Io(format!(
                "failed to bind local daemon web server on {bind_address}:{port}: {error}"
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

    pub fn serve_once(&self, listener: TcpListener) -> RefineResult<()> {
        self.serve_listener(listener, Some(serve_once_shutdown()))
    }

    pub fn serve_until_unhealthy(
        &self,
        listener: TcpListener,
        lifecycle: FileDaemonLifecycleService,
        port: u16,
    ) -> RefineResult<()> {
        let _automation_loop = if agent_automation_loop_disabled() {
            None
        } else {
            Some(self.start_agent_automation_loop(AGENT_WORKFLOW_INTERVAL))
        };
        self.serve_listener(listener, Some(lifecycle_shutdown(lifecycle, port)))
    }

    pub(super) fn start_agent_automation_loop(&self, interval: Duration) -> AgentWorkflowLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let server = self.server.clone();
        let interval = interval.max(Duration::from_millis(10));
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                let started = Instant::now();
                let _ = run_agent_automation_once(&server);
                let elapsed = started.elapsed();
                let remaining = interval.saturating_sub(elapsed);
                if !remaining.is_zero() {
                    thread::sleep(remaining);
                }
            }
        });
        AgentWorkflowLoop {
            stop,
            handle: Some(handle),
        }
    }

    fn serve_listener(
        &self,
        listener: TcpListener,
        shutdown: Option<AxumShutdown>,
    ) -> RefineResult<()> {
        let daemon = self.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| RefineError::Io(format!("failed to start Tokio runtime: {error}")))?;
        runtime.block_on(async move {
            listener.set_nonblocking(true).map_err(|error| {
                RefineError::Io(format!("failed to configure daemon listener: {error}"))
            })?;
            let listener = tokio::net::TcpListener::from_std(listener).map_err(|error| {
                RefineError::Io(format!("failed to hand daemon listener to Tokio: {error}"))
            })?;
            let app = daemon.axum_router(
                shutdown
                    .as_ref()
                    .and_then(|shutdown| shutdown.complete_after_request.as_ref().map(Arc::clone)),
            );
            let server = axum::serve(listener, app);
            match shutdown {
                Some(shutdown) => server
                    .with_graceful_shutdown(shutdown.wait())
                    .await
                    .map_err(|error| {
                        RefineError::Io(format!("Axum daemon server failed: {error}"))
                    }),
                None => server.await.map_err(|error| {
                    RefineError::Io(format!("Axum daemon server failed: {error}"))
                }),
            }
        })
    }

    fn axum_router(&self, complete_after_request: Option<Arc<Notify>>) -> Router {
        Router::new()
            .fallback(any(axum_entrypoint))
            .with_state(AxumDaemonState {
                daemon: self.clone(),
                complete_after_request,
            })
    }

    fn handle_axum_request(&self, request: HttpRequest) -> Response {
        if request.method == "GET" && (request.path == "/events" || request.path == "/api/sse") {
            return self.sse_response("events");
        }
        if request.method == "GET"
            && let Some(session_id) = terminal_events_route(&request.path)
        {
            return self.terminal_sse_response(session_id);
        }
        self.handle_wire_request(request).into_response()
    }

    pub fn handle_wire_request(&self, request: HttpRequest) -> WireResponse {
        let started = Instant::now();
        if request.path == "/events" || request.path == "/api/sse" {
            return match self.server_sent_events("events") {
                Ok(events) => WireResponse::sse(events),
                Err(error) => WireResponse::json(error_response(error)),
            };
        }
        if request.method == "GET" && terminal_events_route(&request.path).is_some() {
            return WireResponse::json(self.server.handle_terminal_events_snapshot(&request.path));
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
            return self.with_request_metric(&request, "static", started, response);
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
                    return self.with_request_metric(
                        &request,
                        "idempotency",
                        started,
                        WireResponse::json(record.response),
                    );
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
        if let (Some(runtime_root), Some(key), Some(fingerprint)) = (
            self.server.runtime_root.as_ref(),
            idempotency_key.as_deref(),
            fingerprint.as_deref(),
        ) {
            if let Err(error) = save_idempotency_record(runtime_root, key, fingerprint, &response) {
                return WireResponse::json(error_response(error));
            }
        }
        self.with_request_metric(
            &HttpRequest {
                method,
                path,
                headers: BTreeMap::new(),
                body: None,
            },
            "hot",
            started,
            WireResponse::json(response),
        )
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
        let entry = match cached_static_asset(static_root, &full_path) {
            Ok(entry) => entry,
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
        Some(WireResponse::bytes(200, entry.content_type, entry.bytes))
    }

    fn with_request_metric(
        &self,
        request: &HttpRequest,
        cache_mode: &'static str,
        started: Instant,
        response: WireResponse,
    ) -> WireResponse {
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        if !record_http_request_metric(&self.server, request, &response, cache_mode, elapsed_ms) {
            return response;
        }
        response
    }
}

struct AxumShutdown {
    complete_after_request: Option<Arc<Notify>>,
    receiver: oneshot::Receiver<()>,
}

impl AxumShutdown {
    async fn wait(self) {
        let _ = self.receiver.await;
    }
}

fn serve_once_shutdown() -> AxumShutdown {
    let notify = Arc::new(Notify::new());
    let wait_notify = Arc::clone(&notify);
    let (tx, rx) = oneshot::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        if let Ok(runtime) = runtime {
            runtime.block_on(wait_notify.notified());
        }
        let _ = tx.send(());
    });
    AxumShutdown {
        complete_after_request: Some(notify),
        receiver: rx,
    }
}

fn lifecycle_shutdown(lifecycle: FileDaemonLifecycleService, port: u16) -> AxumShutdown {
    let (tx, rx) = oneshot::channel();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(500));
            match lifecycle.status(port) {
                Ok(status) if status.daemon_healthy => {}
                _ => break,
            }
        }
        let _ = tx.send(());
    });
    AxumShutdown {
        complete_after_request: None,
        receiver: rx,
    }
}

async fn axum_entrypoint(
    State(state): State<AxumDaemonState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = HttpRequest {
        method: method.as_str().to_string(),
        path: uri_path_and_query(&uri),
        headers: headers_to_map(&headers),
        body: if body.is_empty() {
            None
        } else {
            Some(body.to_vec())
        },
    };
    let response = state.daemon.handle_axum_request(request);
    if let Some(notify) = state.complete_after_request {
        notify.notify_one();
    }
    response
}

fn uri_path_and_query(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string())
}

fn headers_to_map(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

impl LocalDaemonWebServer for LocalHttpDaemon {
    fn serve(&self, port: u16) -> RefineResult<DaemonStatus> {
        self.recover_runtime_state()?;
        let listener = Self::bind_loopback(port)?;
        let addr = Self::local_addr(&listener)?;
        let status = self.server.status.clone();
        self.serve_listener(listener, None)?;
        let mut status = status;
        status.port = addr.port();
        Ok(status)
    }

    fn server_sent_events(&self, stream: &str) -> RefineResult<String> {
        let mut events = String::new();
        events.push_str("retry: 3000\n");
        for frame in self.server_sent_event_frames(stream)? {
            events.push_str(&sse_event(frame.event, frame.data)?);
        }
        Ok(events)
    }
}

impl LocalHttpDaemon {
    fn sse_response(&self, stream_name: &'static str) -> Response {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);
        let daemon = self.clone();
        tokio::spawn(async move {
            let mut last_state_signatures = BTreeMap::new();
            let mut seen_stream_signatures: BTreeMap<&'static str, BTreeSet<String>> =
                BTreeMap::new();
            loop {
                let frames = match daemon.server_sent_event_frames(stream_name) {
                    Ok(frames) => frames,
                    Err(error) => {
                        let event = Event::default()
                            .event("error")
                            .data(json!({"message": error.to_string()}).to_string());
                        let _ = tx.send(Ok(event)).await;
                        break;
                    }
                };
                for frame in frames {
                    if should_write_sse_frame(
                        &frame,
                        &mut last_state_signatures,
                        &mut seen_stream_signatures,
                    ) {
                        let encoded = match serde_json::to_string(&frame.data) {
                            Ok(encoded) => encoded,
                            Err(_) => continue,
                        };
                        if tx
                            .send(Ok(Event::default().event(frame.event).data(encoded)))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        Sse::new(ReceiverStream::new(rx))
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(15))
                    .text("heartbeat"),
            )
            .into_response()
    }

    fn terminal_sse_response(&self, session_id: String) -> Response {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
        tokio::spawn(async move {
            let mut after = 0_u64;
            loop {
                match terminal_events_since(&session_id, after) {
                    Ok(events) => {
                        for payload in events {
                            let seq = payload.get("seq").and_then(Value::as_u64).unwrap_or(after);
                            let event = payload
                                .get("event")
                                .and_then(Value::as_str)
                                .unwrap_or("terminal_output");
                            if seq > after {
                                after = seq;
                            }
                            if tx
                                .send(Ok(Event::default().event(event).data(payload.to_string())))
                                .await
                                .is_err()
                            {
                                return;
                            }
                            if event == "terminal_exit" {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let event = Event::default()
                            .event("terminal_error")
                            .data(json!({"message": error.to_string()}).to_string());
                        let _ = tx.send(Ok(event)).await;
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        });
        Sse::new(ReceiverStream::new(rx))
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(15))
                    .text("heartbeat"),
            )
            .into_response()
    }

    fn server_sent_event_frames(&self, stream: &str) -> RefineResult<Vec<SseEventFrame>> {
        let projection = self.server.current_projection_with_runtime()?;
        let mut events = vec![
            SseEventFrame {
                event: "ready",
                data: json!({
                    "stream": stream,
                    "backend": "native",
                    "timestamp": now_timestamp_web()
                }),
            },
            SseEventFrame {
                event: "project_updated",
                data: json!({
                    "gap_count": projection.gaps.len(),
                    "feature_count": projection.features.len(),
                    "status_counts": projection.status_counts(),
                    "dashboard": projection.dashboard
                }),
            },
            SseEventFrame {
                event: "status_change",
                data: json!({
                    "status_counts": projection.status_counts(),
                    "attention": projection.dashboard.attention_indicators
                }),
            },
        ];
        if let Some(durable_root) = self.server.current_durable_root()? {
            if let Some(entry) = FileActivityService::new(&durable_root)
                .recent(1)?
                .into_iter()
                .next()
            {
                events.push(SseEventFrame {
                    event: "activity_added",
                    data: json!(entry),
                });
            }
            for payload in recent_chat_sse_events(&durable_root, 10)? {
                events.push(SseEventFrame {
                    event: "chat_event",
                    data: payload,
                });
            }
        }
        if let Some(runtime_root) = &self.server.runtime_root {
            let process = process_summary_response(runtime_root).body;
            events.push(SseEventFrame {
                event: "system_operation",
                data: json!({
                    "message": "Process snapshot refreshed",
                    "status": "info",
                    "category": "process",
                    "timestamp": now_timestamp_web(),
                    "details": process
                }),
            });
            for payload in recent_process_sse_events(runtime_root, 10)? {
                events.push(SseEventFrame {
                    event: "process_output",
                    data: payload,
                });
            }
            for payload in recent_operation_sse_events(runtime_root, 10)? {
                events.push(SseEventFrame {
                    event: "operation_progress",
                    data: payload,
                });
            }
            for event in recent_api_mutation_events(runtime_root, 25)? {
                events.push(SseEventFrame {
                    event: "api_mutation",
                    data: json!({
                        "method": event.method,
                        "path": event.path,
                        "status": event.status,
                        "timestamp": event.created_at
                    }),
                });
            }
        }
        Ok(events)
    }
}

fn terminal_events_route(path: &str) -> Option<String> {
    let path = path.split('?').next().unwrap_or(path);
    let rest = path
        .strip_prefix("/api/terminal/")
        .or_else(|| path.strip_prefix("/terminal/"))?;
    let session_id = rest.strip_suffix("/events")?;
    if session_id.is_empty() || session_id.contains('/') {
        return None;
    }
    Some(session_id.to_string())
}

fn run_agent_automation_once(server: &InProcessWebServer) -> RefineResult<()> {
    let Some(runtime_root) = &server.runtime_root else {
        return Ok(());
    };
    let Some(durable_root) = server.current_durable_root()? else {
        return Ok(());
    };
    let automation = WorkflowEngine::with_durable_root(runtime_root, durable_root);
    let result = automation.evaluate_workflow();
    let refresh_result = server.refresh_projection_cache_after_mutation();
    result?;
    refresh_result?;
    Ok(())
}

fn agent_automation_loop_disabled() -> bool {
    matches!(
        env::var("REFINE_AGENT_WORKFLOW_DISABLED")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
}

fn warm_static_cache_dir(static_root: &Path, root: &Path) -> RefineResult<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| {
        RefineError::Io(format!(
            "failed to read static directory {}: {error}",
            root.display()
        ))
    })? {
        let entry = entry
            .map_err(|error| RefineError::Io(format!("failed to read static entry: {error}")))?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat static asset {}: {error}",
                path.display()
            ))
        })?;
        if metadata.is_dir() {
            warm_static_cache_dir(static_root, &path)?;
        } else if metadata.is_file() {
            let _ = cached_static_asset(static_root, &path)?;
        }
    }
    Ok(())
}

fn cached_static_asset(
    static_root: &Path,
    full_path: &Path,
) -> RefineResult<StaticAssetCacheEntry> {
    let metadata = fs::metadata(full_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to stat static asset {}: {error}",
            full_path.display()
        ))
    })?;
    let modified = metadata.modified().ok();
    let key = static_asset_key(static_root, full_path);
    let cache = STATIC_ASSET_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    {
        let cache = cache
            .lock()
            .map_err(|_| RefineError::Io("static asset cache lock was poisoned".to_string()))?;
        if let Some(entry) = cache.get(&key)
            && entry.modified == modified
        {
            return Ok(entry.clone());
        }
    }

    let entry = StaticAssetCacheEntry {
        modified,
        content_type: content_type_for_path(full_path).to_string(),
        bytes: fs::read(full_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read static asset {}: {error}",
                full_path.display()
            ))
        })?,
    };
    cache
        .lock()
        .map_err(|_| RefineError::Io("static asset cache lock was poisoned".to_string()))?
        .insert(key, entry.clone());
    Ok(entry)
}

fn static_asset_key(static_root: &Path, full_path: &Path) -> String {
    format!("{}|{}", static_root.display(), full_path.display())
}

fn record_http_request_metric(
    server: &InProcessWebServer,
    request: &HttpRequest,
    response: &WireResponse,
    cache_mode: &'static str,
    elapsed_ms: f64,
) -> bool {
    let Some(runtime_root) = server.runtime_root.clone() else {
        return false;
    };
    if !screen_critical_http_request(&request.method, &request.path, cache_mode) {
        return false;
    }
    let method = request.method.clone();
    let raw_path = request.path.clone();
    let normalized_path = normalize_api_path(&raw_path);
    let status = response.status;
    let bytes = response.body.len() as u64;
    thread::spawn(move || {
        let _ = FileMetricsService::new(runtime_root).record_operation(
            "http.request",
            elapsed_ms,
            status < 400,
            json!({
                "method": method,
                "path": normalized_path,
                "raw_path": raw_path,
                "status": status,
                "bytes": bytes,
                "cache_mode": cache_mode,
                "budget_ms": 75.0,
                "over_budget": elapsed_ms > 75.0
            }),
        );
    });
    true
}

fn screen_critical_http_request(method: &str, path: &str, cache_mode: &str) -> bool {
    if method != "GET" {
        return false;
    }
    if cache_mode == "static" {
        return true;
    }
    let normalized = normalize_api_path(path);
    if matches!(normalized.as_str(), "/performance" | "/events") {
        return false;
    }
    matches!(
        normalized.as_str(),
        "/project/status"
            | "/apps/status"
            | "/dashboard"
            | "/work/gaps"
            | "/work/features"
            | "/activity"
            | "/changes"
            | "/nodes"
            | "/settings"
            | "/processes"
            | "/diagnostics"
            | "/agents"
            | "/governance"
            | "/guidance"
            | "/reporters"
            | "/cluster"
            | "/quality"
            | "/upgrade"
            | "/target-app/status"
    ) || normalized.starts_with("/work/gaps/")
        || normalized.starts_with("/work/features/")
}

fn should_write_sse_frame(
    frame: &SseEventFrame,
    last_state_signatures: &mut BTreeMap<&'static str, String>,
    seen_stream_signatures: &mut BTreeMap<&'static str, BTreeSet<String>>,
) -> bool {
    if frame.event == "ready" {
        return last_state_signatures
            .insert(frame.event, sse_frame_signature(frame))
            .is_none();
    }
    let signature = sse_frame_signature(frame);
    if state_sse_event(frame.event) {
        if last_state_signatures
            .get(frame.event)
            .is_some_and(|previous| previous == &signature)
        {
            return false;
        }
        last_state_signatures.insert(frame.event, signature);
        return true;
    }
    seen_stream_signatures
        .entry(frame.event)
        .or_default()
        .insert(signature)
}

fn state_sse_event(event: &str) -> bool {
    matches!(
        event,
        "project_updated" | "status_change" | "activity_added" | "system_operation"
    )
}

fn sse_frame_signature(frame: &SseEventFrame) -> String {
    let mut data = frame.data.clone();
    if frame.event == "system_operation"
        && let Some(object) = data.as_object_mut()
    {
        object.remove("timestamp");
    }
    serde_json::to_string(&json!({
        "event": frame.event,
        "data": data
    }))
    .unwrap_or_else(|_| format!("{}:{:?}", frame.event, frame.data))
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
