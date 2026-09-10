//! Admission and revalidation of automated candidate checkouts.
use super::*;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ManagedWorktree {
    pub repository: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    pub commit: Option<String>,
    #[serde(default)]
    pub allow_rebase: bool,
    #[serde(default)]
    pub registration: Option<WorktreeRegistration>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorktreeRegistration {
    common_dir: PathBuf,
    git_dir: PathBuf,
    identity: String,
}

fn workspace_error(message: impl std::fmt::Display) -> RefineError {
    RefineError::Degraded(format!(
        "managed workspace unavailable: {message}; existing work was preserved"
    ))
}

fn canonical(path: &Path) -> RefineResult<PathBuf> {
    fs::canonicalize(path).map_err(|error| workspace_error(format!("{}: {error}", path.display())))
}

impl ManagedWorktree {
    /// Pin the physical registration after logical ownership has been admitted.
    /// Recovered invocations retain this value instead of adopting a replacement.
    pub fn pin(mut self) -> RefineResult<Self> {
        self.validate()?;
        self.registration = Some(self.observe_registration()?);
        Ok(self)
    }

    fn observe_registration(&self) -> RefineResult<WorktreeRegistration> {
        let git = FileGitWorktreeService::new(&self.path);
        let git_dir = canonical(Path::new(
            stdout(git.git_output(&["rev-parse", "--absolute-git-dir"])?)?.trim(),
        ))?;
        let metadata = fs::metadata(&git_dir).map_err(workspace_error)?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            format!(
                "{}:{}:{:?}",
                metadata.dev(),
                metadata.ino(),
                metadata.created().ok()
            )
        };
        #[cfg(not(unix))]
        let identity = format!("{:?}", metadata.created().map_err(workspace_error)?);
        Ok(WorktreeRegistration {
            common_dir: git.common_git_dir()?,
            git_dir,
            identity,
        })
    }

    pub fn validate(&self) -> RefineResult<()> {
        let git = FileGitWorktreeService::new(&self.repository);
        let expected = git.managed_worktree_path(&self.branch)?;
        // Resolve the repository first, then append the owned location. Resolving the whole
        // expected path would bless a managed-directory symlink into a human checkout.
        if canonical(&self.path)? != expected {
            return Err(workspace_error(format!(
                "{} is not the owned location {}",
                self.path.display(),
                expected.display()
            )));
        }
        git.admit_linked_worktree(
            &self.path,
            Some(&self.branch),
            self.commit.as_deref(),
            self.allow_rebase,
        )?;
        if let Some(expected) = &self.registration
            && &self.observe_registration()? != expected
        {
            return Err(workspace_error(
                "linked worktree registration was replaced; recover from the retained Round branch in a new invocation",
            ));
        }
        Ok(())
    }

    pub fn validate_cwd(&self, cwd: &Path) -> RefineResult<()> {
        self.validate()?;
        let root = canonical(&self.path)?;
        let cwd = canonical(cwd)?;
        if !cwd.is_dir() || !cwd.starts_with(&root) {
            return Err(workspace_error(
                "agent cwd must be an existing directory inside its admitted worktree",
            ));
        }
        // A nested repository is contained on disk but Git writes would target a different index.
        let actual = FileGitWorktreeService::new(&cwd).checkout_root()?;
        if actual != root {
            return Err(workspace_error(
                "agent cwd resolves to a different Git checkout",
            ));
        }
        Ok(())
    }
}

pub fn validate_workspace_launch(
    metadata: &Map<String, Value>,
    cwd: Option<&Path>,
) -> RefineResult<()> {
    if let Some(value) = metadata.get("managed_worktree") {
        let workspace: ManagedWorktree =
            serde_json::from_value(value.clone()).map_err(workspace_error)?;
        if workspace.registration.is_none() {
            return Err(workspace_error(
                "managed launch has no pinned registration; recover the Goal workspace before retrying",
            ));
        }
        workspace.validate_cwd(cwd.ok_or_else(|| workspace_error("managed launch has no cwd"))?)?;
    } else if metadata.contains_key("goal_id") && metadata.contains_key("workflow_state") {
        return Err(workspace_error(
            "Goal process has no admitted workspace; recover the Goal workspace before retrying",
        ));
    }
    Ok(())
}

impl FileGitWorktreeService {
    pub fn with_managed_worktree(mut self, workspace: ManagedWorktree) -> RefineResult<Self> {
        workspace.validate_cwd(&self.root)?;
        self.root = canonical(&self.root)?;
        self.managed_worktree = Some(workspace.pin()?);
        Ok(self)
    }

