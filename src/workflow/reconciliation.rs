use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::process::subprocess::write_json_atomically;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitStatus, GitWorktreeService};
use crate::workflow::now_timestamp;

const INTEGRATED_TARGET_RECONCILIATION_LOCK: &str = "refine-reconciliation.lock";
const INTEGRATED_TARGET_TRANSACTION: &str = "refine-integrated-target-transaction.json";
const INTEGRATED_TARGET_RECOVERIES: &str = "refine-integrated-target-recoveries.jsonl";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct IntegratedTargetTransaction {
    goal_id: String,
    round_idx: usize,
    branch: String,
    commit: String,
    started_at: String,
    /// Untracked paths already present when the lane acquired the checkout.
    /// They are a user's files and stay untouched; any untracked path beyond
    /// this baseline appeared inside the marker window and is therefore
    /// Refine residue that quarantine must capture. Markers written before
    /// this field existed decode to an empty baseline, which errs toward
    /// quarantining (into a recoverable stash) rather than leaking.
    #[serde(default)]
    untracked_baseline: Vec<String>,
}

/// A repository-wide lease for integration, build, Quality, and settlement against the shared
/// target checkout.
///
/// A separate lock from the short-lived repository Git lock lets one Goal own the checkout across
/// supervised operations while Git commands inside the lane retain normal coordination. The
/// durable transaction marker proves ownership of residue after a process interruption: a later
/// owner quarantines that residue before starting instead of failing an unrelated Goal.
pub(super) struct IntegratedTargetWorkflowLease {
    file: File,
    target_root: std::path::PathBuf,
    marker_path: std::path::PathBuf,
    untracked_baseline: Vec<String>,
    finished: bool,
}

impl IntegratedTargetWorkflowLease {
    pub(super) fn acquire(
        target_root: &Path,
        goal_id: &str,
        round_idx: usize,
    ) -> RefineResult<Self> {
        let git = FileGitWorktreeService::new(target_root);
        let path = git.git_path(INTEGRATED_TARGET_RECONCILIATION_LOCK)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open integrated-target reconciliation lock {}: {error}",
                    path.display()
                ))
            })?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to acquire integrated-target reconciliation lock for {}: {error}",
                target_root.display()
            ))
        })?;
        let marker_path = git.git_path(INTEGRATED_TARGET_TRANSACTION)?;
        recover_interrupted_transaction(&git, &marker_path, goal_id, round_idx)?;
        let status = git.inspect(target_root.to_str().unwrap_or(""))?;
        if status.dirty_user_changes {
            // A hold, not a failure: a human's in-flight tracked changes in the
            // shared checkout block integration only until they are committed
            // or stashed, so the Goal must stay claimable rather than settle.
            return Err(RefineError::WorkspaceHold(format!(
                "integrated-target workflow lane found unattributed tracked changes at {} ({}); user work was preserved",
                target_root.display(),
                status.describe_dirt()
            )));
        }
        let head = git.head_ref()?;
        let transaction = IntegratedTargetTransaction {
            goal_id: goal_id.to_string(),
            round_idx,
            branch: head.branch.unwrap_or_default(),
            commit: head.commit.unwrap_or_default(),
            started_at: now_timestamp(),
            untracked_baseline: status.untracked_paths.clone(),
        };
        let encoded = serde_json::to_vec_pretty(&transaction).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode integrated-target transaction: {error}"
            ))
        })?;
        write_json_atomically(&marker_path, &encoded, "integrated-target transaction")?;
        Ok(Self {
            file,
            target_root: target_root.to_path_buf(),
            marker_path,
            untracked_baseline: transaction.untracked_baseline,
            finished: false,
        })
    }

    pub(super) fn finish(&mut self) -> RefineResult<()> {
        if self.finished {
            return Ok(());
        }
        let git = FileGitWorktreeService::new(&self.target_root);
        let status = git.inspect(self.target_root.to_str().unwrap_or(""))?;
        // Untracked files beyond the acquire-time baseline appeared inside
        // the marker window, so they are lane residue even though they never
        // reached the index.
        let untracked_residue = untracked_beyond_baseline(&status, &self.untracked_baseline);
        if status.dirty_user_changes || !untracked_residue.is_empty() {
            return Err(RefineError::Conflict(format!(
                "integrated-target workflow left a dirty index or worktree at {} ({}); residue remains attributed to the owning Goal for recovery",
                self.target_root.display(),
                status.describe_dirt()
            )));
        }
        remove_marker(&self.marker_path)?;
        self.finished = true;
        Ok(())
    }

    pub(super) fn finish_if_clean(&mut self) {
        let _ = self.finish();
    }
}

impl Drop for IntegratedTargetWorkflowLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn recover_interrupted_transaction(
    git: &FileGitWorktreeService,
    marker_path: &Path,
    next_goal_id: &str,
    next_round_idx: usize,
) -> RefineResult<()> {
    let Some(bytes) = fs::read(marker_path)
        .map(Some)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            }
        })
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to read integrated-target transaction {}: {error}",
                marker_path.display()
            ))
        })?
    else {
        return Ok(());
    };
    let interrupted: IntegratedTargetTransaction =
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "invalid integrated-target transaction {}: {error}; checkout was preserved",
                marker_path.display()
            ))
        })?;
    let status = git.inspect("")?;
    // The marker proves the dirt window belonged to the interrupted Goal, so
    // untracked files beyond its acquire-time baseline are Refine residue and
    // must be quarantined alongside tracked changes — not silently
    // reattributed to the user.
    let untracked_residue = untracked_beyond_baseline(&status, &interrupted.untracked_baseline);
    let stash_commit = if status.dirty_user_changes || !untracked_residue.is_empty() {
        git.quarantine_worktree_changes(
            &format!(
                "Refine quarantined interrupted integrated-target residue from Goal {} round {}",
                interrupted.goal_id,
                interrupted.round_idx + 1
            ),
            &untracked_residue,
        )?
    } else {
        None
    };
    let recoveries_path = git.git_path(INTEGRATED_TARGET_RECOVERIES)?;
    if let Some(parent) = recoveries_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create integrated-target recovery directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let mut recoveries = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&recoveries_path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open integrated-target recovery log {}: {error}",
                recoveries_path.display()
            ))
        })?;
    let event = json!({
        "recovered_at": now_timestamp(),
        "interrupted": interrupted,
        "stash_commit": stash_commit,
        "untracked_residue": untracked_residue,
        "next_owner": {"goal_id": next_goal_id, "round_idx": next_round_idx}
    });
    serde_json::to_writer(&mut recoveries, &event).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode integrated-target recovery event: {error}"
        ))
    })?;
    recoveries.write_all(b"\n").map_err(|error| {
        RefineError::Io(format!(
            "failed to append integrated-target recovery log {}: {error}",
            recoveries_path.display()
        ))
    })?;
    recoveries.sync_all().map_err(|error| {
        RefineError::Io(format!(
            "failed to synchronize integrated-target recovery log {}: {error}",
            recoveries_path.display()
        ))
    })?;
    remove_marker(marker_path)
}

/// Untracked paths that appeared after the recorded acquire-time baseline:
/// the lane's own residue, as opposed to a user's pre-existing files.
fn untracked_beyond_baseline(status: &GitStatus, baseline: &[String]) -> Vec<String> {
    status
        .untracked_paths
        .iter()
        .filter(|path| !baseline.contains(path))
        .cloned()
        .collect()
}

