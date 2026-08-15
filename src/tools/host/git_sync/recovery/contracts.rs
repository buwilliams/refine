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
    pub live_snapshot: String,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryManifest {
    pub version: u32,
    pub evidence_id: String,
    pub authority: StateRecoveryAuthority,
    pub node: String,
    pub target_identity: String,
    pub repository_identity: String,
    pub configured_remote: String,
    pub local_state_head_before: Option<String>,
    pub remote_state_head_before: String,
    pub live_snapshot_before: String,
    pub local_state_head_after: Option<String>,
    pub remote_state_head_after: Option<String>,
    pub path_counts: StateRecoveryPathCounts,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub outcome: StateRecoveryOutcome,
    pub recovery_location: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryResult {
    pub ok: bool,
    pub authority: StateRecoveryAuthority,
    pub baseline_created: bool,
    pub local_state_head: Option<String>,
    pub remote_state_head: String,
    pub recovery_location: String,
    pub manifest_path: String,
    pub path_counts: StateRecoveryPathCounts,
    pub detail: String,
}
