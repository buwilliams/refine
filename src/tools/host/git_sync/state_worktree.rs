use super::*;

impl FileGitSyncService {
    pub(super) fn recover_interrupted_state_worktree(
        &self,
        state_root: &std::path::Path,
    ) -> RefineResult<bool> {
        let tracked_changes = self.git_at_stdout(
            state_root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--",
                ".refine",
            ],
        )?;
        let untracked = self.git_at_stdout(
            state_root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--",
                ".refine",
            ],
        )?;
        if tracked_changes.is_empty() && untracked.is_empty() {
            return Ok(false);
        }

        if !tracked_changes.is_empty() {
            self.git_at_checked(
                state_root,
                &[
                    "restore",
                    "--source=HEAD",
                    "--staged",
                    "--worktree",
                    "--",
                    ".refine",
                ],
            )?;
        }
        if !untracked.is_empty() {
            self.git_at_checked(state_root, &["clean", "-f", "-d", "-x", "--", ".refine"])?;
        }

        let remaining = self.git_at_stdout(
            state_root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--",
                ".refine",
            ],
        )?;
        if !remaining.is_empty() {
            return Err(RefineError::Conflict(format!(
                "failed to recover interrupted Refine state synchronization: {remaining}"
            )));
        }
        Ok(true)
    }

    /// Return the managed state worktree to a clean checkout after a failed
    /// pass, so a later retry starts from durable Git evidence.
    pub(in crate::tools::host::git_sync) fn restore_managed_state_worktree(
        &self,
    ) -> RefineResult<()> {
        let path = state_worktree_for_target_root(&self.target_root)?;
        if !path.exists() {
            return Ok(());
        }
        let _ = self.git_at(&path, &["rebase", "--abort"]);
        self.recover_interrupted_state_worktree(&path).map(|_| ())
    }

    pub(super) fn fetch_remote(&self, remote: &str) -> RefineResult<()> {
        self.git_checked(&["fetch", "--prune", remote]).map(|_| ())
    }

    pub(super) fn remote_state_exists(&self, remote: &str) -> RefineResult<bool> {
        Ok(self
            .git(&[
                "ls-remote",
                "--exit-code",
                "--heads",
                remote,
                REFINE_STATE_REF,
            ])?
            .success)
    }

    pub(super) fn remote_state_tracking_exists(&self, remote: &str) -> RefineResult<bool> {
        self.git_success(&[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{REFINE_STATE_BRANCH}"),
        ])
    }

    pub(super) fn fetch_state_branch(&self, remote: &str) -> RefineResult<()> {
        let destination = format!("refs/remotes/{remote}/{REFINE_STATE_BRANCH}");
        let refspec = format!("+{REFINE_STATE_REF}:{destination}");
        self.git_checked(&["fetch", remote, &refspec]).map(|_| ())
    }

    pub(super) fn ensure_state_worktree(
        &self,
        remote: &str,
        remote_exists: bool,
        live_refine: &std::path::Path,
    ) -> RefineResult<StateWorktreeSetup> {
        let path = state_worktree_for_target_root(&self.target_root)?;
        let valid = path.exists()
            && self
                .git_at(&path, &["rev-parse", "--is-inside-work-tree"])
                .is_ok_and(|output| output.success);
        if valid {
            let branch = self.git_at_stdout(&path, &["branch", "--show-current"])?;
            if branch == REFINE_STATE_BRANCH {
                return Ok(StateWorktreeSetup {
                    path,
                    pulled: false,
                    created: false,
                });
            }
            return Err(RefineError::Conflict(format!(
                "Refine state worktree is on unexpected branch {branch}"
            )));
        }

        self.git_checked(&["worktree", "prune"])?;
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to clean stale Refine state worktree {}: {error}",
                    path.display()
                ))
            })?;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create Refine state worktree parent {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let local_exists =
            self.git_success(&["show-ref", "--verify", "--quiet", REFINE_STATE_REF])?;
        if !local_exists && remote_exists {
            let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
            self.git_checked(&["branch", "--track", REFINE_STATE_BRANCH, &remote_ref])?;
        }
        if local_exists || remote_exists {
            self.git_checked(&[
                "worktree",
                "add",
                path.to_str().unwrap_or_default(),
                REFINE_STATE_BRANCH,
            ])?;
            return Ok(StateWorktreeSetup {
                path,
                pulled: remote_exists && !local_exists,
                created: false,
            });
        }

        self.git_checked(&[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap_or_default(),
            "HEAD",
        ])?;
        self.git_at_checked(&path, &["switch", "--orphan", REFINE_STATE_BRANCH])?;
        self.git_at_checked(&path, &["rm", "-rf", "--ignore-unmatch", "."])?;
        replace_live_durable_state(live_refine, &path.join(".refine"))?;
        if path.join(".refine").exists() {
            self.git_at_checked(&path, &["add", "-f", "-A", "--", ".refine"])?;
        }
        let initial = durable_state_map(&path.join(".refine"))?;
        let changes = state_change_status(&BTreeMap::new(), &initial);
        let message = if changes.is_empty() {
            "Initialize Refine state".to_string()
        } else {
            state_commit_summary(&changes.join("\n"))
        };
        self.git_at_checked(&path, &["commit", "--allow-empty", "-m", &message])?;
        Ok(StateWorktreeSetup {
            path,
            pulled: false,
            created: true,
        })
    }

    pub(super) fn ensure_local_state_excluded(&self) -> RefineResult<()> {
        let exclude = git_common_dir(&self.target_root)?.join("info/exclude");
        let current = fs::read_to_string(&exclude).unwrap_or_default();
        if !current.lines().any(|line| line.trim() == "/.refine/") {
            if let Some(parent) = exclude.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to create Git exclude directory {}: {error}",
                        parent.display()
                    ))
                })?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&exclude)
                .map_err(|error| {
                    RefineError::Io(format!(
                        "failed to open Git exclude file {}: {error}",
                        exclude.display()
                    ))
                })?;
            if !current.is_empty() && !current.ends_with('\n') {
                writeln!(file).map_err(|error| RefineError::Io(error.to_string()))?;
            }
            writeln!(
                file,
                "# Refine control state lives on {REFINE_STATE_BRANCH}\n/.refine/"
            )
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to update Git exclude file {}: {error}",
                    exclude.display()
                ))
            })?;
        }

        Ok(())
    }

    pub(super) fn configured_remote(&self, refine_dir: &std::path::Path) -> RefineResult<String> {
        let settings =
            FileSettingsService::with_active_root(refine_dir, &self.runtime_root).load()?;
        Ok(settings
            .get("git_remote")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|remote| !remote.is_empty())
            .unwrap_or(DEFAULT_REMOTE)
            .to_string())
    }
}
