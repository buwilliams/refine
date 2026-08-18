use super::*;

use crate::application::persistence_sync::state_merge::{merge_added_state_file, merge_state_file};
use crate::infrastructure::git::ancestry::{Ancestry, classify};

impl FileGitSyncService {
    /// Read-only preview: classify the current heads and summarize what a
    /// sync (or a terminal recovery) would find — divergence shape, per-path
    /// sides, and domain summaries for genuinely contested paths. Writes no
    /// artifact files and is never an apply token.
    pub fn preview_state_recovery(&self) -> RefineResult<StateRecoveryPreview> {
        with_repository_git_lock(&self.target_root, || self.preview_state_recovery_locked())
    }

    fn preview_state_recovery_locked(&self) -> RefineResult<StateRecoveryPreview> {
        self.validate_recovery_target()?;
        let live_refine =
            crate::infrastructure::storage::project_layout::prepare_refine_dir(&self.target_root)?;
        let remote = self.configured_remote(&live_refine)?;
        if !self.remote_exists(&remote)? {
            return Err(RefineError::Conflict(format!(
                "Configured Git remote {remote} is unavailable; recovery never accepts a remote override."
            )));
        }
        let live = durable_state_map(&live_refine)?;
        let local_head = self.local_state_head()?;
        if !self.remote_state_exists(&remote)? {
            let detail = format!(
                "Remote {remote} has no {REFINE_STATE_BRANCH} branch; the next sync publishes it."
            );
            return Ok(StateRecoveryPreview {
                version: 2,
                configured_remote: remote,
                local_state_head: local_head,
                remote_state_head: None,
                merge_base: None,
                ancestry: "remote_missing".to_string(),
                live_pending_paths: Vec::new(),
                local_paths: Vec::new(),
                remote_paths: Vec::new(),
                resolvable_paths: Vec::new(),
                conflicts: Vec::new(),
                decision_question: None,
                detail,
            });
        }
        self.fetch_state_branch(&remote)?;
        let remote_head =
            self.git_stdout(&["rev-parse", &format!("{remote}/{REFINE_STATE_BRANCH}")])?;

        let Some(local_head) = local_head else {
            return self.preview_join(remote, remote_head, &live, &live_refine);
        };

        let live_pending_paths = self.live_pending_paths(&local_head, &live)?;
        let mut preview = StateRecoveryPreview {
            version: 2,
            configured_remote: remote,
            local_state_head: Some(local_head.clone()),
            remote_state_head: Some(remote_head.clone()),
            merge_base: None,
            ancestry: String::new(),
            live_pending_paths,
            local_paths: Vec::new(),
            remote_paths: Vec::new(),
            resolvable_paths: Vec::new(),
            conflicts: Vec::new(),
            decision_question: None,
            detail: String::new(),
        };
        match classify(self, &local_head, &remote_head)? {
            Ancestry::Equal => {
                preview.ancestry = "converged".to_string();
                preview.detail = "Local and remote state heads are equal.".to_string();
            }
            Ancestry::FastForwardToA => {
                preview.ancestry = "local_ahead".to_string();
                preview.local_paths = self.state_paths_changed(&remote_head, &local_head)?;
                preview.detail =
                    "The remote head is an ancestor of local work; sync publishes without merging."
                        .to_string();
            }
            Ancestry::FastForwardToB => {
                preview.ancestry = "remote_ahead".to_string();
                preview.remote_paths = self.state_paths_changed(&local_head, &remote_head)?;
                preview.detail =
                    "Local work is an ancestor of the remote head; sync fast-forwards and hydrates."
                        .to_string();
            }
            Ancestry::Diverged { merge_base } => {
                self.preview_diverged(&mut preview, &merge_base, &local_head, &remote_head)?;
            }
            Ancestry::Unrelated => {
                // Independent bootstraps: preview the empty-tree merge that
                // sync uses to join the histories.
                let empty_tree =
                    crate::infrastructure::git::merge::empty_tree_id(self, &self.target_root)?;
                self.preview_diverged(&mut preview, &empty_tree, &local_head, &remote_head)?;
                preview.ancestry = "unrelated".to_string();
                preview.merge_base = None;
                preview.detail = format!(
                    "Heads share no common ancestor (independent bootstraps): {} path(s) contested, {} provable, {} one-sided; sync joins the histories with a merge commit.",
                    preview.conflicts.len(),
                    preview.resolvable_paths.len(),
                    preview.local_paths.len() + preview.remote_paths.len()
                );
            }
        }
        preview.decision_question =
            self.escalated_decision_question(&remote_head, &preview.conflicts);
        Ok(preview)
    }

