use super::*;

impl GitWorktreeService for FileGitWorktreeService {
    fn inspect(&self, path: &str) -> RefineResult<GitStatus> {
        let service = FileGitWorktreeService {
            root: self.root_for(path),
            runtime_root: self.runtime_root.clone(),
            operation_id: self.operation_id.clone(),
            process_metadata: self.process_metadata.clone(),
        };
        let root = stdout(service.git_output(&["rev-parse", "--show-toplevel"])?)?
            .trim()
            .to_string();
        let branch_output = service.git_raw(&["branch", "--show-current"])?;
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
        let status = stdout(service.git_output(&["status", "--porcelain=v1"])?).unwrap_or_default();
        let mut refine_owned_artifacts = Vec::new();
        let mut dirty_user_changes = false;
        for line in status.lines() {
            let path = line.get(3..).unwrap_or("").trim();
            if is_refine_owned_artifact(path) {
                refine_owned_artifacts.push(path.to_string());
            } else if !path.is_empty() {
                dirty_user_changes = true;
            }
        }
        Ok(GitStatus {
            root,
            branch,
            dirty_user_changes,
            refine_owned_artifacts,
        })
    }

    fn branch(&self, name: &str) -> RefineResult<String> {
        validate_branch_name(name)?;
        self.git_output(&["switch", "-c", name])?;
        self.audit("branch", "ok", json!({"name": name}))?;
        Ok(name.to_string())
    }

    fn switch(&self, branch: &str) -> RefineResult<String> {
        validate_branch_name(branch)?;
        self.git_output(&["switch", branch])?;
        self.audit("switch", "ok", json!({"branch": branch}))?;
        Ok(branch.to_string())
    }

    fn worktree(&self, branch: &str) -> RefineResult<String> {
        validate_branch_name(branch)?;
        self.ensure_head_commit()?;
        let target = self
            .git_path("refine-worktrees")?
            .join(branch.replace('/', "-"));
        create_worktree_parent(&target)?;
        self.git_output(&[
            "worktree",
            "add",
            "-b",
            branch,
            target.to_str().unwrap_or(""),
        ])?;
        self.audit(
            "worktree",
            "ok",
            json!({"branch": branch, "target": target.display().to_string()}),
        )?;
        Ok(target.display().to_string())
    }

    fn ensure_branch_from_head(&self, name: &str) -> RefineResult<String> {
        validate_branch_name(name)?;
        self.ensure_head_commit()?;
        if !self.branch_exists(name)? {
            self.git_output(&["branch", name])?;
            self.audit("branch", "ok", json!({"name": name, "reused": false}))?;
        } else {
            self.audit("branch", "ok", json!({"name": name, "reused": true}))?;
        }
        Ok(name.to_string())
    }

