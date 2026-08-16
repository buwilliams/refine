use super::*;

impl FileGitWorktreeService {
    /// Materialize a managed branch/worktree at one exact commit without moving an existing ref.
    /// This is used to regenerate candidate-bound evidence after the shared target has advanced.
    pub fn ensure_worktree_at_commit(
        &self,
        branch: &str,
        target: &Path,
        commit: &str,
    ) -> RefineResult<String> {
        validate_branch_name(branch)?;
        validate_commitish(commit)?;
        let resolved_commit = self.resolve_commit(commit)?;
        if let Some(existing) = self.worktree_for_branch(branch)? {
            if existing.exists() {
                let existing_git = FileGitWorktreeService {
                    root: existing.clone(),
                    runtime_root: self.runtime_root.clone(),
                    operation_id: self.operation_id.clone(),
                    process_metadata: self.process_metadata.clone(),
                };
                let head = existing_git.head_ref()?;
                let status = existing_git.inspect("")?;
                if head.commit.as_deref() != Some(resolved_commit.as_str())
                    || status.dirty_user_changes
                    || !status.refine_owned_artifacts.is_empty()
                {
                    return Err(RefineError::Conflict(format!(
                        "managed exact-candidate checkout {} no longer names clean commit {resolved_commit}; existing work was preserved",
                        existing.display()
                    )));
                }
                return Ok(existing.display().to_string());
            }
            // The registration outlived its directory (external cleanup); without a
            // prune, `git worktree add` refuses because the branch is still "checked out".
            // A candidate lock would also keep the stale registration alive through the
            // prune, so release it first.
            self.unlock_worktree_tolerant(&existing);
            self.prune_stale_worktrees()?;
        }
        if self.branch_exists(branch)? {
            let existing_commit = self.resolve_commit(branch)?;
            if existing_commit != resolved_commit {
                return Err(RefineError::Conflict(format!(
                    "managed exact-candidate branch {branch} names {existing_commit}, expected {resolved_commit}; the branch was not moved"
                )));
            }
        } else {
            self.git_output(&["branch", branch, &resolved_commit])?;
            self.audit(
                "branch",
                "ok",
                json!({"name": branch, "commit": resolved_commit, "reconciliation": true}),
            )?;
        }
        self.ensure_worktree(branch, target)
    }

    /// Resolve an already materialized candidate worktree without creating or switching it.
    pub fn existing_worktree_for_branch(&self, branch: &str) -> RefineResult<Option<PathBuf>> {
        self.worktree_for_branch(branch)
    }

    pub fn audit_path(&self) -> RefineResult<PathBuf> {
        stdout(self.git_output(&["rev-parse", "--git-path", GIT_AUDIT_FILE])?).map(|path| {
            let path = PathBuf::from(path.trim());
            if path.is_absolute() {
                path
            } else {
                self.root.join(path)
            }
        })
    }

    pub fn recent_changes(&self, limit: usize) -> RefineResult<Vec<GitChange>> {
        let output = self.git_output(&[
            "log",
            "--date=iso-strict",
            "--pretty=format:%H%x1f%cI%x1f%s%x1e",
            "-n",
            &limit.clamp(1, 1000).to_string(),
        ])?;
        let text = stdout(output)?;
        Ok(text
            .split('\x1e')
            .filter_map(parse_git_change)
            .collect::<Vec<_>>())
    }

    /// Return every commit introduced between two exact commit anchors.
    ///
    /// Goal evidence exports use this instead of the current branch history so
    /// review candidates remain auditable before and after their branch is
    /// merged or cleaned up.
    pub fn changes_between(&self, base: &str, candidate: &str) -> RefineResult<Vec<GitChange>> {
        let key = (base.to_string(), candidate.to_string());
        Ok(self
            .changes_between_many(std::slice::from_ref(&key))?
            .remove(&key)
            .unwrap_or_default())
    }

