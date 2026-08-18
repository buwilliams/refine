use super::*;

use crate::application::persistence_sync::goal_ownership::{GoalOwnership, GoalOwnershipPolicy};
use crate::application::persistence_sync::resolution::state::{
    LockAcquisition, StateResolutionHold, StateResolutionSlot,
};
use crate::application::persistence_sync::state_merge::{merge_added_state_file, merge_state_file};
use crate::infrastructure::git::ancestry::{Ancestry, classify};
use crate::infrastructure::git::merge::{
    TreeOperation, build_tree, commit_tree, empty_tree_id, ensure_supported_git, merge_commits,
    write_blob,
};

/// Durable marker that a branch-state adoption (live store lost or never
/// populated while the branch carries records) is underway. Hydration copies
/// records one at a time, so a crash mid-adoption leaves live carrying only
/// some of them; without the marker the rerun would mirror that partial live
/// back onto the branch as deletions of everything not yet copied.
const STATE_ADOPTION_REF: &str = "refs/refine/state-adoption";

/// How much of an adopted commit hydration copies into the live store.
#[derive(Clone, Debug)]
pub(crate) enum HydrationScope {
    /// Only paths the adoption changed relative to the previously observed
    /// local head; deletions propagate because the deleted path was observed.
    ChangedSince(String),
    /// Every durable path in the commit; nothing is deleted from live. Used
    /// when the local tree never matched live (bootstrap adoption, join).
    FullTree,
    /// Only commit paths the pass's live snapshot did not carry at all:
    /// strictly additive. Used when live is authoritative for everything it
    /// already holds (live-authority join).
    MissingOnly,
    /// Every durable path in the commit except `keep`; nothing is deleted
    /// from live. Contested paths decided live stay untouched via `keep`,
    /// and live-only paths — never contested on a join — remain in place to
    /// be published additively by the same pass. Reserved for authority
    /// decisions (remote-authority join with per-path live exceptions).
    FullSync { keep: BTreeSet<PathBuf> },
    /// Exactly the given commit paths, nothing else and no deletions.
    /// Reserved for authority decisions (remote-side path overrides).
    Only(BTreeSet<PathBuf>),
}

/// One synchronization pass's outcome, including what an attached recovery
/// decision did. Ordinary syncs always report empty `settled`/`retained`.
pub(crate) struct StateSyncPass {
    pub(crate) result: GitSyncResult,
    /// Contested paths the decision settled.
    pub(crate) settled: Vec<String>,
    /// Refs retained for displaced state not reachable as a merge parent.
    pub(crate) retained: Vec<String>,
    pub(crate) preserved_goal_owners: Vec<StateRecoveryGoalOwner>,
    /// The conflict-report id of an agent resolution this pass published as
    /// its merge commit; the entry retires its refs and clears the report.
    pub(crate) published_resolution: Option<String>,
}

impl StateSyncPass {
    fn clean(result: GitSyncResult) -> Self {
        Self {
            result,
            settled: Vec::new(),
            retained: Vec::new(),
            preserved_goal_owners: Vec::new(),
            published_resolution: None,
        }
    }
}

impl FileGitSyncService {
    pub fn new(target_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        let target_root = target_root.into();
        let runtime_root = runtime_root.into();
        Self {
            repository: crate::infrastructure::git::repository::FileGitRepository::new(
                &target_root,
                &runtime_root,
            ),
            target_root,
            runtime_root,
            agent_resolution: false,
        }
    }

    /// Allow this entry point to resolve merge conflicts with the installed
    /// agent (subject to the node's `state_sync_agent_resolution` setting).
    /// The daemon's sync worker and the explicit `refine sync` surfaces opt
    /// in; everything else keeps the fail-closed deterministic ladder.
    pub fn with_agent_resolution(mut self) -> Self {
        self.agent_resolution = true;
        self
    }

    /// Synchronize durable `.refine` state through the dedicated
    /// `refine/state` branch. The application branch, index, and worktree are
    /// never checked out, staged, pulled, or pushed by this service.
    pub fn sync(&self) -> RefineResult<GitSyncResult> {
        self.sync_with_attempt(StateSyncAttemptContext::direct())
    }

    pub fn sync_with_attempt(
        &self,
        attempt: StateSyncAttemptContext,
    ) -> RefineResult<GitSyncResult> {
        self.sync_resolving(GitFetchScope::All, &attempt, LockAcquisition::Blocking)
    }

    /// Attempt a best-effort background sync without delaying foreground work.
    pub fn try_sync(&self) -> RefineResult<GitSyncResult> {
        self.try_sync_with(GitFetchScope::All, &StateSyncAttemptContext::direct())
    }

    /// Publish local Refine state without turning state mutations into full
    /// application-branch fetches. The project update pulse owns that cadence.
    pub fn try_sync_state(&self) -> RefineResult<GitSyncResult> {
        self.try_sync_with(GitFetchScope::State, &StateSyncAttemptContext::direct())
    }

    pub fn try_sync_with_attempt(
        &self,
        fetch_all: bool,
        attempt: StateSyncAttemptContext,
    ) -> RefineResult<GitSyncResult> {
        self.try_sync_with(
            if fetch_all {
                GitFetchScope::All
            } else {
                GitFetchScope::State
            },
            &attempt,
        )
    }

    /// Remove state-worktree files that synchronized state no longer admits and
    /// report the ones Git still tracks, so the deletion is recorded as a change.
    ///
    /// Records published under an exclusion rule that has since widened would
    /// otherwise stay tracked forever: the copy that refreshes the state
    /// worktree enumerates through that same exclusion, so it never observes
    /// them to delete. Asking Git what it tracks is what lets a newly excluded
    /// path be retired from the branch.
    pub(crate) fn retire_excluded_tracked_state(
        &self,
        state_root: &std::path::Path,
        state_refine: &std::path::Path,
    ) -> RefineResult<Vec<PathBuf>> {
        let tracked_excluded = self
            .git_at_stdout(state_root, &["ls-files", "--", ".refine"])?
            .lines()
            .filter_map(|path| path.strip_prefix(".refine/"))
            .map(PathBuf::from)
            .filter(|path| is_excluded_from_durable_state(path))
            .collect::<BTreeSet<_>>();
        Ok(remove_excluded_state_files(state_refine)?
            .into_iter()
            .filter(|path| tracked_excluded.contains(path))
            .collect())
    }

