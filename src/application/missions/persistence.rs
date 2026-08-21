// The snapshot, artifact, contribution, and Outcome path helpers are part of
// the durable storage contract and are consumed by later Mission phases.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::{
    acquire_record_lock, record_lock_key, replace_file_durably,
};
use crate::model::mission::Mission;

/// The durable path for a Mission record, sharded like Goals and Features.
/// Ids shorter than two characters cannot be sharded and never name a
/// Mission, since creation enforces a minimum length.
pub(crate) fn mission_json_path(refine_dir: &Path, mission_id: &str) -> Option<PathBuf> {
    let mission_id = mission_id.to_uppercase();
    if mission_id.len() < 3 {
        return None;
    }
    Some(
        refine_dir
            .join("missions")
            .join(&mission_id[..2])
            .join(&mission_id[2..])
            .join("mission.json"),
    )
}

/// The durable path for an immutable snapshot manifest.
pub(crate) fn snapshot_json_path(
    refine_dir: &Path,
    mission_id: &str,
    round: usize,
    version: usize,
) -> Option<PathBuf> {
    let mission_id = mission_id.to_uppercase();
    if mission_id.len() < 3 {
        return None;
    }
    Some(
        refine_dir
            .join("missions")
            .join(&mission_id[..2])
            .join(&mission_id[2..])
            .join("snapshots")
            .join(round.to_string())
            .join(format!("{version}.json")),
    )
}

/// The durable path for an immutable Outcome manifest.
pub(crate) fn outcome_manifest_path(
    refine_dir: &Path,
    mission_id: &str,
    round: usize,
) -> Option<PathBuf> {
    let mission_id = mission_id.to_uppercase();
    if mission_id.len() < 3 {
        return None;
    }
    Some(
        refine_dir
            .join("missions")
            .join(&mission_id[..2])
            .join(&mission_id[2..])
            .join("outcomes")
            .join(round.to_string())
            .join("manifest.json"),
    )
}

/// The durable path for an immutable artifact file.
pub(crate) fn artifact_path(
    refine_dir: &Path,
    mission_id: &str,
    artifact_key: &str,
    sha256: &str,
    extension: &str,
) -> Option<PathBuf> {
    let mission_id = mission_id.to_uppercase();
    if mission_id.len() < 3 {
        return None;
    }
    Some(
        refine_dir
            .join("missions")
            .join(&mission_id[..2])
            .join(&mission_id[2..])
            .join("artifacts")
            .join(artifact_key)
            .join(format!("{sha256}.{extension}")),
    )
}

/// The durable path for an immutable pending-contribution file.
pub(crate) fn contribution_path(
    refine_dir: &Path,
    mission_id: &str,
    goal_id: &str,
    goal_round: usize,
    sha256: &str,
    extension: &str,
) -> Option<PathBuf> {
    let mission_id = mission_id.to_uppercase();
    if mission_id.len() < 3 {
        return None;
    }
    Some(
        refine_dir
            .join("missions")
            .join(&mission_id[..2])
            .join(&mission_id[2..])
            .join("contributions")
            .join(goal_id)
            .join(goal_round.to_string())
            .join(format!("{sha256}.{extension}")),
    )
}

/// Read a Mission record, returning `None` when it does not exist.
pub(crate) fn read_mission_value(
    refine_dir: &Path,
    mission_id: &str,
) -> RefineResult<Option<Value>> {
    let Some(path) = mission_json_path(refine_dir, mission_id) else {
        return Ok(None);
    };
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Mission {}: {error}",
                path.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RefineError::Io(format!(
            "failed to read Mission {}: {error}",
            path.display()
        ))),
    }
}

/// The observed revision of a Mission record, used to fence stale mutations.
pub(crate) fn mission_revision(value: &Value) -> u64 {
    value.get("revision").and_then(Value::as_u64).unwrap_or(0)
}

