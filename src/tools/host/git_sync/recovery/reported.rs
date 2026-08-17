use super::reported_support::*;
use super::*;
use crate::tools::host::git_sync::conflict_report::state_map_digest;
use crate::tools::host::git_sync::recovery::inspection::recovery_path_counts;

const RECOVERY_TARGET_REF_PREFIX: &str = "refs/refine/state-recovery-target";

impl FileGitSyncService {
    /// Convert the latest complete valid-baseline conflict report into a
    /// read-only stale-fenced recovery preview.
    pub(super) fn preview_reported_state_recovery(&self) -> RefineResult<StateRecoveryPreview> {
        self.validate_recovery_target()?;
        let report = latest_state_sync_conflict_report(&self.runtime_root)?.ok_or_else(|| {
            RefineError::Conflict(
                "State recovery requires a complete local state-sync conflict report; run `refine project sync` and retry the preview."
                    .to_string(),
            )
        })?;
        validate_report_identity(&report)?;
        let live_refine =
            crate::tools::host::project_layout::refine_dir_for_target_root(&self.target_root)?;
        let baseline = self
            .load_state_baseline()?
            .ok_or_else(|| stale_recovery("the valid synchronization baseline disappeared"))?;
        if state_map_digest(&baseline) != report.baseline_snapshot {
            return Err(stale_recovery("the synchronization baseline changed"));
        }
        let remote = self.configured_remote(&live_refine)?;
        validate_report_authority(self, &report, &remote)?;
        let local_state_head = self.local_state_head()?;
        if local_state_head != report.local_state_head {
            return Err(stale_recovery("the local refine/state head changed"));
        }
        let remote_head = self
            .remote_state_head(&remote)?
            .ok_or_else(|| stale_recovery("the configured remote state branch disappeared"))?;
        if remote_head != report.remote_state_head {
            return Err(stale_recovery("the remote refine/state head changed"));
        }
        let live = durable_state_map(&live_refine)?;
        if state_tree_digest(&live_refine, &live)? != report.local_snapshot {
            return Err(stale_recovery(
                &super::inspection::stale_live_snapshot_reason(
                    &live_refine,
                    &report.local_fingerprints,
                    &live,
                ),
            ));
        }
        let observation = self.observe_remote_state(&remote, &remote_head)?;
        let remote_refine = observation.path.join(".refine");
        let remote_state = durable_state_map(&remote_refine)?;
        if state_tree_digest(&remote_refine, &remote_state)? != report.remote_snapshot {
            return Err(stale_recovery("the remote state snapshot changed"));
        }
        let resolved = self.prepare_semantic_state_changes(
            &observation.path,
            &live_refine,
            &remote_refine,
            &baseline,
            &live,
            &remote_state,
        )?;
        let resolved_paths = resolved.changes.keys().cloned().collect::<BTreeSet<_>>();
        let conflicts = state_conflicts(&baseline, &live, &remote_state, &resolved_paths);
        if conflicts != report.unresolved_paths {
            return Err(stale_recovery(
                "the reported conflict set no longer matches three-way reconciliation",
            ));
        }
        let mut preview = StateRecoveryPreview {
            version: REPORTED_RECOVERY_VERSION,
            evidence_id: String::new(),
            target_identity: report.target_identity.clone(),
            repository_identity: report.repository_identity.clone(),
            configured_remote: report.configured_remote.clone(),
            local_state_head,
            remote_state_head: remote_head,
            baseline_status: "valid_conflict".to_string(),
            baseline_snapshot: Some(report.baseline_snapshot.clone()),
            conflict_report_id: Some(report.report_id.clone()),
            conflict_report_location: Some(report.report_location.clone()),
            live_snapshot: report.local_snapshot,
            live_fingerprints: super::inspection::recovery_fingerprints(&live),
            remote_snapshot: report.remote_snapshot,
            path_counts: recovery_path_counts(&live, &remote_state),
            conflicting_paths: report.unresolved_paths,
            conflicting_paths_truncated: 0,
        };
        preview.evidence_id = preview_evidence_id(&preview)?;
        Ok(preview)
    }