    pub(crate) fn try_sync_with(
        &self,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
    ) -> RefineResult<GitSyncResult> {
        self.sync_resolving(fetch_scope, attempt, LockAcquisition::Try)
    }

    /// Fingerprint durable Refine state without touching the user's checkout.
    /// The daemon polls this several times a second to debounce nearby
    /// mutations, so it leans on the memoized content-hash map: unchanged
    /// records cost one stat each instead of a full content read.
    pub fn durable_state_fingerprint(&self) -> RefineResult<u64> {
        let root = prepare_refine_dir(&self.target_root)?;
        if !root.exists() {
            return Ok(0);
        }
        let state = durable_state_map(&root)?;
        let mut hasher = DefaultHasher::new();
        for (path, content_hash) in &state {
            path.hash(&mut hasher);
            content_hash.hash(&mut hasher);
        }
        Ok(hasher.finish())
    }

    pub(crate) fn sync_locked_pass(
        &self,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
        decision: Option<&StateRecoveryDecision>,
        automatic_recovery: bool,
        resolution_engaged: bool,
        resolution_slot: &mut Option<StateResolutionSlot>,
    ) -> RefineResult<StateSyncPass> {
        let result = self.sync_locked_inner(
            fetch_scope,
            attempt,
            decision,
            automatic_recovery,
            resolution_engaged,
            resolution_slot,
        );
        if result.is_err() {
            // Sync owns only the managed state worktree. Any failure must leave
            // that checkout clean so a later retry starts from durable Git
            // evidence instead of half-applied snapshot or index scratch work.
            let _ = self.restore_managed_state_worktree();
        }
        result
    }

