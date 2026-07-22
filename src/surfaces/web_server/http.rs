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
use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag, html};
use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
use tokio_stream::wrappers::ReceiverStream;

#[cfg(not(test))]
use crate::process::runner::{FileRunnerWorkerService, GIT_SYNC_RUNNER, WORKFLOW_RUNNER};
#[cfg(test)]
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    DaemonLifecycleService, DaemonStatus, FileDaemonLifecycleService,
};
use crate::tools::host::quality::QualityOperationRunner;
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::metrics::FileMetricsService;
use crate::tools::product::project_state::ProjectionQuery;
#[cfg(test)]
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
#[cfg(test)]
const DEFAULT_REMOTE_FETCH_INTERVAL: Duration = Duration::from_secs(300);
#[cfg(test)]
const DEFAULT_GIT_SYNC_DEBOUNCE: Duration = Duration::from_secs(5);
#[cfg(test)]
const GIT_RECONCILE_POLL_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(test)]
const GIT_RECONCILE_RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct LocalHttpDaemon {
    pub server: InProcessWebServer,
    pub static_root: Option<PathBuf>,
}

pub(super) struct AgentWorkflowLoop {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

pub(super) struct GitSyncLoop {
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

impl Drop for GitSyncLoop {
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
        if let Some(refine_dir) = self.server.current_refine_dir()? {
            report("recovering interrupted chat turns");
            self.server
                .chat_service(&refine_dir)
                .recover_interrupted_turns(
                    "Daemon restarted before the provider turn completed.",
                )?;
            if let (Some(runtime_root), Some(target_root)) = (
                &self.server.runtime_root,
                self.server.current_target_root()?,
            ) {
                report("recovering incomplete Quality cancellations");
                QualityOperationRunner::new(&refine_dir, runtime_root, target_root)
                    .recover_cancelled_operations()?;
            }
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

    pub fn serve_static(&self, listener: TcpListener) -> RefineResult<()> {
        self.serve_listener(listener, None)
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
        let _git_sync_loop = self.start_git_sync_loop();
        self.serve_listener(listener, Some(lifecycle_shutdown(lifecycle, port)))
    }

    #[cfg(test)]
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

    #[cfg(not(test))]
    pub(super) fn start_agent_automation_loop(&self, interval: Duration) -> AgentWorkflowLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let runtime_root = self.server.runtime_root.clone();
        let interval = interval.max(Duration::from_millis(100));
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(runtime_root) = &runtime_root {
                    let _ = FileRunnerWorkerService::new(runtime_root)
                        .ensure_background_worker(WORKFLOW_RUNNER);
                }
                sleep_until_stopped(&thread_stop, interval);
            }
        });
        AgentWorkflowLoop {
            stop,
            handle: Some(handle),
        }
    }

    #[cfg(test)]
    fn start_git_sync_loop(&self) -> GitSyncLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let server = self.server.clone();
        let handle = thread::spawn(move || {
            let mut active_root = None;
            let mut last_observed_fingerprint = None;
            let mut pending_sync = None;
            let mut next_remote_fetch = None;
            let mut active_schedule = None;
            let mut next_attempt = Instant::now();
            while !thread_stop.load(Ordering::Relaxed) {
                let now = Instant::now();
                if now >= next_attempt
                    && let Ok(Some(service)) = server.current_git_sync_service()
                {
                    let root = service
                        .target_root
                        .canonicalize()
                        .unwrap_or_else(|_| service.target_root.clone());
                    if active_root.as_ref() != Some(&root) {
                        active_root = Some(root);
                        last_observed_fingerprint = None;
                        pending_sync = None;
                        next_remote_fetch = None;
                        active_schedule = None;
                    }
                    if let Ok(fingerprint) = service.durable_state_fingerprint() {
                        let schedule = git_sync_configuration(&server).unwrap_or_default();
                        if active_schedule != Some(schedule) {
                            if pending_sync.is_some() {
                                pending_sync = Some(now + schedule.debounce);
                            }
                            next_remote_fetch = schedule
                                .remote_fetch_interval
                                .map(|interval| now + interval);
                            active_schedule = Some(schedule);
                        }
                        if last_observed_fingerprint != Some(fingerprint) {
                            last_observed_fingerprint = Some(fingerprint);
                            pending_sync = Some(now + schedule.debounce);
                        }
                        let demand_due = pending_sync.is_some_and(|deadline| now >= deadline);
                        let remote_fetch_due =
                            next_remote_fetch.is_some_and(|deadline| now >= deadline);
                        if demand_due || remote_fetch_due {
                            let result = if remote_fetch_due {
                                service.try_sync()
                            } else {
                                service.try_sync_state()
                            };
                            match result {
                                Ok(result) if !result.deferred => {
                                    last_observed_fingerprint = service
                                        .durable_state_fingerprint()
                                        .ok()
                                        .or(Some(fingerprint));
                                    pending_sync = None;
                                    next_remote_fetch = schedule
                                        .remote_fetch_interval
                                        .map(|interval| now + interval);
                                    next_attempt = now;
                                    let _ = server.refresh_projection_cache_after_mutation();
                                }
                                Ok(_) | Err(_) => {
                                    next_attempt = now + GIT_RECONCILE_RETRY_INTERVAL;
                                }
                            }
                        }
                    }
                }
                sleep_until_stopped(&thread_stop, GIT_RECONCILE_POLL_INTERVAL);
            }
        });
        GitSyncLoop {
            stop,
            handle: Some(handle),
        }
    }

    #[cfg(not(test))]
    fn start_git_sync_loop(&self) -> GitSyncLoop {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let runtime_root = self.server.runtime_root.clone();
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                if let Some(runtime_root) = &runtime_root {
                    let _ = FileRunnerWorkerService::new(runtime_root)
                        .ensure_background_worker(GIT_SYNC_RUNNER);
                }
                sleep_until_stopped(&thread_stop, Duration::from_secs(1));
            }
        });
        GitSyncLoop {
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

        if request.method == "GET"
            && let Some(response) = self.try_static_response(&request.path)
        {
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
        ) && let Err(error) = save_idempotency_record(runtime_root, key, fingerprint, &response)
        {
            return WireResponse::json(error_response(error));
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
        let path = path.split('?').next().unwrap_or(path);
        let website_mode = website_index_path(static_root).is_file();

        if website_mode && matches!(path, "/docs" | "/docs/") {
            return Some(render_docs_landing_page());
        }

        if website_mode && (path == "/read" || path.starts_with("/read/")) {
            return Some(render_markdown_document(static_root, path));
        }

        let relative = match path {
            "/" if website_mode => PathBuf::from("src/surfaces/website/index.html"),
            "/" => PathBuf::from("index.html"),
            path => {
                let trimmed = path.trim_start_matches('/');
                let trimmed = if website_mode {
                    trimmed
                } else {
                    trimmed.strip_prefix("static/").unwrap_or(trimmed)
                };
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
                if website_mode && !website_public_path_allowed(trimmed) {
                    return None;
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
    let response = if request.method == "GET"
        && (request.path == "/events"
            || request.path == "/api/sse"
            || terminal_events_route(&request.path).is_some())
    {
        state.daemon.handle_axum_request(request)
    } else {
        let daemon = state.daemon.clone();
        match tokio::task::spawn_blocking(move || daemon.handle_axum_request(request)).await {
            Ok(response) => response,
            Err(error) => WireResponse::json(error_response(RefineError::Io(format!(
                "HTTP request worker failed: {error}"
            ))))
            .into_response(),
        }
    };
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
                let frame_daemon = daemon.clone();
                let frames = match tokio::task::spawn_blocking(move || {
                    frame_daemon.server_sent_event_frames(stream_name)
                })
                .await
                {
                    Ok(Ok(frames)) => frames,
                    Ok(Err(error)) => {
                        let event = Event::default()
                            .event("error")
                            .data(json!({"message": error.to_string()}).to_string());
                        let _ = tx.send(Ok(event)).await;
                        break;
                    }
                    Err(error) => {
                        let event = Event::default().event("error").data(
                            json!({"message": format!("SSE projection worker failed: {error}")})
                                .to_string(),
                        );
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
                    "goal_count": projection.goals.len(),
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
        let mut recent_goal_logs = projection
            .activity
            .values()
            .filter(|activity| activity.entry.id.starts_with("round-log:"))
            .map(|activity| activity.entry.clone())
            .collect::<Vec<_>>();
        recent_goal_logs.sort_by(|left, right| {
            left.datetime
                .cmp(&right.datetime)
                .then_with(|| left.id.cmp(&right.id))
        });
        let keep_from = recent_goal_logs.len().saturating_sub(200);
        for entry in recent_goal_logs.drain(keep_from..) {
            events.push(SseEventFrame {
                event: "goal_log_added",
                data: json!(entry),
            });
        }
        if let Some(refine_dir) = self.server.current_refine_dir()? {
            if let Some(entry) = FileActivityService::new(&refine_dir)
                .recent(1)?
                .into_iter()
                .next()
            {
                events.push(SseEventFrame {
                    event: "activity_added",
                    data: json!(entry),
                });
            }
            for payload in recent_chat_sse_events(&refine_dir, 10)? {
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

#[cfg(test)]
fn run_agent_automation_once(server: &InProcessWebServer) -> RefineResult<()> {
    let Some(runtime_root) = &server.runtime_root else {
        return Ok(());
    };
    let Some(target_root) = server.current_target_root()? else {
        return Ok(());
    };
    let automation = WorkflowEngine::with_target_root(runtime_root, target_root);
    let result = automation.evaluate_workflow();
    let refresh_result = server.refresh_projection_cache_after_mutation();
    result?;
    refresh_result?;
    Ok(())
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GitSyncSchedule {
    debounce: Duration,
    remote_fetch_interval: Option<Duration>,
}

#[cfg(test)]
impl Default for GitSyncSchedule {
    fn default() -> Self {
        Self {
            debounce: DEFAULT_GIT_SYNC_DEBOUNCE,
            remote_fetch_interval: Some(DEFAULT_REMOTE_FETCH_INTERVAL),
        }
    }
}

#[cfg(test)]
fn git_sync_configuration(server: &InProcessWebServer) -> RefineResult<GitSyncSchedule> {
    let Some(runtime_root) = &server.runtime_root else {
        return Ok(GitSyncSchedule::default());
    };
    let Some(refine_dir) = server.current_refine_dir()? else {
        return Ok(GitSyncSchedule::default());
    };
    let settings = FileSettingsService::with_active_root(refine_dir, runtime_root).load()?;
    Ok(GitSyncSchedule {
        debounce: positive_duration(
            settings.get("state_sync_debounce_seconds"),
            DEFAULT_GIT_SYNC_DEBOUNCE,
        ),
        remote_fetch_interval: optional_positive_duration(
            settings.get("project_update_pulse_interval_seconds"),
            DEFAULT_REMOTE_FETCH_INTERVAL,
        ),
    })
}

#[cfg(test)]
fn positive_duration(value: Option<&Value>, fallback: Duration) -> Duration {
    let seconds = value
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback.as_secs() as i64);
    if seconds <= 0 {
        return fallback;
    }
    Duration::from_secs(seconds as u64)
}

#[cfg(test)]
fn optional_positive_duration(value: Option<&Value>, fallback: Duration) -> Option<Duration> {
    let Some(value) = value.and_then(Value::as_str) else {
        return Some(fallback);
    };
    let Ok(seconds) = value.trim().parse::<i64>() else {
        return Some(fallback);
    };
    (seconds > 0).then(|| Duration::from_secs(seconds as u64))
}

#[cfg(test)]
mod git_sync_schedule_tests {
    use super::*;

    #[test]
    fn state_sync_schedule_defaults_and_normalizes() {
        assert_eq!(
            positive_duration(None, DEFAULT_GIT_SYNC_DEBOUNCE),
            Duration::from_secs(5)
        );
        assert_eq!(
            positive_duration(
                Some(&Value::String("30".to_string())),
                DEFAULT_GIT_SYNC_DEBOUNCE
            ),
            Duration::from_secs(30)
        );
        assert_eq!(
            optional_positive_duration(None, DEFAULT_REMOTE_FETCH_INTERVAL),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            optional_positive_duration(
                Some(&Value::String("-1".to_string())),
                DEFAULT_REMOTE_FETCH_INTERVAL
            ),
            None
        );
    }
}

fn sleep_until_stopped(stop: &AtomicBool, duration: Duration) {
    let deadline = Instant::now() + duration;
    while !stop.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::sleep(remaining.min(Duration::from_secs(1)));
    }
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
            | "/work/goals"
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
    ) || normalized.starts_with("/work/goals/")
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

fn website_index_path(static_root: &Path) -> PathBuf {
    static_root.join("src/surfaces/website/index.html")
}

fn website_public_path_allowed(path: &str) -> bool {
    path == "README.md"
        || path == "LICENSE"
        || path == "scripts/install.sh"
        || path.starts_with("docs/")
        || path == "src/surfaces/website/index.html"
        || path.starts_with("src/surfaces/website/assets/")
        || path.starts_with("src/surfaces/web/static/images/")
}

fn render_markdown_document(static_root: &Path, path: &str) -> WireResponse {
    let requested = path
        .trim_start_matches("/read")
        .trim_start_matches('/')
        .trim();
    let document = if requested.is_empty() {
        "docs/intent/README.md"
    } else {
        requested
    };
    if !website_markdown_path_allowed(document) {
        return WireResponse::json(ApiResponse::json(
            400,
            json!({
                "error": {
                    "code": "invalid_markdown_path",
                    "message": "markdown reader only serves README.md and docs/**/*.md"
                }
            }),
        ));
    }
    let full_path = static_root.join(document);
    if !is_within(static_root, &full_path) || !full_path.is_file() {
        return WireResponse::json(ApiResponse::json(
            404,
            json!({
                "error": {
                    "code": "markdown_not_found",
                    "message": "markdown document was not found"
                }
            }),
        ));
    }
    let markdown = match fs::read_to_string(&full_path) {
        Ok(markdown) => markdown,
        Err(error) => {
            return WireResponse::json(ApiResponse::json(
                500,
                json!({
                    "error": {
                        "code": "markdown_read_failed",
                        "message": format!("failed to read markdown document: {error}")
                    }
                }),
            ));
        }
    };
    WireResponse::bytes(
        200,
        "text/html; charset=utf-8",
        render_markdown_html(document, &markdown).into_bytes(),
    )
}

fn website_markdown_path_allowed(path: &str) -> bool {
    !path.contains("..")
        && (path == "README.md" || (path.starts_with("docs/") && path.ends_with(".md")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DocsNavItem {
    title: String,
    path: String,
}

struct DocsNavGroup {
    title: &'static str,
    items: &'static [DocsNavEntry],
}

struct DocsNavEntry {
    title: &'static str,
    path: &'static str,
    summary: &'static str,
}

const DOCS_NAV_GROUPS: &[DocsNavGroup] = &[
    DocsNavGroup {
        title: "Start Here",
        items: &[
            DocsNavEntry {
                title: "Design Intent",
                path: "docs/intent/README.md",
                summary: "The durable product principles behind Refine.",
            },
            DocsNavEntry {
                title: "System Design",
                path: "docs/intent/01-design.md",
                summary: "The whole-system model for local agentic delivery.",
            },
        ],
    },
    DocsNavGroup {
        title: "Foundation",
        items: &[
            DocsNavEntry {
                title: "Node",
                path: "docs/intent/02-foundation/01-node.md",
                summary: "How Refine understands local and distributed execution.",
            },
            DocsNavEntry {
                title: "Models",
                path: "docs/intent/02-foundation/02-models.md",
                summary: "The durable records agents and people work through.",
            },
            DocsNavEntry {
                title: "Target App",
                path: "docs/intent/02-foundation/03-target-app.md",
                summary: "The software project Refine is helping improve.",
            },
        ],
    },
    DocsNavGroup {
        title: "Capabilities",
        items: &[
            DocsNavEntry {
                title: "Process",
                path: "docs/intent/03-capabilities/01-process.md",
                summary: "Observable local processes, checks, and runtime work.",
            },
            DocsNavEntry {
                title: "Agents",
                path: "docs/intent/03-capabilities/02-agents/00-overview.md",
                summary: "How agents receive context, act, and leave evidence.",
            },
            DocsNavEntry {
                title: "Agent Tools",
                path: "docs/intent/03-capabilities/02-agents/01-tools/00-overview.md",
                summary: "Shared operations that let agents work consistently.",
            },
            DocsNavEntry {
                title: "Import",
                path: "docs/intent/03-capabilities/02-agents/01-tools/01-import.md",
                summary: "Turning external input into structured Refine work.",
            },
            DocsNavEntry {
                title: "Guidance",
                path: "docs/intent/03-capabilities/02-agents/02-guidance.md",
                summary: "The context agents need before they change code.",
            },
            DocsNavEntry {
                title: "Quality",
                path: "docs/intent/03-capabilities/02-agents/03-quality.md",
                summary: "How Refine keeps checks close to the work.",
            },
            DocsNavEntry {
                title: "Governance",
                path: "docs/intent/03-capabilities/02-agents/04-governance.md",
                summary: "Product rules and review constraints for agent work.",
            },
            DocsNavEntry {
                title: "Merge, Review, And Git Worktrees",
                path: "docs/intent/03-capabilities/02-agents/05-merge-review-git-worktrees.md",
                summary: "How agent work becomes reviewable Git history.",
            },
            DocsNavEntry {
                title: "Activity And Evidence",
                path: "docs/intent/03-capabilities/02-agents/06-activity-evidence.md",
                summary: "The audit trail that makes automation inspectable.",
            },
        ],
    },
    DocsNavGroup {
        title: "Workflow",
        items: &[
            DocsNavEntry {
                title: "Workflow Overview",
                path: "docs/intent/03-capabilities/03-workflow/00-overview.md",
                summary: "The lifecycle that carries Goals from idea to done.",
            },
            DocsNavEntry {
                title: "Backlog",
                path: "docs/intent/03-capabilities/03-workflow/01-backlog.md",
                summary: "Captured work that is not ready to consume capacity.",
            },
            DocsNavEntry {
                title: "Todo",
                path: "docs/intent/03-capabilities/03-workflow/02-todo.md",
                summary: "Ready work that agents or people can pick up.",
            },
            DocsNavEntry {
                title: "In Progress",
                path: "docs/intent/03-capabilities/03-workflow/03-in-progress.md",
                summary: "Active work with implementation context.",
            },
            DocsNavEntry {
                title: "Ready Merge",
                path: "docs/intent/03-capabilities/03-workflow/04-ready-merge.md",
                summary: "Work ready for integration discipline.",
            },
            DocsNavEntry {
                title: "Build",
                path: "docs/intent/03-capabilities/03-workflow/05-build.md",
                summary: "Build verification as part of delivery flow.",
            },
            DocsNavEntry {
                title: "QA",
                path: "docs/intent/03-capabilities/03-workflow/06-qa.md",
                summary: "Quality checks before review and completion.",
            },
            DocsNavEntry {
                title: "Review",
                path: "docs/intent/03-capabilities/03-workflow/07-review.md",
                summary: "Human and automated inspection of changes.",
            },
            DocsNavEntry {
                title: "Done",
                path: "docs/intent/03-capabilities/03-workflow/08-done.md",
                summary: "Completed work with preserved evidence.",
            },
            DocsNavEntry {
                title: "Failed",
                path: "docs/intent/03-capabilities/03-workflow/09-failed.md",
                summary: "Failures that remain inspectable and recoverable.",
            },
            DocsNavEntry {
                title: "Cancelled",
                path: "docs/intent/03-capabilities/03-workflow/10-cancelled.md",
                summary: "Work intentionally stopped without losing context.",
            },
        ],
    },
    DocsNavGroup {
        title: "Surfaces",
        items: &[
            DocsNavEntry {
                title: "Surface Principles",
                path: "docs/intent/04-surfaces/01-surface-principles.md",
                summary: "Why interfaces are adapters over shared capability.",
            },
            DocsNavEntry {
                title: "CLI",
                path: "docs/intent/04-surfaces/02-cli.md",
                summary: "The most reliable surface for people and agents.",
            },
            DocsNavEntry {
                title: "Browser Desktop",
                path: "docs/intent/04-surfaces/03-browser-desktop/00-overview.md",
                summary: "The visual product surface for local software work.",
            },
            DocsNavEntry {
                title: "API",
                path: "docs/intent/04-surfaces/04-api.md",
                summary: "Shared local access for product operations.",
            },
            DocsNavEntry {
                title: "Agent",
                path: "docs/intent/04-surfaces/05-agent.md",
                summary: "How agents understand Refine through the CLI.",
            },
        ],
    },
    DocsNavGroup {
        title: "Browser Details",
        items: &[
            DocsNavEntry {
                title: "Shared Components",
                path: "docs/intent/04-surfaces/03-browser-desktop/01-shared-components/00-overview.md",
                summary: "Reusable interaction patterns for the visual UI.",
            },
            DocsNavEntry {
                title: "Table",
                path: "docs/intent/04-surfaces/03-browser-desktop/01-shared-components/01-table.md",
                summary: "Dense, scannable data views for repeated work.",
            },
            DocsNavEntry {
                title: "Pagination",
                path: "docs/intent/04-surfaces/03-browser-desktop/01-shared-components/02-pagination.md",
                summary: "Navigation for long local records without losing context.",
            },
            DocsNavEntry {
                title: "Nav",
                path: "docs/intent/04-surfaces/03-browser-desktop/02-nav.md",
                summary: "Orientation and movement through the browser product.",
            },
            DocsNavEntry {
                title: "Command Palette",
                path: "docs/intent/04-surfaces/03-browser-desktop/03-command-palette.md",
                summary: "Fast command access without hunting through screens.",
            },
            DocsNavEntry {
                title: "Main",
                path: "docs/intent/04-surfaces/03-browser-desktop/04-main.md",
                summary: "The primary browser workspace.",
            },
            DocsNavEntry {
                title: "Dashboard",
                path: "docs/intent/04-surfaces/03-browser-desktop/05-dashboard.md",
                summary: "A high-level view of work, state, and attention.",
            },
            DocsNavEntry {
                title: "Workflow",
                path: "docs/intent/04-surfaces/03-browser-desktop/06-workflow.md",
                summary: "The browser view of work movement.",
            },
            DocsNavEntry {
                title: "Feature",
                path: "docs/intent/04-surfaces/03-browser-desktop/07-feature.md",
                summary: "Larger product goals composed from Goals.",
            },
            DocsNavEntry {
                title: "Goal",
                path: "docs/intent/04-surfaces/03-browser-desktop/08-goal.md",
                summary: "The core unit of product difference and repair.",
            },
            DocsNavEntry {
                title: "Import",
                path: "docs/intent/04-surfaces/03-browser-desktop/09-import.md",
                summary: "Bringing outside work into the browser surface.",
            },
            DocsNavEntry {
                title: "Changes Visualizations",
                path: "docs/intent/04-surfaces/03-browser-desktop/10-changes-visualizations.md",
                summary: "Readable change views for review and understanding.",
            },
            DocsNavEntry {
                title: "Log",
                path: "docs/intent/04-surfaces/03-browser-desktop/11-log.md",
                summary: "Evidence and event history in the browser.",
            },
            DocsNavEntry {
                title: "Settings",
                path: "docs/intent/04-surfaces/03-browser-desktop/12-settings.md",
                summary: "Project configuration, governance, and local controls.",
            },
            DocsNavEntry {
                title: "Guide",
                path: "docs/intent/04-surfaces/03-browser-desktop/13-guide.md",
                summary: "Contextual help close to the current task.",
            },
            DocsNavEntry {
                title: "Target App",
                path: "docs/intent/04-surfaces/03-browser-desktop/14-target-app.md",
                summary: "The attached software project inside the UI.",
            },
            DocsNavEntry {
                title: "Toolbar",
                path: "docs/intent/04-surfaces/03-browser-desktop/15-toolbar.md",
                summary: "Persistent tools, chat, files, and terminal access.",
            },
            DocsNavEntry {
                title: "System",
                path: "docs/intent/04-surfaces/03-browser-desktop/16-system.md",
                summary: "Runtime, install, and operational state.",
            },
            DocsNavEntry {
                title: "Processes",
                path: "docs/intent/04-surfaces/03-browser-desktop/17-processes.md",
                summary: "Process visibility and control in the browser.",
            },
            DocsNavEntry {
                title: "Files",
                path: "docs/intent/04-surfaces/03-browser-desktop/18-files.md",
                summary: "File inspection close to agent work.",
            },
            DocsNavEntry {
                title: "Terminal",
                path: "docs/intent/04-surfaces/03-browser-desktop/19-terminal.md",
                summary: "Command execution without leaving the product context.",
            },
            DocsNavEntry {
                title: "Chat",
                path: "docs/intent/04-surfaces/03-browser-desktop/20-chat.md",
                summary: "Conversation tied to work and evidence.",
            },
            DocsNavEntry {
                title: "Standalone",
                path: "docs/intent/04-surfaces/03-browser-desktop/21-standalone.md",
                summary: "Focused browser use outside the primary workspace.",
            },
            DocsNavEntry {
                title: "Footer",
                path: "docs/intent/04-surfaces/03-browser-desktop/22-footer.md",
                summary: "Low-priority navigation and product affordances.",
            },
        ],
    },
    DocsNavGroup {
        title: "Install",
        items: &[DocsNavEntry {
            title: "Agent Install Runbook",
            path: "docs/runbooks/install.md",
            summary: "A direct install path for coding agents.",
        }],
    },
];

fn docs_nav_entries() -> impl Iterator<Item = &'static DocsNavEntry> {
    DOCS_NAV_GROUPS.iter().flat_map(|group| group.items.iter())
}

fn render_markdown_html(path: &str, markdown: &str) -> String {
    let markdown = markdown_for_website(path, markdown);
    let rendered = render_markdown_fragment(path, &markdown);
    let nav = render_docs_nav(path);
    let pager = render_docs_pager(path, &docs_nav_items());
    let escaped_path = escape_html(path);
    let escaped_title = escape_html(&docs_title_for_path(path));
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{escaped_title} - Refine Docs</title>
    <link rel="icon" href="/src/surfaces/web/static/images/favicon.svg" type="image/svg+xml">
    <link rel="stylesheet" href="/src/surfaces/website/assets/site.css">
    <script src="/src/surfaces/website/assets/site.js" defer></script>
  </head>
  <body class="reader-body">
    <header class="site-header">
      <a class="brand" href="/" aria-label="Refine home">
        <img src="/src/surfaces/web/static/images/refine_logo_transparent.png" alt="">
      </a>
      <div class="site-menu-shell">
        <button
          class="menu-button"
          type="button"
          data-menu-toggle
          aria-controls="site-menu"
          aria-expanded="false"
          aria-label="Open navigation menu"
        >
          <svg aria-hidden="true" viewBox="0 0 24 24">
            <path d="M4 7h16"></path>
            <path d="M4 12h16"></path>
            <path d="M4 17h16"></path>
          </svg>
        </button>
        <div class="site-menu" id="site-menu" data-menu-panel hidden>
          <nav aria-label="Primary">
            <a href="/">Home</a>
            <a href="/#product">Product</a>
            <a href="/#start">Get Started</a>
            <a href="/docs">Docs</a>
            <a href="/#agents">Agents</a>
            <a href="/read/docs/runbooks/install.md">Install</a>
            <a href="/{escaped_path}">Raw Markdown</a>
            <a href="https://github.com/buwilliams/refine">GitHub</a>
          </nav>
          <div class="menu-docs" aria-label="Documentation sections">
            {nav}
          </div>
        </div>
      </div>
    </header>
    <main class="reader-shell">
      <article class="markdown-card">
        <div class="doc-meta">{escaped_path}</div>
        {pager}
        <div class="markdown-content">{rendered}</div>
        {pager}
      </article>
    </main>
  </body>
</html>"#
    )
}

fn render_docs_landing_page() -> WireResponse {
    let nav = render_docs_nav("docs");
    let groups = render_docs_landing_groups();
    WireResponse::bytes(
        200,
        "text/html; charset=utf-8",
        format!(
            r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Refine Product Guide</title>
    <meta
      name="description"
      content="A web-first guide to Refine's product model, agent workflow, governance, quality, and local software delivery surfaces."
    >
    <link rel="icon" href="/src/surfaces/web/static/images/favicon.svg" type="image/svg+xml">
    <link rel="stylesheet" href="/src/surfaces/website/assets/site.css">
    <script src="/src/surfaces/website/assets/site.js" defer></script>
  </head>
  <body class="reader-body docs-home-body">
    <header class="site-header">
      <a class="brand" href="/" aria-label="Refine home">
        <img src="/src/surfaces/web/static/images/refine_logo_transparent.png" alt="">
      </a>
      <div class="site-menu-shell">
        <button
          class="menu-button"
          type="button"
          data-menu-toggle
          aria-controls="site-menu"
          aria-expanded="false"
          aria-label="Open navigation menu"
        >
          <svg aria-hidden="true" viewBox="0 0 24 24">
            <path d="M4 7h16"></path>
            <path d="M4 12h16"></path>
            <path d="M4 17h16"></path>
          </svg>
        </button>
        <div class="site-menu" id="site-menu" data-menu-panel hidden>
          <nav aria-label="Primary">
            <a href="/">Home</a>
            <a href="/#product">Product</a>
            <a href="/#start">Get Started</a>
            <a href="/docs" aria-current="page">Docs</a>
            <a href="/read/docs/runbooks/install.md">Install</a>
            <a href="https://github.com/buwilliams/refine">GitHub</a>
          </nav>
          <div class="menu-docs" aria-label="Documentation sections">
            {nav}
          </div>
        </div>
      </div>
    </header>
    <main class="docs-home-shell">
      <section class="docs-hero" aria-labelledby="docs-home-title">
        <p class="eyebrow">Product Guide</p>
        <h1 id="docs-home-title">How Refine works.</h1>
        <p>
          Refine is a local operating layer for agentic software delivery. It turns product goals
          into durable work, gives agents enough context to act, keeps processes observable, and
          uses Git, review, governance, and quality checks to make automation inspectable.
        </p>
        <div class="hero-actions" aria-label="Docs starting points">
          <a class="button primary" href="/read/docs/intent/01-design.md">Read the system design</a>
          <a class="button secondary" href="/read/docs/runbooks/install.md">Install with an agent</a>
        </div>
      </section>

      <section class="docs-orientation" aria-labelledby="docs-orientation-title">
        <div>
          <p class="eyebrow">The Model</p>
          <h2 id="docs-orientation-title">One product, three layers.</h2>
        </div>
        <div class="docs-orientation-grid">
          <article>
            <span>01</span>
            <h3>Foundation</h3>
            <p>Node, model, and target-app concepts keep state durable, local, and readable.</p>
          </article>
          <article>
            <span>02</span>
            <h3>Capabilities</h3>
            <p>Process, agents, and workflow are shared powers that every surface can use.</p>
          </article>
          <article>
            <span>03</span>
            <h3>Surfaces</h3>
            <p>CLI, browser, API, desktop, and agent interfaces adapt the same underlying system.</p>
          </article>
        </div>
      </section>

      <section class="docs-map" aria-labelledby="docs-map-title">
        <div class="section-heading">
          <p class="eyebrow">Browse</p>
          <h2 id="docs-map-title">Choose the path that matches what you are trying to understand.</h2>
        </div>
        {groups}
      </section>
    </main>
  </body>
</html>"#
        )
        .into_bytes(),
    )
}

fn render_docs_nav(current_path: &str) -> String {
    let docs_home_current = if current_path == "docs" {
        r#" aria-current="page""#
    } else {
        ""
    };
    let mut html = format!(
        r#"<h2>Docs</h2>
        <a class="docs-home-link" href="/docs"{docs_home_current}>Product Guide</a>"#,
    );
    for group in DOCS_NAV_GROUPS {
        html.push_str(&format!(
            r#"<section class="docs-nav-group"><h3>{}</h3><ul>"#,
            escape_html(group.title)
        ));
        for item in group.items {
            let current = if item.path == current_path {
                r#" aria-current="page""#
            } else {
                ""
            };
            html.push_str(&format!(
                r#"<li><a href="/read/{}"{}>{}</a></li>"#,
                escape_html(item.path),
                current,
                escape_html(item.title)
            ));
        }
        html.push_str("</ul></section>");
    }
    html
}

fn render_docs_landing_groups() -> String {
    let mut html = String::from(r#"<div class="docs-group-list">"#);
    for group in DOCS_NAV_GROUPS {
        html.push_str(&format!(
            r#"<section class="docs-group-card" aria-labelledby="docs-group-{}"><h3 id="docs-group-{}">{}</h3><div class="docs-link-list">"#,
            slugify_docs_id(group.title),
            slugify_docs_id(group.title),
            escape_html(group.title)
        ));
        for item in group.items {
            html.push_str(&format!(
                r#"<a class="docs-link-card" href="/read/{}"><strong>{}</strong><span>{}</span></a>"#,
                escape_html(item.path),
                escape_html(item.title),
                escape_html(item.summary)
            ));
        }
        html.push_str("</div></section>");
    }
    html.push_str("</div>");
    html
}

fn docs_nav_items() -> Vec<DocsNavItem> {
    docs_nav_entries()
        .map(|entry| DocsNavItem {
            title: entry.title.to_string(),
            path: entry.path.to_string(),
        })
        .collect()
}

fn docs_title_for_path(path: &str) -> String {
    docs_nav_entries()
        .find(|entry| entry.path == path)
        .map(|entry| entry.title.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn render_docs_pager(path: &str, items: &[DocsNavItem]) -> String {
    let current_index = items.iter().position(|item| item.path == path);
    let previous = current_index
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| items.get(index));
    let next = current_index.and_then(|index| items.get(index + 1));
    format!(
        r#"<nav class="doc-pager" aria-label="Document navigation">
          {}
          <a class="doc-pager-toc" href="/docs">Docs home</a>
          {}
        </nav>"#,
        render_docs_pager_edge("Previous", previous),
        render_docs_pager_edge("Next", next)
    )
}

fn render_docs_pager_edge(label: &str, item: Option<&DocsNavItem>) -> String {
    match item {
        Some(item) => format!(
            r#"<a class="doc-pager-link" href="/read/{}"><span>{}</span><strong>{}</strong></a>"#,
            escape_html(&item.path),
            label,
            escape_html(&item.title)
        ),
        None => format!(
            r#"<span class="doc-pager-link doc-pager-link-disabled" aria-disabled="true"><span>{label}</span><strong>{}</strong></span>"#,
            if label == "Previous" { "Start" } else { "End" }
        ),
    }
}

fn markdown_for_website(path: &str, markdown: &str) -> String {
    if path != "docs/intent/README.md" {
        return markdown.to_string();
    }
    let body = markdown
        .find("\n## Key Ideas")
        .map(|index| &markdown[index + 1..])
        .unwrap_or(markdown);
    format!(
        "# Design Intent\n\nThese are the durable product principles behind Refine. The full source tree still keeps its GitHub-friendly table of contents, but this web version starts with the ideas a product reader needs first.\n\n{body}"
    )
}

fn slugify_docs_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn render_markdown_fragment(path: &str, markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser =
        Parser::new_ext(markdown, options).map(|event| rewrite_markdown_event(path, event));
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    rendered
}

fn rewrite_markdown_event<'a>(path: &str, event: MarkdownEvent<'a>) -> MarkdownEvent<'a> {
    match event {
        MarkdownEvent::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => MarkdownEvent::Start(Tag::Link {
            link_type,
            dest_url: CowStr::from(rewrite_markdown_link(path, dest_url.as_ref())),
            title,
            id,
        }),
        other => other,
    }
}

fn rewrite_markdown_link(path: &str, destination: &str) -> String {
    if destination.starts_with("http:")
        || destination.starts_with("https:")
        || destination.starts_with("mailto:")
        || destination.starts_with('#')
        || destination.starts_with('/')
    {
        return destination.to_string();
    }

    let (target, anchor) = destination
        .split_once('#')
        .map(|(target, anchor)| (target, format!("#{anchor}")))
        .unwrap_or((destination, String::new()));
    if !target.ends_with(".md") {
        return destination.to_string();
    }

    let base = path.rsplit_once('/').map(|(base, _)| base).unwrap_or("");
    let joined = if base.is_empty() {
        target.to_string()
    } else {
        format!("{base}/{target}")
    };
    match normalize_markdown_path(&joined) {
        Some(normalized) => format!("/read/{normalized}{anchor}"),
        None => destination.to_string(),
    }
}

fn normalize_markdown_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value),
        }
    }
    Some(parts.join("/"))
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("md") | Some("markdown") => "text/markdown; charset=utf-8",
        Some("sh") => "text/x-shellscript; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
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
