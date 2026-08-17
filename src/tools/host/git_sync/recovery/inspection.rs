use super::*;

impl FileGitSyncService {
    /// Inspect missing-baseline or reported valid-baseline recovery without
    /// changing any target repository ref, index, worktree, live state,
    /// baseline, or remote.
    pub fn preview_state_recovery(&self) -> RefineResult<StateRecoveryPreview> {
        with_repository_git_lock(&self.target_root, || self.preview_state_recovery_locked())
    }

    fn preview_state_recovery_locked(&self) -> RefineResult<StateRecoveryPreview> {
        self.validate_recovery_target()?;
        let live_refine =
            crate::tools::host::project_layout::refine_dir_for_target_root(&self.target_root)?;
        if !live_refine.is_dir() {
            return Err(RefineError::Conflict(
                "State recovery requires an existing live Refine state store.".to_string(),
            ));
        }
        if self.load_state_baseline()?.is_some() {
            return self.preview_reported_state_recovery();
        }
        let live = durable_state_map(&live_refine)?;
        #[cfg(test)]
        run_during_recovery_preview_hook(&self.target_root);
        if live.is_empty() || bootstrap_only_state(&live) {
            return Err(RefineError::Conflict(
                "State recovery requires non-bootstrap live Refine state.".to_string(),
            ));
        }
        let remote = self.configured_remote(&live_refine)?;
        if !self.remote_exists(&remote)? {
            return Err(RefineError::Conflict(format!(
                "Configured Git remote {remote} is unavailable; recovery never accepts a remote override."
            )));
        }
        let remote_head = self.remote_state_head(&remote)?.ok_or_else(|| {
            RefineError::Conflict(format!(
                "Configured Git remote {remote} has no {REFINE_STATE_BRANCH} branch."
            ))
        })?;
        let observation = self.observe_remote_state(&remote, &remote_head)?;
        let remote_state = durable_state_map(&observation.path.join(".refine"))?;
        let path_counts = recovery_path_counts(&live, &remote_state);
        let all_conflicts = recovery_conflicting_paths(&live, &remote_state);
        let conflicting_paths = all_conflicts
            .iter()
            .take(RECOVERY_PATH_LIMIT)
            .cloned()
            .collect::<Vec<_>>();
        let conflicting_paths_truncated = all_conflicts.len() - conflicting_paths.len();
        let target_identity = self.target_identity()?;
        let repository_identity = self.repository_identity(&remote)?;
        let local_state_head = self.local_state_head()?;
        let mut preview = StateRecoveryPreview {
            version: 1,
            evidence_id: String::new(),
            target_identity,
            repository_identity,
            configured_remote: remote,
            local_state_head,
            remote_state_head: remote_head,
            baseline_status: "missing".to_string(),
            baseline_snapshot: None,
            conflict_report_id: None,
            conflict_report_location: None,
            live_snapshot: state_tree_digest(&live_refine, &live)?,
            live_fingerprints: recovery_fingerprints(&live),
            remote_snapshot: state_tree_digest(&observation.path.join(".refine"), &remote_state)?,
            path_counts,
            conflicting_paths,
            conflicting_paths_truncated,
        };
        preview.evidence_id = preview_evidence_id(&preview)?;
        Ok(preview)
    }

    pub(super) fn validate_recovery_target(&self) -> RefineResult<()> {
        if !self.target_root.join(".git").exists()
            || !self.git_success(&["rev-parse", "--is-inside-work-tree"])?
        {
            return Err(RefineError::InvalidInput(
                "State recovery requires a Git target app worktree.".to_string(),
            ));
        }
        Ok(())
    }

