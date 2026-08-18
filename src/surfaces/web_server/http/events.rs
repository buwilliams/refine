use super::*;
use crate::infrastructure::process::subprocess::FileProcessSupervisor;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SsePathFingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug)]
struct SseFrameState {
    projection: Arc<ProjectionSnapshot>,
    process_output_paths: BTreeSet<PathBuf>,
    auxiliary: BTreeMap<PathBuf, SsePathFingerprint>,
    state_sync_health: Option<StateSyncHealthFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StateSyncHealthFingerprint {
    node_id: String,
    status: String,
    last_success_at: Option<String>,
    failure_since: Option<String>,
    stale_since: Option<String>,
    last_error: Option<String>,
    last_attempt_id: Option<u64>,
    last_attempt_source: Option<String>,
    last_conflict_report_id: Option<String>,
}

impl From<&StateSyncHealth> for StateSyncHealthFingerprint {
    fn from(health: &StateSyncHealth) -> Self {
        Self {
            node_id: health.node_id.clone(),
            status: health.status.clone(),
            last_success_at: health.last_success_at.clone(),
            failure_since: health.failure_since.clone(),
            stale_since: health.stale_since.clone(),
            last_error: health.last_error.clone(),
            last_attempt_id: health.last_attempt_id,
            last_attempt_source: health.last_attempt_source.clone(),
            last_conflict_report_id: health.last_conflict_report_id.clone(),
        }
    }
}

impl SseFrameState {
    fn same_inputs(&self, previous: &Self) -> bool {
        Arc::ptr_eq(&self.projection, &previous.projection)
            && self.auxiliary == previous.auxiliary
            && self.state_sync_health == previous.state_sync_health
    }
}

impl LocalHttpDaemon {
    pub(super) fn sse_response(&self, stream_name: &'static str) -> Response {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);
        let mut batches = self.subscribe_sse_frame_batches(stream_name);
        tokio::spawn(async move {
            let mut last_state_signatures = BTreeMap::new();
            let mut seen_stream_signatures: BTreeMap<&'static str, BTreeSet<String>> =
                BTreeMap::new();
            loop {
                // The recv is bounded so the stream can observe shutdown and
                // finish; an SSE connection that never ends would hold the
                // graceful drain open forever.
                let received =
                    match tokio::time::timeout(Duration::from_secs(1), batches.recv()).await {
                        Ok(received) => received,
                        Err(_) => {
                            if DAEMON_SHUTTING_DOWN.load(Ordering::SeqCst) {
                                break;
                            }
                            continue;
                        }
                    };
                let frames = match received {
                    Ok(Ok(frames)) => frames,
                    Ok(Err(error)) => {
                        let event = Event::default()
                            .event("error")
                            .data(json!({"message": error.as_ref()}).to_string());
                        let _ = tx.send(Ok(event)).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                for frame in frames.iter() {
                    if should_write_sse_frame(
                        frame,
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

    pub(in crate::surfaces::web_server) fn subscribe_sse_frame_batches(
        &self,
        stream_name: &'static str,
    ) -> broadcast::Receiver<SseFrameBatch> {
        let receiver = self.sse_frame_hub.sender.subscribe();
        let start_producer = {
            let mut producer_running = self
                .sse_frame_hub
                .producer_running
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let start = !*producer_running;
            *producer_running = true;
            start
        };
        if let Some(batch) = self
            .sse_frame_hub
            .latest_batch
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            let _ = self.sse_frame_hub.sender.send(batch);
        }
        if start_producer {
            let daemon = self.clone();
            tokio::spawn(async move {
                daemon.produce_sse_frame_batches(stream_name).await;
            });
        }
        receiver
    }

    async fn produce_sse_frame_batches(self, stream_name: &'static str) {
        let mut previous_state = None;
        loop {
            if DAEMON_SHUTTING_DOWN.load(Ordering::SeqCst) {
                self.mark_sse_frame_producer_stopped();
                return;
            }
            if self.stop_sse_frame_producer_when_idle() {
                return;
            }
            let frame_daemon = self.clone();
            let prior = previous_state.take();
            let poll = match tokio::task::spawn_blocking(move || {
                frame_daemon.changed_sse_frame_batch(stream_name, prior)
            })
            .await
            {
                Ok(Ok(poll)) => poll,
                Ok(Err(error)) => {
                    let batch = Err(Arc::<str>::from(error.to_string()));
                    let _ = self.sse_frame_hub.sender.send(batch);
                    self.mark_sse_frame_producer_stopped();
                    return;
                }
                Err(error) => {
                    let batch = Err(Arc::<str>::from(format!(
                        "SSE projection worker failed: {error}"
                    )));
                    let _ = self.sse_frame_hub.sender.send(batch);
                    self.mark_sse_frame_producer_stopped();
                    return;
                }
            };
            let (state, frames) = poll;
            previous_state = Some(state);
            let Some(frames) = frames else {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            };
            #[cfg(test)]
            self.sse_frame_hub
                .build_count
                .fetch_add(1, Ordering::Relaxed);
            let batch = Ok(Arc::new(frames));
            *self
                .sse_frame_hub
                .latest_batch
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(batch.clone());
            let _ = self.sse_frame_hub.sender.send(batch);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    fn stop_sse_frame_producer_when_idle(&self) -> bool {
        let mut producer_running = self
            .sse_frame_hub
            .producer_running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.sse_frame_hub.sender.receiver_count() != 0 {
            return false;
        }
        *producer_running = false;
        true
    }

    fn mark_sse_frame_producer_stopped(&self) {
        *self
            .sse_frame_hub
            .producer_running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
    }

    #[cfg(test)]
    pub(in crate::surfaces::web_server) fn sse_frame_build_count(&self) -> usize {
        self.sse_frame_hub.build_count.load(Ordering::Relaxed)
    }

    pub(super) fn terminal_sse_response(&self, session_id: String, initial_after: u64) -> Response {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
        let runtime_root = self.server.runtime_root.clone();
        tokio::spawn(async move {
            let mut after = initial_after;
            let mut last_attention = None;
            let mut attached = false;
            loop {
                if DAEMON_SHUTTING_DOWN.load(Ordering::SeqCst) {
                    return;
                }
                let Some(runtime_root) = runtime_root.as_deref() else {
                    let event = Event::default()
                        .event("terminal_error")
                        .data(json!({"message": "runtime root is unavailable"}).to_string());
                    let _ = tx.send(Ok(event)).await;
                    return;
                };
                match terminal_events_since(runtime_root, &session_id, after) {
                    Ok(events) => {
                        attached = true;
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
                        if let Ok(status) = find_agent_session(runtime_root, &session_id) {
                            let attention = (
                                status.attention_state.clone(),
                                status.attention_message.clone(),
                            );
                            if last_attention.as_ref() != Some(&attention) {
                                last_attention = Some(attention);
                                let payload =
                                    serde_json::to_value(status).unwrap_or_else(|_| json!({}));
                                if tx
                                    .send(Ok(Event::default()
                                        .event("terminal_status")
                                        .data(payload.to_string())))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                    }
                    Err(RefineError::NotFound(_)) if attached => {
                        let event = Event::default()
                            .event("terminal_exit")
                            .data(json!({"data": "closed"}).to_string());
                        let _ = tx.send(Ok(event)).await;
                        return;
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

    pub(super) fn server_sent_event_frames(
        &self,
        stream: &str,
    ) -> RefineResult<Vec<SseEventFrame>> {
        let projection = self.server.current_projection_with_runtime_shared()?;
        self.server_sent_event_frames_for_projection(stream, &projection)
    }

    fn changed_sse_frame_batch(
        &self,
        stream: &str,
        previous: Option<SseFrameState>,
    ) -> RefineResult<(SseFrameState, Option<Vec<SseEventFrame>>)> {
        let state = self.sse_frame_state(previous.as_ref())?;
        if previous
            .as_ref()
            .is_some_and(|previous| state.same_inputs(previous))
        {
            return Ok((state, None));
        }
        let frames = self.server_sent_event_frames_for_projection(stream, &state.projection)?;
        Ok((state, Some(frames)))
    }

    fn sse_frame_state(&self, previous: Option<&SseFrameState>) -> RefineResult<SseFrameState> {
        let projection = self.server.current_projection_with_runtime_shared()?;
        let projection_changed =
            previous.is_none_or(|previous| !Arc::ptr_eq(&projection, &previous.projection));
        let process_output_paths = if projection_changed {
            active_process_output_paths(self.server.runtime_root.as_deref())?
        } else {
            previous
                .map(|previous| previous.process_output_paths.clone())
                .unwrap_or_default()
        };
        let auxiliary = sse_auxiliary_fingerprint(
            self.server.runtime_root.as_deref(),
            &projection,
            &process_output_paths,
        )?;
        let state_sync_health = self
            .server
            .current_state_sync_health()?
            .as_ref()
            .map(StateSyncHealthFingerprint::from);
        Ok(SseFrameState {
            projection,
            process_output_paths,
            auxiliary,
            state_sync_health,
        })
    }

    fn server_sent_event_frames_for_projection(
        &self,
        stream: &str,
        projection: &ProjectionSnapshot,
    ) -> RefineResult<Vec<SseEventFrame>> {
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
                    "dashboard": &projection.dashboard
                }),
            },
            SseEventFrame {
                event: "status_change",
                data: json!({
                    "status_counts": projection.status_counts(),
                    "attention": &projection.dashboard.attention_indicators
                }),
            },
            SseEventFrame {
                event: "runtime_change",
                data: json!({
                    "processes": projection.runtime.processes.iter().map(|process| json!({
                        "id": process.get("id"),
                        "kind": process.get("kind"),
                        "status": process.get("status"),
                        "attention_state": process.get("attention_state")
                    })).collect::<Vec<_>>(),
                    "operations": projection.runtime.background_operations.iter().map(|operation| json!({
                        "id": operation.get("id"),
                        "status": operation.get("status"),
                        "progress": operation.get("progress")
                    })).collect::<Vec<_>>(),
                    "target_app": projection.runtime.target_app.as_ref().map(|target_app| json!({
                        "state": target_app.get("state"),
                        "last_operation": target_app.get("last_operation")
                    }))
                }),
            },
        ];
        if let Some(health) = self.server.current_state_sync_health()? {
            events.push(SseEventFrame {
                event: "state_sync_health",
                data: json!(health),
            });
        }
        // Sort references and clone only the kept window; this runs on every
        // frame rebuild, and cloning the full activity set to keep 200 entries
        // charged each rebuild for the whole resident history.
        let mut recent_goal_logs = projection
            .activity
            .values()
            .filter(|activity| activity.entry.id.starts_with("round-log:"))
            .map(|activity| &activity.entry)
            .collect::<Vec<_>>();
        recent_goal_logs.sort_by(|left, right| {
            left.datetime
                .cmp(&right.datetime)
                .then_with(|| left.id.cmp(&right.id))
        });
        let keep_from = recent_goal_logs.len().saturating_sub(200);
        for entry in &recent_goal_logs[keep_from..] {
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
            let mut source_update_published = false;
            if let Ok(checkout) =
                crate::infrastructure::runtime::checkout::discover_refine_checkout()
            {
                let service =
                    crate::application::system::source_promotion::FileSourcePromotionService::new(
                        checkout,
                        runtime_root,
                        self.server.status.port,
                    );
                if let Ok(status) = service.inspect_cached() {
                    let combined = self.server.source_status_body(status);
                    events.push(SseEventFrame {
                        event: "source_update",
                        data: combined,
                    });
                    source_update_published = true;
                }
            }
            if !source_update_published {
                let check = fs::read_to_string(runtime_root.join("source-update-check.json"))
                    .ok()
                    .and_then(|encoded| serde_json::from_str::<Value>(&encoded).ok());
                let operation = fs::read_to_string(runtime_root.join("source-promotion.json"))
                    .ok()
                    .and_then(|encoded| serde_json::from_str::<Value>(&encoded).ok());
                if check.is_some() || operation.is_some() {
                    events.push(SseEventFrame {
                        event: "source_update",
                        data: json!({
                            "source": {"operation": operation},
                            "source_check": check.unwrap_or_else(|| json!({
                                "freshness": "unknown",
                                "in_flight": false
                            })),
                            "source_update": {
                                "visible": true,
                                "enabled": false,
                                "state": "reconnecting",
                                "update_available": false,
                                "title": "Reconnecting to authoritative Refine source state"
                            }
                        }),
                    });
                }
            }
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

fn sse_auxiliary_fingerprint(
    runtime_root: Option<&Path>,
    projection: &ProjectionSnapshot,
    process_output_paths: &BTreeSet<PathBuf>,
) -> RefineResult<BTreeMap<PathBuf, SsePathFingerprint>> {
    let Some(runtime_root) = runtime_root else {
        return Ok(BTreeMap::new());
    };
    let mut paths = BTreeSet::from([
        runtime_root.join(API_EVENTS_FILE),
        runtime_root.join("source-promotion.json"),
        runtime_root.join("source-update-check.json"),
    ]);
    paths.extend(process_output_paths.iter().cloned());
    for operation in &projection.runtime.background_operations {
        if let Some(id) = operation.get("id").and_then(Value::as_str) {
            paths.insert(
                runtime_root
                    .join("operations")
                    .join(format!("{id}.logs.jsonl")),
            );
        }
    }
    let mut fingerprint = BTreeMap::new();
    for path in paths {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to inspect SSE source {}: {error}",
                    path.display()
                )));
            }
        };
        fingerprint.insert(
            path,
            SsePathFingerprint {
                len: metadata.len(),
                modified: metadata.modified().ok(),
            },
        );
    }
    Ok(fingerprint)
}

fn active_process_output_paths(runtime_root: Option<&Path>) -> RefineResult<BTreeSet<PathBuf>> {
    let Some(runtime_root) = runtime_root else {
        return Ok(BTreeSet::new());
    };
    let mut paths = BTreeSet::new();
    for process in FileProcessSupervisor::new(runtime_root).list()? {
        if !process.is_runtime_projection_visible() {
            continue;
        }
        paths.extend(process.stdout_path.map(PathBuf::from));
        paths.extend(process.stderr_path.map(PathBuf::from));
    }
    Ok(paths)
}

pub(super) fn should_write_sse_frame(
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
    let signatures = seen_stream_signatures.entry(frame.event).or_default();
    // Pure dedup memory: the connection re-sends a replayed frame after a
    // wholesale clear, and every client handler already tolerates replays.
    // Without the cap this set grew for the life of the connection.
    if signatures.len() > SEEN_STREAM_SIGNATURE_CAPACITY {
        signatures.clear();
    }
    signatures.insert(signature)
}

/// Distinct stream-frame signatures one SSE connection remembers per event
/// type before the dedup memory is discarded wholesale.
const SEEN_STREAM_SIGNATURE_CAPACITY: usize = 4_096;

pub(super) fn state_sse_event(event: &str) -> bool {
    matches!(
        event,
        "project_updated"
            | "status_change"
            | "runtime_change"
            | "activity_added"
            | "system_operation"
            | "source_update"
            | "state_sync_health"
    )
}

pub(super) fn sse_frame_signature(frame: &SseEventFrame) -> String {
    let mut data = frame.data.clone();
    // The build-time timestamp would make every rebuild's frame unique: the
    // dedup set never matched and retained one full-output signature per
    // second per process for the life of the connection.
    if matches!(
        frame.event,
        "system_operation" | "operation_progress" | "process_output"
    ) && let Some(object) = data.as_object_mut()
    {
        object.remove("timestamp");
    }
    serde_json::to_string(&json!({
        "event": frame.event,
        "data": data
    }))
    .unwrap_or_else(|_| format!("{}:{:?}", frame.event, frame.data))
}