    fn preview_diverged(
        &self,
        preview: &mut StateRecoveryPreview,
        merge_base: &str,
        local_head: &str,
        remote_head: &str,
    ) -> RefineResult<()> {
        preview.ancestry = "diverged".to_string();
        preview.merge_base = Some(merge_base.to_string());
        let local_changed = self
            .state_paths_changed(merge_base, local_head)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let remote_changed = self
            .state_paths_changed(merge_base, remote_head)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for path in local_changed.union(&remote_changed) {
            let relative = PathBuf::from(path);
            match (local_changed.contains(path), remote_changed.contains(path)) {
                (true, false) => preview.local_paths.push(path.clone()),
                (false, true) => preview.remote_paths.push(path.clone()),
                _ => {
                    let tree_path = format!(".refine/{path}");
                    let base = self.state_bytes_at(&self.target_root, merge_base, &tree_path)?;
                    let local = self.state_bytes_at(&self.target_root, local_head, &tree_path)?;
                    let remote = self.state_bytes_at(&self.target_root, remote_head, &tree_path)?;
                    let provable = local == remote
                        || matches!(
                            (&base, &local, &remote),
                            (Some(base), Some(local), Some(remote))
                                if merge_state_file(&relative, base, local, remote).is_some()
                        )
                        || matches!(
                            (&base, &local, &remote),
                            (None, Some(local), Some(remote))
                                if merge_added_state_file(&relative, local, remote).is_some()
                        );
                    if provable {
                        preview.resolvable_paths.push(path.clone());
                    } else {
                        preview.conflicts.push(StateSyncConflictPath {
                            path: path.clone(),
                            summary: conflict_path_summary(
                                &relative,
                                base.as_deref(),
                                local.as_deref(),
                                remote.as_deref(),
                            ),
                        });
                    }
                }
            }
        }
        preview.detail = format!(
            "Heads diverged from merge base {}: {} path(s) contested, {} provable, {} one-sided.",
            &merge_base[..merge_base.len().min(12)],
            preview.conflicts.len(),
            preview.resolvable_paths.len(),
            preview.local_paths.len() + preview.remote_paths.len()
        );
        Ok(())
    }

