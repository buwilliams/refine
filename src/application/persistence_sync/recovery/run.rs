use super::*;
use crate::error::StateRecoveryConflictReason;
use crate::infrastructure::git::ancestry::{Ancestry, classify};

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
    /// The daemon's unattended fallback. It retains whole-record remote
    /// authority for ordinary conflicts, but ambiguous Goal ownership must
    /// fail closed instead of borrowing authority from circumstance.
    Automatic(StateRecoveryDecision),
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
        let (decision, automatic) = match policy {
            StateRecoveryRunPolicy::Decision(decision) => (decision, false),
            StateRecoveryRunPolicy::Automatic(decision) => (decision, true),
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
            match self.run_state_recovery_attempt(attempt, &decision, automatic) {
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
        automatic: bool,
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
                automatic,
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
            let preserved = if pass.preserved_goal_owners.is_empty() {
                String::new()
            } else {
                format!(
                    " and preserved {} explicit Goal transfer(s)",
                    pass.preserved_goal_owners.len()
                )
            };
            let detail = format!(
                "Recovery settled {} contested path(s) with {} authority{preserved}; the displaced side stays reachable as a merge parent{}.",
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
                    preserved_goal_owners: pass.preserved_goal_owners,
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
            crate::infrastructure::storage::project_layout::refine_dir_for_target_root(
                &self.target_root,
            )?;
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

    /// The daemon keeps its established whole-record remote fallback for
    /// ordinary conflicts. Goal ownership is not encoded as path authority:
    /// every recovery attempt reclassifies the fresh three-way operands and
    /// overlays a proven transfer (or fails closed on ambiguity) at merge time.
    pub fn automatic_recovery_decision(
        &self,
        _report: &StateSyncConflictReport,
    ) -> StateRecoveryDecision {
        StateRecoveryDecision::uniform(StateRecoveryAuthority::Remote)
    }
}
