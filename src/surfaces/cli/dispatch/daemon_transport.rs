use super::*;

pub(super) fn daemon_json(
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> RefineResult<serde_json::Value> {
    let body_bytes = body
        .map(|value| serde_json::to_vec(&value))
        .transpose()
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode daemon request: {error}"))
        })?
        .unwrap_or_default();
    let port = daemon_port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| {
        RefineError::Degraded(format!(
            "Refine daemon is required for this CLI command but is not reachable at http://127.0.0.1:{port}: {error}. Start it with `refine system start`."
        ))
    })?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Refine-API-Version: {API_CONTRACT_VERSION}\r\nIdempotency-Key: cli-{}\r\n\r\n",
        body_bytes.len(),
        new_cli_idempotency_key()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(&body_bytes))
        .map_err(|error| RefineError::Io(format!("failed to write daemon request: {error}")))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| RefineError::Io(format!("failed to read daemon response: {error}")))?;
    parse_daemon_response(&response)
}

/// Follow a daemon-routed durable operation to its terminal record. Sync CLI
/// commands are operator diagnostics as well as triggers, so returning the
/// initial `running` receipt would hide the reconciliation result that matters.
pub(super) fn follow_daemon_operation(
    initial: serde_json::Value,
) -> RefineResult<serde_json::Value> {
    let timeout = std::env::var("REFINE_OPERATION_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(120);
    follow_daemon_operation_with(
        initial,
        std::time::Duration::from_secs(timeout),
        |operation_id| daemon_json("GET", &format!("/operations/{operation_id}"), None),
    )
}

pub(in crate::surfaces::cli) fn follow_daemon_operation_with(
    initial: serde_json::Value,
    timeout: std::time::Duration,
    mut fetch_status: impl FnMut(&str) -> RefineResult<serde_json::Value>,
) -> RefineResult<serde_json::Value> {
    let operation = initial.get("operation").cloned().ok_or_else(|| {
        RefineError::Serialization("daemon operation response is missing operation".to_string())
    })?;
    let operation_id = operation
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            RefineError::Serialization("daemon operation response is missing id".to_string())
        })?
        .to_string();
    let deadline = std::time::Instant::now() + timeout;
    let mut current = operation;
    loop {
        let operation_status = current
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        match operation_status {
            "complete" => {
                return Ok(current.get("result").cloned().unwrap_or_else(|| json!({})));
            }
            "failed" | "cancelled" | "interrupted" => {
                let structured = json!({
                    "state": operation_status,
                    "operation_id": operation_id,
                    "error": current.get("error").cloned().unwrap_or_else(|| json!({
                        "code": format!("operation_{operation_status}"),
                        "message": format!("Operation {operation_id} ended {operation_status}")
                    })),
                    "operation": current
                });
                let message = serde_json::to_string_pretty(&structured).unwrap();
                return if operation_status == "failed" {
                    Err(RefineError::Conflict(message))
                } else {
                    Err(RefineError::Degraded(message))
                };
            }
            "pending" | "running" | "cancelling" => {}
            other => {
                return Err(RefineError::Serialization(format!(
                    "daemon operation {operation_id} has unknown status {other:?}"
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(RefineError::Degraded(
                serde_json::to_string_pretty(&json!({
                    "state": "timed_out",
                    "operation_id": operation_id,
                    "error": {
                        "code": "operation_timed_out",
                        "message": format!("Operation {operation_id} did not settle within {}ms", timeout.as_millis())
                    },
                    "operation": current
                }))
                .unwrap(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        let response = fetch_status(&operation_id)?;
        current = response.get("operation").cloned().ok_or_else(|| {
            RefineError::Serialization(format!(
                "daemon operation status for {operation_id} is missing operation"
            ))
        })?;
    }
}

pub(super) fn daemon_port() -> u16 {
    std::env::var("REFINE_DAEMON_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(8082)
}

pub(super) fn parse_daemon_response(response: &[u8]) -> RefineResult<serde_json::Value> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| RefineError::Serialization("daemon response missing headers".to_string()))?;
    let (head, body) = response.split_at(split);
    let body = &body[4..];
    let head = String::from_utf8_lossy(head);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| RefineError::Serialization("daemon response missing status".to_string()))?;
    let value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice::<serde_json::Value>(body).map_err(|error| {
            RefineError::Serialization(format!("failed to parse daemon response body: {error}"))
        })?
    };
    if status < 400 {
        return Ok(value);
    }
    let message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .unwrap_or("daemon request failed")
        .to_string();
    let error_code = value
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(|code| code.as_str());
    match status {
        400 => Err(RefineError::InvalidInput(message)),
        401 | 403 => Err(RefineError::Unauthorized(message)),
        404 => Err(RefineError::NotFound(message)),
        409 => Err(RefineError::Conflict(message)),
        503 if error_code == Some("target_root_unavailable") => Err(RefineError::Degraded(
            format!("missing active app: {message}"),
        )),
        _ => Err(RefineError::Degraded(message)),
    }
}

pub(super) fn print_json(value: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

pub(super) fn path_segment(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                escaped.push(*byte as char)
            }
            other => escaped.push_str(&format!("%{other:02X}")),
        }
    }
    escaped
}

pub(super) fn query_component(value: &str) -> String {
    path_segment(value)
}

pub(super) fn new_cli_idempotency_key() -> String {
    format!(
        "{}:{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

pub(super) fn web_response(status: DaemonStatus) -> serde_json::Value {
    json!({
        "url": format!("http://127.0.0.1:{}/", status.port),
        "status": status
    })
}