    /// The deterministic ladder: snapshot live once as a commit, classify the
    /// commit pair, and only merge genuinely diverged heads from their real
    /// merge base. Live is hydrated before the local branch ever advances to
    /// content this node has not observed, so a crash at any point re-derives
    /// on the next pass without inventing deletions.
    ///
    /// With a `decision` attached the same pass is terminal recovery: every
    /// path the ladder cannot settle takes the decided side instead of
    /// failing closed, inside one merge commit with both heads as parents.
    fn sync_locked_inner(
        &self,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
        decision: Option<&StateRecoveryDecision>,
        automatic_recovery: bool,
        resolution_engaged: bool,
        resolution_slot: &mut Option<StateResolutionSlot>,
    ) -> RefineResult<StateSyncPass> {
        let mut settled = Vec::new();
        let mut retained = Vec::new();
        let mut preserved_goal_owners = Vec::new();
        let mut published_resolution = None;
        if !self.target_root.join(".git").exists() {
            return Ok(StateSyncPass::clean(skipped(
                "Target app is not a Git repository.",
            )));
        }
        if !self.git_success(&["rev-parse", "--is-inside-work-tree"])? {
            return Ok(StateSyncPass::clean(skipped(
                "Target app is not a Git worktree.",
            )));
        }
        // The merge IS `git merge-tree`, so an unsupported Git cannot produce a
        // pass at all. Failing here, typed, gives the operator the version
        // sentence instead of an unrecognized-option failure from somewhere
        // deep in the ladder. Cached, so this costs nothing per pass.
        ensure_supported_git()?;
        let live_refine = prepare_refine_dir(&self.target_root)?;
        self.ensure_local_state_excluded()?;
        self.retire_legacy_baseline()?;
        let remote = self.configured_remote(&live_refine)?;
        let remote_configured = self.remote_exists(&remote)?;
        let mut remote_branch_exists = if remote_configured {
            match fetch_scope {
                GitFetchScope::All => {
                    self.fetch_remote(&remote)?;
                    self.remote_state_tracking_exists(&remote)?
                }
                GitFetchScope::State => {
                    let exists = self.remote_state_exists(&remote)?;
                    if exists {
                        self.fetch_state_branch(&remote)?;
                    }
                    exists
                }
            }
        } else {
            false
        };

        let mut details = if remote_configured {
            Vec::new()
        } else {
            vec![format!(
                "Git remote {remote} is not configured; Refine state was committed locally."
            )]
        };

        // This pass's ONLY read of live durable state. Every later operand is
        // a commit; a record the daemon advances after this point is simply
        // the next pass's delta. (A decision-settled join re-reads once after
        // its own authorized hydration, below.)
        let mut live = durable_state_map(&live_refine)?;

        // Joining an existing fleet: the remote branch exists but this node
        // has no local branch yet, so nothing on the branch has been observed
        // by this live store. Hydrate BEFORE the branch is created — a crash
        // leaves the branch absent and the join re-derives — and fail closed
        // when live already contests remote content, because no merge base
        // exists to arbitrate a first contact.
        let local_branch_exists =
            self.git_success(&["show-ref", "--verify", "--quiet", REFINE_STATE_REF])?;
        let joining = remote_branch_exists && !local_branch_exists;
        if joining {
            let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
            let remote_head = self.git_stdout(&["rev-parse", &remote_ref])?;
            let mut join_hydrated = false;
            if !bootstrap_only_state(&live) {
                let contested = self.join_contested_paths(&remote_head, &live, &live_refine)?;
                if !contested.is_empty() {
                    if automatic_recovery {
                        let ownership = self.join_goal_ownership_policy(
                            &remote_head,
                            &live_refine,
                            &contested,
                        )?;
                        if let Some(question) = ownership.decision_question() {
                            let summary = self.record_conflict_report(
                                StateSyncConflictPhase::FirstPass,
                                attempt,
                                &remote,
                                "",
                                "",
                                &remote_head,
                                &contested,
                                Some(&question),
                            )?;
                            return Err(RefineError::Conflict(summary.to_string()));
                        }
                    }
                    let Some(decision) = decision else {
                        let summary = self.record_conflict_report(
                            StateSyncConflictPhase::FirstPass,
                            attempt,
                            &remote,
                            "",
                            "",
                            &remote_head,
                            &contested,
                            None,
                        )?;
                        return Err(RefineError::Conflict(summary.to_string()));
                    };
                    join_hydrated = true;
                    let live_advanced = self.settle_contested_join(
                        decision,
                        &remote_head,
                        &contested,
                        &live,
                        &live_refine,
                        &mut settled,
                        &mut retained,
                        &mut details,
                    )?;
                    if live_advanced {
                        details.push(LIVE_ADVANCED_DETAIL.to_string());
                    }
                    // The settle just rewrote live under this pass's own
                    // authority (including deletions); refresh the operand
                    // once so the snapshot mirrors what the decision left.
                    live = durable_state_map(&live_refine)?;
                }
            } else {
                details.push(
                    "Adopted synchronized Refine state over locally generated bootstrap metadata."
                        .to_string(),
                );
            }
            if !join_hydrated {
                let live_advanced = self.hydrate_live_from_commit(
                    &self.target_root,
                    HydrationScope::FullTree,
                    &remote_head,
                    &live,
                    &live_refine,
                )?;
                if live_advanced {
                    details.push(LIVE_ADVANCED_DETAIL.to_string());
                }
            }
        }

        let setup = self.ensure_state_worktree(&remote, remote_branch_exists, &live_refine)?;
        let state_root = setup.path;
        let state_refine = state_root.join(".refine");
        let recovered_interrupted_sync = self.recover_interrupted_state_worktree(&state_root)?;
        if recovered_interrupted_sync {
            details.push(
                "Recovered an interrupted Refine state copy before reconciling current live state."
                    .to_string(),
            );
        }

        let branch_state = durable_state_map(&state_refine)?;
        // Bootstrap-only live state against a branch that carries real
        // records is adoption, never mass deletion: the branch side wins and
        // hydrates the live store once the pass converges. A pending adoption
        // marker keeps that stance across a crash: hydrating even one record
        // makes live look non-bootstrap on rerun, and without the marker the
        // snapshot would commit the still-missing records as deletions.
        let adoption_pending =
            self.git_success(&["show-ref", "--verify", "--quiet", STATE_ADOPTION_REF])?;
        let adopt_branch_state = !joining
            && !bootstrap_only_state(&branch_state)
            && (bootstrap_only_state(&live) || adoption_pending);
        if adopt_branch_state {
            details.push(
                "Adopted synchronized Refine state over locally generated bootstrap metadata."
                    .to_string(),
            );
            let head = self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?;
            self.git_checked(&["update-ref", STATE_ADOPTION_REF, &head])?;
        } else if adoption_pending {
            // A join or an emptied branch superseded the marked adoption.
            self.clear_state_adoption_marker()?;
        }

        let mut committed = setup.created;
        let mut commit = if setup.created {
            Some(self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?)
        } else {
            None
        };
        if !adopt_branch_state {
            // Snapshot live onto the branch. While joining, branch-only paths
            // were hydrated into live above, so the mirror cannot invent
            // deletions; skipping deletions besides keeps a mid-join crash
            // recoverable.
            let mut changed =
                snapshot_live_state(&live_refine, &state_refine, &live, &branch_state, !joining)?;
            let removed_excluded =
                self.retire_excluded_tracked_state(&state_root, &state_refine)?;
            changed.extend(
                removed_excluded.into_iter().map(|path| {
                    format!("D  .refine/{}", path.to_string_lossy().replace('\\', "/"))
                }),
            );
            if !changed.is_empty()
                && self.commit_staged_snapshot(
                    &state_root,
                    &format!("Node: {}", self.active_node_id(&live_refine)),
                )?
            {
                committed = true;
                commit = Some(self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?);
            }
        }

        let mut pulled = setup.pulled || joining;
        let mut pushed = false;
        let mut live_advanced = false;

        if !remote_configured {
            if adopt_branch_state {
                let head = self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?;
                live_advanced |= self.hydrate_live_from_commit(
                    &state_root,
                    HydrationScope::FullTree,
                    &head,
                    &live,
                    &live_refine,
                )?;
                self.clear_state_adoption_marker()?;
            }
            if live_advanced {
                details.push(LIVE_ADVANCED_DETAIL.to_string());
            }
            return Ok(StateSyncPass::clean(GitSyncResult {
                ok: true,
                attempted: true,
                committed,
                pulled,
                pushed,
                branch: Some(REFINE_STATE_BRANCH.to_string()),
                commit,
                detail: nonempty_detail(details),
                remote_configured: Some(false),
                deferred: live_advanced,
            }));
        }

        let mut push_attempt = 0usize;
        loop {
            push_attempt += 1;
            let local_head = self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?;
            if !remote_branch_exists {
                // Bootstrap publish: no remote branch to classify against.
                let push =
                    self.git_at(&state_root, &["push", "-u", &remote, REFINE_STATE_BRANCH])?;
                append_output_detail(&mut details, &push);
                if push.success {
                    pushed = true;
                    break;
                }
                if push_attempt >= PUSH_RETRY_LIMIT || !push_rejected_by_race(&push) {
                    return Err(command_failed("git push", &push));
                }
                // Another node published the branch first; classify against
                // the fresh remote head on the next attempt.
                self.fetch_state_branch(&remote)?;
                remote_branch_exists = self.remote_state_tracking_exists(&remote)?;
                thread::sleep(PUSH_RETRY_DELAY);
                continue;
            }

            let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
            let remote_head = self.git_stdout(&["rev-parse", &remote_ref])?;
            match classify(self, &local_head, &remote_head)? {
                Ancestry::Equal => {
                    if adopt_branch_state {
                        live_advanced |= self.hydrate_live_from_commit(
                            &state_root,
                            HydrationScope::FullTree,
                            &local_head,
                            &live,
                            &live_refine,
                        )?;
                    }
                    break;
                }
                Ancestry::FastForwardToA => {
                    // The remote head is a strict ancestor of local work:
                    // publish, never merge.
                    let push =
                        self.git_at(&state_root, &["push", "-u", &remote, REFINE_STATE_BRANCH])?;
                    append_output_detail(&mut details, &push);
                    if push.success {
                        pushed = true;
                        if adopt_branch_state {
                            live_advanced |= self.hydrate_live_from_commit(
                                &state_root,
                                HydrationScope::FullTree,
                                &local_head,
                                &live,
                                &live_refine,
                            )?;
                        }
                        break;
                    }
                    if push_attempt >= PUSH_RETRY_LIMIT || !push_rejected_by_race(&push) {
                        return Err(command_failed("git push", &push));
                    }
                    self.fetch_state_branch(&remote)?;
                    thread::sleep(PUSH_RETRY_DELAY);
                }
                Ancestry::FastForwardToB => {
                    // Hydrate live before the branch advances, so the branch
                    // head never claims records this node has not observed; a
                    // crash between the two steps reruns idempotently.
                    live_advanced |= self.hydrate_live_from_commit(
                        &state_root,
                        if adopt_branch_state {
                            HydrationScope::FullTree
                        } else {
                            HydrationScope::ChangedSince(local_head.clone())
                        },
                        &remote_head,
                        &live,
                        &live_refine,
                    )?;
                    self.git_at_checked(&state_root, &["reset", "--hard", &remote_head])?;
                    pulled = true;
                    break;
                }
                ancestry @ (Ancestry::Diverged { .. } | Ancestry::Unrelated) => {
                    let merge_base = match ancestry {
                        Ancestry::Diverged { merge_base } => merge_base,
                        // Independently bootstrapped orphan histories share no
                        // root: merge from the empty tree and join them with a
                        // two-parent commit instead of wedging forever.
                        _ => empty_tree_id(self, &state_root)?,
                    };
                    // Settled paths from an attempt that loses its push are
                    // re-derived by the retry; only the published attempt's
                    // settlements count once. The same holds for a published
                    // agent resolution.
                    let mut attempt_settled = Vec::new();
                    let mut attempt_preserved_goal_owners = Vec::new();
                    let mut attempt_resolution = None;
                    let merge_commit = self.merge_diverged_state(
                        &state_root,
                        attempt,
                        &remote,
                        &merge_base,
                        &local_head,
                        &remote_head,
                        push_attempt,
                        decision,
                        automatic_recovery,
                        resolution_engaged,
                        resolution_slot,
                        &mut attempt_resolution,
                        &mut attempt_settled,
                        &mut attempt_preserved_goal_owners,
                        &mut details,
                    )?;
                    // The merge commit descends from the fetched remote head,
                    // so a plain refspec push is a fast-forward there and a
                    // rejection can only mean the remote moved (a race). A
                    // decision publishes with force-with-lease against the
                    // head it merged, which is the same guarantee spelled out.
                    let lease = format!("--force-with-lease={REFINE_STATE_REF}:{remote_head}");
                    let refspec = format!("{merge_commit}:{REFINE_STATE_REF}");
                    let push = if decision.is_some() {
                        self.git_at(&state_root, &["push", &lease, &remote, &refspec])?
                    } else {
                        self.git_at(&state_root, &["push", &remote, &refspec])?
                    };
                    append_output_detail(&mut details, &push);
                    if push.success {
                        settled.append(&mut attempt_settled);
                        preserved_goal_owners.append(&mut attempt_preserved_goal_owners);
                        published_resolution = attempt_resolution.take();
                        live_advanced |= self.hydrate_live_from_commit(
                            &state_root,
                            if adopt_branch_state {
                                HydrationScope::FullTree
                            } else {
                                HydrationScope::ChangedSince(local_head.clone())
                            },
                            &merge_commit,
                            &live,
                            &live_refine,
                        )?;
                        self.git_at_checked(&state_root, &["reset", "--hard", &merge_commit])?;
                        pushed = true;
                        pulled = true;
                        committed = true;
                        commit = Some(merge_commit);
                        break;
                    }
                    // The abandoned merge commit was never a ref; its parents
                    // stay reachable, so nothing needs retention here. The
                    // retry re-merges from the fresh remote head with the same
                    // committed local operand — live is never re-read.
                    if push_attempt >= PUSH_RETRY_LIMIT || !push_rejected_by_race(&push) {
                        return Err(command_failed("git push", &push));
                    }
                    self.fetch_state_branch(&remote)?;
                    thread::sleep(PUSH_RETRY_DELAY);
                }
            }
        }

        if adopt_branch_state {
            // The pass converged with every adopted record hydrated (records
            // the daemon advanced mid-pass are its newer state, not losses).
            self.clear_state_adoption_marker()?;
        }
        if live_advanced {
            details.push(LIVE_ADVANCED_DETAIL.to_string());
        }
        Ok(StateSyncPass {
            result: GitSyncResult {
                ok: true,
                attempted: true,
                committed,
                pulled,
                pushed,
                branch: Some(REFINE_STATE_BRANCH.to_string()),
                commit,
                detail: nonempty_detail(details),
                remote_configured: Some(true),
                deferred: live_advanced,
            },
            settled,
            retained,
            preserved_goal_owners,
            published_resolution,
        })
    }

