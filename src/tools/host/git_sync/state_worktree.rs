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
                    local_ahead: self.local_state_ahead(remote, remote_exists)?,
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
                local_ahead: local_exists && self.local_state_ahead(remote, remote_exists)?,
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
            local_ahead: true,
            created: true,
        })
    }

    pub(super) fn local_state_ahead(
        &self,
        remote: &str,
        remote_exists: bool,
    ) -> RefineResult<bool> {
        if !remote_exists {
            return Ok(true);
        }
        let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
        let range = format!("{remote_ref}..{REFINE_STATE_REF}");
        Ok(self
            .git_stdout(&["rev-list", "--count", &range])?
            .parse::<usize>()
            .unwrap_or(0)
            > 0)
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

    pub(super) fn load_state_baseline(&self) -> RefineResult<Option<DurableStateMap>> {
        let path = git_common_dir(&self.target_root)?.join(STATE_BASELINE_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read Refine state baseline {}: {error}",
                    path.display()
                )));
            }
        };
        let stored = serde_json::from_slice::<BTreeMap<String, u64>>(&bytes).map_err(|error| {
            RefineError::Io(format!(
                "failed to parse Refine state baseline {}: {error}",
                path.display()
            ))
        })?;
        Ok(Some(
            stored
                .into_iter()
                .map(|(path, fingerprint)| (PathBuf::from(path), fingerprint))
                .collect(),
        ))
    }

    /// Recover the content represented by a persisted baseline fingerprint.
    ///
    /// Older baseline files intentionally contain hashes rather than a second
    /// copy of all durable state. A conflicting record's prior bytes are still
    /// available in the append-only `refine/state` history, so inspect only that
    /// record's revisions and select the exact hashed version. If history was
    /// rewritten or the hash cannot be reproduced, the caller retains the
    /// normal conflict instead of guessing.
    pub(super) fn load_baseline_file(
        &self,
        state_root: &std::path::Path,
        relative: &std::path::Path,
        fingerprint: u64,
    ) -> RefineResult<Option<Vec<u8>>> {
        let git_path = format!(".refine/{}", relative.to_string_lossy().replace('\\', "/"));
        let revisions = self.git_at_stdout(
            state_root,
            &["log", "--format=%H", "--all", "--", &git_path],
        )?;
        for revision in revisions.lines() {
            let object = format!("{revision}:{git_path}");
            let output = self.git_at(state_root, &["show", &object])?;
            if output.success && state_content_fingerprint(&output.stdout) == fingerprint {
                return Ok(Some(output.stdout));
            }
        }
        Ok(None)
    }

    pub(super) fn save_state_baseline(&self, baseline: &DurableStateMap) -> RefineResult<()> {
        let path = git_common_dir(&self.target_root)?.join(STATE_BASELINE_FILE);
        let stored = baseline
            .iter()
            .map(|(path, fingerprint)| (path.to_string_lossy().replace('\\', "/"), *fingerprint))
            .collect::<BTreeMap<_, _>>();
        let bytes = serde_json::to_vec_pretty(&stored).map_err(|error| {
            RefineError::Io(format!("failed to encode Refine state baseline: {error}"))
        })?;
        let temp = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            STATE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = File::create(&temp).map_err(|error| {
            RefineError::Io(format!(
                "failed to create Refine state baseline {}: {error}",
                temp.display()
            ))
        })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                let _ = fs::remove_file(&temp);
                RefineError::Io(format!(
                    "failed to write Refine state baseline {}: {error}",
                    temp.display()
                ))
            })?;
        fs::rename(&temp, &path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            RefineError::Io(format!(
                "failed to commit Refine state baseline {}: {error}",
                path.display()
            ))
        })
    }
}
