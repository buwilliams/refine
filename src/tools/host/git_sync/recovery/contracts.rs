use super::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateRecoveryAuthority {
    Live,
    Remote,
}

impl StateRecoveryAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Remote => "remote",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryOverride {
    pub path: String,
    pub authority: StateRecoveryAuthority,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryDecision {
    pub default_authority: StateRecoveryAuthority,
    #[serde(default)]
    pub overrides: Vec<StateRecoveryOverride>,
}

impl StateRecoveryDecision {
    pub fn uniform(default_authority: StateRecoveryAuthority) -> Self {
        Self {
            default_authority,
            overrides: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryPathCounts {
    pub live_only: usize,
    pub remote_only: usize,
    pub equal: usize,
    pub differing: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryPreview {
    pub version: u32,
    pub evidence_id: String,
    pub target_identity: String,
    pub repository_identity: String,
    pub configured_remote: String,
    pub local_state_head: Option<String>,
    pub remote_state_head: String,
    pub baseline_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_report_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_report_location: Option<String>,
    pub live_snapshot: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub live_fingerprints: BTreeMap<String, u64>,
    pub remote_snapshot: String,
    pub path_counts: StateRecoveryPathCounts,
    pub conflicting_paths: Vec<String>,
    pub conflicting_paths_truncated: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateRecoveryOutcome {
    Started,
    Failed,
    Succeeded,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateRecoveryStage {
    #[default]
    Started,
    TargetPersisted,
    Published,
    Hydrated,
    BaselineCreated,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryManifest {
    pub version: u32,
    pub evidence_id: String,
    pub authority: StateRecoveryAuthority,
    #[serde(default)]
    pub decision_id: String,
    #[serde(default)]
    pub overrides: Vec<StateRecoveryOverride>,
    pub node: String,
    pub target_identity: String,
    pub repository_identity: String,
    pub configured_remote: String,
    pub local_state_head_before: Option<String>,
    pub remote_state_head_before: String,
    pub live_snapshot_before: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_snapshot_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_report_id: Option<String>,
    pub local_state_head_after: Option<String>,
    pub remote_state_head_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_location: Option<String>,
    pub path_counts: StateRecoveryPathCounts,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub outcome: StateRecoveryOutcome,
    #[serde(default)]
    pub stage: StateRecoveryStage,
    pub recovery_location: String,
    pub message: String,
}

/// Terminal outcome of a one-shot `state-recovery run`: synchronization,
/// recovery evidence, authority application, and verification as a single
/// operation. `recovered` is false when synchronization needed no recovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryRunResult {
    pub ok: bool,
    /// 1-based attempt that produced this result; earlier attempts lost a
    /// bounded race against a moving remote or a concurrent live write.
    pub attempts: u32,
    pub recovered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<StateRecoveryResult>,
    /// The verifying synchronization (or the ordinary one when no recovery
    /// was needed).
    pub sync: GitSyncResult,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryResult {
    pub ok: bool,
    pub authority: StateRecoveryAuthority,
    #[serde(default)]
    pub overrides: Vec<StateRecoveryOverride>,
    pub baseline_created: bool,
    pub local_state_head: Option<String>,
    pub remote_state_head: String,
    pub recovery_location: String,
    pub manifest_path: String,
    pub path_counts: StateRecoveryPathCounts,
    pub detail: String,
}
