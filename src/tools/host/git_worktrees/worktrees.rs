use super::*;

impl FileGitWorktreeService {
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