    fn clear_state_adoption_marker(&self) -> RefineResult<()> {
        if self.git_success(&["show-ref", "--verify", "--quiet", STATE_ADOPTION_REF])? {
            self.git_checked(&["update-ref", "-d", STATE_ADOPTION_REF])?;
        }
        Ok(())
    }

    /// Paths where the joining node's live store and the remote branch both
    /// carry different content: a first contact has no merge base, so these
    /// fail closed for an operator decision.
    fn join_contested_paths(
        &self,
        remote_head: &str,
        live: &DurableStateMap,
        live_refine: &std::path::Path,
    ) -> RefineResult<Vec<StateSyncConflictPath>> {
        let listing =
            self.git_stdout(&["ls-tree", "-r", "--name-only", remote_head, "--", ".refine"])?;
        let mut contested = Vec::new();
        for path in listing.lines() {
            let Some(relative) = path.strip_prefix(".refine/") else {
                continue;
            };
            let relative_path = PathBuf::from(relative);
            if is_excluded_from_durable_state(&relative_path) {
                continue;
            }
            let Some(live_fingerprint) = live.get(&relative_path) else {
                continue;
            };
            let remote_bytes = self.state_bytes_at(&self.target_root, remote_head, path)?;
            let remote_fingerprint = remote_bytes.as_deref().map(state_content_fingerprint);
            if remote_fingerprint == Some(*live_fingerprint) {
                continue;
            }
            let live_bytes = fs::read(live_refine.join(&relative_path)).ok();
            contested.push(StateSyncConflictPath {
                path: relative.to_string(),
                summary: conflict_path_summary(
                    &relative_path,
                    None,
                    live_bytes.as_deref(),
                    remote_bytes.as_deref(),
                ),
            });
        }
        Ok(contested)
    }

