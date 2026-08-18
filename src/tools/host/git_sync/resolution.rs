use super::*;

use std::path::Path;
use std::sync::Arc;

use super::service::{OUTSIDE_STATE_CONFLICT_SUMMARY, StateSyncPass};
use crate::tools::git::resolve::{
    ConflictResolver, ConflictedPath, InstalledAgentResolver, PreparedResolution, ResolutionLock,
    ResolutionOutcome, StateFileGates, buy_contention_attempt, conflict_sides,
    contention_budget_available, materialize_conflicted_workspace, pin_resolution_inputs,
    resolve_conflict, resolver_override, retire_resolution, state_conflict_block,
    state_conflict_context, surviving_resolution_result, sweep_abandoned_resolutions,
    try_claim_resolution,
};
use crate::tools::host::git_worktrees::FileGitWorktreeService;

/// How many resolution rounds one sync entry may run: the initial resolution
/// plus one re-derivation after the heads moved underneath a resolved merge.
/// A second race surfaces as the conflict report.
const RESOLUTION_ROUND_LIMIT: u32 = 2;

/// Everything lock Hold A prepared for one unlocked resolution attempt, plus
/// the report operands the entry needs to escalate in domain terms.
pub(super) struct PreparedStateResolution {
    pub(super) resolution: PreparedResolution,
    pub(super) phase: StateSyncConflictPhase,
    pub(super) remote: String,
    pub(super) merge_base: String,
    pub(super) local_head: String,
    pub(super) remote_head: String,
    pub(super) unresolved: Vec<StateSyncConflictPath>,
}

/// What the locked conflict path produced for the entry level.
pub(super) enum StateResolutionHold {
    /// A gated result from an interrupted earlier run of this exact
    /// divergence survives in the resolve refs: publish it as the merge
    /// commit without invoking anything — the crash-only rerun path.
    Result { report_id: String, commit: String },
    /// A freshly pinned and materialized workspace for the unlocked attempt.
    Prepared(Box<PreparedStateResolution>),
    /// Another operation already claimed this divergence and is resolving it
    /// unlocked right now. This pass neither resolves nor reports it.
    Busy { report_id: String },
}

/// What the locked pass handed the entry for the unlocked middle.
pub(super) enum StateResolutionSlot {
    Prepared(Box<PreparedStateResolution>),
    /// The divergence belongs to another operation's in-flight resolution;
    /// the entry defers to it instead of reporting a conflict of its own.
    Busy,
}

/// How a sync entry takes the repository lock for each of its short holds.
#[derive(Clone, Copy)]
pub(super) enum LockAcquisition {
    Blocking,
    Try,
}

impl FileGitSyncService {
    /// Whether this node's `state_sync_agent_resolution` setting allows agent
    /// call-outs. Errors fail closed: resolution publishes merges, so an
    /// unreadable settings store must not authorize it.
    fn agent_resolution_setting_enabled(&self) -> bool {
        let Ok(refine_dir) = prepare_refine_dir(&self.target_root) else {
            return false;
        };
        let Ok(settings) =
            FileSettingsService::with_active_root(refine_dir, &self.runtime_root).load()
        else {
            return false;
        };
        settings
            .get("state_sync_agent_resolution")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            != Some("off")
    }

    /// The resolver this sync entry may call out to, if any. A test-installed
    /// override always wins; otherwise the installed-agent resolver engages
    /// only on entry points built `with_agent_resolution` and only while the
    /// node's `state_sync_agent_resolution` setting allows it.
    pub(super) fn state_conflict_resolver(
        &self,
    ) -> Option<Arc<dyn ConflictResolver + Send + Sync>> {
        if let Some(resolver) = resolver_override(&self.target_root) {
            return Some(resolver);
        }
        if !self.agent_resolution || !self.agent_resolution_setting_enabled() {
            return None;
        }
        let refine_dir = prepare_refine_dir(&self.target_root).ok()?;
        Some(Arc::new(InstalledAgentResolver::from_settings(
            &refine_dir,
            &self.runtime_root,
        )))
    }

