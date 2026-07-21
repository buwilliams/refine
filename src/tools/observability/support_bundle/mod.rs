use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::project_layout::target_root_for_refine_dir;
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::tools::observability::metrics::{FileMetricsService, PerformanceQuery};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SupportBundle {
    pub path: String,
    pub redacted: bool,
}

pub trait SupportBundleService {
    fn export(&self, redact_secrets: bool) -> RefineResult<SupportBundle>;
}

#[derive(Clone, Debug)]
pub struct FileSupportBundleService {
    pub refine_dir: PathBuf,
    pub runtime_root: PathBuf,
    pub repo_root: PathBuf,
}

impl FileSupportBundleService {
    pub fn new(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        repo_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
            repo_root: repo_root.into(),
        }
    }

    fn output_dir(&self) -> PathBuf {
        self.refine_dir.join("support-bundles")
    }
}

impl SupportBundleService for FileSupportBundleService {
    fn export(&self, redact_secrets: bool) -> RefineResult<SupportBundle> {
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let diagnostics =
            FileDiagnosticsService::new(Some(target_root), &self.runtime_root, &self.repo_root)
                .doctor()?;
        let activity = FileActivityService::new(&self.refine_dir)
            .recent(200)
            .unwrap_or_default();
        let metrics = FileMetricsService::new(&self.runtime_root)
            .report(PerformanceQuery::default())
            .map(|report| json!(report))
            .unwrap_or_else(|error| json!({"error": error.to_string()}));
        let chat_sessions = read_chat_sessions(&self.refine_dir)?;
        let nodes_path = self.refine_dir.join("nodes.json");
        let nodes = read_json_if_exists(&nodes_path)?;
        let bundle = json!({
            "created_at": now_timestamp(),
            "redacted": redact_secrets,
            "diagnostics": diagnostics,
            "activity": activity,
            "metrics": metrics,
            "chat_sessions": chat_sessions,
            "nodes": nodes
        });
        let bundle = if redact_secrets {
            redact_json(bundle)
        } else {
            bundle
        };
        fs::create_dir_all(self.output_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create support bundle directory {}: {error}",
                self.output_dir().display()
            ))
        })?;
        let filename = format!(
            "support-bundle-{}.json",
            Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let path = self.output_dir().join(filename);
        let encoded = serde_json::to_string_pretty(&bundle).map_err(|error| {
            RefineError::Serialization(format!("failed to encode support bundle: {error}"))
        })?;
        fs::write(&path, format!("{encoded}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to write support bundle {}: {error}",
                path.display()
            ))
        })?;
        Ok(SupportBundle {
            path: path.display().to_string(),
            redacted: redact_secrets,
        })
    }
}

fn read_json_if_exists(path: &Path) -> RefineResult<serde_json::Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let bytes = fs::read(path)
        .map_err(|error| RefineError::Io(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
    })
}

fn read_chat_sessions(refine_dir: &Path) -> RefineResult<Vec<serde_json::Value>> {
    let sessions_dir = refine_dir.join("chat/sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(&sessions_dir).map_err(|error| {
        RefineError::Io(format!(
            "failed to read chat sessions directory {}: {error}",
            sessions_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read chat session entry {}: {error}",
                sessions_dir.display()
            ))
        })?;
        if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let value = read_json_if_exists(&entry.path())?;
        sessions.push(value);
    }
    sessions.sort_by(|a, b| {
        b.get("updated_at")
            .and_then(|value| value.as_str())
            .cmp(&a.get("updated_at").and_then(|value| value.as_str()))
    });
    sessions.truncate(20);
    Ok(sessions)
}

fn redact_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if should_redact_key(&key) {
                        (key, serde_json::Value::String("[redacted]".to_string()))
                    } else {
                        (key, redact_json(value))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(redact_json).collect())
        }
        other => other,
    }
}

fn should_redact_key(key: &str) -> bool {
    let lowered = key.to_lowercase();
    lowered.contains("secret")
        || lowered.contains("token")
        || lowered.contains("key")
        || lowered.contains("password")
        || lowered == "provider_session_id"
        || lowered == "authorization"
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_support_bundle_exports_redacted_json() {
        let temp_root = unique_temp_dir("support-bundle");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::write(
            refine_dir.join("nodes.json"),
            r#"{"nodes":[{"id":"default","display_name":"Default","created_at":"2026-06-16T00:00:00Z","updated_at":"2026-06-16T00:00:00Z","settings":{"api_token":"secret","safe":"visible"}}]}"#,
        )
        .unwrap();
        FileActivityService::new(&refine_dir)
            .append(crate::model::log::ActivityEntry {
                id: "act-secret".to_string(),
                datetime: "2026-06-05T00:00:00Z".to_string(),
                severity: "error".to_string(),
                category: "provider".to_string(),
                message: "Provider failed".to_string(),
                goal_id: None,
                actor: Some("test".to_string()),
                details: Some(
                    serde_json::json!({
                        "provider_token": "activity-secret",
                        "visible": "ok"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                actions: Vec::new(),
            })
            .unwrap();
        FileMetricsService::new(&runtime_root)
            .record_operation("cache.rebuild", 42.0, true, json!({"rows": 3}))
            .unwrap();
        let sessions_dir = refine_dir.join("chat/sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(
            sessions_dir.join("CHAT1.json"),
            serde_json::to_string_pretty(&json!({
                "id": "CHAT1",
                "mode": "chat",
                "provider": "smoke-ai",
                "provider_session_id": "provider-secret-session",
                "attachment": "standalone",
                "created_at": "2026-06-05T00:00:00Z",
                "updated_at": "2026-06-05T00:00:01Z",
                "transcript_events": [
                    {
                        "role": "assistant",
                        "text": "diagnostic transcript line",
                        "created_at": "2026-06-05T00:00:01Z"
                    }
                ],
                "importable_artifacts": [],
                "closed": false,
                "interrupted": false,
                "interruption_detail": null
            }))
            .unwrap(),
        )
        .unwrap();
        let service = FileSupportBundleService::new(&refine_dir, &runtime_root, &temp_root);
        let bundle = service.export(true).unwrap();

        let body = fs::read_to_string(&bundle.path).unwrap();
        assert!(body.contains("[redacted]"));
        assert!(!body.contains("\"api_token\": \"secret\""));
        assert!(!body.contains("activity-secret"));
        assert!(!body.contains("provider-secret-session"));
        assert!(body.contains("cache.rebuild"));
        assert!(body.contains("diagnostic transcript line"));
        assert!(body.contains("visible"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
