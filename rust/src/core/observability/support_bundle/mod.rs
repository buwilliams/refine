use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::core::supervisor::errors::{RefineError, RefineResult};

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
    pub durable_root: PathBuf,
    pub runtime_root: PathBuf,
    pub repo_root: PathBuf,
}

impl FileSupportBundleService {
    pub fn new(
        durable_root: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        repo_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: runtime_root.into(),
            repo_root: repo_root.into(),
        }
    }

    fn output_dir(&self) -> PathBuf {
        self.durable_root.join("support-bundles")
    }
}

impl SupportBundleService for FileSupportBundleService {
    fn export(&self, redact_secrets: bool) -> RefineResult<SupportBundle> {
        let diagnostics = FileDiagnosticsService::new(
            Some(self.durable_root.clone()),
            &self.runtime_root,
            &self.repo_root,
        )
        .doctor()?;
        let activity = FileActivityService::new(&self.durable_root)
            .recent(200)
            .unwrap_or_default();
        let settings_path = self.durable_root.join("settings.json");
        let settings = read_json_if_exists(&settings_path)?;
        let bundle = json!({
            "created_at": now_timestamp(),
            "redacted": redact_secrets,
            "diagnostics": diagnostics,
            "activity": activity,
            "settings": settings
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

fn redact_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let lowered = key.to_lowercase();
                    if lowered.contains("secret")
                        || lowered.contains("token")
                        || lowered.contains("key")
                        || lowered.contains("password")
                    {
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
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run");
        fs::create_dir_all(&durable_root).unwrap();
        fs::write(
            durable_root.join("settings.json"),
            r#"{"api_token":"secret","safe":"visible"}"#,
        )
        .unwrap();
        FileActivityService::new(&durable_root)
            .append(crate::model::log::ActivityEntry {
                id: "act-secret".to_string(),
                datetime: "2026-06-05T00:00:00Z".to_string(),
                severity: "error".to_string(),
                category: "provider".to_string(),
                message: "Provider failed".to_string(),
                gap_id: None,
                actor: Some("test".to_string()),
                details: Some(
                    serde_json::json!({
                        "auth_token": "activity-secret",
                        "visible": "ok"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
                actions: Vec::new(),
            })
            .unwrap();
        let service = FileSupportBundleService::new(&durable_root, &runtime_root, &temp_root);
        let bundle = service.export(true).unwrap();

        let body = fs::read_to_string(&bundle.path).unwrap();
        assert!(body.contains("[redacted]"));
        assert!(!body.contains("\"api_token\": \"secret\""));
        assert!(!body.contains("activity-secret"));
        assert!(body.contains("visible"));

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
