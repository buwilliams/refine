use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::process::supervisor::errors::{RefineError, RefineResult};

mod redaction;

pub use redaction::redact_sync_error;

pub const STATE_SYNC_HEALTH_FILE: &str = "state-sync-health.json";
const STATE_SYNC_HEALTH_LOCK_FILE: &str = "state-sync-health.lock";
const FAILURE_REMINDER_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateSyncRecoveryKind {
    MissingBaseline,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncHealthRecord {
    pub target_root: String,
    pub node_id: String,
    pub monitoring_since: String,
    pub last_attempt_at: Option<String>,
    pub last_attempt_outcome: Option<String>,
    #[serde(default)]
    pub attempt_sequence: u64,
    #[serde(default)]
    pub last_attempt_id: Option<u64>,
    #[serde(default)]
    pub last_attempt_source: Option<String>,
    pub last_success_at: Option<String>,
    #[serde(default)]
    pub last_success_attempt_id: Option<u64>,
    pub failure_since: Option<String>,
    pub last_failure_at: Option<String>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_failure_attempt_id: Option<u64>,
    #[serde(default)]
    pub last_failure_source: Option<String>,
    #[serde(default)]
    pub last_conflict_report_id: Option<String>,
    #[serde(default)]
    pub last_conflict_report_location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_kind: Option<StateSyncRecoveryKind>,
    pub last_reminder_at: Option<String>,
    pub remote_configured: Option<bool>,
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncHealth {
    pub status: String,
    pub target_root: String,
    pub node_id: String,
    pub monitoring_since: String,
    pub last_attempt_at: Option<String>,
    pub last_attempt_outcome: Option<String>,
    pub last_attempt_id: Option<u64>,
    pub last_attempt_source: Option<String>,
    pub last_success_at: Option<String>,
    pub failure_since: Option<String>,
    pub stale_since: Option<String>,
    pub last_error: Option<String>,
    pub last_failure_attempt_id: Option<u64>,
    pub last_failure_source: Option<String>,
    pub last_conflict_report_id: Option<String>,
    pub last_conflict_report_location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_kind: Option<StateSyncRecoveryKind>,
    pub remote_configured: Option<bool>,
    pub stale_threshold_seconds: u64,
    pub aggregate_counts_authoritative: bool,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateSyncHealthActivity {
    FailureStarted { error: String },
    FailureReminder { error: String },
    Recovered { failure_since: String },
}

#[derive(Clone, Debug)]
pub struct FileStateSyncHealthService {
    runtime_root: PathBuf,
}

impl FileStateSyncHealthService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.runtime_root.join(STATE_SYNC_HEALTH_FILE)
    }

    pub fn bind(&self, target_root: &Path, node_id: &str) -> RefineResult<()> {
        self.update(target_root, node_id, |_| {})
    }

    pub fn record_attempt(&self, target_root: &Path, node_id: &str) -> RefineResult<()> {
        self.begin_attempt(target_root, node_id, "legacy")?;
        Ok(())
    }

    pub fn begin_attempt(
        &self,
        target_root: &Path,
        node_id: &str,
        source: &str,
    ) -> RefineResult<u64> {
        let mut attempt_id = 0;
        self.update(target_root, node_id, |record| {
            record.attempt_sequence = record
                .attempt_sequence
                .max(record.last_attempt_id.unwrap_or_default())
                .saturating_add(1);
            attempt_id = record.attempt_sequence;
            record.last_attempt_id = Some(attempt_id);
            record.last_attempt_source = Some(source.to_string());
            record.last_attempt_at = Some(now_timestamp());
            record.last_attempt_outcome = Some("attempting".to_string());
        })?;
        Ok(attempt_id)
    }

    pub fn settle_neutral(
        &self,
        target_root: &Path,
        node_id: &str,
        attempt_id: u64,
        source: &str,
        outcome: &str,
        remote_configured: Option<bool>,
    ) -> RefineResult<()> {
        self.update(target_root, node_id, |record| {
            if record.last_attempt_id != Some(attempt_id)
                || record.last_attempt_source.as_deref() != Some(source)
            {
                return;
            }
            record.last_attempt_at = Some(now_timestamp());
            record.last_attempt_outcome = Some(outcome.to_string());
            if let Some(remote_configured) = remote_configured {
                record.remote_configured = Some(remote_configured);
                if !remote_configured {
                    record.recovery_kind = None;
                }
            }
        })
    }

    pub fn record_neutral(
        &self,
        target_root: &Path,
        node_id: &str,
        outcome: &str,
        remote_configured: Option<bool>,
    ) -> RefineResult<()> {
        self.update(target_root, node_id, |record| {
            record.last_attempt_at = Some(now_timestamp());
            record.last_attempt_outcome = Some(outcome.to_string());
            if let Some(remote_configured) = remote_configured {
                record.remote_configured = Some(remote_configured);
                if !remote_configured {
                    record.recovery_kind = None;
                }
            }
        })
    }

    pub fn record_success(
        &self,
        target_root: &Path,
        node_id: &str,
    ) -> RefineResult<Option<StateSyncHealthActivity>> {
        let mut activity = None;
        self.update(target_root, node_id, |record| {
            activity = settle_success_record(record);
        })?;
        Ok(activity)
    }

    pub fn settle_success(
        &self,
        target_root: &Path,
        node_id: &str,
        attempt_id: u64,
        source: &str,
    ) -> RefineResult<Option<StateSyncHealthActivity>> {
        let mut activity = None;
        self.update(target_root, node_id, |record| {
            if record.last_success_attempt_id.unwrap_or_default() > attempt_id {
                return;
            }
            let is_latest_attempt = record.last_attempt_id == Some(attempt_id)
                && record.last_attempt_source.as_deref() == Some(source);
            if let Some(failure_since) = record.failure_since.clone()
                && record.last_failure_attempt_id.unwrap_or_default() <= attempt_id
            {
                activity = Some(StateSyncHealthActivity::Recovered { failure_since });
                record.failure_since = None;
                record.last_failure_at = None;
                record.last_error = None;
                record.last_reminder_at = None;
                record.last_conflict_report_id = None;
                record.last_conflict_report_location = None;
                record.recovery_kind = None;
            }
            let now = now_timestamp();
            if is_latest_attempt {
                record.last_attempt_at = Some(now.clone());
                record.last_attempt_outcome = Some("succeeded".to_string());
            }
            record.last_success_at = Some(now);
            record.last_success_attempt_id = Some(attempt_id);
            record.remote_configured = Some(true);
        })?;
        Ok(activity)
    }

    pub fn record_recovery_success(
        &self,
        target_root: &Path,
        node_id: &str,
        expected_kind: StateSyncRecoveryKind,
        expected_revision: u64,
    ) -> RefineResult<(bool, Option<StateSyncHealthActivity>)> {
        let activity = self.update_if(target_root, node_id, |record| {
            (record.revision == expected_revision && record.recovery_kind == Some(expected_kind))
                .then(|| settle_success_record(record))
        })?;
        Ok(match activity {
            Some(activity) => (true, activity),
            None => (false, None),
        })
    }

    pub fn record_failure(
        &self,
        target_root: &Path,
        node_id: &str,
        error: &str,
    ) -> RefineResult<Option<StateSyncHealthActivity>> {
        self.record_failure_with_recovery_kind(target_root, node_id, error, None)
    }

    pub fn record_failure_with_recovery_kind(
        &self,
        target_root: &Path,
        node_id: &str,
        error: &str,
        recovery_kind: Option<StateSyncRecoveryKind>,
    ) -> RefineResult<Option<StateSyncHealthActivity>> {
        let error = redact_sync_error(error);
        let mut activity = None;
        self.update(target_root, node_id, |record| {
            let now = now_timestamp();
            record.last_attempt_at = Some(now.clone());
            record.last_attempt_outcome = Some("failed".to_string());
            record.last_failure_at = Some(now.clone());
            record.last_error = Some(error.clone());
            record.recovery_kind = recovery_kind;
            record.remote_configured = Some(true);
            if record.failure_since.is_none() {
                record.failure_since = Some(now.clone());
                record.last_reminder_at = Some(now);
                activity = Some(StateSyncHealthActivity::FailureStarted {
                    error: error.clone(),
                });
            } else if reminder_due(record.last_reminder_at.as_deref()) {
                record.last_reminder_at = Some(now);
                activity = Some(StateSyncHealthActivity::FailureReminder {
                    error: error.clone(),
                });
            }
        })?;
        Ok(activity)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn settle_failure(
        &self,
        target_root: &Path,
        node_id: &str,
        attempt_id: u64,
        source: &str,
        error: &str,
        conflict_report_id: Option<&str>,
        conflict_report_location: Option<&str>,
        recovery_kind: Option<StateSyncRecoveryKind>,
    ) -> RefineResult<Option<StateSyncHealthActivity>> {
        let error = redact_sync_error(error);
        let mut activity = None;
        self.update(target_root, node_id, |record| {
            let is_latest_attempt = record.last_attempt_id == Some(attempt_id)
                && record.last_attempt_source.as_deref() == Some(source);
            if is_latest_attempt {
                record.last_attempt_at = Some(now_timestamp());
                record.last_attempt_outcome = Some("failed".to_string());
            }
            if record.last_failure_attempt_id.unwrap_or_default() > attempt_id
                || record.last_success_attempt_id.unwrap_or_default() > attempt_id
            {
                return;
            }
            let now = now_timestamp();
            record.last_failure_attempt_id = Some(attempt_id);
            record.last_failure_source = Some(source.to_string());
            record.last_failure_at = Some(now.clone());
            record.last_error = Some(error.clone());
            record.last_conflict_report_id = conflict_report_id.map(str::to_string);
            record.last_conflict_report_location = conflict_report_location.map(str::to_string);
            record.recovery_kind = recovery_kind;
            record.remote_configured = Some(true);
            if record.failure_since.is_none() {
                record.failure_since = Some(now.clone());
                record.last_reminder_at = Some(now);
                activity = Some(StateSyncHealthActivity::FailureStarted {
                    error: error.clone(),
                });
            } else if reminder_due(record.last_reminder_at.as_deref()) {
                record.last_reminder_at = Some(now);
                activity = Some(StateSyncHealthActivity::FailureReminder {
                    error: error.clone(),
                });
            }
        })?;
        Ok(activity)
    }

    pub fn inspect(
        &self,
        target_root: &Path,
        node_id: &str,
        stale_threshold: Duration,
    ) -> RefineResult<StateSyncHealth> {
        let canonical_target = canonical_target(target_root);
        let record = self
            .read()?
            .filter(|record| record.target_root == canonical_target && record.node_id == node_id);
        let record = record.unwrap_or_else(|| new_record(canonical_target, node_id));
        Ok(derive_health(record, stale_threshold, Utc::now()))
    }

    fn update(
        &self,
        target_root: &Path,
        node_id: &str,
        mutate: impl FnOnce(&mut StateSyncHealthRecord),
    ) -> RefineResult<()> {
        self.update_if(target_root, node_id, |record| {
            mutate(record);
            Some(())
        })?;
        Ok(())
    }

    fn update_if<T>(
        &self,
        target_root: &Path,
        node_id: &str,
        mutate: impl FnOnce(&mut StateSyncHealthRecord) -> Option<T>,
    ) -> RefineResult<Option<T>> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create state-sync health root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let lock_path = self.runtime_root.join(STATE_SYNC_HEALTH_LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open state-sync health lock {}: {error}",
                    lock_path.display()
                ))
            })?;
        lock.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to lock state-sync health {}: {error}",
                lock_path.display()
            ))
        })?;
        let target_root = canonical_target(target_root);
        let existing = self.read()?;
        let previous_revision = existing
            .as_ref()
            .map(|record| record.revision)
            .unwrap_or_default();
        let mut record = existing
            .filter(|record| record.target_root == target_root && record.node_id == node_id)
            .unwrap_or_else(|| {
                let mut record = new_record(target_root, node_id);
                record.revision = previous_revision;
                record
            });
        let Some(result) = mutate(&mut record) else {
            return Ok(None);
        };
        record.revision = record.revision.saturating_add(1);
        self.write(&record)?;
        Ok(Some(result))
    }

    fn read(&self) -> RefineResult<Option<StateSyncHealthRecord>> {
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read state-sync health {}: {error}",
                    path.display()
                )));
            }
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse state-sync health {}: {error}",
                path.display()
            ))
        })
    }

    fn write(&self, record: &StateSyncHealthRecord) -> RefineResult<()> {
        let path = self.path();
        let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(record).map_err(|error| {
            RefineError::Serialization(format!("failed to encode state-sync health: {error}"))
        })?;
        let mut file = fs::File::create(&temp).map_err(|error| {
            RefineError::Io(format!(
                "failed to create state-sync health {}: {error}",
                temp.display()
            ))
        })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to persist state-sync health {}: {error}",
                    temp.display()
                ))
            })?;
        fs::rename(&temp, &path).map_err(|error| {
            RefineError::Io(format!(
                "failed to replace state-sync health {}: {error}",
                path.display()
            ))
        })
    }
}

