use super::*;

impl FileGitWorktreeService {
    pub fn remote_exists(&self, remote: &str) -> RefineResult<bool> {
        if remote.trim().is_empty() {
            return Ok(false);
        }
        Ok(self.git_raw(&["remote", "get-url", remote])?.success)
    }

    pub fn fetch_branch(&self, remote: &str, branch: &str) -> RefineResult<()> {
        validate_branch_name(branch)?;
        if !self.remote_exists(remote)? {
            return Err(RefineError::NotFound(format!(
                "Git remote {remote} was not found"
            )));
        }
        self.git_output(&["fetch", remote, branch])?;
        self.audit(
            "branch_fetch",
            "ok",
            json!({"remote": remote, "branch": branch}),
        )
    }

    pub fn remote_branch_commit(&self, remote: &str, branch: &str) -> RefineResult<Option<String>> {
        validate_branch_name(branch)?;
        if !self.remote_exists(remote)? {
            return Ok(None);
        }
        let reference = format!("refs/heads/{branch}");
        let output = stdout(self.git_output(&["ls-remote", "--refs", remote, &reference])?)?;
        Ok(output
            .split_whitespace()
            .next()
            .map(str::to_string)
            .filter(|commit| !commit.is_empty()))
    }

    /// Inspect the configured target and every Refine-owned remote branch from
    /// one remote advertisement. Callers can therefore reason about one exact
    /// remote snapshot without creating local tracking refs.
    pub fn remote_refine_ref_snapshot(
        &self,
        remote: &str,
        target_branch: &str,
    ) -> RefineResult<GitRemoteRefSnapshot> {
        validate_branch_name(target_branch)?;
        if !self.remote_exists(remote)? {
            return Err(RefineError::NotFound(format!(
                "Git remote {remote} was not found"
            )));
        }
        let target_ref = format!("refs/heads/{target_branch}");
        let output = stdout(self.git_output(&[
            "ls-remote",
            "--refs",
            remote,
            &target_ref,
            "refs/heads/refine/*",
        ])?)?;
        let mut target_commit = None;
        let mut refine_branches = Vec::new();
        for line in output.lines() {
            let mut fields = line.split_whitespace();
            let Some(commit) = fields.next() else {
                continue;
            };
            let Some(reference) = fields.next() else {
                continue;
            };
            if reference == target_ref {
                target_commit = Some(commit.to_string());
            } else if let Some(branch) = reference.strip_prefix("refs/heads/refine/") {
                refine_branches.push(GitRemoteRef {
                    branch: format!("refine/{branch}"),
                    commit: commit.to_string(),
                });
            }
        }
        refine_branches.sort_by(|left, right| left.branch.cmp(&right.branch));
        Ok(GitRemoteRefSnapshot {
            target_commit,
            refine_branches,
        })
    }

    /// Fetch exact advertised commits without writing FETCH_HEAD or a tracking
    /// ref. This makes remote-only tips available for ancestry inspection while
    /// retaining the advertised SHA as the deletion authority.
    pub fn fetch_exact_commits(&self, remote: &str, commits: &[String]) -> RefineResult<()> {
        if commits.is_empty() {
            return Ok(());
        }
        if !self.remote_exists(remote)? {
            return Err(RefineError::NotFound(format!(
                "Git remote {remote} was not found"
            )));
        }
        let mut unique = commits.to_vec();
        unique.sort();
        unique.dedup();
        for commit in &unique {
            validate_commitish(commit)?;
        }
        let mut args = vec!["fetch", "--no-tags", "--no-write-fetch-head", remote];
        args.extend(unique.iter().map(String::as_str));
        self.git_output(&args)?;
        self.audit(
            "exact_commits_fetch",
            "ok",
            json!({"remote": remote, "commits": unique}),
        )
    }

    /// Delete only the exact Refine-owned remote ref previously inspected.
    ///
    /// The force-with-lease is a compare-and-delete fence, not a history
    /// rewrite: if another process advances the branch, Git rejects deletion.
    pub fn delete_remote_branch_if_matches(
        &self,
        remote: &str,
        branch: &str,
        expected_commit: &str,
    ) -> RefineResult<()> {
        validate_branch_name(branch)?;
        validate_commitish(expected_commit)?;
        let reference = format!("refs/heads/{branch}");
        let lease = format!("--force-with-lease={reference}:{expected_commit}");
        let deletion = format!(":{reference}");
        self.git_output(&["push", &lease, remote, &deletion])?;
        self.audit(
            "remote_branch_delete",
            "ok",
            json!({
                "remote": remote,
                "branch": branch,
                "expected_commit": expected_commit,
                "exact_sha_fence": true
            }),
        )
    }

