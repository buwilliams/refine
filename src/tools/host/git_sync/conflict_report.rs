use super::*;

const CONFLICT_REPORT_VERSION: u32 = 2;
const CONFLICT_REPORT_DIRECTORY: &str = "state-sync-conflicts";
const CONFLICT_REPORT_FILE: &str = "latest.json";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateSyncConflictPhase {
    FirstPass,
    PushRetry,
}

impl StateSyncConflictPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstPass => "first_pass",
            Self::PushRetry => "push_retry",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncRecoveryMetadata {
    pub available: bool,
    pub preview_command: String,
    pub apply_command: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncConflictReport {
    pub version: u32,
    pub report_id: String,
    pub phase: StateSyncConflictPhase,
    pub attempt_id: String,
    pub attempt_source: String,
    pub created_at: String,
    pub target_identity: String,
    pub repository_identity: String,
    pub configured_remote: String,
    pub baseline_snapshot: String,
    pub local_snapshot: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub local_fingerprints: BTreeMap<String, u64>,
    pub remote_snapshot: String,
    pub local_state_head: Option<String>,
    pub remote_state_head: String,
    pub unresolved_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconciliation_outcomes: Vec<StateSyncReconciliationOutcome>,
    pub recovery: StateSyncRecoveryMetadata,
    pub report_location: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncConflictSummary {
    pub report_id: String,
    pub phase: StateSyncConflictPhase,
    pub unresolved_count: usize,
    pub report_location: String,
    pub recovery_command: String,
    pub diagnostics: Vec<String>,
}

impl std::fmt::Display for StateSyncConflictSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Refine state changed on multiple nodes during {}: {} unresolved path(s){}; complete conflict report {} is at {}. Run `{}` to review a stale-fenced recovery.",
            self.phase.as_str(),
            self.unresolved_count,
            if self.diagnostics.is_empty() {
                String::new()
            } else {
                format!(" ({})", self.diagnostics.join("; "))
            },
            self.report_id,
            self.report_location,
            self.recovery_command
        )
    }
}

pub fn latest_state_sync_conflict_report(
    runtime_root: &std::path::Path,
) -> RefineResult<Option<StateSyncConflictReport>> {
    let path = conflict_report_path(runtime_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read state-sync conflict report {}: {error}",
                path.display()
            )));
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse state-sync conflict report {}: {error}",
            path.display()
        ))
    })
}