    /// Merge two genuinely diverged state heads from their real merge base.
    /// Cleanly merging paths are accepted from Git's own tree merge;
    /// textually conflicted state files go to the structural driver; anything
    /// still contested fails closed with a conflict report — unless a
    /// recovery decision is attached, in which case each remaining path takes
    /// its decided side and no conflict can survive. With agent resolution
    /// engaged, the fail-closed path first performs lock Hold A (pin the
    /// operands, materialize the conflicted workspace) so the entry can
    /// resolve UNLOCKED, and a gated result surviving from an earlier run of
    /// this exact divergence is published directly as the merge commit.
    #[allow(clippy::too_many_arguments)]
    fn merge_diverged_state(
        &self,
        state_root: &std::path::Path,
        attempt: &StateSyncAttemptContext,
        remote: &str,
        merge_base: &str,
        local_head: &str,
        remote_head: &str,
        push_attempt: usize,
        decision: Option<&StateRecoveryDecision>,
        automatic_recovery: bool,
        resolution_engaged: bool,
        resolution_slot: &mut Option<StateResolutionSlot>,
        published_resolution: &mut Option<String>,
        settled: &mut Vec<String>,
        preserved_goal_owners: &mut Vec<StateRecoveryGoalOwner>,
        details: &mut Vec<String>,
    ) -> RefineResult<String> {
        let (mut tree, resolved, mut unresolved) =
            self.merge_diverged_trees(state_root, merge_base, local_head, remote_head, &[])?;
        // The conflict-report id hashes the sorted unresolved paths; keep the
        // resolution id, the report, and the prompt in that one stable order.
        unresolved.sort_by(|left, right| left.path.cmp(&right.path));
        let ownership = self.goal_ownership_policy(
            state_root,
            merge_base,
            local_head,
            remote_head,
            &unresolved,
        )?;
        let phase = if push_attempt > 1 {
            StateSyncConflictPhase::PushRetry
        } else {
            StateSyncConflictPhase::FirstPass
        };
        if (automatic_recovery || decision.is_none())
            && let Some(question) = ownership.decision_question()
        {
            let summary = self.record_conflict_report(
                phase,
                attempt,
                remote,
                merge_base,
                local_head,
                remote_head,
                &unresolved,
                Some(&question),
            )?;
            return Err(RefineError::Conflict(summary.to_string()));
        }
        let mut parents_message = None;
        if !unresolved.is_empty() {
            let Some(decision) = decision else {
                if resolution_engaged {
                    match self.prepare_state_resolution(
                        state_root,
                        phase,
                        remote,
                        merge_base,
                        local_head,
                        remote_head,
                        &tree,
                        &unresolved,
                    )? {
                        Some(StateResolutionHold::Result { report_id, commit }) => {
                            details.push(format!(
                                "Published the agent-resolved merge for state conflict {report_id}."
                            ));
                            *published_resolution = Some(report_id);
                            return Ok(commit);
                        }
                        Some(StateResolutionHold::Prepared(prepared)) => {
                            *resolution_slot = Some(StateResolutionSlot::Prepared(prepared));
                        }
                        Some(StateResolutionHold::NeedsDecision { question }) => {
                            let summary = self.record_conflict_report(
                                phase,
                                attempt,
                                remote,
                                merge_base,
                                local_head,
                                remote_head,
                                &unresolved,
                                Some(&question),
                            )?;
                            return Err(RefineError::Conflict(summary.to_string()));
                        }
                        Some(StateResolutionHold::Busy { report_id }) => {
                            // Reporting this divergence would race the
                            // operation already resolving it: defer instead,
                            // leaving its report — and its question — alone.
                            *resolution_slot = Some(StateResolutionSlot::Busy);
                            return Err(RefineError::Conflict(format!(
                                "Another operation is resolving state conflict {report_id}."
                            )));
                        }
                        None => {}
                    }
                }
                // An escalation already on file for this contention keeps its
                // question: a later pass that re-reports the same contested
                // records against the same remote head must not replace the
                // agent's words with the generic headline — not even when
                // this node's own live writes have moved the local head since.
                let escalated = self.escalated_decision_question(remote_head, &unresolved);
                let summary = self.record_conflict_report(
                    phase,
                    attempt,
                    remote,
                    merge_base,
                    local_head,
                    remote_head,
                    &unresolved,
                    escalated.as_deref(),
                )?;
                return Err(RefineError::Conflict(summary.to_string()));
            };
            let authority_commit = |authority: StateRecoveryAuthority| match authority {
                StateRecoveryAuthority::Live => local_head.to_string(),
                StateRecoveryAuthority::Remote => remote_head.to_string(),
            };
            // Conflict paths inside `.refine/` are recorded stripped; anything
            // else keeps its full tree path. Offer both spellings so the
            // forced re-merge settles either kind.
            let forced = unresolved
                .iter()
                .flat_map(|conflict| {
                    let source = authority_commit(decision.authority_for(&conflict.path));
                    [
                        (format!(".refine/{}", conflict.path), source.clone()),
                        (conflict.path.clone(), source),
                    ]
                })
                .collect::<Vec<_>>();
            let (forced_tree, _, still_unresolved) = self.merge_diverged_trees(
                state_root,
                merge_base,
                local_head,
                remote_head,
                &forced,
            )?;
            if !still_unresolved.is_empty() {
                return Err(RefineError::Conflict(format!(
                    "State recovery could not settle {} path(s) even with authority; this is a bug.",
                    still_unresolved.len()
                )));
            }
            tree = self.overlay_goal_ownership(
                state_root,
                &forced_tree,
                &ownership,
                preserved_goal_owners,
                details,
            )?;
            settled.extend(unresolved.iter().map(|conflict| conflict.path.clone()));
            details.push(format!(
                "Recovery settled contested state with {} authority: {}",
                decision.default_authority.as_str(),
                unresolved
                    .iter()
                    .map(|conflict| conflict.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            parents_message = Some(format!(
                "Recover Refine state with {} authority",
                decision.default_authority.as_str()
            ));
        }
        if !resolved.is_empty() {
            details.push(format!(
                "Merged divergent state records structurally: {}",
                resolved.join(", ")
            ));
        }
        let live_refine = prepare_refine_dir(&self.target_root)?;
        let message = format!(
            "{}\n\nNode: {}",
            parents_message.unwrap_or_else(|| "Merge Refine state".to_string()),
            self.active_node_id(&live_refine)
        );
        commit_tree(
            self,
            state_root,
            &tree,
            &[local_head, remote_head],
            &message,
        )
    }

    fn goal_ownership_policy(
        &self,
        state_root: &std::path::Path,
        merge_base: &str,
        local_head: &str,
        remote_head: &str,
        unresolved: &[StateSyncConflictPath],
    ) -> RefineResult<GoalOwnershipPolicy> {
        let mut ownership = GoalOwnershipPolicy::default();
        for conflict in unresolved {
            let path = format!(".refine/{}", conflict.path);
            let base = self.state_bytes_at(state_root, merge_base, &path)?;
            let local = self.state_bytes_at(state_root, local_head, &path)?;
            let remote = self.state_bytes_at(state_root, remote_head, &path)?;
            ownership.include(&path, base.as_deref(), local.as_deref(), remote.as_deref());
        }
        Ok(ownership)
    }

    fn join_goal_ownership_policy(
        &self,
        remote_head: &str,
        live_refine: &std::path::Path,
        contested: &[StateSyncConflictPath],
    ) -> RefineResult<GoalOwnershipPolicy> {
        let mut ownership = GoalOwnershipPolicy::default();
        for conflict in contested {
            let path = format!(".refine/{}", conflict.path);
            let local = match fs::read(live_refine.join(&conflict.path)) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to read live Goal ownership operand {}: {error}",
                        live_refine.join(&conflict.path).display()
                    )));
                }
            };
            let remote = self.state_bytes_at(&self.target_root, remote_head, &path)?;
            ownership.include(&path, None, local.as_deref(), remote.as_deref());
        }
        Ok(ownership)
    }

    /// Overlay the independently proven owner onto a whole-record recovery
    /// choice. This preserves the existing live/remote authority decision for
    /// every non-ownership member without letting that coarse choice reverse
    /// an explicit transfer.
    fn overlay_goal_ownership(
        &self,
        state_root: &std::path::Path,
        tree: &str,
        ownership: &GoalOwnershipPolicy,
        preserved_goal_owners: &mut Vec<StateRecoveryGoalOwner>,
        details: &mut Vec<String>,
    ) -> RefineResult<String> {
        let mut operations = Vec::new();
        let mut transferred = Vec::new();
        for (path, decision) in ownership.iter() {
            let GoalOwnership::Preserve {
                node_id,
                transferred: was_transferred,
            } = decision
            else {
                // Explicit operator recovery is allowed to settle ambiguity
                // by selecting one complete side. Automatic recovery returned
                // before this point.
                continue;
            };
            let bytes = self.state_bytes_at(state_root, tree, path)?;
            let Some(bytes) = bytes else {
                return Err(RefineError::Conflict(format!(
                    "State recovery cannot delete {path} while preserving explicit Goal owner {node_id}"
                )));
            };
            let mut record =
                serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
                    RefineError::Conflict(format!(
                        "State recovery selected invalid Goal JSON at {path}: {error}"
                    ))
                })?;
            let object = record.as_object_mut().ok_or_else(|| {
                RefineError::Conflict(format!(
                    "State recovery selected a non-object Goal record at {path}"
                ))
            })?;
            if *was_transferred {
                transferred.push(format!("{path} -> {node_id}"));
                preserved_goal_owners.push(StateRecoveryGoalOwner {
                    path: path.strip_prefix(".refine/").unwrap_or(path).to_string(),
                    node_id: node_id.clone(),
                });
            }
            if object.get("node_id").and_then(serde_json::Value::as_str) == Some(node_id.as_str()) {
                continue;
            }
            object.insert(
                "node_id".to_string(),
                serde_json::Value::String(node_id.clone()),
            );
            let mut encoded = serde_json::to_vec_pretty(&record).map_err(|error| {
                RefineError::Conflict(format!(
                    "State recovery could not encode Goal ownership at {path}: {error}"
                ))
            })?;
            encoded.push(b'\n');
            operations.push(TreeOperation::set(
                path.to_string(),
                write_blob(self, state_root, &encoded)?,
            ));
        }
        if !transferred.is_empty() {
            details.push(format!(
                "Preserved explicit Goal transfer(s): {}",
                transferred.join(", ")
            ));
        }
        if operations.is_empty() {
            Ok(tree.to_string())
        } else {
            build_tree(self, state_root, tree, &operations)
        }
    }

    /// Settle a contested first contact with an authority decision. There is
    /// no merge base and the live store was never a commit, so the losing
    /// live content is retained by ref before anything overwrites it; the
    /// remaining join (branch creation, additive snapshot, publish) proceeds
    /// as the ordinary pipeline.
    #[allow(clippy::too_many_arguments)]
    fn settle_contested_join(
        &self,
        decision: &StateRecoveryDecision,
        remote_head: &str,
        contested: &[StateSyncConflictPath],
        live: &DurableStateMap,
        live_refine: &std::path::Path,
        settled: &mut Vec<String>,
        retained: &mut Vec<String>,
        details: &mut Vec<String>,
    ) -> RefineResult<bool> {
        let mut live_kept = BTreeSet::new();
        let mut remote_taken = BTreeSet::new();
        for conflict in contested {
            match decision.authority_for(&conflict.path) {
                StateRecoveryAuthority::Live => {
                    live_kept.insert(PathBuf::from(&conflict.path));
                }
                StateRecoveryAuthority::Remote => {
                    remote_taken.insert(PathBuf::from(&conflict.path));
                }
            }
            settled.push(conflict.path.clone());
        }
        let live_advanced = match decision.default_authority {
            StateRecoveryAuthority::Remote => {
                // Contested paths take the remote branch except explicitly
                // kept ones; uncontested live-only records stay in place and
                // publish additively with the rest of the join. The whole
                // displaced live store stays findable by ref.
                let retained_ref = self.retain_live_snapshot(live_refine, remote_head)?;
                retained.push(retained_ref);
                self.hydrate_live_from_commit(
                    &self.target_root,
                    HydrationScope::FullSync { keep: live_kept },
                    remote_head,
                    live,
                    live_refine,
                )?
            }
            StateRecoveryAuthority::Live => {
                // Live keeps everything it holds; remote-only records and
                // explicitly remote-decided paths hydrate in. The remote head
                // becomes a parent of the join snapshot, so nothing needs a
                // retained ref.
                let mut advanced = self.hydrate_live_from_commit(
                    &self.target_root,
                    HydrationScope::MissingOnly,
                    remote_head,
                    live,
                    live_refine,
                )?;
                if !remote_taken.is_empty() {
                    advanced |= self.hydrate_live_from_commit(
                        &self.target_root,
                        HydrationScope::Only(remote_taken),
                        remote_head,
                        live,
                        live_refine,
                    )?;
                }
                advanced
            }
        };
        details.push(format!(
            "Recovery settled a contested first contact with {} authority: {}",
            decision.default_authority.as_str(),
            settled.join(", ")
        ));
        Ok(live_advanced)
    }

    /// Run the tree merge plus the structural driver and return the merged
    /// tree, the driver-resolved state paths, and the still-conflicted paths
    /// with domain summaries. `forced` paths take the given side's blob
    /// instead of conflicting (operator authority).
    pub(crate) fn merge_diverged_trees(
        &self,
        state_root: &std::path::Path,
        merge_base: &str,
        local_head: &str,
        remote_head: &str,
        forced: &[(String, String)],
    ) -> RefineResult<(String, Vec<String>, Vec<StateSyncConflictPath>)> {
        // Git finds the merge base itself; `merge_base` stays the driver's
        // reading base below, where the structural merge needs the bytes.
        let merged = merge_commits(self, state_root, local_head, remote_head)?;
        let forced = forced.iter().cloned().collect::<BTreeMap<_, _>>();
        let mut operations = Vec::new();
        let mut resolved = Vec::new();
        let mut unresolved = Vec::new();
        for path in &merged.conflicted_paths {
            if let Some(source) = forced.get(path) {
                match self.state_bytes_at(state_root, source, path)? {
                    Some(bytes) => {
                        let blob = write_blob(self, state_root, &bytes)?;
                        operations.push(TreeOperation::set(path.clone(), blob));
                    }
                    None => operations.push(TreeOperation::Remove { path: path.clone() }),
                }
                continue;
            }
            let Some(relative) = path.strip_prefix(".refine/") else {
                unresolved.push(StateSyncConflictPath {
                    path: path.clone(),
                    summary: OUTSIDE_STATE_CONFLICT_SUMMARY.to_string(),
                });
                continue;
            };
            let relative_path = PathBuf::from(relative);
            if is_excluded_from_durable_state(&relative_path) {
                // Excluded paths never belong on the branch; retire them
                // rather than contest them.
                operations.push(TreeOperation::Remove { path: path.clone() });
                continue;
            }
            let base_bytes = self.state_bytes_at(state_root, merge_base, path)?;
            let local_bytes = self.state_bytes_at(state_root, local_head, path)?;
            let remote_bytes = self.state_bytes_at(state_root, remote_head, path)?;
            let driver_merged = match (&base_bytes, &local_bytes, &remote_bytes) {
                (Some(base), Some(local), Some(remote)) => {
                    merge_state_file(&relative_path, base, local, remote)
                }
                // Added independently on both sides (an empty-tree merge base
                // joining unrelated bootstraps): only files with an
                // identity-keyed empty form merge.
                (None, Some(local), Some(remote)) => {
                    merge_added_state_file(&relative_path, local, remote)
                }
                _ => None,
            };
            match driver_merged {
                Some(bytes) => {
                    let blob = write_blob(self, state_root, &bytes)?;
                    operations.push(TreeOperation::set(path.clone(), blob));
                    resolved.push(relative.to_string());
                }
                None => unresolved.push(StateSyncConflictPath {
                    path: relative.to_string(),
                    summary: conflict_path_summary(
                        &relative_path,
                        base_bytes.as_deref(),
                        local_bytes.as_deref(),
                        remote_bytes.as_deref(),
                    ),
                }),
            }
        }
        let tree = if operations.is_empty() {
            merged.tree
        } else {
            build_tree(self, state_root, &merged.tree, &operations)?
        };
        Ok((tree, resolved, unresolved))
    }

    pub(crate) fn state_bytes_at(
        &self,
        root: &std::path::Path,
        commitish: &str,
        path: &str,
    ) -> RefineResult<Option<Vec<u8>>> {
        let output = self.git_at(root, &["show", &format!("{commitish}:{path}")])?;
        Ok(output.success.then_some(output.stdout))
    }

    /// The baseline system is gone: `git merge-base` is the baseline, shared
    /// and durable by construction. The first sync of an upgraded node
    /// retires the legacy fingerprint file and its retained anchor refs.
    fn retire_legacy_baseline(&self) -> RefineResult<()> {
        let path = git_common_dir(&self.target_root)?.join("refine-state-baseline.json");
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to retire legacy Refine state baseline {}: {error}",
                    path.display()
                ))
            })?;
        }
        let anchors = self.git_stdout(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/refine/state-baseline",
        ])?;
        for anchor in anchors.lines() {
            let _ = self.git(&["update-ref", "-d", anchor]);
        }
        Ok(())
    }

    /// Stage the snapshotted `.refine` tree and commit it when anything is
    /// actually staged. Snapshot copies can settle to bytes identical to the
    /// branch (for example after this pass's own hydration), so the commit
    /// decision reads Git's staged status, and the summary is derived from it.
    pub(crate) fn commit_staged_snapshot(
        &self,
        state_root: &std::path::Path,
        trailer: &str,
    ) -> RefineResult<bool> {
        self.git_at_checked(state_root, &["add", "-f", "-A", "--", ".refine"])?;
        let status = self.git_at_stdout(
            state_root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--",
                ".refine",
            ],
        )?;
        if status.is_empty() {
            return Ok(false);
        }
        let summary = state_commit_summary(&status);
        self.git_at_checked(state_root, &["commit", "-m", &summary, "-m", trailer])?;
        Ok(true)
    }

    pub(crate) fn active_node_id(&self, live_refine: &std::path::Path) -> String {
        FileNodeRegistryService::with_active_root(live_refine, &self.runtime_root)
            .active_node_id()
            .unwrap_or_else(|_| "default".to_string())
    }

    /// Copy records an adopted commit carries into the live store under
    /// per-record leases with preimage compare-and-swap: a record the daemon
    /// advanced after the pass's snapshot is left alone and becomes the next
    /// pass's delta. Bootstrap-class metadata hydrates first so an
    /// interrupted adoption still looks like an adoption on rerun.
    pub(crate) fn hydrate_live_from_commit(
        &self,
        git_root: &std::path::Path,
        scope: HydrationScope,
        commit: &str,
        original: &DurableStateMap,
        live_root: &std::path::Path,
    ) -> RefineResult<bool> {
        let candidates: BTreeSet<PathBuf> = match &scope {
            HydrationScope::ChangedSince(base) => self
                .git_at_stdout(
                    git_root,
                    &["diff", "--name-only", base, commit, "--", ".refine"],
                )?
                .lines()
                .filter_map(|path| path.strip_prefix(".refine/"))
                .map(PathBuf::from)
                .collect(),
            HydrationScope::Only(paths) => paths.clone(),
            HydrationScope::FullTree
            | HydrationScope::FullSync { .. }
            | HydrationScope::MissingOnly => {
                let mut paths = self
                    .git_at_stdout(
                        git_root,
                        &["ls-tree", "-r", "--name-only", commit, "--", ".refine"],
                    )?
                    .lines()
                    .filter_map(|path| path.strip_prefix(".refine/"))
                    .map(PathBuf::from)
                    .collect::<BTreeSet<_>>();
                match &scope {
                    HydrationScope::FullSync { keep } => {
                        paths.retain(|path| !keep.contains(path));
                    }
                    HydrationScope::MissingOnly => {
                        paths.retain(|path| !original.contains_key(path));
                    }
                    _ => {}
                }
                paths
            }
        };
        let mut ordered = candidates
            .into_iter()
            .filter(|relative| !is_excluded_from_durable_state(relative))
            .collect::<Vec<_>>();
        ordered.sort_by_key(|relative| (!is_bootstrap_state_path(relative), relative.clone()));
        let mut concurrent_change = false;
        for relative in ordered {
            let target_bytes = self.state_bytes_at(
                git_root,
                commit,
                &format!(".refine/{}", relative.to_string_lossy().replace('\\', "/")),
            )?;
            let desired = target_bytes.as_deref().map(state_content_fingerprint);
            if desired.is_none()
                && matches!(
                    scope,
                    HydrationScope::FullTree
                        | HydrationScope::FullSync { .. }
                        | HydrationScope::MissingOnly
                        | HydrationScope::Only(_)
                )
            {
                continue;
            }
            let destination = live_root.join(&relative);
            let key = record_lock_key(&destination);
            with_record_lock(live_root, &key, || {
                let current = live_path_fingerprint(&destination)?;
                let before = original.get(&relative).copied();
                if current == desired {
                    reconcile_hydrated_index(live_root, &relative, &destination, desired);
                    return Ok(());
                }
                if current != before {
                    concurrent_change = true;
                    return Ok(());
                }
                if let Some(bytes) = &target_bytes {
                    replace_file_durably(&destination, bytes)?;
                } else if destination.exists() {
                    fs::remove_file(&destination).map_err(|error| {
                        RefineError::Io(format!(
                            "failed to hydrate synchronized deletion {}: {error}",
                            destination.display()
                        ))
                    })?;
                    if let Some(parent) = destination.parent() {
                        File::open(parent)
                            .and_then(|directory| directory.sync_all())
                            .map_err(|error| {
                                RefineError::Io(format!(
                                    "failed to durably commit synchronized deletion {}: {error}",
                                    destination.display()
                                ))
                            })?;
                    }
                }
                reconcile_hydrated_index(live_root, &relative, &destination, desired);
                Ok(())
            })?;
        }
        Ok(concurrent_change)
    }
}

