use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::process::supervisor::errors::{RefineError, RefineResult};

pub(crate) fn live_daemon_executable(port: u16) -> RefineResult<PathBuf> {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}")
            .parse()
            .map_err(|error| RefineError::InvalidInput(format!("invalid daemon port: {error}")))?,
        Duration::from_secs(1),
    )
    .map_err(|error| {
        RefineError::Io(format!(
            "failed to connect to daemon identity route: {error}"
        ))
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| {
            RefineError::Io(format!("failed to set daemon identity timeout: {error}"))
        })?;
    stream
        .write_all(b"GET /system/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|error| RefineError::Io(format!("failed to request daemon identity: {error}")))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| RefineError::Io(format!("failed to read daemon identity: {error}")))?;
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            RefineError::Serialization("daemon identity response missing headers".to_string())
        })?;
    let (head, body) = response.split_at(split);
    if !String::from_utf8_lossy(head).starts_with("HTTP/1.1 200") {
        return Err(RefineError::Degraded(
            "daemon identity route returned a non-success response".to_string(),
        ));
    }
    let value: Value = serde_json::from_slice(&body[4..]).map_err(|error| {
        RefineError::Serialization(format!("failed to parse daemon identity response: {error}"))
    })?;
    value
        .get("executable_path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            RefineError::Degraded(
                "live daemon did not report an executable path in /system/version".to_string(),
            )
        })
}
