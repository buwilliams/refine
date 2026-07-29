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
        Ok(self
            .git_raw(&["merge-base", "--is-ancestor", ancestor, descendant])?
            .success)
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
