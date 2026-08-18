use super::*;

#[derive(Clone, Debug)]
pub struct ShellReleaseHost {
    repo_root: PathBuf,
}

impl ShellReleaseHost {
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
        }
    }

    fn git(&self, args: &[&str]) -> RefineResult<String> {
        command_text(&self.repo_root, "git", args)
    }

    fn local_tag_commit(&self, tag: &str) -> RefineResult<Option<String>> {
        command_optional(
            &self.repo_root,
            "git",
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("refs/tags/{tag}^{{}}"),
            ],
        )
    }

    fn remote_tag_commit(&self, remote: &str, tag: &str) -> RefineResult<Option<String>> {
        let output = self.git(&[
            "ls-remote",
            remote,
            &format!("refs/tags/{tag}"),
            &format!("refs/tags/{tag}^{{}}"),
        ])?;
        Ok(output
            .lines()
            .find(|line| line.ends_with("^{}"))
            .or_else(|| output.lines().next())
            .and_then(|line| line.split_whitespace().next())
            .map(ToString::to_string))
    }

    fn github_release(&self, tag: &str) -> RefineResult<Option<Value>> {
        let Some(output) = command_optional(
            &self.repo_root,
            "gh",
            &["release", "view", tag, "--json", "url,tagName"],
        )?
        else {
            return Ok(None);
        };
        serde_json::from_str(&output).map(Some).map_err(|error| {
            RefineError::Serialization(format!("failed to parse GitHub release state: {error}"))
        })
    }
}

