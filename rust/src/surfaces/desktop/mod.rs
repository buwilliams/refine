use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::lifecycle::{
    DaemonLifecycleService, DaemonStatus, FileDaemonLifecycleService,
};
use crate::core::supervisor::runtime::RuntimeRoot;

pub const DESKTOP_STATE_FILE: &str = "desktop-state.json";
pub const DESKTOP_EVENTS_FILE: &str = "desktop-events.jsonl";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopBridgeCommand {
    BootstrapDaemon,
    DaemonStatus,
    WindowControl,
    TrayMenu,
    Notification,
    DeepLink,
    SubscribeEvents,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DesktopShellState {
    pub local_url: Option<String>,
    pub badge_count: u64,
    pub tray_status: Option<String>,
    pub last_notification_title: Option<String>,
    pub last_notification_body: Option<String>,
    pub last_deep_link: Option<String>,
    pub last_tray_action: Option<String>,
    pub last_event_stream_at: Option<String>,
    pub updated_at: String,
}

pub trait DesktopShellBridge {
    fn bootstrap_daemon(&self) -> RefineResult<DaemonStatus>;
    fn daemon_status(&self) -> RefineResult<DaemonStatus>;
    fn open_webview(&self, local_url: &str) -> RefineResult<()>;
    fn notify(&self, title: &str, body: &str) -> RefineResult<()>;
    fn tray_menu_action(&self, action: &str) -> RefineResult<()>;
    fn handle_deep_link(&self, link: &str) -> RefineResult<()>;
    fn apply_event_stream(&self, event_stream: &str) -> RefineResult<DesktopShellState>;
}

#[derive(Clone, Debug)]
pub struct FileDesktopShellBridge {
    pub runtime_root: PathBuf,
    pub port: u16,
}

impl FileDesktopShellBridge {
    pub fn new(runtime_root: impl Into<PathBuf>, port: u16) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            port,
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.runtime_root.join(DESKTOP_STATE_FILE)
    }

    pub fn events_path(&self) -> PathBuf {
        self.runtime_root.join(DESKTOP_EVENTS_FILE)
    }

    pub fn load_state(&self) -> RefineResult<DesktopShellState> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(default_state());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read desktop shell state {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<DesktopShellState>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse desktop shell state {}: {error}",
                path.display()
            ))
        })
    }

    fn save_state(&self, state: &DesktopShellState) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create desktop runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!("failed to encode desktop shell state: {error}"))
        })?;
        fs::write(self.state_path(), encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write desktop shell state {}: {error}",
                self.state_path().display()
            ))
        })
    }

    fn append_event(&self, event: &str, payload: serde_json::Value) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create desktop runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let line = serde_json::to_string(&json!({
            "event": event,
            "payload": payload,
            "created_at": now_timestamp()
        }))
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode desktop shell event: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.events_path())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open desktop shell events {}: {error}",
                    self.events_path().display()
                ))
            })?;
        writeln!(file, "{line}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append desktop shell event {}: {error}",
                self.events_path().display()
            ))
        })
    }

    fn update_state(
        &self,
        apply: impl FnOnce(&mut DesktopShellState),
        event: &str,
        payload: serde_json::Value,
    ) -> RefineResult<()> {
        let mut state = self.load_state()?;
        apply(&mut state);
        state.updated_at = now_timestamp();
        self.save_state(&state)?;
        self.append_event(event, payload)
    }
}

impl DesktopShellBridge for FileDesktopShellBridge {
    fn bootstrap_daemon(&self) -> RefineResult<DaemonStatus> {
        let status = FileDaemonLifecycleService::new(RuntimeRoot {
            root: self.runtime_root.clone(),
        })
        .start(self.port)?;
        self.append_event(
            "daemon_bootstrap",
            json!({"port": self.port, "status": status}),
        )?;
        Ok(status)
    }

    fn daemon_status(&self) -> RefineResult<DaemonStatus> {
        let status = FileDaemonLifecycleService::new(RuntimeRoot {
            root: self.runtime_root.clone(),
        })
        .status(self.port)?;
        self.append_event(
            "daemon_status",
            json!({"port": self.port, "status": status}),
        )?;
        Ok(status)
    }

    fn open_webview(&self, local_url: &str) -> RefineResult<()> {
        let local_url = local_url.trim();
        if !is_local_refine_url(local_url) {
            return Err(RefineError::InvalidInput(
                "desktop webview URL must be local http://127.0.0.1 or http://localhost"
                    .to_string(),
            ));
        }
        self.update_state(
            |state| state.local_url = Some(local_url.to_string()),
            "open_webview",
            json!({"local_url": local_url}),
        )
    }

    fn notify(&self, title: &str, body: &str) -> RefineResult<()> {
        let title = title.trim();
        let body = body.trim();
        if title.is_empty() {
            return Err(RefineError::InvalidInput(
                "notification title is required".to_string(),
            ));
        }
        self.update_state(
            |state| {
                state.last_notification_title = Some(title.to_string());
                state.last_notification_body = Some(body.to_string());
            },
            "notification",
            json!({"title": title, "body": body}),
        )
    }

    fn tray_menu_action(&self, action: &str) -> RefineResult<()> {
        let action = action.trim();
        if action.is_empty() {
            return Err(RefineError::InvalidInput(
                "tray action is required".to_string(),
            ));
        }
        self.update_state(
            |state| state.last_tray_action = Some(action.to_string()),
            "tray_menu",
            json!({"action": action}),
        )
    }

