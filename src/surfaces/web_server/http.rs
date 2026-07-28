mod background_loops;
mod daemon_adapter;
mod docs;
mod events;
mod runtime;
mod static_content;
mod transport;
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
use docs::*;
#[cfg(test)]
use events::should_write_sse_frame;
use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag, html};
use serde_json::{Value, json};
use static_content::*;
use tokio::sync::{Notify, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::process::agent_sessions::find_agent_session;
#[cfg(not(test))]
use crate::process::runner::{FileRunnerWorkerService, GIT_SYNC_RUNNER, WORKFLOW_RUNNER};
#[cfg(test)]
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    DaemonRuntimeService, DaemonStatus, FileDaemonLifecycleService,
};
use crate::process::supervisor::operations::FileOperationRegistry;
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

fn terminal_events_snapshot_requested(path: &str) -> bool {
    path.split_once('?')
        .map(|(_, query)| {
            query.split('&').any(|part| {
                let (key, value) = part.split_once('=').unwrap_or((part, ""));
                key == "snapshot" && value == "1"
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod terminal_event_route_tests {
    use super::*;

    #[test]
    fn terminal_event_snapshot_query_uses_finite_json_transport() {
        let path = "/api/terminal/session-1/events?snapshot=1&after=42";
        assert_eq!(terminal_events_route(path).as_deref(), Some("session-1"));
        assert!(terminal_events_snapshot_requested(path));
        assert!(!terminal_events_snapshot_requested(
            "/api/terminal/session-1/events?after=42"
        ));
    }

    #[test]
    fn operation_progress_sse_deduplicates_timestamp_only_replays() {
        let first = SseEventFrame {
            event: "operation_progress",
            data: json!({
                "operation": {"id": "op-1", "status": "running"},
                "timestamp": "first"
            }),
        };
        let replay = SseEventFrame {
            event: "operation_progress",
            data: json!({
                "operation": {"id": "op-1", "status": "running"},
                "timestamp": "second"
            }),
        };
        let mut states = BTreeMap::new();
        let mut streams = BTreeMap::new();

        assert!(should_write_sse_frame(&first, &mut states, &mut streams));
        assert!(!should_write_sse_frame(&replay, &mut states, &mut streams));
    }
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