    /// Resolve exact commit evidence for any number of Goal ranges with one Git history lookup.
    ///
    /// The returned map is keyed by `(base, candidate)`. Refine loads the parent graph for every
    /// unique endpoint once, then computes each `base..candidate` reachability difference in
    /// memory. This keeps bulk evidence export from spawning one `git log` process per Goal.
    pub fn changes_between_many(
        &self,
        ranges: &[(String, String)],
    ) -> RefineResult<BTreeMap<(String, String), Vec<GitChange>>> {
        let ranges = ranges.iter().cloned().collect::<BTreeSet<_>>();
        if ranges.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut endpoints = BTreeSet::new();
        for (base, candidate) in &ranges {
            validate_commitish(base)?;
            validate_commitish(candidate)?;
            endpoints.insert(base.clone());
            endpoints.insert(candidate.clone());
        }

        let endpoint_names = endpoints.into_iter().collect::<Vec<_>>();
        let resolve_input = endpoint_names
            .iter()
            .map(|endpoint| format!("{endpoint}^{{commit}}"))
            .collect::<Vec<_>>()
            .join("\n");
        let resolved_values = stdout(self.git_output_with_stdin(
            &["cat-file", "--batch-check=%(objectname) %(objecttype)"],
            &format!("{resolve_input}\n"),
        )?)?
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.split_whitespace().collect::<Vec<_>>())
        .map(|parts| match parts.as_slice() {
            [commit, "commit"] => Ok((*commit).to_string()),
            _ => Err(RefineError::Conflict(
                "Git did not resolve a Goal commit-evidence endpoint to a commit".to_string(),
            )),
        })
        .collect::<RefineResult<Vec<_>>>()?;
        if resolved_values.len() != endpoint_names.len() {
            return Err(RefineError::Conflict(
                "Git did not resolve every Goal commit-evidence endpoint".to_string(),
            ));
        }
        let resolved = endpoint_names
            .iter()
            .cloned()
            .zip(resolved_values)
            .collect::<BTreeMap<_, _>>();

        let args = [
            "log",
            "--topo-order",
            "--date=iso-strict",
            "--pretty=format:%H%x1f%P%x1f%cI%x1f%s%x1e",
            "--stdin",
        ];
        let revision_input = resolved.values().cloned().collect::<Vec<_>>().join("\n");
        let graph = parse_git_change_graph(&stdout(
            self.git_output_with_stdin(&args, &format!("{revision_input}\n"))?,
        )?);

        let mut result = BTreeMap::new();
        for range in ranges {
            let (base, candidate) = (&range.0, &range.1);
            let base_ancestors = reachable_commits(&graph.nodes, &resolved[base]);
            let candidate_ancestors = reachable_commits(&graph.nodes, &resolved[candidate]);
            let mut commits = graph
                .order
                .iter()
                .filter(|commit| {
                    candidate_ancestors.contains(*commit) && !base_ancestors.contains(*commit)
                })
                .filter_map(|commit| graph.nodes.get(commit).map(|node| node.change.clone()))
                .collect::<Vec<_>>();
            commits.reverse();
            result.insert(range, commits);
        }
        Ok(result)
    }

    pub fn head_ref(&self) -> RefineResult<GitHeadRef> {
        let branch_output = self.git_raw(&["branch", "--show-current"])?;
        let branch = if branch_output.success {
            let branch = stdout(branch_output)?.trim().to_string();
            if branch.is_empty() {
                None
            } else {
                Some(branch)
            }
        } else {
            None
        };

        let commit_output = self.git_raw(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let commit = if commit_output.success {
            let commit = stdout(commit_output)?.trim().to_string();
            if commit.is_empty() {
                None
            } else {
                Some(commit)
            }
        } else {
            None
        };

        Ok(GitHeadRef { branch, commit })
    }

    pub fn git_path(&self, path: &str) -> RefineResult<PathBuf> {
        let resolved = stdout(self.git_output(&["rev-parse", "--git-path", path])?)?;
        let resolved = PathBuf::from(resolved.trim());
        let path = if resolved.is_absolute() {
            resolved
        } else {
            self.root.join(resolved)
        };
        self.audit(
            "git_path",
            "ok",
            json!({"path": path.display().to_string()}),
        )?;
        Ok(path)
    }

    pub fn has_commits_since(&self, base_branch: &str) -> RefineResult<bool> {
        validate_branch_name(base_branch)?;
        let output =
            stdout(self.git_output(&["rev-list", "--count", &format!("{base_branch}..HEAD")])?)?;
        Ok(output.trim().parse::<usize>().unwrap_or(0) > 0)
    }

    pub(super) fn head_commit(&self) -> RefineResult<String> {
        stdout(self.git_output(&["rev-parse", "HEAD"])?).map(|commit| commit.trim().to_string())
    }

    pub(super) fn is_clean(&self) -> RefineResult<bool> {
        stdout(self.git_output(&["status", "--porcelain=v1"])?)
            .map(|status| status.trim().is_empty())
    }

    pub(super) fn current_clean_commit_since(
        &self,
        base_branch: &str,
    ) -> RefineResult<Option<String>> {
        if self.is_clean()? && self.has_commits_since(base_branch)? {
            return self.head_commit().map(Some);
        }
        Ok(None)
    }
}