    pub fn common_git_dir(&self) -> RefineResult<PathBuf> {
        let raw = stdout(self.git_output(&["rev-parse", "--git-common-dir"])?)?;
        canonical(&self.root.join(raw.trim()))
    }

    fn checkout_root(&self) -> RefineResult<PathBuf> {
        let raw = stdout(self.git_output(&["rev-parse", "--show-toplevel"])?)?;
        canonical(Path::new(raw.trim()))
    }

    pub fn managed_worktree_path(&self, branch: &str) -> RefineResult<PathBuf> {
        validate_branch_name(branch)?;
        let parent = self.common_git_dir()?.join("refine-worktrees");
        if parent.exists() && canonical(&parent)? != parent {
            return Err(workspace_error("managed worktree parent was redirected"));
        }
        Ok(parent.join(branch.replace('/', "-")))
    }

    /// The requested path is the ownership commitment. A matching branch elsewhere is
    /// never permission to adopt, lock, switch, reset, or remove that checkout.
    pub fn admit_linked_worktree(
        &self,
        path: &Path,
        branch: Option<&str>,
        commit: Option<&str>,
        allow_rebase: bool,
    ) -> RefineResult<()> {
        let path = canonical(path)?;
        let local = FileGitWorktreeService::new(&path);
        let common = self.common_git_dir()?;
        let git_dir = canonical(
            &path.join(stdout(local.git_output(&["rev-parse", "--absolute-git-dir"])?)?.trim()),
        )?;
        if git_dir == common || local.common_git_dir()? != common || local.checkout_root()? != path
        {
            return Err(workspace_error(
                "checkout is primary, nested, or belongs to another repository",
            ));
        }
        // Verify both sides of the linked-checkout registration, including its backlink.
        if !git_dir.starts_with(common.join("worktrees"))
            || canonical(Path::new(
                fs::read_to_string(git_dir.join("gitdir"))
                    .map_err(workspace_error)?
                    .trim(),
            ))? != path.join(".git")
            || !self
                .list_linked_worktrees()?
                .iter()
                .any(|entry| same_existing_path(&entry.path, &path))
        {
            return Err(workspace_error("linked checkout registration changed"));
        }
        if branch.is_some() && git_dir.join("locked").exists() {
            let reason = fs::read_to_string(git_dir.join("locked")).map_err(workspace_error)?;
            if reason.trim() != CANDIDATE_WORKTREE_LOCK_REASON {
                return Err(workspace_error(
                    "checkout has a conflicting Git worktree lock owner",
                ));
            }
        }
        let head = local.head_ref()?;
        let rebasing_branch = if allow_rebase {
            ["rebase-merge/head-name", "rebase-apply/head-name"]
                .iter()
                .find_map(|name| fs::read_to_string(git_dir.join(name)).ok())
        } else {
            None
        };
        let branch_matches = head.branch.as_deref() == branch
            || (head.branch.is_none()
                && branch.is_some_and(|branch| {
                    rebasing_branch.as_deref().map(str::trim)
                        == Some(format!("refs/heads/{branch}").as_str())
                }));
        if !branch_matches || commit.is_some_and(|commit| head.commit.as_deref() != Some(commit)) {
            return Err(workspace_error("checkout branch or commit changed"));
        }
        Ok(())
    }

    pub(super) fn check_worktree_reuse_target(
        &self,
        existing: &Path,
        target: &Path,
    ) -> RefineResult<()> {
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.root.join(target)
        };
        let matches = if existing.exists() && target.exists() {
            same_existing_path(existing, &target)
        } else {
            existing == target
        };
        if !matches {
            return Err(workspace_error(format!(
                "branch is registered at {}, expected {}",
                existing.display(),
                target.display()
            )));
        }
        Ok(())
    }
}

pub fn agent_worktree_cwd(worktree_path: &str, agent_subpath: &str) -> RefineResult<PathBuf> {
    let root = std::fs::canonicalize(worktree_path).map_err(|error| {
        RefineError::Degraded(format!("managed worktree is unavailable: {error}"))
    })?;
    let relative = Path::new(agent_subpath.trim());
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(RefineError::Degraded(
            "agent_subpath must be a relative path inside the worktree".to_string(),
        ));
    }
    let cwd = std::fs::canonicalize(root.join(relative)).map_err(|error| {
        RefineError::Degraded(format!(
            "agent_subpath must name an existing directory inside the worktree: {error}"
        ))
    })?;
    if !cwd.is_dir() || !cwd.starts_with(&root) {
        return Err(RefineError::Degraded(
            "agent_subpath must resolve to a directory inside the worktree".to_string(),
        ));
    }
    Ok(cwd)
}
