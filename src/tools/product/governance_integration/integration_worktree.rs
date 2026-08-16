use std::path::{Path, PathBuf};

use crate::process::supervisor::errors::RefineResult;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};

/// Refine-owned detached worktree where integrated-target porcelain runs, so
/// the shared human checkout never has to be clean for an integration.
pub(crate) struct IntegrationWorktree {
    // Production callers act through `git()`; tests locate the tree on disk.
    #[cfg_attr(not(test), allow(dead_code))]
    pub path: PathBuf,
    git: FileGitWorktreeService,
}

impl IntegrationWorktree {
    pub(crate) fn git(&self) -> &FileGitWorktreeService {
        &self.git
    }
}

/// Ensure the persistent integration worktree exists, is detached at
/// `anchor_commit`, is locked, and is pristine.
///
/// The tree lives at `.git/refine-integration/target` — deliberately OUTSIDE
/// `.git/refine-worktrees`, because the cleanup sweep only scans that managed
/// root (and refuses detached worktrees anyway). It persists between
/// integrations: large checkouts make `worktree add` expensive.
pub(crate) fn ensure_integration_worktree(
    repo_git: &FileGitWorktreeService,
    runtime_root: &Path,
    anchor_commit: &str,
) -> RefineResult<IntegrationWorktree> {
    let path = integration_worktree_path(repo_git)?;
    let git = FileGitWorktreeService::with_runtime_root(&path, runtime_root);
    let valid = path.exists() && git.is_inside_work_tree().unwrap_or(false);
    if valid {
        let pristine = git
            .checkout_detached(anchor_commit)
            .and_then(|_| git.inspect(""))
            .map(|status| status.is_pristine())
            .unwrap_or(false);
        if pristine {
            return Ok(IntegrationWorktree { path, git });
        }
        // This tree is Refine-owned, so residue is never user work: recreate
        // instead of quarantining. `worktree remove --force` refuses a locked
        // worktree, hence the unlock first.
        let _ = repo_git.unlock_worktree(&path);
        repo_git.remove_worktree(&path, true)?;
    }
    repo_git.ensure_detached_worktree(&path, anchor_commit)?;
    Ok(IntegrationWorktree { path, git })
}

/// Filesystem location of the persistent integration worktree.
pub(crate) fn integration_worktree_path(
    repo_git: &FileGitWorktreeService,
) -> RefineResult<PathBuf> {
    Ok(repo_git.git_path("refine-integration")?.join("target"))
}

/// Tear the integration worktree down so the next integration recreates it
/// fresh. The tree is Refine-owned, so nothing in it is ever user work — no
/// stash, no quarantine. Returns the removed path, or `None` when no tree
/// existed.
pub(crate) fn remove_integration_worktree(
    repo_git: &FileGitWorktreeService,
) -> RefineResult<Option<PathBuf>> {
    let path = integration_worktree_path(repo_git)?;
    if !path.exists() {
        return Ok(None);
    }
    // `worktree remove --force` refuses a locked worktree, hence the
    // best-effort unlock first.
    let _ = repo_git.unlock_worktree(&path);
    repo_git.remove_worktree(&path, true)?;
    Ok(Some(path))
}