    pub(super) fn apply_reported_state_recovery_locked(
        &self,
        decision: &StateRecoveryDecision,
        preview: &StateRecoveryPreview,
    ) -> RefineResult<StateRecoveryResult> {
        self.validate_recovery_target()?;
        self.reject_foreign_git_operation()?;
        validate_reported_preview_shape(preview)?;
        let overrides = validate_recovery_decision(decision, &preview.conflicting_paths)?;
        let decision_id = recovery_decision_id(decision)?;
        let live_refine =
            crate::tools::host::project_layout::refine_dir_for_target_root(&self.target_root)?;
        let remote = self.configured_remote(&live_refine)?;
        let manifest_path = self.reported_recovery_manifest_path(preview)?;
        let existing = load_manifest(&manifest_path)?;
        if let Some(manifest) = &existing {
            validate_owned_manifest(manifest, preview, decision, &decision_id)?;
        }

        if existing.is_none() {
            let report = latest_state_sync_conflict_report(&self.runtime_root)?
                .ok_or_else(|| stale_recovery("the complete local conflict report disappeared"))?;
            validate_report_identity(&report)?;
            if Some(report.report_id.as_str()) != preview.conflict_report_id.as_deref() {
                return Err(stale_recovery(
                    "a newer conflict report replaced the reviewed report",
                ));
            }
            validate_report_authority(self, &report, &remote)?;
        } else if remote != preview.configured_remote
            || self.target_identity()? != preview.target_identity
            || self.repository_identity(&remote)? != preview.repository_identity
        {
            return Err(stale_recovery(
                "the target, repository, or configured remote identity changed",
            ));
        }

        let baseline = self
            .load_state_baseline()?
            .ok_or_else(|| stale_recovery("the valid synchronization baseline disappeared"))?;
        let baseline_digest = state_map_digest(&baseline);
        let manifest_target_snapshot = existing
            .as_ref()
            .and_then(|manifest| manifest.target_snapshot.as_deref());
        let baseline_is_original =
            Some(baseline_digest.as_str()) == preview.baseline_snapshot.as_deref();
        let baseline_is_owned_target = manifest_target_snapshot == Some(baseline_digest.as_str());
        if !baseline_is_original && !baseline_is_owned_target {
            return Err(RefineError::Conflict(
                "State recovery found a baseline that belongs to neither the preview nor its owned recovery target."
                    .to_string(),
            ));
        }

        let source_ref = self.reported_source_ref(preview);
        let target_ref = self.reported_target_ref(preview, &decision_id);
        if baseline_is_owned_target {
            if existing
                .as_ref()
                .is_none_or(|manifest| manifest.stage < StateRecoveryStage::Hydrated)
            {
                return Err(RefineError::Conflict(
                    "The owned recovery baseline appeared before the manifest recorded its hydration boundary."
                        .to_string(),
                ));
            }
            return self.finalize_reported_recovery(
                preview,
                decision,
                &remote,
                &manifest_path,
                existing.as_ref(),
                &baseline,
            );
        }

        let observed_remote_head = self
            .remote_state_head(&remote)?
            .ok_or_else(|| stale_recovery("the configured remote state branch disappeared"))?;
        let owned_target_head = existing
            .as_ref()
            .and_then(|manifest| manifest.remote_state_head_after.as_deref());
        if observed_remote_head != preview.remote_state_head
            && owned_target_head != Some(observed_remote_head.as_str())
        {
            return Err(stale_recovery("the remote refine/state head changed"));
        }
        let current_local_head = self.local_state_head()?;
        if current_local_head != preview.local_state_head
            && owned_target_head != current_local_head.as_deref()
        {
            return Err(stale_recovery("the local refine/state head changed"));
        }
        self.validate_managed_state_worktree()?;

        let remote_preview = self.observe_repository_ref(&preview.remote_state_head)?;
        let remote_refine = remote_preview.path.join(".refine");
        let remote_state = durable_state_map(&remote_refine)?;
        if state_tree_digest(&remote_refine, &remote_state)? != preview.remote_snapshot {
            return Err(stale_recovery(
                "the retained preview remote snapshot changed",
            ));
        }

        let source_exists = self.git_success(&["show-ref", "--verify", "--quiet", &source_ref])?;
        let existing_source = if source_exists {
            Some(self.observe_repository_ref(&source_ref)?)
        } else {
            None
        };
        let comparison_refine = existing_source
            .as_ref()
            .map(|source| source.path.join(".refine"))
            .unwrap_or_else(|| live_refine.clone());
        let original_live = durable_state_map(&comparison_refine)?;
        if state_tree_digest(&comparison_refine, &original_live)? != preview.live_snapshot {
            return if source_exists {
                Err(RefineError::Conflict(
                    "The owned pre-recovery snapshot does not match the supplied preview."
                        .to_string(),
                ))
            } else {
                Err(stale_recovery(
                    &super::inspection::stale_live_snapshot_reason(
                        &live_refine,
                        &preview.live_fingerprints,
                        &durable_state_map(&live_refine)?,
                    ),
                ))
            };
        }

        let resolved = self.prepare_semantic_state_changes(
            &remote_preview.path,
            &comparison_refine,
            &remote_refine,
            &baseline,
            &original_live,
            &remote_state,
        )?;
        let resolved_paths = resolved.changes.keys().cloned().collect::<BTreeSet<_>>();
        let conflicts = state_conflicts(&baseline, &original_live, &remote_state, &resolved_paths);
        if conflicts != preview.conflicting_paths {
            return Err(stale_recovery("the exact conflict set changed"));
        }
        for path in &conflicts {
            let relative = PathBuf::from(path);
            if let Some(fingerprint) = baseline.get(&relative)
                && self
                    .load_baseline_file(&remote_preview.path, &relative, *fingerprint)?
                    .is_none()
            {
                return Err(RefineError::Conflict(format!(
                    "State recovery cannot reproduce baseline bytes for {path}; no side effects were applied."
                )));
            }
        }
        if !source_exists {
            self.preserve_live_snapshot(preview, &live_refine, &source_ref)?;
        }
        let preserved = self.observe_repository_ref(&source_ref)?;
        let preserved_refine = preserved.path.join(".refine");
        let preserved_state = durable_state_map(&preserved_refine)?;
        if preserved_state != original_live
            || state_tree_digest(&preserved_refine, &preserved_state)? != preview.live_snapshot
        {
            return Err(RefineError::Conflict(
                "The owned pre-recovery snapshot does not match the validated live state."
                    .to_string(),
            ));
        }

        let manifest_existed = existing.is_some();
        let mut manifest = existing.unwrap_or_else(|| StateRecoveryManifest {
            version: REPORTED_RECOVERY_VERSION,
            evidence_id: preview.evidence_id.clone(),
            authority: decision.default_authority,
            decision_id: decision_id.clone(),
            overrides: decision.overrides.clone(),
            node: FileNodeRegistryService::with_active_root(&live_refine, &self.runtime_root)
                .active_node_id()
                .unwrap_or_else(|_| "default".to_string()),
            target_identity: preview.target_identity.clone(),
            repository_identity: preview.repository_identity.clone(),
            configured_remote: remote.clone(),
            local_state_head_before: preview.local_state_head.clone(),
            remote_state_head_before: preview.remote_state_head.clone(),
            live_snapshot_before: preview.live_snapshot.clone(),
            baseline_snapshot_before: preview.baseline_snapshot.clone(),
            conflict_report_id: preview.conflict_report_id.clone(),
            local_state_head_after: None,
            remote_state_head_after: None,
            target_snapshot: None,
            target_location: None,
            path_counts: preview.path_counts.clone(),
            started_at: recovery_timestamp(),
            completed_at: None,
            outcome: StateRecoveryOutcome::Started,
            stage: StateRecoveryStage::Started,
            recovery_location: source_ref.clone(),
            message: "Recovery decision reserved; remote and live state are unchanged.".to_string(),
        });
        if !manifest_existed {
            write_manifest(&manifest_path, &manifest)?;
        }

        let persisted_target = match (
            manifest.local_state_head_after.clone(),
            manifest.target_snapshot.clone(),
            manifest.target_location.as_deref(),
        ) {
            (Some(target_head), Some(target_snapshot), Some(location)) => {
                if location != target_ref {
                    return Err(RefineError::Conflict(
                        "Owned recovery manifest points at a different target ref.".to_string(),
                    ));
                }
                validate_owned_target(self, &target_ref, &target_head, &target_snapshot)?;
                Some((target_head, target_snapshot))
            }
            (None, None, None) if manifest.stage == StateRecoveryStage::Started => None,
            _ => {
                return Err(RefineError::Conflict(
                    "Owned recovery manifest has an incomplete target boundary.".to_string(),
                ));
            }
        };
        let (target_head, target_snapshot) = match persisted_target {
            Some(target) => target,
            None => self.build_reported_recovery_target(
                preview,
                decision,
                &overrides,
                &remote_preview,
                &preserved_refine,
                &baseline,
                &original_live,
                &remote_state,
                &resolved.changes,
                &target_ref,
            )?,
        };

        if manifest.stage == StateRecoveryStage::Started {
            manifest.local_state_head_after = Some(target_head.clone());
            manifest.remote_state_head_after = Some(target_head.clone());
            manifest.target_snapshot = Some(target_snapshot.clone());
            manifest.target_location = Some(target_ref.clone());
            manifest.stage = StateRecoveryStage::TargetPersisted;
            manifest.outcome = StateRecoveryOutcome::Started;
            manifest.message =
                "Recovery target persisted; remote and live state are unchanged.".to_string();
            write_manifest(&manifest_path, &manifest)?;
        }

        if manifest.stage < StateRecoveryStage::Published {
            let current_remote = self.remote_state_head(&remote)?.ok_or_else(|| {
                stale_recovery("the remote state branch disappeared before publish")
            })?;
            if current_remote != target_head {
                if current_remote != preview.remote_state_head {
                    return Err(stale_recovery(
                        "the remote state branch changed before recovery publish",
                    ));
                }
                self.git_checked(&[
                    "push",
                    &format!(
                        "--force-with-lease={REFINE_STATE_REF}:{}",
                        preview.remote_state_head
                    ),
                    &remote,
                    &format!("{target_ref}:{REFINE_STATE_REF}"),
                ])?;
            }
            let published = self
                .remote_state_head(&remote)?
                .ok_or_else(|| stale_recovery("the published remote state branch disappeared"))?;
            if published != target_head {
                return Err(stale_recovery(
                    "the remote state branch did not retain the owned recovery target",
                ));
            }
            self.fetch_state_branch(&remote)?;
            let setup = self.ensure_state_worktree(&remote, true, &live_refine)?;
            self.git_at_checked(&setup.path, &["reset", "--hard", &target_ref])?;
            manifest.stage = StateRecoveryStage::Published;
            manifest.message =
                "Owned recovery target published with remote-head compare-and-swap.".to_string();
            write_manifest(&manifest_path, &manifest)?;
        }

        #[cfg(test)]
        run_after_recovery_authority_hook(&self.target_root);

        let target = self.observe_repository_ref(&target_ref)?;
        let target_refine = target.path.join(".refine");
        let target_state = durable_state_map(&target_refine)?;
        if state_map_digest(&target_state) != target_snapshot {
            return Err(RefineError::Conflict(
                "Owned recovery target no longer matches its manifest snapshot.".to_string(),
            ));
        }
        if manifest.stage < StateRecoveryStage::Hydrated {
            let concurrent =
                hydrate_recovery_target_with_cas(&preserved_refine, &target_refine, &live_refine)?;
            manifest.stage = StateRecoveryStage::Hydrated;
            manifest.message = if concurrent {
                "Recovery target hydrated; later live writes were preserved as the next local delta."
                    .to_string()
            } else {
                "Recovery target hydrated under record leases and preimage fences.".to_string()
            };
            write_manifest(&manifest_path, &manifest)?;
        }

        verify_owned_heads(self, &remote, &target_head)?;
        if manifest.stage < StateRecoveryStage::BaselineCreated {
            self.save_state_baseline(&target_state)?;
            #[cfg(test)]
            if run_after_recovery_baseline_hook(&self.target_root) {
                return Err(RefineError::Io(TEST_BASELINE_INTERRUPTION.to_string()));
            }
            manifest.stage = StateRecoveryStage::BaselineCreated;
            manifest.message =
                "Owned target is published and hydrated; baseline created.".to_string();
            write_manifest(&manifest_path, &manifest)?;
        }
        manifest.stage = StateRecoveryStage::Completed;
        manifest.completed_at = Some(recovery_timestamp());
        manifest.outcome = StateRecoveryOutcome::Succeeded;
        manifest.message =
            "Recovery completed; later live writes, if any, remain the next local delta."
                .to_string();
        write_manifest(&manifest_path, &manifest)?;
        reported_result(preview, decision, &manifest_path, manifest)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_reported_recovery_target(
        &self,
        preview: &StateRecoveryPreview,
        decision: &StateRecoveryDecision,
        overrides: &BTreeMap<String, StateRecoveryAuthority>,
        remote_preview: &DisposableCheckout,
        preserved_refine: &std::path::Path,
        baseline: &DurableStateMap,
        original_live: &DurableStateMap,
        remote_state: &DurableStateMap,
        resolved: &BTreeMap<PathBuf, Vec<u8>>,
        target_ref: &str,
    ) -> RefineResult<(String, String)> {
        let target = self.disposable_checkout("recovery-target")?;
        let source = remote_preview.path.display().to_string();
        let destination = target.path.display().to_string();
        self.git_checked(&[
            "clone",
            "-q",
            "--no-checkout",
            "--no-local",
            "--",
            &source,
            &destination,
        ])?;
        self.git_at_checked(
            &target.path,
            &["checkout", "-q", "--detach", &preview.remote_state_head],
        )?;
        self.git_at_checked(&target.path, &["config", "user.email", "refine@localhost"])?;
        self.git_at_checked(
            &target.path,
            &["config", "user.name", "Refine State Recovery"],
        )?;
        let target_refine = target.path.join(".refine");
        for (relative, bytes) in resolved {
            write_state_file_bytes(&target_refine.join(relative), bytes)?;
        }
        let resolved_paths = resolved.keys().cloned().collect::<BTreeSet<_>>();
        let conflicts = preview
            .conflicting_paths
            .iter()
            .map(PathBuf::from)
            .collect::<BTreeSet<_>>();
        let paths = baseline
            .keys()
            .chain(original_live.keys())
            .chain(remote_state.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for relative in paths {
            if resolved_paths.contains(&relative) {
                continue;
            }
            let normalized = relative.to_string_lossy().replace('\\', "/");
            let authority = if conflicts.contains(&relative) {
                overrides
                    .get(&normalized)
                    .copied()
                    .unwrap_or(decision.default_authority)
            } else if original_live.get(&relative) != baseline.get(&relative)
                && remote_state.get(&relative) == baseline.get(&relative)
            {
                StateRecoveryAuthority::Live
            } else {
                StateRecoveryAuthority::Remote
            };
            if authority == StateRecoveryAuthority::Live {
                replace_target_path(preserved_refine, &target_refine, &relative, original_live)?;
            }
        }
        self.git_at_checked(&target.path, &["add", "-f", "-A", "--", ".refine"])?;
        self.git_at_checked(
            &target.path,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "Recover conflicted Refine state",
                "-m",
                &format!("Recovery evidence: {}", preview.evidence_id),
            ],
        )?;
        let target_head = self.git_at_stdout(&target.path, &["rev-parse", "HEAD"])?;
        let target_state = durable_state_map(&target_refine)?;
        let target_snapshot = state_map_digest(&target_state);
        let source_path = target.path.display().to_string();
        if self.git_success(&["show-ref", "--verify", "--quiet", target_ref])? {
            let existing_head = self.git_stdout(&["rev-parse", target_ref])?;
            validate_owned_target(self, target_ref, &existing_head, &target_snapshot)?;
            return Ok((existing_head, target_snapshot));
        } else {
            self.git_checked(&[
                "fetch",
                "--no-tags",
                &source_path,
                &format!("HEAD:{target_ref}"),
            ])?;
        }
        Ok((target_head, target_snapshot))
    }

    fn finalize_reported_recovery(
        &self,
        preview: &StateRecoveryPreview,
        decision: &StateRecoveryDecision,
        remote: &str,
        manifest_path: &std::path::Path,
        manifest: Option<&StateRecoveryManifest>,
        baseline: &DurableStateMap,
    ) -> RefineResult<StateRecoveryResult> {
        let Some(manifest) = manifest else {
            return Err(RefineError::Conflict(
                "A changed baseline has no matching recovery manifest.".to_string(),
            ));
        };
        let target_head = manifest.local_state_head_after.as_deref().ok_or_else(|| {
            RefineError::Conflict("Owned recovery manifest has no target head.".to_string())
        })?;
        let target_snapshot = manifest.target_snapshot.as_deref().ok_or_else(|| {
            RefineError::Conflict("Owned recovery manifest has no target snapshot.".to_string())
        })?;
        if state_map_digest(baseline) != target_snapshot {
            return Err(RefineError::Conflict(
                "The synchronization baseline does not match the owned recovery target."
                    .to_string(),
            ));
        }
        verify_owned_heads(self, remote, target_head)?;
        validate_owned_target(
            self,
            manifest.target_location.as_deref().unwrap_or_default(),
            target_head,
            target_snapshot,
        )?;
        let mut completed = manifest.clone();
        completed.stage = StateRecoveryStage::Completed;
        completed.completed_at = Some(recovery_timestamp());
        completed.outcome = StateRecoveryOutcome::Succeeded;
        completed.message = "Recovery completed from its owned baseline boundary.".to_string();
        write_manifest(manifest_path, &completed)?;
        reported_result(preview, decision, manifest_path, completed)
    }

    fn reported_recovery_manifest_path(
        &self,
        preview: &StateRecoveryPreview,
    ) -> RefineResult<PathBuf> {
        Ok(git_common_dir(&self.target_root)?
            .join(RECOVERY_MANIFEST_DIR)
            .join(format!("{}-reported.json", preview.evidence_id)))
    }

    fn reported_source_ref(&self, preview: &StateRecoveryPreview) -> String {
        format!(
            "{RECOVERY_REF_PREFIX}/{}-source",
            &preview.evidence_id[..preview.evidence_id.len().min(24)]
        )
    }

    fn reported_target_ref(&self, preview: &StateRecoveryPreview, decision_id: &str) -> String {
        format!(
            "{RECOVERY_TARGET_REF_PREFIX}/{}-{}",
            &preview.evidence_id[..preview.evidence_id.len().min(24)],
            &decision_id[..decision_id.len().min(24)]
        )
    }
}
