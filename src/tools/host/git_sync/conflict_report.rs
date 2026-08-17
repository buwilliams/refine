use super::*;

const CONFLICT_REPORT_VERSION: u32 = 3;
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
    /// One-shot recovery that re-derives and applies evidence atomically with
    /// bounded race retries. Reports written before this field existed
    /// deserialize with an empty command.
    #[serde(default)]
    pub run_command: String,
    pub preview_command: String,
}

/// One conflicted path with a short domain-terms summary an operator can read
/// (which goal, which members each side changed).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncConflictPath {
    pub path: String,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncConflictReport {
    pub version: u32,
    /// Stable across attempts of the same divergence by construction: derived
    /// from the merge base, both heads, and the sorted unresolved paths —
    /// every operand a commit, none of them wall-clock or attempt state.
    pub report_id: String,
    pub phase: StateSyncConflictPhase,
    pub attempt_id: String,
    pub attempt_source: String,
    pub created_at: String,
    pub target_identity: String,
    pub repository_identity: String,
    pub configured_remote: String,
    pub merge_base: String,
    pub local_state_head: String,
    pub remote_state_head: String,
    pub unresolved_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<StateSyncConflictPath>,
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
            "Refine state changed on multiple nodes during {}: {} unresolved path(s){}; complete conflict report {} is at {}. Run `{}` for one-shot recovery, or `refine sync --preview` to review the comparison first.",
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
        merge_base: &str,
        local_state_head: &str,
        remote_state_head: &str,
        unresolved: &[StateSyncConflictPath],
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
        let mut unresolved = unresolved.to_vec();
        unresolved.sort_by(|left, right| left.path.cmp(&right.path));
        let report = StateSyncConflictReport {
            version: CONFLICT_REPORT_VERSION,
            report_id: conflict_report_id(
                merge_base,
                local_state_head,
                remote_state_head,
                &unresolved,
            ),
            phase,
            attempt_id: attempt.id.clone(),
            attempt_source: attempt.source.clone(),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            target_identity: self
                .target_root
                .canonicalize()
                .unwrap_or_else(|_| self.target_root.clone())
                .display()
                .to_string(),
            repository_identity,
            configured_remote: remote.to_string(),
            merge_base: merge_base.to_string(),
            local_state_head: local_state_head.to_string(),
            remote_state_head: remote_state_head.to_string(),
            unresolved_paths: unresolved
                .iter()
                .map(|conflict| conflict.path.clone())
                .collect(),
            conflicts: unresolved,
            recovery: StateSyncRecoveryMetadata {
                available: true,
                run_command: "refine sync --authority <live|remote> [--path <contested-path>]"
                    .to_string(),
                preview_command: "refine sync --preview".to_string(),
            },
            report_location: report_location.display().to_string(),
        };
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
            recovery_command: report.recovery.run_command,
            diagnostics: report
                .conflicts
                .iter()
                .map(|conflict| conflict.summary.clone())
                .collect(),
        })
    }
}

/// The report's identity is the divergence itself: same operands, same
/// unresolved paths, same id — regardless of when or how often it is retried.
pub(super) fn conflict_report_id(
    merge_base: &str,
    local_state_head: &str,
    remote_state_head: &str,
    unresolved: &[StateSyncConflictPath],
) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for operand in [merge_base, local_state_head, remote_state_head] {
        digest.update(operand.as_bytes());
        digest.update([0]);
    }
    for conflict in unresolved {
        digest.update(conflict.path.as_bytes());
        digest.update([0xff]);
    }
    format!("{:x}", digest.finalize())
}

/// A one-line domain summary of one contested path: the record's identity and
/// the top-level members each side changed since the shared base.
pub(super) fn conflict_path_summary(
    relative: &std::path::Path,
    base: Option<&[u8]>,
    local: Option<&[u8]>,
    remote: Option<&[u8]>,
) -> String {
    let subject = if is_goal_record(relative) {
        let goal_id = [local, remote, base]
            .into_iter()
            .flatten()
            .find_map(|bytes| {
                serde_json::from_slice::<serde_json::Value>(bytes)
                    .ok()?
                    .get("id")?
                    .as_str()
                    .map(str::to_string)
            });
        match goal_id {
            Some(id) => format!("goal {id}"),
            None => format!("goal record {}", relative.display()),
        }
    } else {
        relative.display().to_string()
    };
    match (base, local, remote) {
        (_, None, Some(_)) => format!("{subject}: deleted on this node, changed on another"),
        (_, Some(_), None) => format!("{subject}: changed on this node, deleted on another"),
        (Some(base), Some(local), Some(remote)) => {
            let local_members = changed_members(base, local);
            let remote_members = changed_members(base, remote);
            match (local_members, remote_members) {
                (Some(local_members), Some(remote_members)) => {
                    let contested = local_members
                        .intersection(&remote_members)
                        .cloned()
                        .collect::<Vec<_>>();
                    if contested.is_empty() {
                        format!(
                            "{subject}: this node changed [{}], another changed [{}]",
                            local_members.into_iter().collect::<Vec<_>>().join(", "),
                            remote_members.into_iter().collect::<Vec<_>>().join(", ")
                        )
                    } else {
                        format!("{subject}: both nodes changed {}", contested.join(", "))
                    }
                }
                _ => format!("{subject}: changed on both nodes"),
            }
        }
        _ => format!("{subject}: added independently on both nodes"),
    }
}

fn changed_members(base: &[u8], side: &[u8]) -> Option<BTreeSet<String>> {
    let base = serde_json::from_slice::<serde_json::Value>(base).ok()?;
    let side = serde_json::from_slice::<serde_json::Value>(side).ok()?;
    let base = base.as_object()?;
    let side = side.as_object()?;
    Some(
        base.keys()
            .chain(side.keys())
            .filter(|key| base.get(*key) != side.get(*key))
            .cloned()
            .collect(),
    )
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
