use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{RefineError, RefineResult};

/// Decode a JSON value Refine itself persisted (goal.json evidence, config
/// artifacts). Unlike agent output, this data was written canonically, so a
/// failure means state corruption or a schema change — the path-aware
/// diagnostic names the exact field.
pub fn decode_persisted<T: DeserializeOwned>(value: Value, what: &str) -> RefineResult<T> {
    serde_path_to_error::deserialize(value).map_err(|error| {
        let path = error.path().to_string();
        let location = if path.is_empty() {
            String::new()
        } else {
            format!(" at `{path}`")
        };
        RefineError::Serialization(format!(
            "invalid persisted {what}{location}: {}",
            error.inner()
        ))
    })
}