    /// First contact: no local branch, so the comparison is live versus the
    /// remote tree and there is no merge base to arbitrate with.
    fn preview_join(
        &self,
        remote: String,
        remote_head: String,
        live: &DurableStateMap,
        live_refine: &std::path::Path,
    ) -> RefineResult<StateRecoveryPreview> {
        let remote_paths = self.state_tree_paths(&remote_head)?;
        let mut preview = StateRecoveryPreview {
            version: 2,
            configured_remote: remote,
            local_state_head: None,
            remote_state_head: Some(remote_head.clone()),
            merge_base: None,
            ancestry: "join".to_string(),
            live_pending_paths: Vec::new(),
            local_paths: Vec::new(),
            remote_paths: Vec::new(),
            resolvable_paths: Vec::new(),
            conflicts: Vec::new(),
            decision_question: None,
            detail: String::new(),
        };
        let live_keys = live
            .keys()
            .filter(|path| !is_excluded_from_durable_state(path))
            .cloned()
            .collect::<BTreeSet<_>>();
        for relative in live_keys.union(&remote_paths) {
            let path = relative.to_string_lossy().replace('\\', "/");
            match (live.get(relative), remote_paths.contains(relative)) {
                (Some(_), false) => preview.local_paths.push(path),
                (None, true) => preview.remote_paths.push(path),
                (Some(fingerprint), true) => {
                    let remote_bytes = self.state_bytes_at(
                        &self.target_root,
                        &remote_head,
                        &format!(".refine/{path}"),
                    )?;
                    if remote_bytes.as_deref().map(state_content_fingerprint) == Some(*fingerprint)
                    {
                        continue;
                    }
                    let live_bytes = fs::read(live_refine.join(relative)).ok();
                    preview.conflicts.push(StateSyncConflictPath {
                        path,
                        summary: conflict_path_summary(
                            relative,
                            None,
                            live_bytes.as_deref(),
                            remote_bytes.as_deref(),
                        ),
                    });
                }
                (None, false) => unreachable!(),
            }
        }
        preview.detail = if bootstrap_only_state(live) {
            "First contact with bootstrap-only live state; the next sync adopts the remote branch."
                .to_string()
        } else if preview.conflicts.is_empty() {
            "First contact without contested content; the next sync joins additively.".to_string()
        } else {
            format!(
                "First contact with {} contested path(s) and no merge base; an authority decision settles the join.",
                preview.conflicts.len()
            )
        };
        preview.decision_question =
            self.escalated_decision_question(&remote_head, &preview.conflicts);
        Ok(preview)
    }

    /// Live records whose bytes differ from the local branch head: the next
    /// pass's local delta.
    fn live_pending_paths(
        &self,
        local_head: &str,
        live: &DurableStateMap,
    ) -> RefineResult<Vec<String>> {
        let tree = self.state_tree_paths(local_head)?;
        let mut pending = Vec::new();
        for relative in live
            .keys()
            .filter(|path| !is_excluded_from_durable_state(path))
            .cloned()
            .collect::<BTreeSet<_>>()
            .union(&tree)
        {
            let path = relative.to_string_lossy().replace('\\', "/");
            let committed = self
                .state_bytes_at(&self.target_root, local_head, &format!(".refine/{path}"))?
                .as_deref()
                .map(state_content_fingerprint);
            if live.get(relative).copied() != committed {
                pending.push(path);
            }
        }
        Ok(pending)
    }

    fn state_tree_paths(&self, commit: &str) -> RefineResult<BTreeSet<PathBuf>> {
        Ok(self
            .git_stdout(&["ls-tree", "-r", "--name-only", commit, "--", ".refine"])?
            .lines()
            .filter_map(|path| path.strip_prefix(".refine/"))
            .map(PathBuf::from)
            .filter(|path| !is_excluded_from_durable_state(path))
            .collect())
    }

    /// State-relative paths whose content differs between two commits,
    /// excluding paths that never belong in durable state.
    fn state_paths_changed(&self, from: &str, to: &str) -> RefineResult<Vec<String>> {
        Ok(self
            .git_stdout(&["diff", "--name-only", from, to, "--", ".refine"])?
            .lines()
            .filter_map(|path| path.strip_prefix(".refine/"))
            .filter(|path| !is_excluded_from_durable_state(std::path::Path::new(path)))
            .map(str::to_string)
            .collect())
    }

    pub(crate) fn validate_recovery_target(&self) -> RefineResult<()> {
        if !self.target_root.join(".git").exists()
            || !self.git_success(&["rev-parse", "--is-inside-work-tree"])?
        {
            return Err(RefineError::InvalidInput(
                "State recovery requires a Git target app worktree.".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn local_state_head(&self) -> RefineResult<Option<String>> {
        if !self.git_success(&["show-ref", "--verify", "--quiet", REFINE_STATE_REF])? {
            return Ok(None);
        }
        self.git_stdout(&["rev-parse", REFINE_STATE_REF]).map(Some)
    }
}
