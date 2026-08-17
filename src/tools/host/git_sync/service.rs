use super::*;

impl FileGitSyncService {
    pub fn new(target_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            target_root: target_root.into(),
            runtime_root: runtime_root.into(),
        }
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
        with_repository_git_lock(&self.target_root, || {
            self.sync_locked(GitFetchScope::All, &attempt)
        })
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
    pub(super) fn retire_excluded_tracked_state(
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

    pub(super) fn try_sync_with(
        &self,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
    ) -> RefineResult<GitSyncResult> {
        let lock = repository_git_lock(&self.target_root)?;
        let _guard = match lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Ok(deferred(
                    "Repository Git operations are busy; sync will retry on the next cadence.",
                ));
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(RefineError::Conflict(
                    "Repository Git lock was poisoned".to_string(),
                ));
            }
        };
        let _file_guard = match RepositoryFileLock::try_acquire(&self.target_root)? {
            Some(guard) => guard,
            None => {
                return Ok(deferred(
                    "Repository Git operations are busy; sync will retry on the next cadence.",
                ));
            }
        };
        self.sync_locked(fetch_scope, attempt)
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

    pub(super) fn sync_locked(
        &self,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
    ) -> RefineResult<GitSyncResult> {
        let result = self.sync_locked_inner(fetch_scope, attempt);
        if result.is_err() {
            // Sync owns only the managed state worktree. Any failure must leave
            // that checkout clean so a later retry starts from durable Git
            // evidence instead of half-applied retirement or reconciliation.
            let _ = self.restore_managed_state_worktree();
        }
        result
    }