fn settle_success_record(record: &mut StateSyncHealthRecord) -> Option<StateSyncHealthActivity> {
    let activity = record
        .failure_since
        .clone()
        .map(|failure_since| StateSyncHealthActivity::Recovered { failure_since });
    let now = now_timestamp();
    record.last_attempt_at = Some(now.clone());
    record.last_attempt_outcome = Some("succeeded".to_string());
    record.last_success_at = Some(now);
    record.last_success_attempt_id = record.last_attempt_id;
    record.failure_since = None;
    record.last_failure_at = None;
    record.last_error = None;
    record.last_conflict_report_id = None;
    record.last_conflict_report_location = None;
    record.recovery_kind = None;
    record.last_reminder_at = None;
    record.remote_configured = Some(true);
    activity
}

fn derive_health(
    record: StateSyncHealthRecord,
    stale_threshold: Duration,
    now: DateTime<Utc>,
) -> StateSyncHealth {
    let freshness_base = record
        .last_success_at
        .as_deref()
        .or(Some(record.monitoring_since.as_str()));
    let stale_boundary = freshness_base
        .and_then(parse_timestamp)
        .map(|timestamp| timestamp + chrono::Duration::seconds(stale_threshold.as_secs() as i64));
    let stale = stale_boundary.is_some_and(|boundary| now >= boundary);
    let status = if record.remote_configured == Some(false) {
        "unconfigured"
    } else if record.failure_since.is_some() {
        "failed"
    } else if stale {
        "stale"
    } else if record.last_success_at.is_some() {
        "healthy"
    } else {
        "unknown"
    };
    StateSyncHealth {
        status: status.to_string(),
        target_root: record.target_root,
        node_id: record.node_id,
        monitoring_since: record.monitoring_since,
        last_attempt_at: record.last_attempt_at,
        last_attempt_outcome: record.last_attempt_outcome,
        last_attempt_id: record.last_attempt_id,
        last_attempt_source: record.last_attempt_source,
        last_success_at: record.last_success_at,
        failure_since: record.failure_since,
        stale_since: stale.then(|| timestamp(stale_boundary.expect("stale boundary"))),
        last_error: record.last_error,
        last_failure_attempt_id: record.last_failure_attempt_id,
        last_failure_source: record.last_failure_source,
        last_conflict_report_id: record.last_conflict_report_id,
        last_conflict_report_location: record.last_conflict_report_location,
        recovery_kind: record.recovery_kind,
        remote_configured: record.remote_configured,
        stale_threshold_seconds: stale_threshold.as_secs(),
        aggregate_counts_authoritative: !matches!(status, "failed" | "stale"),
        revision: record.revision,
    }
}