/// Serialize a mutation of one Mission against other writers of that Mission.
///
/// The durable revision check inside the lock window is what makes the write
/// safe; the lock only needs to be atomic per record. The written record's
/// revision is incremented here, matching the Goal record pattern. The
/// written value (with its bumped revision) is returned so callers can hand
/// back the authoritative read-back rather than their stale pre-write copy.
pub(crate) fn write_mission_atomically(
    refine_dir: &Path,
    mission_id: &str,
    value: &Value,
) -> RefineResult<Value> {
    let Some(path) = mission_json_path(refine_dir, mission_id) else {
        return Err(RefineError::InvalidInput(format!(
            "Mission id must be at least three characters: {mission_id}"
        )));
    };
    let key = record_lock_key(&path);
    let _lease = acquire_record_lock(refine_dir, &key)?;
    let expected_revision = mission_revision(value);
    let current = read_mission_value(refine_dir, mission_id)?;
    match current.as_ref() {
        Some(current) if mission_revision(current) != expected_revision => {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} changed after it was read (expected revision {expected_revision}, current revision {})",
                mission_revision(current)
            )));
        }
        Some(_) => {}
        None if expected_revision != 0 => {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} was removed after it was read"
            )));
        }
        None => {}
    }
    let mut next = value.clone();
    let object = next.as_object_mut().ok_or_else(|| {
        RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
    })?;
    object.insert(
        "revision".to_string(),
        Value::from(expected_revision.saturating_add(1)),
    );
    let encoded = serde_json::to_vec_pretty(&next).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Mission JSON: {error}"))
    })?;
    replace_file_durably(&path, &encoded)?;
    Ok(next)
}

/// Write an immutable file, failing closed when the same path holds different
/// bytes. The same path with the same bytes is idempotent success.
pub(crate) fn write_immutable_file(path: &Path, bytes: &[u8]) -> RefineResult<()> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => Ok(()),
        Ok(_) => Err(RefineError::Conflict(format!(
            "immutable path {} already exists with different bytes",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            replace_file_durably(path, bytes)
        }
        Err(error) => Err(RefineError::Io(format!(
            "failed to read immutable path {}: {error}",
            path.display()
        ))),
    }
}

/// Validate an internal identifier used for artifact keys and extensions.
pub(crate) fn validate_internal_identifier(value: &str, label: &str) -> RefineResult<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(RefineError::InvalidInput(format!(
            "{label} must be a validated internal identifier"
        )));
    }
    Ok(())
}

/// Build a fresh Mission record object for a new Draft.
pub(crate) fn new_mission_value(
    id: &str,
    name: &str,
    intent: &str,
    reporter: Option<&str>,
    coordinator_node_id: Option<&str>,
    now: &str,
) -> Value {
    let mut object = Map::new();
    object.insert("id".to_string(), Value::String(id.to_string()));
    object.insert("name".to_string(), Value::String(name.to_string()));
    object.insert("intent".to_string(), Value::String(intent.to_string()));
    object.insert("status".to_string(), Value::String("draft".to_string()));
    object.insert(
        "reporter".to_string(),
        reporter
            .filter(|value| !value.is_empty())
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    object.insert("assignee".to_string(), Value::Null);
    object.insert(
        "coordinator_node_id".to_string(),
        coordinator_node_id
            .filter(|value| !value.is_empty())
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    object.insert("success_criteria".to_string(), Value::Array(Vec::new()));
    object.insert("artifact_contract".to_string(), Value::Array(Vec::new()));
    object.insert("current_round".to_string(), Value::Null);
    object.insert("revision".to_string(), Value::from(0));
    object.insert("rounds".to_string(), Value::Array(Vec::new()));
    object.insert("created".to_string(), Value::String(now.to_string()));
    object.insert("updated".to_string(), Value::String(now.to_string()));
    Value::Object(object)
}

/// Parse a Mission record into its typed model.
pub(crate) fn parse_mission(value: &Value) -> RefineResult<Mission> {
    serde_json::from_value(value.clone()).map_err(|error| {
        RefineError::Serialization(format!("failed to parse Mission record: {error}"))
    })
}