impl ReleaseHost for ShellReleaseHost {
    fn plan(&mut self, bump: ReleaseBump) -> RefineResult<ReleasePlan> {
        ensure_git_checkout(&self.repo_root)?;
        let current_version = read_package_version(&self.repo_root.join("Cargo.toml"))?;
        let proposed_version = bump_version(&current_version, bump)?;
        let previous_tag = latest_semver_tag(&self.repo_root)?;
        let range = previous_tag
            .as_ref()
            .map(|tag| format!("{tag}..HEAD"))
            .unwrap_or_else(|| "HEAD".to_string());
        let log = self.git(&["log", "--format=%H%x09%s", &range])?;
        let changes = log
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(commit, summary)| ReleaseChange {
                commit: commit.to_string(),
                summary: summary.to_string(),
                breaking: summary.contains("BREAKING CHANGE")
                    || summary.contains("!:")
                    || summary.starts_with("breaking:"),
            })
            .collect::<Vec<_>>();
        let breaking_changes = changes
            .iter()
            .filter(|change| change.breaking)
            .map(|change| change.summary.clone())
            .collect();
        let completed_goals = completed_goal_summaries(&self.repo_root)?;
        let mut version_files = vec!["Cargo.toml".to_string(), "Cargo.lock".to_string()];
        version_files.retain(|path| self.repo_root.join(path).is_file());
        let documentation_files = ["RELEASE_NOTES.md", "docs/story.md"]
            .into_iter()
            .filter(|path| self.repo_root.join(path).exists() || *path == "RELEASE_NOTES.md")
            .map(str::to_string)
            .collect();
        let gates = release_gate_commands(&self.repo_root);
        let tag_prefix = previous_tag
            .as_deref()
            .filter(|tag| tag.starts_with('v'))
            .map(|_| "v")
            .unwrap_or("");
        Ok(ReleasePlan {
            proposed_tag: format!("{tag_prefix}{proposed_version}"),
            current_version,
            proposed_version,
            previous_tag,
            bump,
            changes,
            completed_goals,
            breaking_changes,
            version_files,
            documentation_files,
            gates,
        })
    }

    fn preflight(
        &mut self,
        preparation: &TrustedPreparation,
    ) -> RefineResult<PublicationPreflight> {
        ensure_git_checkout(&self.repo_root)?;
        let branch = self.git(&["symbolic-ref", "--short", "HEAD"])?;
        if branch != preparation.target_branch {
            return Err(RefineError::Conflict(format!(
                "publication requires {}; current branch is {branch}",
                preparation.target_branch
            )));
        }
        if !self.git(&["status", "--porcelain"])?.is_empty() {
            return Err(RefineError::Conflict(
                "publication requires a clean target branch".to_string(),
            ));
        }
        let upstream = self.git(&[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])?;
        let (remote, upstream_branch) = upstream.split_once('/').ok_or_else(|| {
            RefineError::Conflict(format!("configured upstream {upstream} has no remote"))
        })?;
        self.git(&["fetch", "--no-tags", remote, upstream_branch])?;
        let main_commit = self.git(&["rev-parse", "HEAD"])?;
        let remote_commit = self.git(&["rev-parse", &format!("{remote}/{upstream_branch}")])?;
        if main_commit != remote_commit {
            return Err(RefineError::Conflict(format!(
                "publication requires synchronized {branch} and {upstream}"
            )));
        }
        let ancestry = Command::new("git")
            .args([
                "merge-base",
                "--is-ancestor",
                &preparation.candidate_commit,
                &main_commit,
            ])
            .current_dir(&self.repo_root)
            .status()
            .map_err(|error| RefineError::Io(format!("failed to check merge ancestry: {error}")))?;
        if !ancestry.success() {
            return Err(RefineError::Conflict(format!(
                "approved preparation commit {} is not merged into {branch}",
                preparation.candidate_commit
            )));
        }
        let version = read_package_version(&self.repo_root.join("Cargo.toml"))?;
        if version != preparation.version
            || preparation
                .tag
                .strip_prefix('v')
                .unwrap_or(&preparation.tag)
                != version
        {
            return Err(RefineError::Conflict(
                "merged version and semantic tag are not aligned with the trusted preparation"
                    .to_string(),
            ));
        }
        if !self.repo_root.join(&preparation.release_notes).is_file() {
            return Err(RefineError::Conflict(format!(
                "merged release notes {} were not found",
                preparation.release_notes
            )));
        }
        if let Some(commit) = self.local_tag_commit(&preparation.tag)?
            && commit != main_commit
        {
            return Err(RefineError::Conflict(format!(
                "local tag {} points at {commit}, expected {main_commit}",
                preparation.tag
            )));
        }
        if let Some(commit) = self.remote_tag_commit(remote, &preparation.tag)?
            && commit != main_commit
        {
            return Err(RefineError::Conflict(format!(
                "remote tag {} points at {commit}, expected {main_commit}",
                preparation.tag
            )));
        }
        command_text(&self.repo_root, "gh", &["auth", "status"])?;
        if let Some(release) = self.github_release(&preparation.tag)?
            && release.get("tagName").and_then(Value::as_str) != Some(&preparation.tag)
        {
            return Err(RefineError::Conflict(format!(
                "GitHub release for {} has conflicting identity",
                preparation.tag
            )));
        }
        Ok(PublicationPreflight {
            main_commit,
            remote: remote.to_string(),
            branch,
        })
    }

    fn ensure_local_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()> {
        match self.local_tag_commit(&preparation.tag)? {
            Some(commit) if commit == preflight.main_commit => Ok(()),
            Some(commit) => Err(RefineError::Conflict(format!(
                "local tag {} points at {commit}, expected {}",
                preparation.tag, preflight.main_commit
            ))),
            None => self
                .git(&[
                    "tag",
                    "-a",
                    &preparation.tag,
                    &preflight.main_commit,
                    "-m",
                    &format!("Release {}", preparation.version),
                ])
                .map(|_| ()),
        }
    }

    fn ensure_remote_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()> {
        match self.remote_tag_commit(&preflight.remote, &preparation.tag)? {
            Some(commit) if commit == preflight.main_commit => Ok(()),
            Some(commit) => Err(RefineError::Conflict(format!(
                "remote tag {} points at {commit}, expected {}",
                preparation.tag, preflight.main_commit
            ))),
            None => self
                .git(&["push", &preflight.remote, &preparation.tag])
                .map(|_| ()),
        }
    }

    fn ensure_github_release(
        &mut self,
        preparation: &TrustedPreparation,
        _preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        if let Some(release) = self.github_release(&preparation.tag)? {
            return release
                .get("url")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .ok_or_else(|| {
                    RefineError::Conflict("existing GitHub release has no URL".to_string())
                });
        }
        command_text(
            &self.repo_root,
            "gh",
            &[
                "release",
                "create",
                &preparation.tag,
                "--title",
                &preparation.tag,
                "--notes-file",
                &preparation.release_notes,
                "--verify-tag",
            ],
        )?;
        self.github_release(&preparation.tag)?
            .and_then(|release| {
                release
                    .get("url")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .ok_or_else(|| {
                RefineError::Degraded("created GitHub release could not be read back".to_string())
            })
    }

    fn observe_delivery(
        &mut self,
        _preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        let list = || {
            command_text(
                &self.repo_root,
                "gh",
                &[
                    "run",
                    "list",
                    "--commit",
                    &preflight.main_commit,
                    "--limit",
                    "20",
                    "--json",
                    "databaseId,name,status,conclusion,url",
                ],
            )
        };
        let configured = delivery_workflows_configured(&self.repo_root)?;
        let mut runs: Vec<Value> = Vec::new();
        for attempt in 0..3 {
            runs = serde_json::from_str(&list()?).map_err(|error| {
                RefineError::Serialization(format!("failed to parse workflow runs: {error}"))
            })?;
            if !runs.is_empty() || !configured || attempt == 2 {
                break;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        if runs.is_empty() {
            if configured {
                return Err(RefineError::Degraded(
                    "delivery workflows are configured, but GitHub reported no run for the release commit"
                        .to_string(),
                ));
            }
            return Ok(
                "No deployment or package workflows are configured for this release commit."
                    .to_string(),
            );
        }
        for run in &runs {
            if run.get("status").and_then(Value::as_str) != Some("completed") {
                let id = run
                    .get("databaseId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        RefineError::Serialization("workflow run has no databaseId".to_string())
                    })?;
                command_text(
                    &self.repo_root,
                    "gh",
                    &["run", "watch", &id.to_string(), "--exit-status"],
                )?;
            }
        }
        runs = serde_json::from_str(&list()?).map_err(|error| {
            RefineError::Serialization(format!("failed to parse terminal workflow runs: {error}"))
        })?;
        for run in &runs {
            let status = run.get("status").and_then(Value::as_str).unwrap_or("");
            let conclusion = run.get("conclusion").and_then(Value::as_str).unwrap_or("");
            if status != "completed" || !matches!(conclusion, "success" | "neutral" | "skipped") {
                return Err(RefineError::Degraded(format!(
                    "release workflow {} ended with status {status} and conclusion {conclusion}",
                    run.get("name").and_then(Value::as_str).unwrap_or("unknown")
                )));
            }
        }
        serde_json::to_string(&runs).map_err(|error| {
            RefineError::Serialization(format!("failed to encode workflow results: {error}"))
        })
    }

    fn verify(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        let remote = self
            .remote_tag_commit(&preflight.remote, &preparation.tag)?
            .ok_or_else(|| {
                RefineError::Degraded("published remote tag was not found".to_string())
            })?;
        if remote != preflight.main_commit {
            return Err(RefineError::Conflict(format!(
                "published tag resolves to {remote}, expected {}",
                preflight.main_commit
            )));
        }
        let release = self.github_release(&preparation.tag)?.ok_or_else(|| {
            RefineError::Degraded("published GitHub release was not found".to_string())
        })?;
        if release.get("tagName").and_then(Value::as_str) != Some(&preparation.tag) {
            return Err(RefineError::Conflict(
                "published GitHub release tag does not match".to_string(),
            ));
        }
        release
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| RefineError::Degraded("published GitHub release has no URL".to_string()))
    }
}