    fn handle_deep_link(&self, link: &str) -> RefineResult<()> {
        let link = link.trim();
        if !link.starts_with("refine://") {
            return Err(RefineError::InvalidInput(
                "desktop deep link must use the refine:// scheme".to_string(),
            ));
        }
        self.update_state(
            |state| state.last_deep_link = Some(link.to_string()),
            "deep_link",
            json!({"link": link}),
        )
    }

    fn apply_event_stream(&self, event_stream: &str) -> RefineResult<DesktopShellState> {
        let event_names = sse_event_names(event_stream);
        let badge_count = event_names
            .iter()
            .filter(|event| desktop_badge_event(event))
            .count() as u64;
        let tray_status = desktop_tray_status(&event_names);
        let now = now_timestamp();
        let mut state = self.load_state()?;
        state.badge_count = badge_count;
        state.tray_status = Some(tray_status.to_string());
        state.last_event_stream_at = Some(now.clone());
        state.updated_at = now;
        self.save_state(&state)?;
        self.append_event(
            "event_stream_applied",
            json!({
                "badge_count": state.badge_count,
                "tray_status": state.tray_status,
                "events": event_names
            }),
        )?;
        Ok(state)
    }
}

pub fn desktop_bridge_commands() -> [DesktopBridgeCommand; 7] {
    [
        DesktopBridgeCommand::BootstrapDaemon,
        DesktopBridgeCommand::DaemonStatus,
        DesktopBridgeCommand::WindowControl,
        DesktopBridgeCommand::TrayMenu,
        DesktopBridgeCommand::Notification,
        DesktopBridgeCommand::DeepLink,
        DesktopBridgeCommand::SubscribeEvents,
    ]
}

fn default_state() -> DesktopShellState {
    DesktopShellState {
        local_url: None,
        badge_count: 0,
        tray_status: None,
        last_notification_title: None,
        last_notification_body: None,
        last_deep_link: None,
        last_tray_action: None,
        last_event_stream_at: None,
        updated_at: now_timestamp(),
    }
}

fn sse_event_names(event_stream: &str) -> Vec<String> {
    event_stream
        .lines()
        .filter_map(|line| line.strip_prefix("event:"))
        .map(str::trim)
        .filter(|event| !event.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn desktop_badge_event(event: &str) -> bool {
    matches!(
        event,
        "activity_added" | "job_progress" | "chat_event" | "process_output"
    )
}

fn desktop_tray_status(events: &[String]) -> &'static str {
    if events.iter().any(|event| event == "status_change") {
        "updated"
    } else if events.iter().any(|event| event == "ready") {
        "ready"
    } else {
        "idle"
    }
}

fn is_local_refine_url(value: &str) -> bool {
    value.starts_with("http://127.0.0.1:") || value.starts_with("http://localhost:")
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_desktop_shell_bridge_bootstraps_daemon_and_records_shell_events() {
        let temp_root = unique_temp_dir("desktop-bridge");
        let runtime_root = temp_root.join("run");
        let bridge = FileDesktopShellBridge::new(&runtime_root, 8123);

        let status = bridge.bootstrap_daemon().unwrap();
        assert_eq!(status.port, 8123);
        assert!(status.daemon_healthy);
        assert!(bridge.daemon_status().unwrap().daemon_healthy);

        bridge.open_webview("http://127.0.0.1:8123").unwrap();
        bridge.notify("Refine", "Ready").unwrap();
        bridge.tray_menu_action("show").unwrap();
        bridge.handle_deep_link("refine://gap/GAP1").unwrap();
        let subscribed = bridge
            .apply_event_stream(
                "event: ready\ndata: {}\n\n\
                 event: status_change\ndata: {}\n\n\
                 event: job_progress\ndata: {}\n\n\
                 event: chat_event\ndata: {}\n\n",
            )
            .unwrap();
        assert_eq!(subscribed.badge_count, 2);
        assert_eq!(subscribed.tray_status.as_deref(), Some("updated"));

        let state = bridge.load_state().unwrap();
        assert_eq!(state.local_url.as_deref(), Some("http://127.0.0.1:8123"));
        assert_eq!(state.badge_count, 2);
        assert_eq!(state.last_notification_title.as_deref(), Some("Refine"));
        assert_eq!(state.last_tray_action.as_deref(), Some("show"));
        assert_eq!(state.last_deep_link.as_deref(), Some("refine://gap/GAP1"));
        assert!(state.last_event_stream_at.is_some());
        let events = fs::read_to_string(bridge.events_path()).unwrap();
        assert!(events.contains("daemon_bootstrap"));
        assert!(events.contains("notification"));
        assert!(events.contains("event_stream_applied"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_desktop_shell_bridge_rejects_non_local_webview_urls() {
        let temp_root = unique_temp_dir("desktop-bridge-invalid");
        let bridge = FileDesktopShellBridge::new(temp_root.join("run"), 8123);

        assert!(bridge.open_webview("https://example.com").is_err());
        assert!(bridge.handle_deep_link("https://example.com").is_err());

        fs::remove_dir_all(temp_root).unwrap_or(());
    }

    #[test]
    fn desktop_bridge_command_catalog_matches_native_shell_surface() {
        assert_eq!(
            desktop_bridge_commands(),
            [
                DesktopBridgeCommand::BootstrapDaemon,
                DesktopBridgeCommand::DaemonStatus,
                DesktopBridgeCommand::WindowControl,
                DesktopBridgeCommand::TrayMenu,
                DesktopBridgeCommand::Notification,
                DesktopBridgeCommand::DeepLink,
                DesktopBridgeCommand::SubscribeEvents,
            ]
        );
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
