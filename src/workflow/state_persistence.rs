use std::fs;
use std::path::Path;

use uuid::Uuid;

use crate::process::supervisor::errors::{RefineError, RefineResult};

use super::WorkflowAutomationState;
use super::claim_history::CLAIM_HISTORY_VERSION;

pub(super) fn read_state(path: &Path) -> RefineResult<WorkflowAutomationState> {
    if !path.exists() {
        return Ok(WorkflowAutomationState::default());
    }
    let bytes = fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read automation state {}: {error}",
            path.display()
        ))
    })?;
    let mut state = serde_json::from_slice::<WorkflowAutomationState>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse automation state {}: {error}",
            path.display()
        ))
    })?;
    state.normalize_claim_history();
    Ok(state)
}

pub(super) fn write_state(path: &Path, state: &WorkflowAutomationState) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create automation state directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let mut normalized = state.clone();
    normalized.normalize_claim_history();
    normalized.claim_history_version = CLAIM_HISTORY_VERSION;
    let encoded = serde_json::to_vec_pretty(&normalized).map_err(|error| {
        RefineError::Serialization(format!("failed to encode automation state: {error}"))
    })?;
    let temp_path = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    fs::write(&temp_path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write automation state {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        RefineError::Io(format!(
            "failed to publish automation state {}: {error}",
            path.display()
        ))
    })
}
