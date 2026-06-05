use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::core::supervisor::errors::{RefineError, RefineResult};

pub const SESSION_REGISTRY_FILE: &str = "surface-sessions.json";
pub const SESSION_EVENTS_FILE: &str = "surface-events.jsonl";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    Desktop,
    Browser,
    Cli,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SurfaceSession {
    pub token: String,
    pub surface: SurfaceKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredSurfaceSession {
    pub token: String,
    pub surface: SurfaceKind,
    pub created_at: String,
    pub last_seen_at: String,
    pub revoked: bool,
}

pub trait SessionService {
    fn authenticate_local_surface(&self, surface: SurfaceKind) -> RefineResult<SurfaceSession>;
    fn open_ui(&self) -> RefineResult<String>;
    fn stream_state(&self, session: &SurfaceSession) -> RefineResult<String>;
    fn deliver_notification(&self, session: &SurfaceSession, message: &str) -> RefineResult<()>;
}

#[derive(Clone, Debug)]
pub struct FileSessionService {
    pub runtime_root: PathBuf,
    pub local_url: String,
}

impl FileSessionService {
    pub fn new(runtime_root: impl Into<PathBuf>, local_url: impl Into<String>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            local_url: local_url.into(),
        }
    }

    pub fn sessions_path(&self) -> PathBuf {
        self.runtime_root.join(SESSION_REGISTRY_FILE)
    }

    pub fn events_path(&self) -> PathBuf {
        self.runtime_root.join(SESSION_EVENTS_FILE)
    }

    pub fn validate_session(&self, session: &SurfaceSession) -> RefineResult<StoredSurfaceSession> {
        validate_session_record(&self.runtime_root, session)
    }

    pub fn revoke(&self, session: &SurfaceSession) -> RefineResult<()> {
        let mut records = read_session_records(&self.runtime_root)?;
        let Some(record) = records
            .iter_mut()
            .find(|record| record.token == session.token && record.surface == session.surface)
        else {
            return Err(RefineError::Unauthorized(
                "surface session is not recognized".to_string(),
            ));
        };
        record.revoked = true;
        record.last_seen_at = now_timestamp();
        write_session_records(&self.runtime_root, &records)?;
        append_session_event(
            &self.runtime_root,
            "session_revoked",
            json!({"surface": session.surface, "token": session.token}),
        )
    }
}

impl SessionService for FileSessionService {
    fn authenticate_local_surface(&self, surface: SurfaceKind) -> RefineResult<SurfaceSession> {
        let session = SurfaceSession {
            token: Uuid::new_v4().to_string(),
            surface,
        };
        let now = now_timestamp();
        let record = StoredSurfaceSession {
            token: session.token.clone(),
            surface: session.surface.clone(),
            created_at: now.clone(),
            last_seen_at: now,
            revoked: false,
        };
        let mut records = read_session_records(&self.runtime_root)?;
        records.push(record);
        write_session_records(&self.runtime_root, &records)?;
        append_session_event(
            &self.runtime_root,
            "session_authenticated",
            json!({"surface": session.surface, "token": session.token}),
        )?;
        Ok(session)
    }

    fn open_ui(&self) -> RefineResult<String> {
        let local_url = self.local_url.trim();
        if !local_url.starts_with("http://127.0.0.1:")
            && !local_url.starts_with("http://localhost:")
        {
            return Err(RefineError::InvalidInput(
                "local UI URL must be http://127.0.0.1 or http://localhost".to_string(),
            ));
        }
        append_session_event(
            &self.runtime_root,
            "ui_opened",
            json!({"local_url": local_url}),
        )?;
        Ok(local_url.to_string())
    }

    fn stream_state(&self, session: &SurfaceSession) -> RefineResult<String> {
        let record = self.validate_session(session)?;
        append_session_event(
            &self.runtime_root,
            "state_stream_opened",
            json!({"surface": record.surface, "token": record.token}),
        )?;
        serde_json::to_string(&json!({
            "session": record,
            "local_url": self.local_url,
            "events_path": self.events_path(),
        }))
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode session state stream: {error}"))
        })
    }

    fn deliver_notification(&self, session: &SurfaceSession, message: &str) -> RefineResult<()> {
        let message = message.trim();
        if message.is_empty() {
            return Err(RefineError::InvalidInput(
                "notification message is required".to_string(),
            ));
        }
        let record = self.validate_session(session)?;
        append_session_event(
            &self.runtime_root,
            "notification_delivered",
            json!({"surface": record.surface, "token": record.token, "message": message}),
        )
    }
}