    /// Atomically compare both advertised refs and delete only the branch.
    ///
    /// The target refspec is intentionally a no-op when the snapshot still
    /// matches. If the target moved, it becomes a rejected stale lease instead
    /// of silently validating ancestry against an obsolete target.
    pub fn delete_remote_branch_if_snapshot_matches(
        &self,
        remote: &str,
        branch: &str,
        expected_branch_commit: &str,
        target_branch: &str,
        expected_target_commit: &str,
    ) -> RefineResult<GitRemoteRefDeleteOutcome> {
        validate_branch_name(branch)?;
        validate_branch_name(target_branch)?;
        validate_commitish(expected_branch_commit)?;
        validate_commitish(expected_target_commit)?;
        if branch == target_branch {
            return Err(RefineError::InvalidInput(
                "cleanup branch and merge target must be distinct".to_string(),
            ));
        }
        let snapshot = self.remote_refine_ref_snapshot(remote, target_branch)?;
        if snapshot.target_commit.as_deref() != Some(expected_target_commit) {
            return Ok(GitRemoteRefDeleteOutcome::TargetChanged);
        }
        if snapshot
            .refine_branches
            .iter()
            .find(|candidate| candidate.branch == branch)
            .map(|candidate| candidate.commit.as_str())
            != Some(expected_branch_commit)
        {
            return Ok(GitRemoteRefDeleteOutcome::BranchChanged);
        }

        let branch_ref = format!("refs/heads/{branch}");
        let target_ref = format!("refs/heads/{target_branch}");
        let branch_lease = format!("--force-with-lease={branch_ref}:{expected_branch_commit}");
        let target_lease = format!("--force-with-lease={target_ref}:{expected_target_commit}");
        let deletion = format!(":{branch_ref}");
        let target_noop = format!("{expected_target_commit}:{target_ref}");
        let output = self.git_raw(&[
            "push",
            "--atomic",
            &branch_lease,
            &target_lease,
            remote,
            &deletion,
            &target_noop,
        ])?;
        if output.success {
            self.audit(
                "remote_branch_delete",
                "ok",
                json!({
                    "remote": remote,
                    "branch": branch,
                    "expected_commit": expected_branch_commit,
                    "target_branch": target_branch,
                    "expected_target_commit": expected_target_commit,
                    "exact_sha_fence": true,
                    "atomic_target_fence": true
                }),
            )?;
            return Ok(GitRemoteRefDeleteOutcome::Deleted);
        }

        let message = trimmed_command_text(&output);
        let current = self.remote_refine_ref_snapshot(remote, target_branch)?;
        if current.target_commit.as_deref() != Some(expected_target_commit) {
            return Ok(GitRemoteRefDeleteOutcome::TargetChanged);
        }
        if current
            .refine_branches
            .iter()
            .find(|candidate| candidate.branch == branch)
            .map(|candidate| candidate.commit.as_str())
            != Some(expected_branch_commit)
        {
            return Ok(GitRemoteRefDeleteOutcome::BranchChanged);
        }
        let lower = message.to_ascii_lowercase();
        if lower.contains("does not support --atomic")
            || lower.contains("does not support atomic")
            || lower.contains("atomic push is not supported")
        {
            return Ok(GitRemoteRefDeleteOutcome::AtomicUnsupported);
        }
        Err(RefineError::Conflict(message))
    }

    pub fn fast_forward_from_remote(&self, remote: &str, branch: &str) -> RefineResult<()> {
        self.fetch_branch(remote, branch)?;
        let remote_branch = format!("{remote}/{branch}");
        self.git_output(&["merge", "--ff-only", &remote_branch])?;
        self.audit(
            "branch_fast_forward",
            "ok",
            json!({"remote": remote, "branch": branch}),
        )
    }

    pub fn resolve_commit(&self, commitish: &str) -> RefineResult<String> {
        validate_commitish(commitish)?;
        stdout(self.git_output(&["rev-parse", "--verify", &format!("{commitish}^{{commit}}")])?)
            .map(|value| value.trim().to_string())
    }

    pub fn commit_is_ancestor(&self, ancestor: &str, descendant: &str) -> RefineResult<bool> {
        validate_commitish(ancestor)?;
        validate_commitish(descendant)?;
        let output = self.git_raw(&["merge-base", "--is-ancestor", ancestor, descendant])?;
        match output.exit_code {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            exit_code => Err(RefineError::Conflict(format!(
                "Git ancestry inspection failed with exit code {exit_code:?}: {}",
                trimmed_command_text(&output)
            ))),
        }
    }