fn remove_marker(marker_path: &Path) -> RefineResult<()> {
    match fs::remove_file(marker_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RefineError::Io(format!(
            "failed to remove integrated-target transaction {}: {error}",
            marker_path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::Value;

    use super::*;

    fn repository(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "refine-integrated-target-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "refine-test@example.invalid"],
            vec!["config", "user.name", "Refine Test"],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(root.join("app.txt"), "base\n").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["add", "app.txt"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["commit", "-q", "-m", "base"])
                .status()
                .unwrap()
                .success()
        );
        root
    }

    #[test]
    fn completed_integrated_target_lane_removes_its_transaction_marker() {
        let root = repository("finish");
        let marker = FileGitWorktreeService::new(&root)
            .git_path(INTEGRATED_TARGET_TRANSACTION)
            .unwrap();
        let mut lane = IntegratedTargetWorkflowLease::acquire(&root, "GOAL1", 0).unwrap();
        assert!(marker.exists());

        lane.finish().unwrap();

        assert!(!marker.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lane_acquires_despite_untracked_user_files_in_the_shared_checkout() {
        let root = repository("untracked");
        fs::write(root.join("scratch-notes.txt"), "human scratch file\n").unwrap();

        let mut lane = IntegratedTargetWorkflowLease::acquire(&root, "GOAL1", 0).unwrap();
        lane.finish().unwrap();

        // The untracked file was neither integration-blocking nor touched.
        assert_eq!(
            fs::read_to_string(root.join("scratch-notes.txt")).unwrap(),
            "human scratch file\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lane_holds_on_tracked_user_changes_and_names_the_blocking_path() {
        let root = repository("tracked-dirt");
        fs::write(root.join("app.txt"), "uncommitted human edit\n").unwrap();

        let error = match IntegratedTargetWorkflowLease::acquire(&root, "GOAL1", 0) {
            Ok(_) => panic!("acquire succeeded despite tracked user changes"),
            Err(error) => error,
        };

        match &error {
            RefineError::WorkspaceHold(message) => {
                assert!(message.contains("app.txt"), "{message}");
                assert!(message.ends_with("; user work was preserved"), "{message}");
            }
            other => panic!("expected WorkspaceHold, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap(),
            "uncommitted human edit\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn next_lane_quarantines_residue_owned_by_an_interrupted_goal() {
        let root = repository("recovery");
        let git = FileGitWorktreeService::new(&root);
        let recoveries = git.git_path(INTEGRATED_TARGET_RECOVERIES).unwrap();
        {
            let _interrupted = IntegratedTargetWorkflowLease::acquire(&root, "GOAL1", 0).unwrap();
            fs::write(root.join("app.txt"), "interrupted mutation\n").unwrap();
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(["add", "app.txt"])
                    .status()
                    .unwrap()
                    .success()
            );
        }

        let mut successor = IntegratedTargetWorkflowLease::acquire(&root, "GOAL2", 1).unwrap();

        assert_eq!(fs::read_to_string(root.join("app.txt")).unwrap(), "base\n");
        let status = git.inspect("").unwrap();
        assert!(!status.dirty_user_changes);
        let event: Value = serde_json::from_str(
            fs::read_to_string(&recoveries)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(event["interrupted"]["goal_id"], "GOAL1");
        assert_eq!(event["next_owner"]["goal_id"], "GOAL2");
        assert!(event["stash_commit"].as_str().is_some());
        successor.finish().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn next_lane_quarantines_untracked_residue_but_preserves_the_users_files() {
        let root = repository("untracked-recovery");
        let git = FileGitWorktreeService::new(&root);
        let recoveries = git.git_path(INTEGRATED_TARGET_RECOVERIES).unwrap();
        fs::write(root.join("scratch-notes.txt"), "human scratch file\n").unwrap();
        {
            let _interrupted = IntegratedTargetWorkflowLease::acquire(&root, "GOAL1", 0).unwrap();
            // The interrupted lane wrote a file it never `git add`ed: the
            // marker attributes it to GOAL1, so it must be quarantined, not
            // silently reattributed to the user.
            fs::write(root.join("residue.txt"), "interrupted untracked residue\n").unwrap();
        }

        let mut successor = IntegratedTargetWorkflowLease::acquire(&root, "GOAL2", 1).unwrap();

        assert!(!root.join("residue.txt").exists());
        assert_eq!(
            fs::read_to_string(root.join("scratch-notes.txt")).unwrap(),
            "human scratch file\n"
        );
        let event: Value = serde_json::from_str(
            fs::read_to_string(&recoveries)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(event["interrupted"]["goal_id"], "GOAL1");
        assert!(event["stash_commit"].as_str().is_some());
        assert_eq!(
            event["untracked_residue"],
            serde_json::json!(["residue.txt"])
        );
        successor.finish().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn next_lane_quarantines_mixed_tracked_and_untracked_residue_together() {
        let root = repository("mixed-recovery");
        let git = FileGitWorktreeService::new(&root);
        {
            let _interrupted = IntegratedTargetWorkflowLease::acquire(&root, "GOAL1", 0).unwrap();
            fs::write(root.join("app.txt"), "interrupted mutation\n").unwrap();
            fs::write(root.join("residue.txt"), "interrupted untracked residue\n").unwrap();
        }

        let mut successor = IntegratedTargetWorkflowLease::acquire(&root, "GOAL2", 1).unwrap();

        assert_eq!(fs::read_to_string(root.join("app.txt")).unwrap(), "base\n");
        assert!(!root.join("residue.txt").exists());
        let status = git.inspect("").unwrap();
        assert!(!status.dirty_user_changes);
        assert!(status.untracked_paths.is_empty(), "{status:?}");
        successor.finish().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finish_keeps_the_marker_when_the_lane_leaves_untracked_residue() {
        let root = repository("finish-residue");
        let marker = FileGitWorktreeService::new(&root)
            .git_path(INTEGRATED_TARGET_TRANSACTION)
            .unwrap();
        let mut lane = IntegratedTargetWorkflowLease::acquire(&root, "GOAL1", 0).unwrap();
        fs::write(root.join("residue.txt"), "lane residue\n").unwrap();

        let error = lane.finish().unwrap_err();

        match &error {
            RefineError::Conflict(message) => {
                assert!(message.contains("residue.txt"), "{message}");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // The marker survives so the next acquire quarantines the residue
        // while it is still attributed to this Goal.
        assert!(marker.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
