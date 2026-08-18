use super::*;

/// Write conflict markers that carry the merge base as well as both sides.
///
/// A conflict a resolver is asked to understand is not readable from two
/// versions alone: without the base section it cannot tell which side changed
/// what. Refine's state workspaces render the base explicitly; a conflicted
/// rebase gets it from Git, whatever the user's own `merge.conflictStyle`
/// happens to be, through this per-invocation configuration.
pub(super) const BASE_IN_CONFLICT_MARKERS: [(&str, &str); 3] = [
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "merge.conflictStyle"),
    ("GIT_CONFIG_VALUE_0", "diff3"),
];

impl FileGitWorktreeService {
    /// Whether a merge, rebase, revert, or cherry-pick is stopped
    /// mid-operation in this worktree. A crash between Refine's two holds
    /// leaves exactly that, and the checkout stays wedged until something
    /// aborts it.
    pub fn operation_in_progress(&self) -> RefineResult<bool> {
        let rebase_merge = self.git_path("rebase-merge")?;
        let Some(git_dir) = rebase_merge.parent() else {
            return Ok(false);
        };
        Ok([
            "rebase-merge",
            "rebase-apply",
            "MERGE_HEAD",
            "REVERT_HEAD",
            "CHERRY_PICK_HEAD",
        ]
        .iter()
        .any(|marker| git_dir.join(marker).exists()))
    }

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

    /// Stage the resolved paths of a stopped rebase and continue it. Refine
    /// never skips a pick: a further conflicted stop or any other failure
    /// comes back as a non-ok `MergeResult` for the caller to route.
    pub fn rebase_continue(&self, resolved_paths: &[String]) -> RefineResult<MergeResult> {
        if resolved_paths.is_empty() {
            return Err(RefineError::InvalidInput(
                "rebase continue requires the resolved paths of the current stop".to_string(),
            ));
        }
        let mut add_args = vec!["add", "--"];
        add_args.extend(resolved_paths.iter().map(String::as_str));
        self.git_output(&add_args)?;
        let mut env = vec![("GIT_EDITOR", "true")];
        env.extend(BASE_IN_CONFLICT_MARKERS);
        let output = self.git_raw_with_env(&["rebase", "--continue"], &env)?;
        let result = MergeResult {
            ok: output.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit("rebase_continue", "ok", json!({"result": &result}))?;
        } else {
            let _ = self.audit("rebase_continue", "conflict", json!({"result": &result}));
        }
        Ok(result)
    }

    /// Commits in `base..tip` that touched any of `paths`, newest first,
    /// capped so provenance lookups over the conflicting range stay bounded.
    pub fn commits_in_range_touching(
        &self,
        base: &str,
        tip: &str,
        paths: &[String],
    ) -> RefineResult<Vec<String>> {
        validate_commitish(base)?;
        validate_commitish(tip)?;
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let range = format!("{base}..{tip}");
        let mut args = vec!["rev-list", "-n", "50", &range, "--"];
        args.extend(paths.iter().map(String::as_str));
        let output = stdout(self.git_output(&args)?)?;
        Ok(output.split_whitespace().map(str::to_string).collect())
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
        let target = require_worktree_target(path)?;
        let valid = path.exists() && self.service_rooted_at(path).is_inside_work_tree()?;
        if !valid {
            self.purge_worktree(path)?;
            create_worktree_parent(path)?;
            self.git_output(&["worktree", "add", "--detach", &target, commit])?;
        }
        self.lock_worktree(path, "refine integration workspace")?;
        self.audit(
            "worktree_detached_ensure",
            "ok",
            json!({"target": target, "commit": commit, "created": !valid}),
        )
    }

    /// Tear a worktree down even when its registration is broken — a crash
    /// mid `worktree add` before the `.git` link exists, or a pruned
    /// registration with the directory left behind — states in which a single
    /// `git worktree remove --force` refuses ("is not a working tree") and
    /// would wedge the caller. The unlock and Git-side remove are
    /// best-effort; the directory removal and registration prune are
    /// authoritative.
    pub fn purge_worktree(&self, path: &Path) -> RefineResult<()> {
        let target = require_worktree_target(path)?;
        // A stale registration may still be locked, and both `worktree
        // remove` and `worktree prune` skip locked entries, so unlock
        // best-effort before tearing the remnants down.
        self.unlock_worktree_tolerant(path);
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
        self.audit("worktree_purge", "ok", json!({"target": target}))
    }

    /// Detach the service's worktree at `commit`. Run this with the service
    /// rooted at the worktree being repositioned, not the primary checkout.
    pub fn checkout_detached(&self, commit: &str) -> RefineResult<()> {
        validate_commitish(commit)?;
        self.git_output(&["checkout", "--detach", commit])?;
        self.audit("checkout_detached", "ok", json!({"commit": commit}))
    }

    /// Two-tree merge of the `from`→`to` delta into the index and working
    /// tree. Git refuses atomically when a working-tree file collides with the
    /// delta; that refusal is classified here — next to the invocation, where
    /// knowledge of Git's exact refusal wordings belongs — and returned as the
    /// typed `Collision` outcome. Every other failure is an error.
    pub fn read_tree_merge_update(
        &self,
        from_commit: &str,
        to_commit: &str,
    ) -> RefineResult<ReadTreeMergeOutcome> {
        validate_commitish(from_commit)?;
        validate_commitish(to_commit)?;
        let output = self.git_raw(&["read-tree", "-m", "-u", from_commit, to_commit])?;
        if !output.success {
            let detail = trimmed_command_text(&output);
            // Only `read-tree -m -u`'s refusal messages for working-tree or
            // index collisions. Reporting any other failure — `index.lock`
            // contention from the human's own tooling, missing objects,
            // repository corruption — as a collision would tell the human to
            // commit or stash files that do not collide.
            let lower = detail.to_ascii_lowercase();
            if lower.contains("would be overwritten") || lower.contains("not uptodate") {
                return Ok(ReadTreeMergeOutcome::Collision { detail });
            }
            return Err(RefineError::Conflict(detail));
        }
        self.audit(
            "read_tree_merge_update",
            "ok",
            json!({"from_commit": from_commit, "to_commit": to_commit}),
        )?;
        Ok(ReadTreeMergeOutcome::Applied)
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
