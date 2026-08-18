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

    pub(crate) fn authority_for(&self, path: &str) -> StateRecoveryAuthority {
        self.overrides
            .iter()
            .find(|chosen| chosen.path == path)
            .map(|chosen| chosen.authority)
            .unwrap_or(self.default_authority)
    }
}

/// Read-only divergence summary: what one classify plus a dry-run merge of the
/// current heads would find. Produced fresh on every call; never written to
/// disk and never used as an apply token — the terminal run re-derives
/// everything itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryPreview {
    pub version: u32,
    pub configured_remote: String,
    pub local_state_head: Option<String>,
    pub remote_state_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_base: Option<String>,
    /// `converged`, `local_ahead`, `remote_ahead`, `diverged`, `unrelated`,
    /// `join`, or `remote_missing`.
    pub ancestry: String,
    /// Live records not yet committed to the local state branch; they become
    /// the next pass's local delta.
    pub live_pending_paths: Vec<String>,
    /// Paths only this side changed (or only this side holds, on a join).
    pub local_paths: Vec<String>,
    /// Paths only the remote side changed (or only the remote holds).
    pub remote_paths: Vec<String>,
    /// Both-changed paths the structural driver can settle without judgment.
    pub resolvable_paths: Vec<String>,
    /// Genuinely contested paths with domain-terms summaries; these are the
    /// paths an authority decision would settle.
    pub conflicts: Vec<StateSyncConflictPath>,
    /// The domain-terms question agent resolution escalated with for exactly
    /// this contention — the same remote head and the same contested records
    /// the sync surface keys on — when one is on file: what is contested and
    /// what must be chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_question: Option<String>,
    pub detail: String,
}

/// Terminal outcome of a one-shot `sync` run: with an authority the
/// pipeline settles every contested path on the chosen side inside one merge
/// commit and verifies convergence with a read; without one it is the
/// ordinary sync pipeline. `recovered` is false when nothing was contested.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryRunResult {
    pub ok: bool,
    /// 1-based attempt that produced this result; earlier attempts lost a
    /// bounded race against a moving remote.
    pub attempts: u32,
    pub recovered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<StateRecoveryResult>,
    /// What the synchronization pass itself did.
    pub sync: GitSyncResult,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryResult {
    pub ok: bool,
    pub authority: StateRecoveryAuthority,
    #[serde(default)]
    pub overrides: Vec<StateRecoveryOverride>,
    pub local_state_head: Option<String>,
    pub remote_state_head: Option<String>,
    /// Contested paths the decision settled. Both pre-merge heads are parents
    /// of the recovery merge commit, so every displaced version stays
    /// reachable without a side ledger.
    pub settled_paths: Vec<String>,
    /// Goal owners independently proven from the three-way operands and
    /// preserved across a coarser whole-record authority choice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preserved_goal_owners: Vec<StateRecoveryGoalOwner>,
    /// Refs retained for displaced state that is not otherwise reachable as a
    /// merge parent (a joining node's live store).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_refs: Vec<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateRecoveryGoalOwner {
    pub path: String,
    pub node_id: String,
}