    fn sync_locked_inner(
        &self,
        fetch_scope: GitFetchScope,
        attempt: &StateSyncAttemptContext,
    ) -> RefineResult<GitSyncResult> {
        if !self.target_root.join(".git").exists() {
            return Ok(skipped("Target app is not a Git repository."));
        }
        if !self.git_success(&["rev-parse", "--is-inside-work-tree"])? {
            return Ok(skipped("Target app is not a Git worktree."));
        }
        let live_refine = prepare_refine_dir(&self.target_root)?;
        self.ensure_local_state_excluded()?;
        let remote = self.configured_remote(&live_refine)?;
        let remote_configured = self.remote_exists(&remote)?;
        let remote_exists = if remote_configured {
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
        // Missing three-way authority is an exceptional operator recovery,
        // not an empty baseline. Fail before creating, rebasing, or retiring
        // anything in the managed worktree. Bootstrap-only metadata retains
        // its established remote-first initialization behavior.
        let stored_base = self.load_state_baseline()?;
        let local = durable_state_map(&live_refine)?;
        if stored_base.is_none() && remote_exists && !bootstrap_only_state(&local) {
            return Err(RefineError::StateSyncMissingBaseline(format!(
                "Refine state synchronization baseline is missing while non-bootstrap live state and {remote}/{REFINE_STATE_BRANCH} both exist. Ordinary sync remains fail-closed. Run `refine project state-recovery preview`, review the bounded comparison, then apply it with explicit live or remote authority. Do not recover live state from an isolated candidate worktree."
            )));
        }
        let setup = self.ensure_state_worktree(&remote, remote_exists, &live_refine)?;
        let state_root = setup.path;
        let state_refine = state_root.join(".refine");
        let recovered_interrupted_sync = self.recover_interrupted_state_worktree(&state_root)?;
        // The checked-out state branch is not a synchronization baseline: it can
        // advance before a failed first reconciliation has copied remote records
        // into the live store. Persist the last successfully copied state so an
        // absent local record is only interpreted as a deletion after this node
        // has actually observed it.
        let before = self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?;

        let mut pulled = setup.pulled;
        let mut details = if remote_configured {
            Vec::new()
        } else {
            vec![format!(
                "Git remote {remote} is not configured; Refine state was committed locally."
            )]
        };
        if recovered_interrupted_sync {
            details.push(
                "Recovered an interrupted Refine state copy before reconciling current live state."
                    .to_string(),
            );
        }
        let mut observed_remote_head = before.clone();
        if remote_exists {
            let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
            let remote_head = self.git_stdout(&["rev-parse", &remote_ref])?;
            observed_remote_head = remote_head.clone();
            pulled |= before != remote_head;
            let rebase = self.git_at(&state_root, &["rebase", &remote_ref])?;
            append_output_detail(&mut details, &rebase);
            if !rebase.success {
                let _ = self.git_at(&state_root, &["rebase", "--abort"]);
                return Err(command_failed(&format!("git rebase {remote_ref}"), &rebase));
            }
        }

        let remote_state = durable_state_map(&state_refine)?;
        let bootstrap_remote_state =
            stored_base.is_none() && remote_exists && bootstrap_only_state(&local);
        let base = if bootstrap_remote_state {
            details.push(
                "Adopted remote Refine state over locally generated bootstrap metadata."
                    .to_string(),
            );
            remote_first_bootstrap_baseline(&local, &remote_state)
        } else {
            stored_base.unwrap_or_default()
        };
        let reconciled = self.prepare_semantic_state_changes(
            &state_root,
            &live_refine,
            &state_refine,
            &base,
            &local,
            &remote_state,
        )?;
        let resolved_paths = reconciled.changes.keys().cloned().collect::<BTreeSet<_>>();
        append_reconciliation_details(&mut details, &reconciled.outcomes, false);
        let conflicts = state_conflicts(&base, &local, &remote_state, &resolved_paths);
        if !conflicts.is_empty() {
            let summary = self.record_conflict_report(
                StateSyncConflictPhase::FirstPass,
                attempt,
                &remote,
                &base,
                &live_refine,
                &local,
                &state_refine,
                &remote_state,
                self.local_state_head()?,
                observed_remote_head.clone(),
                &conflicts,
                &reconciled.outcomes,
            )?;
            return Err(RefineError::Conflict(summary.to_string()));
        }
        // Retirement mutates the managed checkout, so it follows every
        // fail-closed validation. Successful sync still records the intended
        // deletions in the next state commit.
        let removed_excluded = self.retire_excluded_tracked_state(&state_root, &state_refine)?;
        write_reconciled_state_changes(&state_refine, &reconciled.changes)?;
        apply_local_state_delta(&live_refine, &state_refine, &base, &local, &resolved_paths)?;

        let updated = durable_state_map(&state_refine)?;
        let mut changed = state_change_status(&remote_state, &updated);
        changed.extend(
            removed_excluded
                .into_iter()
                .map(|path| format!("D  .refine/{}", path.to_string_lossy().replace('\\', "/"))),
        );
        let delta_committed = !changed.is_empty();
        let mut committed = setup.created || delta_committed;
        let mut commit = if delta_committed {
            self.git_at_checked(&state_root, &["add", "-f", "-A", "--", ".refine"])?;
            let node_id =
                FileNodeRegistryService::with_active_root(&live_refine, &self.runtime_root)
                    .active_node_id()
                    .unwrap_or_else(|_| "default".to_string());
            let summary = state_commit_summary(&changed.join("\n"));
            self.git_at_checked(
                &state_root,
                &["commit", "-m", &summary, "-m", &format!("Node: {node_id}")],
            )?;
            Some(self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?)
        } else if setup.created {
            Some(before.clone())
        } else {
            None
        };

        let mut pushed = false;
        if remote_configured && (!remote_exists || committed || setup.local_ahead) {
            for push_attempt in 1..=PUSH_RETRY_LIMIT {
                let push =
                    self.git_at(&state_root, &["push", "-u", &remote, REFINE_STATE_BRANCH])?;
                append_output_detail(&mut details, &push);
                if push.success {
                    pushed = true;
                    break;
                }
                if push_attempt == PUSH_RETRY_LIMIT || !push_rejected_by_race(&push) {
                    return Err(command_failed("git push", &push));
                }
                self.fetch_state_branch(&remote)?;
                let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
                let retry_remote_head = self.git_stdout(&["rev-parse", &remote_ref])?;
                self.git_at_checked(&state_root, &["reset", "--hard", &remote_ref])?;
                let retry_removed_excluded =
                    self.retire_excluded_tracked_state(&state_root, &state_refine)?;
                let retry_remote_state = durable_state_map(&state_refine)?;
                // A rejected push means both the remote and the live store may have advanced
                // since the original reconciliation. Re-evaluate the original observed base
                // against both fresh sides before replaying any local delta.
                let retry_local = durable_state_map(&live_refine)?;
                let retry_reconciled = self.prepare_semantic_state_changes(
                    &state_root,
                    &live_refine,
                    &state_refine,
                    &base,
                    &retry_local,
                    &retry_remote_state,
                )?;
                let retry_resolved_paths = retry_reconciled
                    .changes
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                append_reconciliation_details(&mut details, &retry_reconciled.outcomes, true);
                let retry_conflicts = state_conflicts(
                    &base,
                    &retry_local,
                    &retry_remote_state,
                    &retry_resolved_paths,
                );
                if !retry_conflicts.is_empty() {
                    let summary = self.record_conflict_report(
                        StateSyncConflictPhase::PushRetry,
                        attempt,
                        &remote,
                        &base,
                        &live_refine,
                        &retry_local,
                        &state_refine,
                        &retry_remote_state,
                        self.local_state_head()?,
                        retry_remote_head,
                        &retry_conflicts,
                        &retry_reconciled.outcomes,
                    )?;
                    return Err(RefineError::Conflict(summary.to_string()));
                }
                write_reconciled_state_changes(&state_refine, &retry_reconciled.changes)?;
                apply_local_state_delta(
                    &live_refine,
                    &state_refine,
                    &base,
                    &retry_local,
                    &retry_resolved_paths,
                )?;
                let retry_updated = durable_state_map(&state_refine)?;
                let mut retry_changed = state_change_status(&retry_remote_state, &retry_updated);
                // Both sides of that comparison are enumerated through the
                // exclusion, so a retired path is invisible to it. Record the
                // removals explicitly or the reset would silently restore them.
                retry_changed.extend(retry_removed_excluded.into_iter().map(|path| {
                    format!("D  .refine/{}", path.to_string_lossy().replace('\\', "/"))
                }));
                committed = !retry_changed.is_empty();
                if committed {
                    self.git_at_checked(&state_root, &["add", "-f", "-A", "--", ".refine"])?;
                    let node_id =
                        FileNodeRegistryService::with_active_root(&live_refine, &self.runtime_root)
                            .active_node_id()
                            .unwrap_or_else(|_| "default".to_string());
                    let summary = state_commit_summary(&retry_changed.join("\n"));
                    self.git_at_checked(
                        &state_root,
                        &["commit", "-m", &summary, "-m", &format!("Node: {node_id}")],
                    )?;
                }
                pulled = true;
                if committed {
                    commit = Some(self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?);
                } else {
                    commit = None;
                }
                thread::sleep(PUSH_RETRY_DELAY);
            }
        }

        let concurrent_local_change = merge_state_into_live(&state_refine, &live_refine, &local)?;
        self.save_state_baseline(&durable_state_map(&state_refine)?)?;
        if concurrent_local_change {
            details.push(
                "A newer local state mutation arrived during synchronization; it was preserved and will be published in the next batch."
                    .to_string(),
            );
        }
        Ok(GitSyncResult {
            ok: true,
            attempted: true,
            committed,
            pulled,
            pushed,
            branch: Some(REFINE_STATE_BRANCH.to_string()),
            commit,
            detail: nonempty_detail(details),
            remote_configured: Some(remote_configured),
            deferred: concurrent_local_change,
        })
    }
}

fn display_state_paths(paths: &BTreeSet<PathBuf>) -> String {
    paths
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn append_reconciliation_details(
    details: &mut Vec<String>,
    outcomes: &[StateSyncReconciliationOutcome],
    push_retry: bool,
) {
    let resolved = outcomes
        .iter()
        .filter(|outcome| {
            matches!(
                outcome.outcome,
                StateSyncReconciliationKind::ThreeWayMerged
                    | StateSyncReconciliationKind::BaselineUnavailableFallbackMerged
            )
        })
        .map(|outcome| PathBuf::from(&outcome.path))
        .collect::<BTreeSet<_>>();
    let goals = resolved
        .iter()
        .filter(|path| is_goal_record(path))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !goals.is_empty() {
        details.push(format!(
            "Merged non-overlapping Goal changes{}: {}",
            if push_retry { " during push retry" } else { "" },
            display_state_paths(&goals)
        ));
    }
    if resolved.contains(std::path::Path::new("nodes.json")) {
        let fallback = outcomes.iter().any(|outcome| {
            outcome.path == "nodes.json"
                && outcome.outcome == StateSyncReconciliationKind::BaselineUnavailableFallbackMerged
        });
        details.push(format!(
            "Merged per-node registry changes{}{}: nodes.json",
            if push_retry { " during push retry" } else { "" },
            if fallback {
                " with safe baseline-unavailable fallback"
            } else {
                ""
            }
        ));
    }
}

fn write_reconciled_state_changes(
    state_refine: &std::path::Path,
    reconciled: &BTreeMap<PathBuf, Vec<u8>>,
) -> RefineResult<()> {
    for (relative, bytes) in reconciled {
        write_state_file_bytes(&state_refine.join(relative), bytes)?;
    }
    Ok(())
}
