use std::fs;
use std::path::Path;

use chrono::Utc;
use serde_json::Value;

use crate::core::supervisor::errors::{RefineError, RefineResult};

pub(in crate::surfaces::web_server) fn now_timestamp_web() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(in crate::surfaces::web_server) fn write_json_atomically_web(
    path: &Path,
    value: &Value,
) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let temp_path = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    fs::write(&temp_path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write temp file {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        RefineError::Io(format!(
            "failed to commit JSON file {}: {error}",
            path.display()
        ))
    })
}
