use super::*;
use crate::process::supervisor::errors::StateRecoveryConflictReason;

/// How many times a run re-derives fresh evidence after losing a bounded race
/// before surfacing the last race to the caller.
const RUN_ATTEMPT_LIMIT: u32 = 3;
/// Pause between racing attempts so a moving remote head or a concurrent live
/// writer can settle before evidence is derived again.
const RUN_ATTEMPT_DELAY: Duration = Duration::from_secs(2);

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
        let mut last_race = None;
        for attempt in 1..=RUN_ATTEMPT_LIMIT {
            if attempt > 1 {
                thread::sleep(RUN_ATTEMPT_DELAY);
            }
            match self.run_state_recovery_attempt(attempt, &decision) {
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
        decision: &StateRecoveryDecision,
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
            let recovery = self.apply_state_recovery_decision_locked(decision.clone(), preview)?;
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
}
