use super::*;

impl FileGitSyncService {
    /// Apply an operator-authorized recovery. The complete preview is the
    /// compare-and-swap token; authority alone is deliberately insufficient.
    pub fn apply_state_recovery(
        &self,
        authority: StateRecoveryAuthority,
        preview: StateRecoveryPreview,
    ) -> RefineResult<StateRecoveryResult> {
        let lock = repository_git_lock(&self.target_root)?;
        let _guard = match lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Err(RefineError::Conflict(
                    "Repository Git operations are busy; recovery was not started.".to_string(),
                ));
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(RefineError::Conflict(
                    "Repository Git lock was poisoned".to_string(),
                ));
            }
        };
        let Some(_file_guard) = RepositoryFileLock::try_acquire(&self.target_root)? else {
            return Err(RefineError::Conflict(
                "Repository Git operations are busy; recovery was not started.".to_string(),
            ));
        };
        let result = self.apply_state_recovery_locked(authority, &preview);
        if let Err(error) = &result {
            #[cfg(test)]
            let simulated_interruption = error.to_string() == TEST_BASELINE_INTERRUPTION;
            #[cfg(not(test))]
            let simulated_interruption = false;
            let baseline_present = self
                .load_state_baseline()
                .is_ok_and(|baseline| baseline.is_some());
            if !simulated_interruption && !baseline_present {
                let _ = self.record_recovery_failure(authority, &preview, &error.to_string());
            }
            let _ = self.restore_managed_state_worktree();
        }
        result
    }

    fn apply_state_recovery_locked(
        &self,
        authority: StateRecoveryAuthority,
        preview: &StateRecoveryPreview,
    ) -> RefineResult<StateRecoveryResult> {
        self.validate_recovery_preview(preview)?;
        self.validate_recovery_target()?;
        self.reject_foreign_git_operation()?;

        let live_refine =
            crate::tools::host::project_layout::refine_dir_for_target_root(&self.target_root)?;
        let remote = self.configured_remote(&live_refine)?;
        if remote != preview.configured_remote {
            return Err(stale_recovery("the configured remote changed"));
        }
        if self.target_identity()? != preview.target_identity {
            return Err(stale_recovery("the target identity changed"));
        }
        if self.repository_identity(&remote)? != preview.repository_identity {
            return Err(stale_recovery("the repository identity changed"));
        }
        let manifest_path = self.recovery_manifest_path(preview, authority)?;
        let existing = load_manifest(&manifest_path)?;
        if existing.as_ref().is_some_and(|manifest| {
            manifest.evidence_id != preview.evidence_id || manifest.authority != authority
        }) {
            return Err(RefineError::Conflict(
                "Recovery evidence location belongs to a different apply request.".to_string(),
            ));
        }
        if let Some(baseline) = self.load_state_baseline()? {
            if let Some(result) = self.finalize_owned_recovery(
                authority,
                preview,
                &remote,
                &manifest_path,
                existing.as_ref(),
                &baseline,
            )? {
                return Ok(result);
            }
            return Err(RefineError::Conflict(
                "State recovery was rejected because the synchronization baseline now exists and does not match an owned interrupted apply."
                    .to_string(),
            ));
        }
        let current_local_head = self.local_state_head()?;
        let owned_local_head = existing
            .as_ref()
            .and_then(|manifest| manifest.local_state_head_after.as_ref());
        if current_local_head != preview.local_state_head
            && owned_local_head != current_local_head.as_ref()
        {
            return Err(stale_recovery("the local refine/state head changed"));
        }
        self.validate_managed_state_worktree()?;
        let owned_after_head = existing
            .as_ref()
            .and_then(|manifest| manifest.remote_state_head_after.as_deref());
        let observed_remote_head = self
            .remote_state_head(&remote)?
            .ok_or_else(|| stale_recovery("the configured remote state branch disappeared"))?;
        if observed_remote_head != preview.remote_state_head
            && owned_after_head != Some(observed_remote_head.as_str())
        {
            return Err(stale_recovery("the remote refine/state head changed"));
        }
        let remote_observation = if observed_remote_head == preview.remote_state_head {
            self.observe_remote_state(&remote, &observed_remote_head)?
        } else {
            self.observe_repository_ref(&observed_remote_head)?
        };
        let remote_refine = remote_observation.path.join(".refine");
        let remote_state = durable_state_map(&remote_refine)?;
        if observed_remote_head == preview.remote_state_head
            && state_tree_digest(&remote_refine, &remote_state)? != preview.remote_snapshot
        {
            return Err(stale_recovery("the fetched remote state snapshot changed"));
        }
        let prevalidated_live = if existing.is_none() {
            let live_now = durable_state_map(&live_refine)?;
            if state_tree_digest(&live_refine, &live_now)? != preview.live_snapshot {
                return Err(stale_recovery("the live state snapshot changed"));
            }
            // Reject fabricated or internally inconsistent preview metadata
            // before journaling or preserving any recovery ref. The stable
            // owned snapshot below repeats this fence to catch a live write
            // racing the initial read.
            super::inspection::validate_recovery_comparison(preview, &live_now, &remote_state)?;
            Some(live_now)
        } else {
            None
        };

        let recovery_ref = self.recovery_ref(preview, authority);
        let mut manifest = existing.unwrap_or_else(|| StateRecoveryManifest {
            version: 1,
            evidence_id: preview.evidence_id.clone(),
            authority,
            node: FileNodeRegistryService::with_active_root(&live_refine, &self.runtime_root)
                .active_node_id()
                .unwrap_or_else(|_| "default".to_string()),
            target_identity: preview.target_identity.clone(),
            repository_identity: preview.repository_identity.clone(),
            configured_remote: remote.clone(),
            local_state_head_before: preview.local_state_head.clone(),
            remote_state_head_before: preview.remote_state_head.clone(),
            live_snapshot_before: preview.live_snapshot.clone(),
            local_state_head_after: None,
            remote_state_head_after: None,
            path_counts: preview.path_counts.clone(),
            started_at: recovery_timestamp(),
            completed_at: None,
            outcome: StateRecoveryOutcome::Started,
            recovery_location: recovery_ref.clone(),
            message: "Recovery started; no baseline has been created.".to_string(),
        });
        manifest.outcome = StateRecoveryOutcome::Started;
        manifest.completed_at = None;
        // Record the owned anchor before any branch creation/reset. If the
        // process stops between those Git steps and the authority-specific
        // work, the next apply can distinguish its own remote anchor from an
        // unrelated local-head change and resume from the recovery snapshot.
        manifest.local_state_head_after = Some(observed_remote_head.clone());
        manifest.message = "Recovery started; no baseline has been created.".to_string();
        write_manifest(&manifest_path, &manifest)?;

        let live_now = match prevalidated_live {
            Some(live) => live,
            None => durable_state_map(&live_refine)?,
        };
        let resume = self.git_success(&["show-ref", "--verify", "--quiet", &recovery_ref])?;
        if !resume && state_tree_digest(&live_refine, &live_now)? != preview.live_snapshot {
            return Err(stale_recovery("the live state snapshot changed"));
        }
        if !resume {
            self.preserve_live_snapshot(preview, &live_refine, &recovery_ref)?;
        }
        let preserved = self.observe_repository_ref(&recovery_ref)?;
        let preserved_refine = preserved.path.join(".refine");
        let original_live = durable_state_map(&preserved_refine)?;
        if state_tree_digest(&preserved_refine, &original_live)? != preview.live_snapshot {
            return Err(RefineError::Conflict(
                "The owned recovery snapshot does not match the supplied preview.".to_string(),
            ));
        }
        if observed_remote_head == preview.remote_state_head {
            super::inspection::validate_recovery_comparison(
                preview,
                &original_live,
                &remote_state,
            )?;
        } else {
            // A resumed live-authority apply may already have published its
            // owned descendant. Re-read the exact preview head from retained
            // history so the original operator-reviewed comparison remains
            // part of the apply fence rather than trusting client-supplied
            // counts or paths.
            let preview_remote = self.observe_repository_ref(&preview.remote_state_head)?;
            let preview_remote_refine = preview_remote.path.join(".refine");
            let preview_remote_state = durable_state_map(&preview_remote_refine)?;
            if state_tree_digest(&preview_remote_refine, &preview_remote_state)?
                != preview.remote_snapshot
            {
                return Err(stale_recovery(
                    "the retained preview remote snapshot changed",
                ));
            }
            super::inspection::validate_recovery_comparison(
                preview,
                &original_live,
                &preview_remote_state,
            )?;
        }

        self.fetch_state_branch(&remote)?;
        let fetched_head =
            self.git_stdout(&["rev-parse", &format!("{remote}/{REFINE_STATE_BRANCH}")])?;
        if fetched_head != observed_remote_head {
            return Err(stale_recovery(
                "the remote head changed while recovery was fetching",
            ));
        }
        let setup = self.ensure_state_worktree(&remote, true, &live_refine)?;
        let state_root = setup.path;
        let state_refine = state_root.join(".refine");
        self.git_at_checked(
            &state_root,
            &[
                "reset",
                "--hard",
                &format!("{remote}/{REFINE_STATE_BRANCH}"),
            ],
        )?;

        match authority {
            StateRecoveryAuthority::Live => {
                apply_local_state_delta(
                    &preserved_refine,
                    &state_refine,
                    &remote_state,
                    &original_live,
                    &BTreeSet::new(),
                )?;
                let removed = self.retire_excluded_tracked_state(&state_root, &state_refine)?;
                let updated = durable_state_map(&state_refine)?;
                let mut changes = state_change_status(&remote_state, &updated);
                changes.extend(removed.into_iter().map(|path| {
                    format!("D  .refine/{}", path.to_string_lossy().replace('\\', "/"))
                }));
                if !changes.is_empty() {
                    self.git_at_checked(&state_root, &["add", "-f", "-A", "--", ".refine"])?;
                    let summary = state_commit_summary(&changes.join("\n"));
                    self.git_at_checked(
                        &state_root,
                        &[
                            "commit",
                            "-m",
                            &summary,
                            "-m",
                            &format!("State recovery authority: live\nNode: {}", manifest.node),
                        ],
                    )?;
                }
                let after = self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?;
                manifest.local_state_head_after = Some(after.clone());
                manifest.remote_state_head_after = Some(after.clone());
                write_manifest(&manifest_path, &manifest)?;
                let remote_before_push = self.remote_state_head(&remote)?.ok_or_else(|| {
                    stale_recovery("the remote state branch disappeared before publish")
                })?;
                if remote_before_push != after {
                    if remote_before_push != preview.remote_state_head {
                        return Err(stale_recovery(
                            "the remote state branch changed before recovery publish",
                        ));
                    }
                    self.git_at_checked(
                        &state_root,
                        &["push", &remote, &format!("HEAD:{REFINE_STATE_REF}")],
                    )?;
                }
                let published = self.remote_state_head(&remote)?.ok_or_else(|| {
                    stale_recovery("the published remote state branch disappeared")
                })?;
                if published != after {
                    return Err(stale_recovery(
                        "the remote state branch did not retain the recovery commit",
                    ));
                }
                let _concurrent =
                    merge_state_into_live(&state_refine, &live_refine, &original_live)?;
            }
            StateRecoveryAuthority::Remote => {
                hydrate_remote_with_recovery_cas(&preserved_refine, &remote_refine, &live_refine)?;
                if durable_state_map(&live_refine)? != remote_state {
                    return Err(RefineError::Conflict(
                        "Live state changed during remote-authority recovery; no baseline was created and the apply is retryable."
                            .to_string(),
                    ));
                }
                manifest.local_state_head_after = Some(observed_remote_head.clone());
                manifest.remote_state_head_after = Some(observed_remote_head.clone());
                write_manifest(&manifest_path, &manifest)?;
            }
        }

        #[cfg(test)]
        run_after_recovery_authority_hook(&self.target_root);

        let expected_remote = manifest.remote_state_head_after.as_deref().ok_or_else(|| {
            RefineError::Conflict("Recovery did not establish a final remote head.".to_string())
        })?;
        let final_remote = self.remote_state_head(&remote)?.ok_or_else(|| {
            RefineError::Conflict(
                "Remote state disappeared after recovery; no baseline was created and the apply is retryable."
                    .to_string(),
            )
        })?;
        if final_remote != expected_remote {
            return Err(stale_recovery(
                "the remote refine/state head changed after authority work",
            ));
        }
        let final_local = self.local_state_head()?;
        if final_local.as_deref() != manifest.local_state_head_after.as_deref() {
            return Err(stale_recovery(
                "the local refine/state head changed after authority work",
            ));
        }
        let settled_state = durable_state_map(&state_refine)?;
        if let Err(error) = self.save_state_baseline(&settled_state) {
            self.remove_state_baseline_if_owned(&settled_state)?;
            return Err(error);
        }
        #[cfg(test)]
        if run_after_recovery_baseline_hook(&self.target_root) {
            return Err(RefineError::Io(TEST_BASELINE_INTERRUPTION.to_string()));
        }
        manifest.local_state_head_after = final_local;
        manifest.remote_state_head_after = Some(final_remote.clone());
        manifest.completed_at = Some(recovery_timestamp());
        manifest.outcome = StateRecoveryOutcome::Succeeded;
        manifest.message =
            "Recovery completed and the synchronization baseline was created.".to_string();
        if let Err(error) = write_manifest(&manifest_path, &manifest) {
            self.remove_state_baseline_if_owned(&settled_state)?;
            return Err(error);
        }

        Ok(StateRecoveryResult {
            ok: true,
            authority,
            baseline_created: true,
            local_state_head: manifest.local_state_head_after,
            remote_state_head: final_remote,
            recovery_location: recovery_ref,
            manifest_path: manifest_path.display().to_string(),
            path_counts: preview.path_counts.clone(),
            detail: manifest.message,
        })
    }

    fn finalize_owned_recovery(
        &self,
        authority: StateRecoveryAuthority,
        preview: &StateRecoveryPreview,
        remote: &str,
        manifest_path: &std::path::Path,
        manifest: Option<&StateRecoveryManifest>,
        baseline: &DurableStateMap,
    ) -> RefineResult<Option<StateRecoveryResult>> {
        let Some(manifest) = manifest else {
            return Ok(None);
        };
        if manifest.outcome != StateRecoveryOutcome::Started
            || manifest.evidence_id != preview.evidence_id
            || manifest.authority != authority
            || manifest.target_identity != preview.target_identity
            || manifest.repository_identity != preview.repository_identity
            || manifest.configured_remote != preview.configured_remote
            || manifest.local_state_head_before != preview.local_state_head
            || manifest.remote_state_head_before != preview.remote_state_head
            || manifest.live_snapshot_before != preview.live_snapshot
            || manifest.path_counts != preview.path_counts
            || manifest.recovery_location != self.recovery_ref(preview, authority)
        {
            return Ok(None);
        }
        let (Some(expected_local), Some(expected_remote)) = (
            manifest.local_state_head_after.as_deref(),
            manifest.remote_state_head_after.as_deref(),
        ) else {
            return Ok(None);
        };
        if self.local_state_head()?.as_deref() != Some(expected_local)
            || self.remote_state_head(remote)?.as_deref() != Some(expected_remote)
            || expected_local != expected_remote
            || !self.git_success(&[
                "show-ref",
                "--verify",
                "--quiet",
                &manifest.recovery_location,
            ])?
        {
            return Ok(None);
        }
        let settled = self.observe_repository_ref(expected_local)?;
        if durable_state_map(&settled.path.join(".refine"))? != *baseline {
            return Ok(None);
        }

        let mut completed = manifest.clone();
        completed.completed_at = Some(recovery_timestamp());
        completed.outcome = StateRecoveryOutcome::Succeeded;
        completed.message =
            "Recovery completed and the synchronization baseline was created.".to_string();
        write_manifest(manifest_path, &completed)?;
        Ok(Some(StateRecoveryResult {
            ok: true,
            authority,
            baseline_created: true,
            local_state_head: completed.local_state_head_after,
            remote_state_head: expected_remote.to_string(),
            recovery_location: completed.recovery_location,
            manifest_path: manifest_path.display().to_string(),
            path_counts: preview.path_counts.clone(),
            detail: completed.message,
        }))
    }

    fn validate_recovery_preview(&self, preview: &StateRecoveryPreview) -> RefineResult<()> {
        if preview.version != 1
            || preview.baseline_status != "missing"
            || preview.evidence_id.is_empty()
            || preview.evidence_id != preview_evidence_id(preview)?
        {
            return Err(RefineError::InvalidInput(
                "The supplied state recovery preview is invalid or incomplete.".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_managed_state_worktree(&self) -> RefineResult<()> {
        let path = state_worktree_for_target_root(&self.target_root)?;
        if !path.exists() {
            return Ok(());
        }
        let branch = self.git_at_stdout(&path, &["branch", "--show-current"])?;
        let status = self.git_at_stdout(
            &path,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?;
        if branch != REFINE_STATE_BRANCH || !status.is_empty() {
            return Err(RefineError::Conflict(format!(
                "Managed state worktree is unsafe for recovery (branch={branch}, changes={status})."
            )));
        }
        Ok(())
    }

    fn reject_foreign_git_operation(&self) -> RefineResult<()> {
        let common = git_common_dir(&self.target_root)?;
        let markers = [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "rebase-apply",
            "rebase-merge",
            "sequencer",
        ];
        if let Some(marker) = markers.iter().find(|marker| common.join(marker).exists()) {
            return Err(RefineError::Conflict(format!(
                "Git operation marker {marker} is active; recovery was not started."
            )));
        }
        Ok(())
    }

    pub(in crate::tools::host::git_sync) fn restore_managed_state_worktree(
        &self,
    ) -> RefineResult<()> {
        let path = state_worktree_for_target_root(&self.target_root)?;
        if !path.exists() {
            return Ok(());
        }
        let _ = self.git_at(&path, &["rebase", "--abort"]);
        self.recover_interrupted_state_worktree(&path).map(|_| ())
    }

    fn record_recovery_failure(
        &self,
        authority: StateRecoveryAuthority,
        preview: &StateRecoveryPreview,
        message: &str,
    ) -> RefineResult<()> {
        let path = self.recovery_manifest_path(preview, authority)?;
        let Some(mut manifest) = load_manifest(&path)? else {
            return Ok(());
        };
        if manifest.outcome == StateRecoveryOutcome::Succeeded {
            return Ok(());
        }
        manifest.outcome = StateRecoveryOutcome::Failed;
        manifest.completed_at = Some(recovery_timestamp());
        manifest.message = bounded_message(message);
        write_manifest(&path, &manifest)
    }

    fn observe_repository_ref(&self, reference: &str) -> RefineResult<DisposableCheckout> {
        let checkout = self.disposable_checkout("owned-recovery")?;
        let path = checkout.path.display().to_string();
        let source = self.target_root.display().to_string();
        self.git_checked(&["clone", "-q", "--no-checkout", "--", &source, &path])?;
        self.git_at_checked(&checkout.path, &["fetch", "-q", "origin", reference])?;
        self.git_at_checked(
            &checkout.path,
            &["checkout", "-q", "--detach", "FETCH_HEAD"],
        )?;
        Ok(checkout)
    }

    fn preserve_live_snapshot(
        &self,
        preview: &StateRecoveryPreview,
        live_refine: &std::path::Path,
        recovery_ref: &str,
    ) -> RefineResult<()> {
        let checkout = self.disposable_checkout("live-snapshot")?;
        let path = checkout.path.display().to_string();
        let (source, branch) = if preview.local_state_head.is_some() {
            (
                self.target_root.display().to_string(),
                REFINE_STATE_BRANCH.to_string(),
            )
        } else {
            let remote_url = self.git_stdout(&["remote", "get-url", &preview.configured_remote])?;
            (remote_url, REFINE_STATE_BRANCH.to_string())
        };
        self.git_checked(&[
            "clone",
            "-q",
            "--single-branch",
            "--branch",
            &branch,
            "--",
            &source,
            &path,
        ])?;
        let destination = checkout.path.join(".refine");
        if destination.exists() {
            fs::remove_dir_all(&destination).map_err(|error| {
                RefineError::Io(format!(
                    "failed to prepare disposable recovery snapshot {}: {error}",
                    destination.display()
                ))
            })?;
        }
        replace_live_durable_state(live_refine, &destination)?;
        self.git_at_checked(&checkout.path, &["add", "-f", "-A", "--", ".refine"])?;
        self.git_at_checked(
            &checkout.path,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "Preserve pre-recovery Refine live state",
                "-m",
                &format!("Recovery evidence: {}", preview.evidence_id),
            ],
        )?;
        let source_path = checkout.path.display().to_string();
        self.git_checked(&[
            "fetch",
            "--no-tags",
            &source_path,
            &format!("HEAD:{recovery_ref}"),
        ])?;
        Ok(())
    }

    fn recovery_ref(
        &self,
        preview: &StateRecoveryPreview,
        authority: StateRecoveryAuthority,
    ) -> String {
        format!(
            "{RECOVERY_REF_PREFIX}/{}-{}",
            &preview.evidence_id[..preview.evidence_id.len().min(24)],
            authority.as_str()
        )
    }

    fn recovery_manifest_path(
        &self,
        preview: &StateRecoveryPreview,
        authority: StateRecoveryAuthority,
    ) -> RefineResult<PathBuf> {
        Ok(git_common_dir(&self.target_root)?
            .join(RECOVERY_MANIFEST_DIR)
            .join(format!(
                "{}-{}.json",
                preview.evidence_id,
                authority.as_str()
            )))
    }
}
