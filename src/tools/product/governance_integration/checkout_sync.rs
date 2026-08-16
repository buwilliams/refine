use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::process::subprocess::write_json_atomically;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::FileGitWorktreeService;

const CHECKOUT_SYNC_PENDING: &str = "refine-checkout-sync-pending.json";

/// Durable record of a checkout sync that a working-tree collision (or a
/// non-collision sync failure) skipped. While it exists, the branch ref
/// already points at the integrated commit but the checkout's index and files
/// still hold the pre-integration content, so the human checkout shows the
/// integration delta as staged-reverse. The marker file holds one record per
/// target branch: goals in the same repository may target different branches,
/// and one branch's collision must never discard another's pending repair.
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

/// True only for `git read-tree -m -u`'s refusal messages for working-tree or
/// index collisions. Every other failure — `index.lock` contention from the
/// human's own tooling, missing objects, repository corruption — is a real
/// error: reporting it as a collision would tell the human to commit or stash
/// files that do not collide, and success-with-marker would hide it forever.
fn is_working_tree_collision(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("would be overwritten") || lower.contains("not uptodate")
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
        Err(RefineError::Conflict(detail)) if is_working_tree_collision(&detail) => {
            record_pending_sync(
                repo_git,
                &reference,
                from_commit,
                to_commit,
                |from_commit| {
                    format!(
                        "a working-tree collision at {} made Git refuse the {from_commit}..{to_commit} sync ({detail}); \
                     while pending, the human checkout shows the integration delta as staged-reverse; \
                     repair_pending_checkout_sync retries the sync once the colliding files are committed, stashed, or restored",
                        target_root.display()
                    )
                },
            )?;
            Ok(CheckoutSyncOutcome::SkippedDirtyCollision { detail })
        }
        Err(error) => {
            // The ref already moved, so the missed sync must survive this
            // failure durably: record it for repair before surfacing the
            // error, so a transient cause (the human's tooling holding
            // `index.lock`, say) self-heals on a later repair pass.
            let recorded = record_pending_sync(
                repo_git,
                &reference,
                from_commit,
                to_commit,
                |from_commit| {
                    format!(
                        "the {from_commit}..{to_commit} sync at {} failed without a working-tree collision ({error}); \
                         while pending, the human checkout shows the integration delta as staged-reverse; \
                         repair_pending_checkout_sync retries the sync",
                        target_root.display()
                    )
                },
            );
            if let Err(marker_error) = recorded {
                eprintln!(
                    "refine: failed to record the pending checkout sync after a sync failure: {marker_error}"
                );
            }
            Err(error)
        }
    }
}

/// Retry a skipped checkout sync for the branch the checkout is on. The
/// record survives until one retry succeeds while the checkout is on the
/// record's branch; a sync of an already-caught-up checkout is a Git no-op
/// and also clears it. Records for other branches are left untouched.
pub(crate) fn repair_pending_checkout_sync(
    repo_git: &FileGitWorktreeService,
    _target_root: &Path,
) -> RefineResult<()> {
    let pending = read_pending_records(repo_git)?;
    if pending.is_empty() {
        return Ok(());
    }
    let Some(head) = repo_git.symbolic_head_branch()? else {
        return Ok(());
    };
    let Some(record) = pending.iter().find(|record| record.reference == head) else {
        // The staged-reverse state belongs to that branch's checkout; syncing
        // from another branch would splice the delta into unrelated content.
        return Ok(());
    };
    // The ref may have advanced again since the skip; sync to wherever it
    // points now so one repair covers every advance the collision blocked.
    let current_tip = repo_git.resolve_commit(&record.reference)?;
    match repo_git.read_tree_merge_update(&record.from_commit, &current_tip) {
        Ok(()) => remove_pending_record(repo_git, &head),
        // Still colliding: keep the record and retry later. Any other
        // failure surfaces instead of silently retrying forever.
        Err(RefineError::Conflict(detail)) if is_working_tree_collision(&detail) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Upsert the pending record for `reference`, preserving both the other
/// branches' records and an existing record's older `from_commit`: an earlier
/// skipped sync means the index still sits at that record's baseline, so the
/// eventual repair must replay the full pending delta. `reason` receives the
/// effective baseline commit.
fn record_pending_sync(
    repo_git: &FileGitWorktreeService,
    reference: &str,
    from_commit: &str,
    to_commit: &str,
    reason: impl FnOnce(&str) -> String,
) -> RefineResult<()> {
    let mut pending = read_pending_records(repo_git)?;
    let from_commit = pending
        .iter()
        .find(|record| record.reference == reference)
        .map(|record| record.from_commit.clone())
        .unwrap_or_else(|| from_commit.to_string());
    let reason = reason(&from_commit);
    pending.retain(|record| record.reference != reference);
    pending.push(PendingCheckoutSync {
        reference: reference.to_string(),
        from_commit,
        to_commit: to_commit.to_string(),
        reason,
        recorded_at: Utc::now().to_rfc3339(),
    });
    write_pending_records(repo_git, &pending)
}

fn pending_marker_path(repo_git: &FileGitWorktreeService) -> RefineResult<PathBuf> {
    repo_git.git_path(CHECKOUT_SYNC_PENDING)
}

fn read_pending_records(
    repo_git: &FileGitWorktreeService,
) -> RefineResult<Vec<PendingCheckoutSync>> {
    let path = pending_marker_path(repo_git)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read pending checkout sync {}: {error}",
                path.display()
            )));
        }
    };
    let invalid = |error| {
        RefineError::Serialization(format!(
            "invalid pending checkout sync {}: {error}",
            path.display()
        ))
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(invalid)?;
    // Deployed single-record builds wrote one object, not an array.
    if value.is_array() {
        serde_json::from_value(value).map_err(invalid)
    } else {
        serde_json::from_value(value)
            .map(|record| vec![record])
            .map_err(invalid)
    }
}

fn write_pending_records(
    repo_git: &FileGitWorktreeService,
    records: &[PendingCheckoutSync],
) -> RefineResult<()> {
    let encoded = serde_json::to_vec_pretty(records).map_err(|error| {
        RefineError::Serialization(format!("failed to encode pending checkout sync: {error}"))
    })?;
    write_json_atomically(&pending_marker_path(repo_git)?, &encoded, "checkout sync")
}

fn remove_pending_record(repo_git: &FileGitWorktreeService, reference: &str) -> RefineResult<()> {
    let mut pending = read_pending_records(repo_git)?;
    pending.retain(|record| record.reference != reference);
    if !pending.is_empty() {
        return write_pending_records(repo_git, &pending);
    }
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
