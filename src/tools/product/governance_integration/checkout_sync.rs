use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::process::subprocess::write_json_atomically;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::FileGitWorktreeService;

const CHECKOUT_SYNC_PENDING: &str = "refine-checkout-sync-pending.json";

/// Durable record of a checkout sync that a working-tree collision skipped.
/// While it exists, the branch ref already points at the integrated commit but
/// the checkout's index and files still hold the pre-integration content, so
/// the human checkout shows the integration delta as staged-reverse.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct PendingCheckoutSync {
    reference: String,
    from_commit: String,
    to_commit: String,
    reason: String,
    recorded_at: String,
}

pub(crate) enum CheckoutSyncOutcome {
    Synced,
    NotOnTarget,
    SkippedDirtyCollision { detail: String },
}

/// Mirror an already-applied target ref advance into the shared human
/// checkout's index and working tree.
///
/// The merge itself was computed in the detached integration worktree before
/// any ref motion, so only the index and files of the human checkout still
/// need catching up from `from_commit` to `to_commit`. A checkout on another
/// branch (or detached) needs nothing: it receives the new tip natively the
/// next time it switches to the target branch.
pub(crate) fn sync_human_checkout_after_ref_move(
    repo_git: &FileGitWorktreeService,
    target_root: &Path,
    target_branch: &str,
    from_commit: &str,
    to_commit: &str,
) -> RefineResult<CheckoutSyncOutcome> {
    let reference = format!("refs/heads/{target_branch}");
    if repo_git.symbolic_head_branch()?.as_deref() != Some(reference.as_str()) {
        return Ok(CheckoutSyncOutcome::NotOnTarget);
    }
    match repo_git.read_tree_merge_update(from_commit, to_commit) {
        Ok(()) => Ok(CheckoutSyncOutcome::Synced),
        Err(RefineError::Conflict(detail)) => {
            // An earlier skipped sync means the index still sits at that
            // marker's from_commit, not at this advance's; keep the older
            // baseline so the eventual repair replays the full pending delta.
            let from_commit = read_pending_marker(repo_git)?
                .filter(|pending| pending.reference == reference)
                .map(|pending| pending.from_commit)
                .unwrap_or_else(|| from_commit.to_string());
            let reason = format!(
                "a working-tree collision at {} made Git refuse the {from_commit}..{to_commit} sync ({detail}); \
                 while pending, the human checkout shows the integration delta as staged-reverse; \
                 repair_pending_checkout_sync retries the sync once the colliding files are committed, stashed, or restored",
                target_root.display()
            );
            let pending = PendingCheckoutSync {
                reference,
                from_commit,
                to_commit: to_commit.to_string(),
                reason,
                recorded_at: Utc::now().to_rfc3339(),
            };
            let encoded = serde_json::to_vec_pretty(&pending).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to encode pending checkout sync: {error}"
                ))
            })?;
            write_json_atomically(&pending_marker_path(repo_git)?, &encoded, "checkout sync")?;
            Ok(CheckoutSyncOutcome::SkippedDirtyCollision { detail })
        }
        Err(error) => Err(error),
    }
}

/// Retry a collision-skipped checkout sync. The marker survives until one
/// retry succeeds while the checkout is on the marker's branch; a sync of an
/// already-caught-up checkout is a Git no-op and also clears it.
pub(crate) fn repair_pending_checkout_sync(
    repo_git: &FileGitWorktreeService,
    _target_root: &Path,
) -> RefineResult<()> {
    let Some(pending) = read_pending_marker(repo_git)? else {
        return Ok(());
    };
    if repo_git.symbolic_head_branch()?.as_deref() != Some(pending.reference.as_str()) {
        // The staged-reverse state belongs to that branch's checkout; syncing
        // from another branch would splice the delta into unrelated content.
        return Ok(());
    }
    // The ref may have advanced again since the skip; sync to wherever it
    // points now so one repair covers every advance the collision blocked.
    let current_tip = repo_git.resolve_commit(&pending.reference)?;
    match repo_git.read_tree_merge_update(&pending.from_commit, &current_tip) {
        Ok(()) => remove_pending_marker(repo_git),
        Err(RefineError::Conflict(_)) => Ok(()),
        Err(error) => Err(error),
    }
}

fn pending_marker_path(repo_git: &FileGitWorktreeService) -> RefineResult<PathBuf> {
    repo_git.git_path(CHECKOUT_SYNC_PENDING)
}

fn read_pending_marker(
    repo_git: &FileGitWorktreeService,
) -> RefineResult<Option<PendingCheckoutSync>> {
    let path = pending_marker_path(repo_git)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read pending checkout sync {}: {error}",
                path.display()
            )));
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        RefineError::Serialization(format!(
            "invalid pending checkout sync {}: {error}",
            path.display()
        ))
    })
}

fn remove_pending_marker(repo_git: &FileGitWorktreeService) -> RefineResult<()> {
    let path = pending_marker_path(repo_git)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RefineError::Io(format!(
            "failed to remove pending checkout sync {}: {error}",
            path.display()
        ))),
    }
}