pub(crate) fn validate_session_token(runtime_root: &Path, token: &str) -> RefineResult<()> {
    let token = token.trim();
    if token.is_empty() {
        return Err(RefineError::Unauthorized(
            "authorization token is required".to_string(),
        ));
    }
    let records = read_session_records(runtime_root)?;
    if records
        .iter()
        .any(|record| record.token == token && !record.revoked)
    {
        return Ok(());
    }
    Err(RefineError::Unauthorized(
        "authorization token is not recognized".to_string(),
    ))
}

pub(crate) fn active_session_tokens(runtime_root: &Path) -> RefineResult<Vec<String>> {
    Ok(read_session_records(runtime_root)?
        .into_iter()
        .filter(|record| !record.revoked)
        .map(|record| record.token)
        .collect())
}

fn validate_session_record(
    runtime_root: &Path,
    session: &SurfaceSession,
) -> RefineResult<StoredSurfaceSession> {
    if session.token.trim().is_empty() {
        return Err(RefineError::Unauthorized(
            "surface session token is required".to_string(),
        ));
    }
    let records = read_session_records(runtime_root)?;
    records
        .into_iter()
        .find(|record| {
            record.token == session.token && record.surface == session.surface && !record.revoked
        })
        .ok_or_else(|| RefineError::Unauthorized("surface session is not recognized".to_string()))
}

fn read_session_records(runtime_root: &Path) -> RefineResult<Vec<StoredSurfaceSession>> {
    let path = runtime_root.join(SESSION_REGISTRY_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read surface sessions {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice::<Vec<StoredSurfaceSession>>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse surface sessions {}: {error}",
            path.display()
        ))
    })
}

fn write_session_records(
    runtime_root: &Path,
    records: &[StoredSurfaceSession],
) -> RefineResult<()> {
    fs::create_dir_all(runtime_root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create runtime root {}: {error}",
            runtime_root.display()
        ))
    })?;
    let encoded = serde_json::to_vec_pretty(records).map_err(|error| {
        RefineError::Serialization(format!("failed to encode sessions: {error}"))
    })?;
    let path = runtime_root.join(SESSION_REGISTRY_FILE);
    fs::write(&path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write surface sessions {}: {error}",
            path.display()
        ))
    })
}

fn append_session_event(
    runtime_root: &Path,
    event: &str,
    payload: serde_json::Value,
) -> RefineResult<()> {
    fs::create_dir_all(runtime_root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create runtime root {}: {error}",
            runtime_root.display()
        ))
    })?;
    let line = serde_json::to_string(&json!({
        "event": event,
        "payload": payload,
        "created_at": now_timestamp()
    }))
    .map_err(|error| {
        RefineError::Serialization(format!("failed to encode surface event: {error}"))
    })?;
    let path = runtime_root.join(SESSION_EVENTS_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open surface events {}: {error}",
                path.display()
            ))
        })?;
    writeln!(file, "{line}").map_err(|error| {
        RefineError::Io(format!(
            "failed to append surface event {}: {error}",
            path.display()
        ))
    })
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_session_service_authenticates_streams_and_notifies_surfaces() {
        let temp_root = unique_temp_dir("sessions");
        let service = FileSessionService::new(&temp_root, "http://127.0.0.1:8080");

        let session = service
            .authenticate_local_surface(SurfaceKind::Desktop)
            .unwrap();
        assert_eq!(session.surface, SurfaceKind::Desktop);
        assert_eq!(service.open_ui().unwrap(), "http://127.0.0.1:8080");

        let stream = service.stream_state(&session).unwrap();
        assert!(stream.contains("desktop"));
        service.deliver_notification(&session, "Ready").unwrap();
        let events = fs::read_to_string(service.events_path()).unwrap();
        assert!(events.contains("session_authenticated"));
        assert!(events.contains("notification_delivered"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_session_service_rejects_revoked_or_invalid_sessions() {
        let temp_root = unique_temp_dir("sessions-revoked");
        let service = FileSessionService::new(&temp_root, "http://localhost:8080");
        let session = service
            .authenticate_local_surface(SurfaceKind::Cli)
            .unwrap();

        service.revoke(&session).unwrap();
        assert!(service.stream_state(&session).is_err());
        assert!(service.open_ui().is_ok());
        assert!(
            FileSessionService::new(&temp_root, "https://example.com")
                .open_ui()
                .is_err()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}