pub(crate) const LIVE_ADVANCED_DETAIL: &str = "A newer local state mutation arrived during synchronization; it was preserved and will be published in the next batch.";

/// The summary recorded for a conflicted path that is not synchronized Refine
/// state. Agent resolution never judges these; they always fail closed.
pub(crate) const OUTSIDE_STATE_CONFLICT_SUMMARY: &str =
    "path outside synchronized Refine state changed on both nodes";

/// Make the state worktree's durable set mirror the live snapshot and report
/// the change lines. Only differing records are rewritten, so an idle pass
/// touches nothing. With `include_deletions` false, branch-only paths are
/// left in place (join passes, where absence means unobserved, not deleted).
pub(crate) fn snapshot_live_state(
    live_refine: &std::path::Path,
    state_refine: &std::path::Path,
    live: &DurableStateMap,
    branch_state: &DurableStateMap,
    include_deletions: bool,
) -> RefineResult<Vec<String>> {
    let mut changed = Vec::new();
    let paths = live
        .keys()
        .chain(branch_state.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relative in paths {
        let status = match (branch_state.get(&relative), live.get(&relative)) {
            (None, Some(_)) => "A",
            (Some(_), None) if include_deletions => "D",
            (Some(_), None) => continue,
            (Some(before), Some(after)) if before != after => "M",
            _ => continue,
        };
        let destination = state_refine.join(&relative);
        if live.contains_key(&relative) {
            copy_state_file(&live_refine.join(&relative), &destination)?;
        } else if destination.exists() {
            fs::remove_file(&destination).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove synchronized Refine state {}: {error}",
                    destination.display()
                ))
            })?;
        }
        changed.push(format!(
            "{status}  .refine/{}",
            relative.to_string_lossy().replace('\\', "/")
        ));
    }
    Ok(changed)
}

fn reconcile_hydrated_index(
    live_root: &std::path::Path,
    relative: &std::path::Path,
    destination: &std::path::Path,
    desired: Option<u64>,
) {
    if desired.is_some() {
        record_synchronized_goal(live_root, relative, destination);
    } else {
        forget_synchronized_goal(live_root, relative);
    }
}

fn live_path_fingerprint(path: &std::path::Path) -> RefineResult<Option<u64>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(state_content_fingerprint(&bytes))),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RefineError::Io(format!(
            "failed to inspect live Refine state {}: {error}",
            path.display()
        ))),
    }
}