    /// The sync entry: run the locked deterministic ladder, and when it fails
    /// closed on a genuine conflict, resolve it inline with the agent —
    /// UNLOCKED, between two short holds. A resolved conflict reruns the
    /// locked pass, which finds the gated result ref and publishes it (Hold
    /// B: the pins are compared against the fresh heads, so a moved head
    /// re-derives — once — instead of publishing a stale resolution).
    /// `NeedsDecision` rewrites the conflict report with the domain-terms
    /// question as its headline; an unavailable resolver leaves today's
    /// fail-closed behavior untouched.
    pub(super) fn sync_resolving(
        &self,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
        acquisition: LockAcquisition,
    ) -> RefineResult<GitSyncResult> {
        let resolver = self.state_conflict_resolver();
        let mut rounds = 0u32;
        let mut superseded: Option<String> = None;
        loop {
            let mut slot = None;
            let engaged = resolver.is_some();
            match self.run_sync_locked(acquisition, fetch_scope, attempt, engaged, &mut slot) {
                Ok(None) => {
                    return Ok(deferred(
                        "Repository Git operations are busy; sync will retry on the next cadence.",
                    ));
                }
                Ok(Some(pass)) => {
                    if let Some(report_id) = &pass.published_resolution {
                        self.finish_resolution(report_id);
                    }
                    return Ok(pass.result);
                }
                Err(error) => {
                    if let Some(StateResolutionSlot::Busy) = slot {
                        return Ok(deferred(
                            "Another operation is resolving this state conflict; sync will retry on the next cadence.",
                        ));
                    }
                    let (Some(StateResolutionSlot::Prepared(prepared)), Some(resolver)) =
                        (slot.take(), resolver.as_ref())
                    else {
                        return Err(error);
                    };
                    // A re-derived pass that conflicted under a NEW id makes
                    // the earlier round's resolution moot: its heads moved,
                    // so retire its now-stale resolution.
                    if let Some(stale) = superseded
                        .replace(prepared.resolution.pinned.id.clone())
                        .filter(|stale| *stale != prepared.resolution.pinned.id)
                    {
                        self.retire_state_resolution(&stale, None);
                    }
                    if rounds >= RESOLUTION_ROUND_LIMIT {
                        self.retire_state_resolution(
                            &prepared.resolution.pinned.id,
                            Some(&prepared.resolution.lock),
                        );
                        return Err(error);
                    }
                    rounds += 1;
                    match resolve_conflict(
                        self,
                        &prepared.resolution,
                        resolver.as_ref(),
                        &StateFileGates,
                    )? {
                        // Rerun the locked pass: the surviving result ref
                        // publishes under the next hold, and heads that moved
                        // meanwhile discard it and re-derive instead.
                        ResolutionOutcome::Resolved { .. } => continue,
                        ResolutionOutcome::Unavailable => {
                            self.retire_state_resolution(
                                &prepared.resolution.pinned.id,
                                Some(&prepared.resolution.lock),
                            );
                            return Err(error);
                        }
                        ResolutionOutcome::NeedsDecision { question } => {
                            self.retire_state_resolution(
                                &prepared.resolution.pinned.id,
                                Some(&prepared.resolution.lock),
                            );
                            let summary = self.record_conflict_report(
                                prepared.phase,
                                attempt,
                                &prepared.remote,
                                &prepared.merge_base,
                                &prepared.local_head,
                                &prepared.remote_head,
                                &prepared.unresolved,
                                Some(&question),
                            )?;
                            return Err(RefineError::Conflict(summary.to_string()));
                        }
                    }
                }
            }
        }
    }

    /// One locked ladder pass under the requested acquisition. `Ok(None)`
    /// means the lock was busy (try mode) and the caller defers.
    fn run_sync_locked(
        &self,
        acquisition: LockAcquisition,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
        engaged: bool,
        slot: &mut Option<StateResolutionSlot>,
    ) -> RefineResult<Option<StateSyncPass>> {
        match acquisition {
            LockAcquisition::Blocking => with_repository_git_lock(&self.target_root, || {
                self.sync_locked_pass(fetch_scope, attempt, None, engaged, slot)
            })
            .map(Some),
            LockAcquisition::Try => {
                let lock = repository_git_lock(&self.target_root)?;
                let _guard = match lock.try_lock() {
                    Ok(guard) => guard,
                    Err(TryLockError::WouldBlock) => return Ok(None),
                    Err(TryLockError::Poisoned(_)) => {
                        return Err(RefineError::Conflict(
                            "Repository Git lock was poisoned".to_string(),
                        ));
                    }
                };
                let Some(_file_guard) = RepositoryFileLock::try_acquire(&self.target_root)? else {
                    return Ok(None);
                };
                self.sync_locked_pass(fetch_scope, attempt, None, engaged, slot)
                    .map(Some)
            }
        }
    }

    /// Delete the durable traces of a published resolution: its refs, its
    /// workspace, and the conflict report it settled. Best-effort — the sync
    /// itself already converged, `clear_conflict_report` is id-scoped, and
    /// what an interruption leaves behind is swept by the next divergence's
    /// Hold A.
    fn finish_resolution(&self, report_id: &str) {
        self.retire_state_resolution(report_id, None);
        let _ = self.clear_conflict_report(report_id);
    }