impl FileGitSyncService {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_conflict_report(
        &self,
        phase: StateSyncConflictPhase,
        attempt: &StateSyncAttemptContext,
        remote: &str,
        baseline: &DurableStateMap,
        local_root: &std::path::Path,
        local: &DurableStateMap,
        remote_root: &std::path::Path,
        remote_state: &DurableStateMap,
        local_state_head: Option<String>,
        remote_state_head: String,
        unresolved: &[String],
        reconciliation_outcomes: &[StateSyncReconciliationOutcome],
    ) -> RefineResult<StateSyncConflictSummary> {
        use sha2::{Digest, Sha256};

        let report_location = conflict_report_path(&self.runtime_root);
        let common = git_common_dir(&self.target_root)?
            .canonicalize()
            .unwrap_or_else(|_| git_common_dir(&self.target_root).unwrap_or_default());
        let remote_url = self.git_stdout(&["remote", "get-url", remote])?;
        let repository_identity = format!(
            "sha256:{:x}",
            Sha256::digest(format!("{}\0{remote_url}", common.display()).as_bytes())
        );
        let mut report = StateSyncConflictReport {
            version: CONFLICT_REPORT_VERSION,
            report_id: String::new(),
            phase,
            attempt_id: attempt.id.clone(),
            attempt_source: attempt.source.clone(),
            created_at: chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            target_identity: self
                .target_root
                .canonicalize()
                .unwrap_or_else(|_| self.target_root.clone())
                .display()
                .to_string(),
            repository_identity,
            configured_remote: remote.to_string(),
            baseline_snapshot: state_map_digest(baseline),
            local_snapshot: state_tree_digest_for_report(local_root, local)?,
            local_fingerprints: local
                .iter()
                .map(|(path, fingerprint)| {
                    (path.to_string_lossy().replace('\\', "/"), *fingerprint)
                })
                .collect(),
            remote_snapshot: state_tree_digest_for_report(remote_root, remote_state)?,
            local_state_head,
            remote_state_head,
            unresolved_paths: unresolved.to_vec(),
            reconciliation_outcomes: reconciliation_outcomes.to_vec(),
            recovery: StateSyncRecoveryMetadata {
                available: true,
                preview_command: "refine project state-recovery preview".to_string(),
                apply_command: "refine project state-recovery apply --authority <live|remote> --preview-file <preview.json> [--live-path <path> | --remote-path <path>]".to_string(),
            },
            report_location: report_location.display().to_string(),
        };
        report.report_id = conflict_report_id(&report)?;
        let bytes = serde_json::to_vec_pretty(&report).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode state-sync conflict report: {error}"
            ))
        })?;
        write_conflict_report_atomically(&report_location, &bytes)?;
        Ok(StateSyncConflictSummary {
            report_id: report.report_id,
            phase,
            unresolved_count: report.unresolved_paths.len(),
            report_location: report.report_location,
            recovery_command: report.recovery.preview_command,
            diagnostics: report
                .reconciliation_outcomes
                .iter()
                .filter(|outcome| report.unresolved_paths.contains(&outcome.path))
                .map(|outcome| outcome.detail.clone())
                .collect(),
        })
    }
}

pub(super) fn conflict_report_id(report: &StateSyncConflictReport) -> RefineResult<String> {
    use sha2::{Digest, Sha256};
    let mut unsigned = report.clone();
    unsigned.report_id.clear();
    let bytes = serde_json::to_vec(&unsigned).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode state-sync conflict report identity: {error}"
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn state_map_digest(state: &DurableStateMap) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for (path, fingerprint) in state {
        digest.update(path.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        digest.update(fingerprint.to_be_bytes());
        digest.update([0xff]);
    }
    format!("{:x}", digest.finalize())
}

fn state_tree_digest_for_report(
    root: &std::path::Path,
    state: &DurableStateMap,
) -> RefineResult<String> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for path in state.keys() {
        digest.update(path.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        let bytes = fs::read(root.join(path)).map_err(|error| {
            RefineError::Io(format!(
                "failed to hash state-sync conflict snapshot {}: {error}",
                root.join(path).display()
            ))
        })?;
        digest.update(Sha256::digest(bytes));
        digest.update([0xff]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn conflict_report_path(runtime_root: &std::path::Path) -> PathBuf {
    runtime_root
        .join(CONFLICT_REPORT_DIRECTORY)
        .join(CONFLICT_REPORT_FILE)
}

fn write_conflict_report_atomically(path: &std::path::Path, bytes: &[u8]) -> RefineResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RefineError::Io(format!(
            "conflict report path {} has no parent",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        RefineError::Io(format!(
            "failed to create state-sync conflict report directory {}: {error}",
            parent.display()
        ))
    })?;
    let temp = parent.join(format!(
        ".state-sync-conflict-{}-{}.tmp",
        std::process::id(),
        STATE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = File::create(&temp).map_err(|error| {
            RefineError::Io(format!(
                "failed to create state-sync conflict report {}: {error}",
                temp.display()
            ))
        })?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to persist state-sync conflict report {}: {error}",
                    temp.display()
                ))
            })?;
        fs::rename(&temp, path).map_err(|error| {
            RefineError::Io(format!(
                "failed to replace state-sync conflict report {}: {error}",
                path.display()
            ))
        })?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to durably commit state-sync conflict report directory {}: {error}",
                    parent.display()
                ))
            })
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
