use super::*;

use crate::model::goal::PlanningGitObservation;

impl FileGitWorktreeService {
    /// Exact, non-mutating Git evidence used to prove planning phases were observational.
    pub fn implementation_planning_observation(&self) -> RefineResult<PlanningGitObservation> {
        let head_commit = stdout(self.git_output(&["rev-parse", "HEAD"])?)?
            .trim()
            .to_string();
        let branch = stdout(self.git_output(&["branch", "--show-current"])?)?
            .trim()
            .to_string();
        let status =
            stdout(self.git_output(&["status", "--porcelain=v1", "--untracked-files=all"])?)?;
        Ok(PlanningGitObservation {
            head_commit,
            branch: (!branch.is_empty()).then_some(branch),
            status_porcelain: status.lines().map(ToString::to_string).collect(),
        })
    }

    pub fn list_refine_owned_branches(&self) -> RefineResult<Vec<String>> {
        let output = stdout(self.git_output(&[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/refine/",
        ])?)?;
        Ok(output
            .lines()
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn list_linked_worktrees(&self) -> RefineResult<Vec<GitLinkedWorktree>> {
        let output = stdout(self.git_output(&["worktree", "list", "--porcelain"])?)?;
        let mut worktrees = Vec::new();
        let mut current_path = None;
        let mut current_branch = None;
        for line in output.lines().chain(std::iter::once("")) {
            if let Some(path) = line.strip_prefix("worktree ") {
                if let Some(path) = current_path.take() {
                    worktrees.push(GitLinkedWorktree {
                        path,
                        branch: current_branch.take(),
                    });
                }
                current_path = Some(PathBuf::from(path));
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                current_branch = Some(branch.to_string());
            } else if line.is_empty()
                && let Some(path) = current_path.take()
            {
                worktrees.push(GitLinkedWorktree {
                    path,
                    branch: current_branch.take(),
                });
            }
        }
        Ok(worktrees)
    }

    pub fn worktree_is_clean(&self, path: &Path) -> RefineResult<bool> {
        let service = FileGitWorktreeService {
            root: path.to_path_buf(),
            runtime_root: self.runtime_root.clone(),
            operation_id: self.operation_id.clone(),
            process_metadata: self.process_metadata.clone(),
        };
        service.is_clean()
    }

    pub fn worktree_ignored_paths(&self, path: &Path) -> RefineResult<Vec<String>> {
        let service = FileGitWorktreeService {
            root: path.to_path_buf(),
            runtime_root: self.runtime_root.clone(),
            operation_id: self.operation_id.clone(),
            process_metadata: self.process_metadata.clone(),
        };
        let status = stdout(service.git_output(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
        ])?)?;
        Ok(status
            .split('\0')
            .filter_map(|entry| entry.strip_prefix("!! "))
            .map(|path| path.trim_end_matches('/').replace('\\', "/"))
            .filter(|path| !path.is_empty())
            .collect())
    }

    /// Preserve tracked work left in a Refine-owned shared checkout, then return
    /// the exact stash commit that quarantines it. Callers must establish ownership before using
    /// this: the Git capability intentionally does not guess whether dirty work belongs to Refine.
    pub fn quarantine_worktree_changes(&self, message: &str) -> RefineResult<Option<String>> {
        if self.is_clean()? {
            return Ok(None);
        }
        if message.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "Git quarantine message is required".to_string(),
            ));
        }
        let previous = self.resolve_commit("refs/stash").ok();
        self.git_output(&["stash", "push", "-m", message])?;
        let commit = self.resolve_commit("refs/stash")?;
        if previous.as_deref() == Some(commit.as_str()) {
            return Err(RefineError::Conflict(
                "dirty integrated-target worktree had no tracked changes to quarantine; untracked work was preserved"
                    .to_string(),
            ));
        }
        self.audit(
            "worktree_quarantine",
            "ok",
            json!({"message": message, "stash_commit": commit}),
        )?;
        Ok(Some(commit))
    }

    pub fn remove_worktree(&self, path: &Path, force: bool) -> RefineResult<()> {
        let target = path.to_str().unwrap_or("");
        if target.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "worktree path is required".to_string(),
            ));
        }
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(target);
        self.git_output(&args)?;
        self.audit(
            "worktree_remove",
            "ok",
            json!({"target": path.display().to_string(), "force": force}),
        )
    }

    pub fn delete_branch(&self, branch: &str, force: bool) -> RefineResult<()> {
        validate_branch_name(branch)?;
        self.git_output(&["branch", if force { "-D" } else { "-d" }, branch])?;
        self.audit(
            "branch_delete",
            "ok",
            json!({"branch": branch, "force": force}),
        )
    }

    pub fn delete_branch_if_matches(
        &self,
        branch: &str,
        expected_commit: &str,
    ) -> RefineResult<()> {
        validate_branch_name(branch)?;
        validate_commitish(expected_commit)?;
        let reference = format!("refs/heads/{branch}");
        self.git_output(&["update-ref", "-d", &reference, expected_commit])?;
        self.audit(
            "branch_delete",
            "ok",
            json!({
                "branch": branch,
                "expected_commit": expected_commit,
                "exact_sha_fence": true
            }),
        )
    }

    pub fn cleanup_merged_branch(&self, branch: &str) -> RefineResult<MergedBranchCleanup> {
        validate_branch_name(branch)?;
        let worktree_path = self.worktree_for_branch(branch)?;
        let mut worktree_removed = false;
        if let Some(path) = worktree_path.as_deref() {
            if same_existing_path(path, &self.root) {
                return Err(RefineError::Conflict(format!(
                    "refusing to remove primary worktree {} for branch {branch}",
                    path.display()
                )));
            }
            self.remove_worktree(path, true)?;
            worktree_removed = true;
        }

        let mut branch_deleted = false;
        if self.branch_exists(branch)? {
            self.delete_branch(branch, false)?;
            branch_deleted = true;
        }

        let cleanup = MergedBranchCleanup {
            branch: branch.to_string(),
            worktree_path: worktree_path.map(|path| path.display().to_string()),
            worktree_removed,
            branch_deleted,
        };
        self.audit("merged_branch_cleanup", "ok", json!({"cleanup": &cleanup}))?;
        Ok(cleanup)
    }

    pub(super) fn root_for(&self, path: &str) -> PathBuf {
        let path = path.trim();
        if path.is_empty() {
            self.root.clone()
        } else {
            PathBuf::from(path)
        }
    }

    pub(super) fn head_commit_exists(&self) -> RefineResult<bool> {
        Ok(self
            .git_raw(&["rev-parse", "--verify", "HEAD^{commit}"])?
            .success)
    }

    pub(super) fn ensure_head_commit(&self) -> RefineResult<()> {
        if self.head_commit_exists()? {
            return Ok(());
        }
        self.git_output_with_env(
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "--only",
                "--no-verify",
                "-m",
                "Initialize Refine workspace",
            ],
            &[
                ("GIT_AUTHOR_NAME", "Refine"),
                ("GIT_AUTHOR_EMAIL", "refine@example.invalid"),
                ("GIT_COMMITTER_NAME", "Refine"),
                ("GIT_COMMITTER_EMAIL", "refine@example.invalid"),
            ],
        )?;
        let commit = stdout(self.git_output(&["rev-parse", "HEAD"])?)?
            .trim()
            .to_string();
        self.audit(
            "bootstrap_head",
            "ok",
            json!({"commit": commit, "message": "Initialize Refine workspace"}),
        )
    }

    pub(super) fn branch_exists(&self, branch: &str) -> RefineResult<bool> {
        validate_branch_name(branch)?;
        Ok(self
            .git_raw(&["rev-parse", "--verify", &format!("refs/heads/{branch}")])?
            .success)
    }

    pub(super) fn worktree_for_branch(&self, branch: &str) -> RefineResult<Option<PathBuf>> {
        validate_branch_name(branch)?;
        let output = stdout(self.git_output(&["worktree", "list", "--porcelain"])?)?;
        let mut current_path: Option<PathBuf> = None;
        for line in output.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path));
            } else if let Some(head_branch) = line.strip_prefix("branch refs/heads/")
                && head_branch == branch
            {
                return Ok(current_path);
            }
        }
        Ok(None)
    }
}
