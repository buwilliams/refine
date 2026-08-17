use super::*;
use crate::process::supervisor::errors::StateRecoveryConflictReason;

/// How many times a run re-derives fresh evidence after losing a bounded race
/// before surfacing the last race to the caller.
const RUN_ATTEMPT_LIMIT: u32 = 3;
/// Pause between racing attempts so a moving remote head or a concurrent live
/// writer can settle before evidence is derived again.
const RUN_ATTEMPT_DELAY: Duration = Duration::from_secs(2);

/// How a one-shot run chooses authority once it has derived fresh evidence.
#[derive(Clone, Debug)]
pub enum StateRecoveryRunPolicy {
    /// Apply the caller's explicit decision unchanged.
    Decision(StateRecoveryDecision),
    /// Remote is authoritative for every conflicting path except Goal records
    /// this node owned at the last agreed baseline, which keep their live
    /// copy.
    ///
    /// A stale local understanding is not a wrong one: the owner of a Goal is
    /// the only node that legitimately advances its record, so staleness alone
    /// must never discard work only this node could have produced, while
    /// everything it does not own converges to the fleet. Ownership is read
    /// from the baseline — the last state both sides agreed on — so neither
    /// side's contested edit can vote in its own favor. Missing-baseline
    /// recovery has no agreement to read ownership from and stays uniformly
    /// remote (the pre-recovery live state remains retained on the recovery
    /// ref either way).
    OwnershipPrefersRemote,
}

impl FileGitSyncService {
    /// Synchronize and, when synchronization is rejected with recoverable
    /// evidence, preview and apply recovery with the given decision as one
    /// operation, then verify with a fresh synchronization.
    ///
    /// The separate preview/apply pair is an operator compare-and-swap: any
    /// movement between the two invocations fails closed as a stale preview.
    /// Driving that pair from caller-side loops re-created the race between
    /// every step, so this method owns the whole sequence instead: evidence is
    /// derived and consumed under a single repository lock hold, and a lost
    /// race against the moving remote or a concurrent live write is retried
    /// here, bounded. Every non-race failure surfaces immediately.
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
        let mut last_race = None;
        for attempt in 1..=RUN_ATTEMPT_LIMIT {
            if attempt > 1 {
                thread::sleep(RUN_ATTEMPT_DELAY);
            }
            match self.run_state_recovery_attempt(attempt, &policy) {
                Ok(result) => return Ok(result),
                Err(
                    error @ RefineError::StateRecoveryConflict {
                        reason:
                            StateRecoveryConflictReason::StalePreview
                            | StateRecoveryConflictReason::GitBusy,
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
        policy: &StateRecoveryRunPolicy,
    ) -> RefineResult<StateRecoveryRunResult> {
        with_repository_git_lock(&self.target_root, || {
            let context = StateSyncAttemptContext::new(
                uuid::Uuid::new_v4().to_string(),
                "state_recovery_run",
            );
            let report_before = latest_state_sync_conflict_report(&self.runtime_root)?
                .map(|report| report.report_id);
            let sync_error = match self.sync_locked(GitFetchScope::All, &context) {
                Ok(sync) => {
                    return Ok(StateRecoveryRunResult {
                        ok: true,
                        attempts: attempt,
                        recovered: false,
                        recovery: None,
                        sync,
                        detail: "State synchronized without recovery.".to_string(),
                    });
                }
                Err(error) => error,
            };
            // Recovery consumes exactly two synchronization rejections: a
            // missing three-way baseline, and a semantic conflict this same
            // attempt just recorded. Every other failure (network, push,
            // storage) surfaces as itself; recovering through it would only
            // re-report the real problem as a confusing stale fence.
            let report_after = latest_state_sync_conflict_report(&self.runtime_root)?
                .map(|report| report.report_id);
            let recorded_fresh_conflict = report_after.is_some() && report_after != report_before;
            if !matches!(sync_error, RefineError::StateSyncMissingBaseline(_))
                && !recorded_fresh_conflict
            {
                return Err(sync_error);
            }
            let preview = self.preview_state_recovery_locked()?;
            let decision = self.resolve_run_decision(policy, &preview)?;
            let recovery = self.apply_state_recovery_decision_locked(decision, preview)?;
            let sync = match self.sync_locked(GitFetchScope::State, &context) {
                Ok(sync) => sync,
                // A semantic conflict here means state diverged again between
                // the recovered baseline and this verification — the same
                // moving-target race the outer bounded retry exists for.
                Err(RefineError::Conflict(summary)) => {
                    return Err(stale_recovery(&format!(
                        "state diverged again while recovery was being verified: {summary}"
                    )));
                }
                Err(error) => return Err(error),
            };
            Ok(StateRecoveryRunResult {
                ok: true,
                attempts: attempt,
                recovered: true,
                recovery: Some(recovery),
                sync,
                detail: "Recovery applied and verified by a fresh synchronization.".to_string(),
            })
        })
    }

    /// Turn the run policy into the concrete per-path decision for this
    /// attempt's freshly derived preview.
    fn resolve_run_decision(
        &self,
        policy: &StateRecoveryRunPolicy,
        preview: &StateRecoveryPreview,
    ) -> RefineResult<StateRecoveryDecision> {
        let StateRecoveryRunPolicy::OwnershipPrefersRemote = policy else {
            let StateRecoveryRunPolicy::Decision(decision) = policy else {
                unreachable!("run policy is either a fixed decision or ownership");
            };
            return Ok(decision.clone());
        };
        if preview.baseline_status != "valid_conflict" {
            return Ok(StateRecoveryDecision::uniform(
                StateRecoveryAuthority::Remote,
            ));
        }
        let live_refine =
            crate::tools::host::project_layout::refine_dir_for_target_root(&self.target_root)?;
        let node_id = FileNodeRegistryService::with_active_root(&live_refine, &self.runtime_root)
            .active_node_id()
            .unwrap_or_else(|_| "default".to_string());
        let baseline = self.load_state_baseline()?.unwrap_or_default();
        let state_root = state_worktree_for_target_root(&self.target_root)?;
        let mut overrides = Vec::new();
        for path in &preview.conflicting_paths {
            let relative = PathBuf::from(path);
            if !is_goal_record(&relative) {
                continue;
            }
            // Every unknown stays remote: a record absent from the last
            // agreement, or whose agreed bytes cannot be reproduced, gives
            // this node no ownership claim — and its live copy is still
            // retained on the recovery ref.
            let Some(fingerprint) = baseline.get(&relative) else {
                continue;
            };
            if !state_root.exists() {
                continue;
            }
            let Some(bytes) = self.load_baseline_file(&state_root, &relative, *fingerprint)? else {
                continue;
            };
            let owner = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|goal| {
                    goal.get("node_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "default".to_string());
            if owner == node_id {
                overrides.push(StateRecoveryOverride {
                    path: path.clone(),
                    authority: StateRecoveryAuthority::Live,
                });
            }
        }
        Ok(StateRecoveryDecision {
            default_authority: StateRecoveryAuthority::Remote,
            overrides,
        })
    }
}