    pub fn commit_timestamp(&self, commit: &str) -> RefineResult<DateTime<Utc>> {
        validate_commitish(commit)?;
        let value = stdout(self.git_output(&["show", "-s", "--format=%cI", commit])?)?;
        DateTime::parse_from_rfc3339(value.trim())
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "Git commit {commit} has an invalid timestamp: {error}"
                ))
            })
    }

    pub fn commit_parents(&self, commit: &str) -> RefineResult<Vec<String>> {
        validate_commitish(commit)?;
        let output = stdout(self.git_output(&["rev-list", "--parents", "-n", "1", commit])?)?;
        Ok(output
            .split_whitespace()
            .skip(1)
            .map(ToString::to_string)
            .collect())
    }

    /// Prove that a base-to-candidate delta contains no merge commits.
    ///
    /// A linear range can be replayed by rebase without choosing an implicit merge mainline.
    pub fn commit_range_is_linear(&self, base: &str, candidate: &str) -> RefineResult<bool> {
        validate_commitish(base)?;
        validate_commitish(candidate)?;
        let range = format!("{base}..{candidate}");
        let output = stdout(self.git_output(&["rev-list", "--merges", "--count", &range])?)?;
        output
            .trim()
            .parse::<usize>()
            .map(|count| count == 0)
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "Git returned an invalid merge count for {range}: {error}"
                ))
            })
    }

    /// Return the paths changed by `commit` relative to the merge base with
    /// `base`. Keeping this query in the Git capability lets workflow policy
    /// classify the committed candidate without bypassing the shared command
    /// and repository ownership boundary.
    pub fn changed_paths_since(&self, base: &str, commit: &str) -> RefineResult<Vec<String>> {
        validate_commitish(base)?;
        validate_commitish(commit)?;
        let range = format!("{base}...{commit}");
        let output = self.git_output(&["diff", "--name-only", "-z", &range])?;
        Ok(output
            .stdout
            .split(|byte| *byte == b'\0')
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).to_string())
            .collect())
    }

    pub fn merge_commit_no_ff(&self, commit: &str) -> RefineResult<MergeResult> {
        validate_commitish(commit)?;
        let output = self.git_raw(&["merge", "--no-ff", "--no-edit", commit])?;
        if output.success {
            let result = MergeResult {
                ok: true,
                conflicts: Vec::new(),
                message: Some(trimmed_command_text(&output)),
            };
            self.audit(
                "merge_commit_no_ff",
                "ok",
                json!({"commit": commit, "result": &result}),
            )?;
            return Ok(result);
        }
        let result = MergeResult {
            ok: false,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        let _ = self.audit(
            "merge_commit_no_ff",
            "conflict",
            json!({"commit": commit, "result": &result}),
        );
        Ok(result)
    }

    pub fn ensure_branch_from_remote(&self, remote: &str, branch: &str) -> RefineResult<()> {
        validate_branch_name(branch)?;
        if self.branch_exists(branch)? {
            return Ok(());
        }
        if !self.remote_exists(remote)? {
            return Err(RefineError::NotFound(format!(
                "Git remote {remote} was not found"
            )));
        }
        self.fetch_branch(remote, branch)?;
        let remote_branch = format!("{remote}/{branch}");
        self.git_output(&["branch", branch, &remote_branch])?;
        self.audit(
            "branch_fetch",
            "ok",
            json!({"remote": remote, "branch": branch}),
        )
    }

    pub fn revert_commit(&self, commit: &str) -> RefineResult<MergeResult> {
        validate_commitish(commit)?;
        let output = self.git_raw(&["revert", "--no-edit", commit])?;
        if output.success {
            let result = MergeResult {
                ok: true,
                conflicts: Vec::new(),
                message: Some(trimmed_command_text(&output)),
            };
            self.audit("revert", "ok", json!({"commit": commit, "result": &result}))?;
            return Ok(result);
        }
        let conflicts = self.conflicts().unwrap_or_default();
        let result = MergeResult {
            ok: false,
            conflicts,
            message: Some(trimmed_command_text(&output)),
        };
        let _ = self.audit(
            "revert",
            "conflict",
            json!({"commit": commit, "result": &result}),
        );
        Ok(result)
    }

    pub fn revert_merge_commit(&self, commit: &str, mainline: usize) -> RefineResult<MergeResult> {
        validate_commitish(commit)?;
        if mainline == 0 {
            return Err(RefineError::InvalidInput(
                "Git revert mainline must be at least 1".to_string(),
            ));
        }
        let mainline = mainline.to_string();
        let output = self.git_raw(&["revert", "--no-edit", "-m", &mainline, commit])?;
        if output.success {
            let result = MergeResult {
                ok: true,
                conflicts: Vec::new(),
                message: Some(trimmed_command_text(&output)),
            };
            self.audit(
                "revert_merge",
                "ok",
                json!({"commit": commit, "mainline": mainline, "result": &result}),
            )?;
            return Ok(result);
        }
        let result = MergeResult {
            ok: false,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        let _ = self.audit(
            "revert_merge",
            "conflict",
            json!({"commit": commit, "mainline": mainline, "result": &result}),
        );
        Ok(result)
    }

    pub fn reset_hard_to(&self, commit: &str) -> RefineResult<()> {
        validate_commitish(commit)?;
        self.git_output(&["reset", "--hard", commit])?;
        self.audit("reset_hard_to", "ok", json!({"commit": commit}))
    }

    pub fn commit_or_clean_noop_since(
        &self,
        message: &str,
        pathspecs: &[String],
        base_branch: &str,
    ) -> RefineResult<GitCommitOutcome> {
        if let Some(commit) = self.current_clean_commit_since(base_branch)? {
            self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
            return Ok(GitCommitOutcome {
                commit,
                has_changes_since_base: true,
            });
        }
        match self.commit_inner(message, pathspecs, false) {
            Ok(commit) => Ok(GitCommitOutcome {
                commit,
                has_changes_since_base: true,
            }),
            Err(error) if is_nothing_to_commit_error(&error) => {
                if let Some(commit) = self.current_clean_commit_since(base_branch)? {
                    self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
                    Ok(GitCommitOutcome {
                        commit,
                        has_changes_since_base: true,
                    })
                } else if self.is_clean()? {
                    let commit = self.head_commit()?;
                    self.audit(
                        "commit_noop",
                        "ok",
                        json!({
                            "commit": commit,
                            "message": message,
                            "pathspecs": pathspecs,
                            "base_branch": base_branch
                        }),
                    )?;
                    Ok(GitCommitOutcome {
                        commit,
                        has_changes_since_base: false,
                    })
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn audit_existing_commit(
        &self,
        commit: &str,
        message: &str,
        pathspecs: &[String],
        base_branch: &str,
    ) -> RefineResult<()> {
        self.audit(
            "commit_existing",
            "ok",
            json!({
                "commit": commit,
                "message": message,
                "pathspecs": pathspecs,
                "base_branch": base_branch
            }),
        )
    }

    /// True when the service root is inside a valid Git work tree.
    ///
    /// `rev-parse --is-inside-work-tree` prints `false` while still exiting 0
    /// from a path inside a `.git` directory, so the printed value is the
    /// answer, not the exit code.
    pub fn is_inside_work_tree(&self) -> RefineResult<bool> {
        let output = self.git_raw(&["rev-parse", "--is-inside-work-tree"])?;
        Ok(output.success && String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    /// Ensure a locked, detached worktree exists at `path` checked out at
    /// `commit`. Detached because Git refuses to check out a branch that is
    /// already checked out in another worktree; locked so `worktree prune` and
    /// cleanup sweeps never reclaim it. Idempotent: a valid existing
    /// registration is kept (and re-locked), a broken registration or stale
    /// directory is recreated.
    pub fn ensure_detached_worktree(&self, path: &Path, commit: &str) -> RefineResult<()> {
        validate_commitish(commit)?;
        let target = path.to_str().unwrap_or("").trim().to_string();
        if target.is_empty() {
            return Err(RefineError::InvalidInput(
                "worktree path is required".to_string(),
            ));
        }
        let valid = path.exists() && self.service_rooted_at(path).is_inside_work_tree()?;
        if !valid {
            // A stale registration may still be locked, and both `worktree
            // remove` and `worktree prune` skip locked entries, so unlock
            // best-effort before tearing the remnants down.
            let _ = self.git_raw(&["worktree", "unlock", &target]);
            let _ = self.git_raw(&["worktree", "remove", "--force", "--force", &target]);
            if path.exists() {
                fs::remove_dir_all(path).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to remove stale worktree {}: {error}",
                        path.display()
                    ))
                })?;
            }
            self.git_output(&["worktree", "prune"])?;
            create_worktree_parent(path)?;
            self.git_output(&["worktree", "add", "--detach", &target, commit])?;
        }
        let lock = self.git_raw(&[
            "worktree",
            "lock",
            "--reason",
            "refine integration workspace",
            &target,
        ])?;
        if !lock.success && !trimmed_command_text(&lock).contains("already locked") {
            return Err(RefineError::Conflict(trimmed_command_text(&lock)));
        }
        self.audit(
            "worktree_detached_ensure",
            "ok",
            json!({"target": target, "commit": commit, "created": !valid}),
        )
    }

    pub fn unlock_worktree(&self, path: &Path) -> RefineResult<()> {
        let target = path.to_str().unwrap_or("").trim().to_string();
        if target.is_empty() {
            return Err(RefineError::InvalidInput(
                "worktree path is required".to_string(),
            ));
        }
        self.git_output(&["worktree", "unlock", &target])?;
        self.audit("worktree_unlock", "ok", json!({"target": target}))
    }

    /// Detach the service's worktree at `commit`. Run this with the service
    /// rooted at the worktree being repositioned, not the primary checkout.
    pub fn checkout_detached(&self, commit: &str) -> RefineResult<()> {
        validate_commitish(commit)?;
        self.git_output(&["checkout", "--detach", commit])?;
        self.audit("checkout_detached", "ok", json!({"commit": commit}))
    }

    /// Compare-and-swap advance of a fully qualified branch ref.
    ///
    /// Unlike `branch -f`, `update-ref` moves a branch even while it is
    /// checked out in another worktree, which is what lets a detached
    /// integration worktree advance the target branch of the shared human
    /// checkout. Losing the race maps to `Conflict` naming the current commit
    /// so callers can rejoin their refresh loop.
    pub fn update_ref_cas(
        &self,
        reference: &str,
        to_commit: &str,
        expected_old: &str,
    ) -> RefineResult<()> {
        validate_head_branch_ref(reference)?;
        validate_commitish(to_commit)?;
        validate_commitish(expected_old)?;
        let output = self.git_raw(&["update-ref", reference, to_commit, expected_old])?;
        if output.success {
            return self.audit(
                "update_ref_cas",
                "ok",
                json!({
                    "reference": reference,
                    "to_commit": to_commit,
                    "expected_old": expected_old,
                    "exact_sha_fence": true
                }),
            );
        }
        let message = trimmed_command_text(&output);
        let expected = self.resolve_commit(expected_old)?;
        if let Ok(current) = self.resolve_commit(reference)
            && current != expected
        {
            let _ = self.audit(
                "update_ref_cas",
                "conflict",
                json!({"reference": reference, "expected_old": expected, "current": current}),
            );
            return Err(RefineError::Conflict(format!(
                "target advanced: {reference} moved from {expected} to {current} before the update"
            )));
        }
        Err(RefineError::Conflict(message))
    }

    /// Two-tree merge of the `from`→`to` delta into the index and working
    /// tree. Git refuses atomically when a working-tree file collides with the
    /// delta; the collision detail from stderr is carried in the returned
    /// error because callers surface it.
    pub fn read_tree_merge_update(&self, from_commit: &str, to_commit: &str) -> RefineResult<()> {
        validate_commitish(from_commit)?;
        validate_commitish(to_commit)?;
        self.git_output(&["read-tree", "-m", "-u", from_commit, to_commit])?;
        self.audit(
            "read_tree_merge_update",
            "ok",
            json!({"from_commit": from_commit, "to_commit": to_commit}),
        )
    }

    /// The fully qualified branch HEAD points at, or `None` when detached.
    /// Detached HEAD makes `rev-parse` exit nonzero or print the literal
    /// `HEAD` depending on the Git version, so both are handled.
    pub fn symbolic_head_branch(&self) -> RefineResult<Option<String>> {
        let output = self.git_raw(&["rev-parse", "--symbolic-full-name", "HEAD"])?;
        if !output.success {
            return Ok(None);
        }
        let reference = stdout(output)?.trim().to_string();
        if reference.is_empty() || reference == "HEAD" {
            return Ok(None);
        }
        Ok(Some(reference))
    }

    fn service_rooted_at(&self, path: &Path) -> FileGitWorktreeService {
        FileGitWorktreeService {
            root: path.to_path_buf(),
            runtime_root: self.runtime_root.clone(),
            operation_id: self.operation_id.clone(),
            process_metadata: self.process_metadata.clone(),
        }
    }

    pub(super) fn conflicts(&self) -> RefineResult<Vec<String>> {
        let output = self.git_output(&["diff", "--name-only", "--diff-filter=U"])?;
        Ok(stdout(output)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }
}
