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
