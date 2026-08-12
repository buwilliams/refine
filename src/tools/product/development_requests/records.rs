use std::fs;
use std::path::Path;

use chrono::DateTime;
use serde::Deserialize;

use super::{
    DevelopmentRequestRecord, DevelopmentRequestStatus, REQUEST_SCHEMA_VERSION, request_id,
};
use crate::process::subprocess::write_json_atomically;
use crate::process::supervisor::errors::{RefineError, RefineResult};

const LEGACY_SCHEMA_VERSION: u64 = 1;

#[derive(Deserialize)]
struct RecordVersion {
    schema_version: u64,
}

pub(super) fn read_record(path: &Path) -> RefineResult<DevelopmentRequestRecord> {
    let bytes = fs::read(path)
        .map_err(|error| RefineError::Io(format!("failed to read {}: {error}", path.display())))?;
    let version = serde_json::from_slice::<RecordVersion>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to decode schema_version in {}: {error}",
            path.display()
        ))
    })?;
    if !matches!(
        version.schema_version,
        LEGACY_SCHEMA_VERSION | REQUEST_SCHEMA_VERSION
    ) {
        return Err(RefineError::InvalidInput(format!(
            "unsupported development-request schema_version {} in {}; supported versions are 1 and 2",
            version.schema_version,
            path.display()
        )));
    }
    let record = serde_json::from_slice::<DevelopmentRequestRecord>(&bytes).map_err(|error| {
        RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
    })?;
    validate_record(path, &record)?;
    Ok(record)
}

pub(super) fn validate_record(path: &Path, record: &DevelopmentRequestRecord) -> RefineResult<()> {
    let path_id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let expected_id = request_id(&record.provider_email_id);
    if record.provider_email_id.trim().is_empty()
        || record.id != expected_id
        || path_id != record.id
    {
        return Err(RefineError::InvalidInput(format!(
            "invalid development-request identity in {}: path id {}, record id {}, expected id {}",
            path.display(),
            path_id,
            record.id,
            expected_id
        )));
    }
    for (name, timestamp) in [
        ("received_at", record.received_at.as_str()),
        ("updated_at", record.updated_at.as_str()),
    ] {
        DateTime::parse_from_rfc3339(timestamp).map_err(|error| {
            RefineError::InvalidInput(format!(
                "invalid {name} in development-request record {}: {error}",
                path.display()
            ))
        })?;
    }
    if record.notification_message_id.trim().is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "development-request record {} has no notification_message_id",
            path.display()
        )));
    }
    if record.sender.trim().is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "development-request record {} has no sender identity",
            path.display()
        )));
    }
    let linked = record.goal_id.as_deref().is_some_and(|id| id == record.id)
        && record
            .goal_name
            .as_deref()
            .is_some_and(|name| !name.trim().is_empty());
    match record.status {
        DevelopmentRequestStatus::Received
            if record.goal_id.is_some() || record.goal_name.is_some() =>
        {
            return Err(RefineError::InvalidInput(format!(
                "received development-request record {} must not contain a Goal link",
                path.display()
            )));
        }
        DevelopmentRequestStatus::GoalCreated | DevelopmentRequestStatus::Resolved if !linked => {
            return Err(RefineError::InvalidInput(format!(
                "linked development-request record {} has incomplete Goal identity",
                path.display()
            )));
        }
        DevelopmentRequestStatus::Notified if !linked || record.notified_at.is_none() => {
            return Err(RefineError::InvalidInput(format!(
                "notified development-request record {} lacks Goal or notification evidence",
                path.display()
            )));
        }
        _ => {}
    }
    if let Some(notified_at) = record.notified_at.as_deref() {
        DateTime::parse_from_rfc3339(notified_at).map_err(|error| {
            RefineError::InvalidInput(format!(
                "invalid notified_at in development-request record {}: {error}",
                path.display()
            ))
        })?;
    }
    if record.schema_version == REQUEST_SCHEMA_VERSION
        && !super::email_source::source_is_authoritative(record)
    {
        return Err(RefineError::InvalidInput(format!(
            "schema 2 development-request record {} has no valid authoritative email source",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn write_record(path: &Path, record: &DevelopmentRequestRecord) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let encoded = serde_json::to_vec_pretty(record).map_err(|error| {
        RefineError::Serialization(format!("failed to encode request {}: {error}", record.id))
    })?;
    write_json_atomically(path, &encoded, "development request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unsupported_schema_is_rejected_without_mutating_bytes() {
        let root = std::env::temp_dir().join(format!("refine-record-{}", uuid::Uuid::new_v4()));
        let id = request_id("future-mail");
        let path = root.join(&id).join("request.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = serde_json::to_vec_pretty(&json!({"schema_version": 99})).unwrap();
        fs::write(&path, &bytes).unwrap();
        let error = read_record(&path).unwrap_err().to_string();
        assert!(error.contains("schema_version 99"));
        assert!(error.contains(path.to_str().unwrap()));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }
}