fn reminder_due(last_reminder_at: Option<&str>) -> bool {
    let Some(last_reminder_at) = last_reminder_at.and_then(parse_timestamp) else {
        return true;
    };
    Utc::now() - last_reminder_at
        >= chrono::Duration::seconds(FAILURE_REMINDER_INTERVAL.as_secs() as i64)
}

fn canonical_target(target_root: &Path) -> String {
    target_root
        .canonicalize()
        .unwrap_or_else(|_| target_root.to_path_buf())
        .display()
        .to_string()
}

fn new_record(target_root: String, node_id: &str) -> StateSyncHealthRecord {
    StateSyncHealthRecord {
        target_root,
        node_id: node_id.to_string(),
        monitoring_since: now_timestamp(),
        last_attempt_at: None,
        last_attempt_outcome: None,
        attempt_sequence: 0,
        last_attempt_id: None,
        last_attempt_source: None,
        last_success_at: None,
        last_success_attempt_id: None,
        failure_since: None,
        last_failure_at: None,
        last_error: None,
        last_failure_attempt_id: None,
        last_failure_source: None,
        last_conflict_report_id: None,
        last_conflict_report_location: None,
        recovery_kind: None,
        last_reminder_at: None,
        remote_configured: None,
        revision: 0,
    }
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn now_timestamp() -> String {
    timestamp(Utc::now())
}

#[cfg(test)]
mod tests;
