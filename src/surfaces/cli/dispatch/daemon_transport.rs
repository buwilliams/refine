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
    match status {
        400 => Err(RefineError::InvalidInput(message)),
        401 | 403 => Err(RefineError::Unauthorized(message)),
        404 => Err(RefineError::NotFound(message)),
        409 => Err(RefineError::Conflict(message)),
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
