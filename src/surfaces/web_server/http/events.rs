use super::*;

impl LocalHttpDaemon {
    pub(super) fn sse_response(&self, stream_name: &'static str) -> Response {
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
                tokio::time::sleep(Duration::from_millis(500)).await;
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

    pub(super) fn terminal_sse_response(&self, session_id: String, initial_after: u64) -> Response {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
        let runtime_root = self.server.runtime_root.clone();
        tokio::spawn(async move {
            let mut after = initial_after;
            let mut last_attention = None;
            let mut attached = false;
            loop {
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
            let mut source_update_published = false;
            if let Ok(checkout) = crate::tools::host::deployed_update::discover_refine_checkout() {
                let service = crate::tools::host::source_promotion::FileSourcePromotionService::new(
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
    seen_stream_signatures
        .entry(frame.event)
        .or_default()
        .insert(signature)
}

pub(super) fn state_sse_event(event: &str) -> bool {
    matches!(
        event,
        "project_updated"
            | "status_change"
            | "runtime_change"
            | "activity_added"
            | "system_operation"
            | "source_update"
    )
}

pub(super) fn sse_frame_signature(frame: &SseEventFrame) -> String {
    let mut data = frame.data.clone();
    if matches!(frame.event, "system_operation" | "operation_progress")
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