    fn ensure_worktree(&self, branch: &str, target: &Path) -> RefineResult<String> {
        validate_branch_name(branch)?;
        if let Some(existing) = self.worktree_for_branch(branch)? {
            if existing.exists() {
                self.audit(
                    "worktree",
                    "ok",
                    json!({"branch": branch, "target": existing.display().to_string(), "reused": true}),
                )?;
                return Ok(existing.display().to_string());
            }
            // The registration outlived its directory (external cleanup); without a
            // prune, `git worktree add` refuses because the branch is still "checked out".
            // A candidate lock would also keep the stale registration alive through the
            // prune, so release it first.
            self.unlock_worktree_tolerant(&existing);
            self.prune_stale_worktrees()?;
        }
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.root.join(target)
        };
        create_worktree_parent(&target)?;
        if !self.branch_exists(branch)? {
            self.ensure_head_commit()?;
            self.git_output(&[
                "worktree",
                "add",
                "-b",
                branch,
                target.to_str().unwrap_or(""),
            ])?;
        } else {
            self.git_output(&["worktree", "add", target.to_str().unwrap_or(""), branch])?;
        }
        // Candidate worktrees must survive the repo-wide `git worktree prune` sweeps
        // run by state sync and source promotion; locked worktrees are exempt.
        self.lock_worktree(&target)?;
        self.audit(
            "worktree",
            "ok",
            json!({"branch": branch, "target": target.display().to_string(), "reused": false}),
        )?;
        Ok(target.display().to_string())
    }

    fn diff(&self, pathspecs: &[String]) -> RefineResult<String> {
        let mut args = vec!["diff", "--"];
        for pathspec in pathspecs {
            args.push(pathspec.as_str());
        }
        let diff = stdout(self.git_output(&args)?)?;
        self.audit("diff", "ok", json!({"pathspecs": pathspecs}))?;
        Ok(diff)
    }

    fn merge(&self, branch: &str) -> RefineResult<MergeResult> {
        validate_branch_name(branch)?;
        let output = self.git_raw(&["merge", "--no-edit", branch])?;
        let result = MergeResult {
            ok: output.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit("merge", "ok", json!({"branch": branch, "result": &result}))?;
        } else {
            let _ = self.audit(
                "merge",
                "conflict",
                json!({"branch": branch, "result": &result}),
            );
        }
        Ok(result)
    }

    fn merge_no_ff(&self, branch: &str) -> RefineResult<MergeResult> {
        validate_branch_name(branch)?;
        let output = self.git_raw(&["merge", "--no-ff", "--no-edit", branch])?;
        let result = MergeResult {
            ok: output.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit(
                "merge_no_ff",
                "ok",
                json!({"branch": branch, "result": &result}),
            )?;
        } else {
            let _ = self.audit(
                "merge_no_ff",
                "conflict",
                json!({"branch": branch, "result": &result}),
            );
        }
        Ok(result)
    }

    fn rebase(&self, branch: &str) -> RefineResult<MergeResult> {
        validate_branch_name(branch)?;
        let output = self.git_raw(&["rebase", branch])?;
        let result = MergeResult {
            ok: output.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit("rebase", "ok", json!({"branch": branch, "result": &result}))?;
        } else {
            let _ = self.audit(
                "rebase",
                "conflict",
                json!({"branch": branch, "result": &result}),
            );
        }
        Ok(result)
    }

    fn commit(&self, message: &str, pathspecs: &[String]) -> RefineResult<String> {
        self.commit_inner(message, pathspecs, false)
    }

    fn commit_or_current_if_clean_since(
        &self,
        message: &str,
        pathspecs: &[String],
        base_branch: &str,
    ) -> RefineResult<String> {
        if let Some(commit) = self.current_clean_commit_since(base_branch)? {
            self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
            return Ok(commit);
        }
        match self.commit_inner(message, pathspecs, false) {
            Ok(commit) => Ok(commit),
            Err(error) if is_nothing_to_commit_error(&error) => {
                if let Some(commit) = self.current_clean_commit_since(base_branch)? {
                    self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
                    Ok(commit)
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn commit_allow_empty(&self, message: &str, pathspecs: &[String]) -> RefineResult<String> {
        self.commit_inner(message, pathspecs, true)
    }

    fn push(&self, remote: &str, branch: &str) -> RefineResult<()> {
        validate_branch_name(branch)?;
        if remote.trim().is_empty() {
            return Err(RefineError::InvalidInput("remote is required".to_string()));
        }
        self.git_output(&["push", remote, branch])?;
        self.audit("push", "ok", json!({"remote": remote, "branch": branch}))
    }

    fn hard_reset(&self) -> RefineResult<MergeResult> {
        let reset = self.git_raw(&["reset", "--hard", "HEAD"])?;
        let mut clean_args = vec![
            "clean".to_string(),
            "-fd".to_string(),
            "-e".to_string(),
            "run".to_string(),
            "-e".to_string(),
            "run/**".to_string(),
            "-e".to_string(),
            "target".to_string(),
            "-e".to_string(),
            "target/**".to_string(),
        ];
        if let Some(runtime_root) = &self.runtime_root
            && let Some(relative_runtime) = relative_child_path(&self.root, runtime_root)
        {
            clean_args.extend([
                "-e".to_string(),
                relative_runtime.clone(),
                "-e".to_string(),
                format!("{relative_runtime}/**"),
            ]);
        }
        let clean_refs = clean_args.iter().map(String::as_str).collect::<Vec<_>>();
        let clean = self.git_raw(&clean_refs)?;
        let result = MergeResult {
            ok: reset.success && clean.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(
                [trimmed_command_text(&reset), trimmed_command_text(&clean)]
                    .into_iter()
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        };
        if result.ok {
            self.audit("hard_reset", "ok", json!({"result": &result}))?;
        } else {
            let _ = self.audit("hard_reset", "failed", json!({"result": &result}));
        }
        Ok(result)
    }

    fn recover(&self) -> RefineResult<MergeResult> {
        let merge = self.git_raw(&["merge", "--abort"])?;
        let rebase = self.git_raw(&["rebase", "--abort"])?;
        let revert = self.git_raw(&["revert", "--abort"])?;
        let result = MergeResult {
            ok: merge.success || rebase.success || revert.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(format!(
                "{}\n{}\n{}",
                trimmed_command_text(&merge),
                trimmed_command_text(&rebase),
                trimmed_command_text(&revert)
            )),
        };
        if result.ok {
            self.audit("recover", "ok", json!({"result": &result}))?;
        } else {
            let _ = self.audit("recover", "failed", json!({"result": &result}));
        }
        Ok(result)
    }
}