    /// Retire one resolution's refs and workspace. `held` is this entry's own
    /// claim on the id; without one the retire claims the id first and leaves
    /// a resolution another operation is running to that operation.
    fn retire_state_resolution(&self, id: &str, held: Option<&ResolutionLock>) {
        let worktrees =
            FileGitWorktreeService::with_runtime_root(&self.target_root, &self.runtime_root);
        let _ = retire_resolution(&worktrees, self, &self.target_root, id, held);
    }

    /// Lock Hold A of a resolution: claim the divergence, pin the merge
    /// operands under `refs/refine/resolve/<report_id>`, read the per-path
    /// sides, materialize the isolated conflicted workspace, and render the
    /// domain context — everything the unlocked attempt needs. A surviving
    /// result ref for the same operands short-circuits to publication.
    ///
    /// Returns `None` when resolution does not apply: a contested path lies
    /// outside synchronized Refine state, which agent resolution never
    /// judges; this contention already escalated with a question nobody has
    /// answered yet — re-asking it spends an agent to re-derive the question
    /// already on file; or every contended record has spent its contention
    /// budget against this remote head, so resolution holds until something
    /// that needs deciding actually changes.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_state_resolution(
        &self,
        state_root: &Path,
        phase: StateSyncConflictPhase,
        remote: &str,
        merge_base: &str,
        local_head: &str,
        remote_head: &str,
        merged_tree: &str,
        unresolved: &[StateSyncConflictPath],
    ) -> RefineResult<Option<StateResolutionHold>> {
        if unresolved
            .iter()
            .any(|conflict| conflict.summary == OUTSIDE_STATE_CONFLICT_SUMMARY)
        {
            return Ok(None);
        }
        if self
            .escalated_decision_question(remote_head, unresolved)
            .is_some()
        {
            return Ok(None);
        }
        let tree_paths = unresolved
            .iter()
            .map(|conflict| format!(".refine/{}", conflict.path))
            .collect::<Vec<_>>();
        let report_id = conflict_report_id(merge_base, local_head, remote_head, unresolved);
        let worktrees =
            FileGitWorktreeService::with_runtime_root(&self.target_root, &self.runtime_root);
        let Some(lock) = try_claim_resolution(&worktrees, &report_id)? else {
            return Ok(Some(StateResolutionHold::Busy { report_id }));
        };
        // Everything the claim proves is unowned is finished work: retire it
        // now, while a lock is held anyway, so no workspace outlives the
        // divergence it belongs to.
        let _ = sweep_abandoned_resolutions(&worktrees, self, &self.target_root, &report_id);
        // A gated result an interrupted run left behind publishes without any
        // agent, so it is never charged.
        let publishable = surviving_resolution_result(self, state_root, &report_id)?.is_some();
        let contended = contended_records(unresolved);
        if !publishable && !contention_budget_available(self, state_root, remote_head, &contended)?
        {
            // Every contended record has spent its budget against this remote
            // head. Holding is not fencing: the pass still reports the
            // contention in domain terms, `--authority` and the daemon's
            // ownership policy still settle it in one command, and movement
            // on the side that needs deciding re-engages the resolver at once.
            self.retire_state_resolution(&report_id, Some(&lock));
            return Ok(None);
        }
        let pinned = pin_resolution_inputs(
            self,
            state_root,
            &report_id,
            merge_base,
            local_head,
            remote_head,
        )?;
        if let Some(commit) = &pinned.result {
            return Ok(Some(StateResolutionHold::Result {
                report_id,
                commit: commit.clone(),
            }));
        }
        let sides = conflict_sides(self, state_root, &pinned, &tree_paths)?;
        let conflicts = unresolved
            .iter()
            .map(|conflict| ConflictedPath {
                path: format!(".refine/{}", conflict.path),
                summary: conflict.summary.clone(),
            })
            .collect::<Vec<_>>();
        let workspace = materialize_conflicted_workspace(&worktrees, self, &pinned, &sides)?;
        let context = state_conflict_context(&state_conflict_block(&conflicts, &sides));
        // The attempt is charged here: everything above can fail the same way
        // on every pass, and a repeatable preparation failure that spent the
        // budget would hold a contention no agent has yet been handed. This is
        // still under lock Hold A and still before the unlocked agent call, so
        // a crash mid-resolution has already paid for the attempt it started.
        let _ = buy_contention_attempt(self, state_root, remote_head, &contended)?;
        Ok(Some(StateResolutionHold::Prepared(Box::new(
            PreparedStateResolution {
                resolution: PreparedResolution {
                    lock,
                    pinned,
                    workspace,
                    merged_tree: merged_tree.to_string(),
                    conflicts,
                    sides,
                    ancestry: format!("diverged from merge base {merge_base}"),
                    context,
                },
                phase,
                remote: remote.to_string(),
                merge_base: merge_base.to_string(),
                local_head: local_head.to_string(),
                remote_head: remote_head.to_string(),
                unresolved: unresolved.to_vec(),
            },
        ))))
    }
}