    pub(super) fn target_identity(&self) -> RefineResult<String> {
        self.target_root
            .canonicalize()
            .map(|path| path.display().to_string())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to resolve target identity {}: {error}",
                    self.target_root.display()
                ))
            })
    }

    pub(super) fn repository_identity(&self, remote: &str) -> RefineResult<String> {
        use sha2::{Digest, Sha256};

        let common = git_common_dir(&self.target_root)?
            .canonicalize()
            .map_err(|error| RefineError::Io(format!("failed to resolve Git identity: {error}")))?;
        let url = self.git_stdout(&["remote", "get-url", remote])?;
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(format!("{}\0{url}", common.display()).as_bytes())
        ))
    }

    pub(in crate::tools::host::git_sync) fn local_state_head(
        &self,
    ) -> RefineResult<Option<String>> {
        if !self.git_success(&["show-ref", "--verify", "--quiet", REFINE_STATE_REF])? {
            return Ok(None);
        }
        self.git_stdout(&["rev-parse", REFINE_STATE_REF]).map(Some)
    }

    pub(super) fn remote_state_head(&self, remote: &str) -> RefineResult<Option<String>> {
        let output = self.git(&["ls-remote", "--heads", remote, REFINE_STATE_REF])?;
        if !output.success {
            return Err(command_failed("git ls-remote", &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .map(str::to_string))
    }

    pub(super) fn observe_remote_state(
        &self,
        remote: &str,
        expected_head: &str,
    ) -> RefineResult<DisposableCheckout> {
        let url = self.git_stdout(&["remote", "get-url", remote])?;
        let checkout = self.disposable_checkout("remote-observation")?;
        let path = checkout.path.display().to_string();
        self.git_checked(&[
            "clone",
            "-q",
            "--single-branch",
            "--branch",
            REFINE_STATE_BRANCH,
            "--",
            &url,
            &path,
        ])?;
        let observed = self.git_at_stdout(&checkout.path, &["rev-parse", "HEAD"])?;
        if observed != expected_head {
            return Err(stale_recovery(
                "the remote state head changed during disposable observation",
            ));
        }
        Ok(checkout)
    }

    pub(super) fn disposable_checkout(&self, label: &str) -> RefineResult<DisposableCheckout> {
        let path = std::env::temp_dir().join(format!(
            "refine-state-recovery-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        if path.exists() {
            return Err(RefineError::Conflict(format!(
                "disposable recovery path already exists: {}",
                path.display()
            )));
        }
        Ok(DisposableCheckout { path })
    }
}

pub(super) fn recovery_fingerprints(state: &DurableStateMap) -> BTreeMap<String, u64> {
    state
        .iter()
        .map(|(path, fingerprint)| (path.to_string_lossy().replace('\\', "/"), *fingerprint))
        .collect()
}

pub(super) fn stale_live_snapshot_reason(
    _live_refine: &std::path::Path,
    expected: &BTreeMap<String, u64>,
    current: &DurableStateMap,
) -> String {
    let current = recovery_fingerprints(current);
    let changed = expected
        .keys()
        .chain(current.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| expected.get(path) != current.get(path))
        .take(8)
        .collect::<Vec<_>>();
    if changed.is_empty() {
        return "the live state snapshot changed while it was being read".to_string();
    }
    format!("the live state snapshot changed at {}", changed.join(", "))
}

pub(super) fn recovery_path_counts(
    live: &DurableStateMap,
    remote: &DurableStateMap,
) -> StateRecoveryPathCounts {
    let mut counts = StateRecoveryPathCounts::default();
    for path in live.keys().chain(remote.keys()).collect::<BTreeSet<_>>() {
        match (live.get(path), remote.get(path)) {
            (Some(_), None) => counts.live_only += 1,
            (None, Some(_)) => counts.remote_only += 1,
            (Some(left), Some(right)) if left == right => counts.equal += 1,
            (Some(_), Some(_)) => counts.differing += 1,
            (None, None) => unreachable!(),
        }
    }
    counts
}

pub(super) fn recovery_conflicting_paths(
    live: &DurableStateMap,
    remote: &DurableStateMap,
) -> Vec<String> {
    live.keys()
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| live.get(path) != remote.get(path))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

pub(super) fn validate_recovery_comparison(
    preview: &StateRecoveryPreview,
    live: &DurableStateMap,
    remote: &DurableStateMap,
) -> RefineResult<()> {
    let path_counts = recovery_path_counts(live, remote);
    let all_conflicts = recovery_conflicting_paths(live, remote);
    let conflicting_paths = all_conflicts
        .iter()
        .take(RECOVERY_PATH_LIMIT)
        .cloned()
        .collect::<Vec<_>>();
    let conflicting_paths_truncated = all_conflicts.len() - conflicting_paths.len();
    if preview.path_counts != path_counts
        || preview.conflicting_paths != conflicting_paths
        || preview.conflicting_paths_truncated != conflicting_paths_truncated
    {
        return Err(stale_recovery(
            "the bounded path comparison does not match the observed snapshots",
        ));
    }
    Ok(())
}
