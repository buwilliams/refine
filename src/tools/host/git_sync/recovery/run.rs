use super::*;
use crate::process::supervisor::errors::StateRecoveryConflictReason;
use crate::tools::git::ancestry::{Ancestry, classify};

/// How many times a run re-derives after losing a bounded race against a
/// moving remote before surfacing the last race to the caller.
const RUN_ATTEMPT_LIMIT: u32 = 3;
/// Pause between racing attempts so a moving remote head can settle before
/// the divergence is derived again.
const RUN_ATTEMPT_DELAY: Duration = Duration::from_secs(2);

/// What a one-shot run does.
#[derive(Clone, Debug)]
pub enum StateRecoveryRunPolicy {
    /// Terminal: run the sync pipeline with the decision attached, so every
    /// contested path takes the decided side inside one merge commit.
    Decision(StateRecoveryDecision),
    /// No authority: the ordinary sync pipeline. A conflict fails closed with
    /// its stable report id for the operator (or the daemon's ownership
    /// policy) to answer.
    SyncOnly,
}

impl FileGitSyncService {
    /// Recovery is sync with a decision attached: one pass of the ordinary
    /// deterministic ladder in which every path the ladder cannot settle
    /// takes the decided side, committed as a single merge commit with both
    /// heads as parents. It never re-enters the merge it is clearing —
    /// rerunning classifies Equal and is a no-op by construction — and it is
    /// verified with a read (`classify == Equal`), never a re-merge.
    pub fn run_state_recovery(
        &self,
        decision: StateRecoveryDecision,
    ) -> RefineResult<StateRecoveryRunResult> {
        self.run_state_recovery_with_policy(StateRecoveryRunPolicy::Decision(decision))
    }

    pub fn run_state_recovery_with_policy(
        &self,
        policy: StateRecoveryRunPolicy,
    ) -> RefineResult<StateRecoveryRunResult> {
        let decision = match policy {
            StateRecoveryRunPolicy::Decision(decision) => decision,
            StateRecoveryRunPolicy::SyncOnly => {
                let context = StateSyncAttemptContext::new(
                    uuid::Uuid::new_v4().to_string(),
                    "state_recovery_run",
                );
                let sync = self.sync_with_attempt(context)?;
                return Ok(StateRecoveryRunResult {
                    ok: true,
                    attempts: 1,
                    recovered: false,
                    recovery: None,
                    sync,
                    detail: "State synchronized without recovery.".to_string(),
                });
            }
        };
        let mut last_race = None;
        for attempt in 1..=RUN_ATTEMPT_LIMIT {
            if attempt > 1 {
                thread::sleep(RUN_ATTEMPT_DELAY);
            }
            match self.run_state_recovery_attempt(attempt, &decision) {
                Ok(result) => return Ok(result),
                Err(
                    error @ RefineError::StateRecoveryConflict {
                        reason: StateRecoveryConflictReason::StateMoved,
                        ..
                    },
                ) => {
                    last_race = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_race.expect("every exhausted recovery attempt records its race"))
    }

    fn run_state_recovery_attempt(
        &self,
        attempt: u32,
        decision: &StateRecoveryDecision,
    ) -> RefineResult<StateRecoveryRunResult> {
        with_repository_git_lock(&self.target_root, || {
            self.reject_foreign_git_operation()?;
            let context = StateSyncAttemptContext::new(
                uuid::Uuid::new_v4().to_string(),
                "state_recovery_run",
            );
            let pass = self.sync_locked_pass(
                GitFetchScope::All,
                &context,
                Some(decision),
                false,
                &mut None,
            )?;
            if pass.settled.is_empty() && pass.retained.is_empty() {
                return Ok(StateRecoveryRunResult {
                    ok: true,
                    attempts: attempt,
                    recovered: false,
                    recovery: None,
                    sync: pass.result,
                    detail: "State synchronized without recovery.".to_string(),
                });
            }
            let (local_head, remote_head) = self.verify_recovery_converged()?;
            let detail = format!(
                "Recovery settled {} contested path(s) with {} authority; the displaced side stays reachable as a merge parent{}.",
                pass.settled.len(),
                decision.default_authority.as_str(),
                if pass.retained.is_empty() {
                    String::new()
                } else {
                    format!(" or retained ref ({})", pass.retained.join(", "))
                }
            );
            Ok(StateRecoveryRunResult {
                ok: true,
                attempts: attempt,
                recovered: true,
                recovery: Some(StateRecoveryResult {
                    ok: true,
                    authority: decision.default_authority,
                    overrides: decision.overrides.clone(),
                    local_state_head: local_head,
                    remote_state_head: remote_head,
                    settled_paths: pass.settled,
                    retained_refs: pass.retained,
                    detail: detail.clone(),
                }),
                sync: pass.result,
                detail,
            })
        })
    }

    /// Verification is a read, never a re-merge: after a terminal pass the
    /// local branch and the remote must classify Equal. Anything else means
    /// the remote moved between publish and verification — the bounded race
    /// the outer retry exists for.
    fn verify_recovery_converged(&self) -> RefineResult<(Option<String>, Option<String>)> {
        let live_refine =
            crate::tools::host::project_layout::refine_dir_for_target_root(&self.target_root)?;
        let remote = self.configured_remote(&live_refine)?;
        if !self.remote_exists(&remote)? {
            return Ok((self.local_state_head()?, None));
        }
        if !self.remote_state_exists(&remote)? {
            return Ok((self.local_state_head()?, None));
        }
        self.fetch_state_branch(&remote)?;
        let remote_head =
            self.git_stdout(&["rev-parse", &format!("{remote}/{REFINE_STATE_BRANCH}")])?;
        let Some(local_head) = self.local_state_head()? else {
            return Err(raced_recovery(
                "the local refine/state branch disappeared during verification",
            ));
        };
        match classify(self, &local_head, &remote_head)? {
            Ancestry::Equal => Ok((Some(local_head), Some(remote_head))),
            _ => Err(raced_recovery(
                "state diverged again while recovery was being verified",
            )),
        }
    }

    /// The daemon's ownership policy, read from the merge base so no side
    /// votes for itself: remote authority by default, with a live override
    /// for each contested goal record whose merge-base bytes name this node
    /// as `node_id`. First-contact conflicts have no merge base and settle
    /// uniformly remote.
    pub fn merge_base_ownership_decision(
        &self,
        report: &StateSyncConflictReport,
        node_id: &str,
    ) -> StateRecoveryDecision {
        let mut overrides = Vec::new();
        if !report.merge_base.is_empty() {
            for path in &report.unresolved_paths {
                let relative = PathBuf::from(path);
                if !is_goal_record(&relative) {
                    continue;
                }
                let Ok(Some(bytes)) = self.state_bytes_at(
                    &self.target_root,
                    &report.merge_base,
                    &format!(".refine/{path}"),
                ) else {
                    continue;
                };
                let owner = serde_json::from_slice::<serde_json::Value>(&bytes)
                    .ok()
                    .and_then(|record| {
                        record
                            .get("node_id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    });
                if owner.as_deref() == Some(node_id) {
                    overrides.push(StateRecoveryOverride {
                        path: path.clone(),
                        authority: StateRecoveryAuthority::Live,
                    });
                }
            }
        }
        StateRecoveryDecision {
            default_authority: StateRecoveryAuthority::Remote,
            overrides,
        }
    }
}
