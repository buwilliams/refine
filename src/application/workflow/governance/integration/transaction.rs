//! Durable integrated-target transaction lifecycle: the marker proving which
//! Goal owns residue across process interruptions, the post-CAS
//! checkout-sync window stamped onto it, and recovery of interrupted
//! transactions. The workflow's integrated-target lease serializes callers
//! and delegates one-way into this module; the marker itself is product
//! state and lives here alongside `checkout_sync` and `integration_worktree`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::infrastructure::process::subprocess::write_json_atomically;

use super::checkout_sync::{
    CheckoutSyncOutcome, repair_pending_checkout_sync, sync_human_checkout_after_ref_move,
};
use super::integration_worktree::{integration_worktree_path, remove_integration_worktree};

const INTEGRATED_TARGET_TRANSACTION: &str = "refine-integrated-target-transaction.json";
const INTEGRATED_TARGET_RECOVERIES: &str = "refine-integrated-target-recoveries.jsonl";

/// Which workspace the marker's owning lane mutated. Markers written before
/// integration porcelain moved off the shared checkout lack the field and
/// decode to `SharedCheckout`, which routes recovery through the legacy
/// quarantine path.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TransactionWorkspace {
    #[default]
    SharedCheckout,
    IntegrationWorktree,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct IntegratedTargetTransaction {
    goal_id: String,
    round_idx: usize,
    /// Legacy shared-checkout markers recorded the checkout's head position;
    /// the integration-worktree lane neither records nor depends on it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    commit: String,
    started_at: String,
    #[serde(default)]
    workspace: TransactionWorkspace,
    /// Open only while the post-CAS human-checkout sync runs, so an
    /// interruption there is attributed to the sync phase — the one moment the
    /// integration-worktree lane touches the shared checkout — and recovery
    /// re-attempts exactly that sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout_sync: Option<CheckoutSyncWindow>,
}

/// The post-CAS window in which the target ref already points at the
/// integrated commit but the shared checkout's index and files may not.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CheckoutSyncWindow {
    pub(crate) branch: String,
    pub(crate) from: String,
    pub(crate) to: String,
}

/// Recovers any interrupted transaction, repairs a pending checkout sync
/// from an earlier integration, and opens a fresh durable marker owned by
/// `goal_id`/`round_idx`. The caller must already hold the integrated-target
/// lease that serializes this lifecycle across processes.
pub(crate) fn open_integrated_target_transaction(
    target_root: &Path,
    goal_id: &str,
    round_idx: usize,
) -> RefineResult<()> {
    let git = FileGitWorktreeService::new(target_root);
    let marker_path = git.git_path(INTEGRATED_TARGET_TRANSACTION)?;
    recover_interrupted_transaction(&git, target_root, &marker_path, goal_id, round_idx)?;
    // A collision-skipped human-checkout sync from an earlier integration
    // self-heals here, before this lane opens its own marker window.
    // Best-effort: still-colliding dirt must not block integration, which
    // no longer needs the shared checkout to be clean — or even present on
    // the target branch — at all.
    if let Err(error) = repair_pending_checkout_sync(&git, target_root) {
        eprintln!("refine: pending checkout sync repair failed: {error}");
    }
    let transaction = IntegratedTargetTransaction {
        goal_id: goal_id.to_string(),
        round_idx,
        branch: String::new(),
        commit: String::new(),
        started_at: now_timestamp(),
        workspace: TransactionWorkspace::IntegrationWorktree,
        checkout_sync: None,
    };
    let encoded = serde_json::to_vec_pretty(&transaction).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode integrated-target transaction: {error}"
        ))
    })?;
    write_json_atomically(&marker_path, &encoded, "integrated-target transaction")
}

/// Closes the transaction opened by `open_integrated_target_transaction`.
/// Lane residue can only live in the Refine-owned integration worktree, and
/// `ensure_integration_worktree` refuses to reuse a non-pristine tree, so
/// residue is surfaced log-only instead of holding the marker open: the next
/// integration recreates the worktree wholesale.
pub(crate) fn finish_integrated_target_transaction(target_root: &Path) -> RefineResult<()> {
    let git = FileGitWorktreeService::new(target_root);
    match integration_worktree_dirt(&git) {
        Ok(None) => {}
        Ok(Some(dirt)) => eprintln!(
            "refine: integrated-target lane left residue in the integration worktree ({dirt}); the next integration recreates it"
        ),
        Err(error) => eprintln!(
            "refine: integrated-target lane could not inspect the integration worktree: {error}"
        ),
    }
    remove_marker(&git.git_path(INTEGRATED_TARGET_TRANSACTION)?)
}

/// Open the post-CAS checkout-sync window on the lane's transaction marker so
/// an interruption inside the sync is attributed to that exact phase. A
/// missing marker means integration is running outside the integrated-target
/// lane, where there is nothing to attribute.
pub(crate) fn record_checkout_sync_window(
    target_root: &Path,
    window: CheckoutSyncWindow,
) -> RefineResult<()> {
    update_checkout_sync_window(target_root, Some(window))
}

pub(crate) fn clear_checkout_sync_window(target_root: &Path) -> RefineResult<()> {
    update_checkout_sync_window(target_root, None)
}

fn update_checkout_sync_window(
    target_root: &Path,
    window: Option<CheckoutSyncWindow>,
) -> RefineResult<()> {
    let git = FileGitWorktreeService::new(target_root);
    let marker_path = git.git_path(INTEGRATED_TARGET_TRANSACTION)?;
    let Some(mut marker) = read_marker(&marker_path)? else {
        return Ok(());
    };
    if marker.checkout_sync.is_none() && window.is_none() {
        return Ok(());
    }
    marker.checkout_sync = window;
    let encoded = serde_json::to_vec_pretty(&marker).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode integrated-target transaction: {error}"
        ))
    })?;
    write_json_atomically(&marker_path, &encoded, "integrated-target transaction")
}

fn recover_interrupted_transaction(
    git: &FileGitWorktreeService,
    target_root: &Path,
    marker_path: &Path,
    next_goal_id: &str,
    next_round_idx: usize,
) -> RefineResult<()> {
    let Some(interrupted) = read_marker(marker_path)? else {
        return Ok(());
    };
    let details = match interrupted.workspace {
        TransactionWorkspace::IntegrationWorktree => {
            // An interruption inside the checkout-sync window may have left
            // the human checkout behind the ref: re-attempt that sync. The
            // window opens before the CAS and the lane may have been dead for
            // an arbitrary interval, so the recorded `to` proves nothing about
            // the ref anymore — the CAS may never have happened, or a human
            // may have committed or reset the branch since. Replaying toward
            // the branch's CURRENT tip (exactly like
            // `repair_pending_checkout_sync`) makes a never-applied CAS a
            // no-op and never forces a stale `to` over newer human work. A
            // collision leaves the durable pending-sync marker for
            // `repair_pending_checkout_sync` instead of blocking recovery.
            let checkout_sync = interrupted
                .checkout_sync
                .as_ref()
                .map(|window| -> RefineResult<Value> {
                    let current_tip =
                        git.resolve_commit(&format!("refs/heads/{}", window.branch))?;
                    let resolution = match sync_human_checkout_after_ref_move(
                        git,
                        target_root,
                        &window.branch,
                        &window.from,
                        &current_tip,
                    )? {
                        CheckoutSyncOutcome::Synced => "synced",
                        CheckoutSyncOutcome::NotOnTarget => "not_on_target",
                        CheckoutSyncOutcome::SkippedDirtyCollision { .. } => "pending_sync_marker",
                    };
                    Ok(json!({
                        "window": window,
                        "current_tip": current_tip,
                        "resolution": resolution
                    }))
                })
                .transpose()?;
            // The integration worktree is Refine-owned: interrupted residue in
            // it is never user work, so it is recreated, not stashed. The
            // shared checkout stays untouched.
            let removed = remove_integration_worktree(git)?;
            json!({
                "workspace": "integration_worktree",
                "integration_worktree_removed": removed.map(|path| path.display().to_string()),
                "checkout_sync": checkout_sync
            })
        }
        TransactionWorkspace::SharedCheckout => {
            // Legacy markers were written while integration porcelain still
            // ran in the shared checkout, so their residue window really can
            // hold shared-checkout mutations that the marker attributes to
            // the interrupted Goal. Those builds accepted a user's untracked
            // files at acquire and preserved them in place, so recovery
            // quarantines only tracked changes, exactly as those builds
            // themselves did. Fleet-migration path: remove one release after
            // every node writes only integration-worktree markers.
            let status = git.inspect("")?;
            let stash_commit = if status.dirty_user_changes {
                git.quarantine_worktree_changes(&format!(
                    "Refine quarantined interrupted integrated-target residue from Goal {} round {}",
                    interrupted.goal_id,
                    interrupted.round_idx + 1
                ))?
            } else {
                None
            };
            json!({
                "workspace": "shared_checkout",
                "stash_commit": stash_commit
            })
        }
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
    let mut event = json!({
        "recovered_at": now_timestamp(),
        "interrupted": &interrupted,
        "next_owner": {"goal_id": next_goal_id, "round_idx": next_round_idx}
    });
    if let (Some(object), Value::Object(details)) = (event.as_object_mut(), details) {
        object.extend(details);
    }
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

fn read_marker(marker_path: &Path) -> RefineResult<Option<IntegratedTargetTransaction>> {
    let bytes = match fs::read(marker_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read integrated-target transaction {}: {error}",
                marker_path.display()
            )));
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        RefineError::Serialization(format!(
            "invalid integrated-target transaction {}: {error}; checkout was preserved",
            marker_path.display()
        ))
    })
}

/// What is dirty inside the integration worktree, or `None` when the tree is
/// pristine or absent.
fn integration_worktree_dirt(repo_git: &FileGitWorktreeService) -> RefineResult<Option<String>> {
    let path = integration_worktree_path(repo_git)?;
    if !path.exists() {
        return Ok(None);
    }
    let status = FileGitWorktreeService::new(&path).inspect("")?;
    if status.is_pristine() {
        Ok(None)
    } else {
        Ok(Some(status.describe_dirt()))
    }
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

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::Value;

    use super::*;

    fn git(root: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success(),
            "git {args:?} failed in {}",
            root.display()
        );
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn repository(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "refine-integrated-target-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(
            &root,
            &["config", "user.email", "refine-test@example.invalid"],
        );
        git(&root, &["config", "user.name", "Refine Test"]);
        fs::write(root.join("app.txt"), "base\n").unwrap();
        git(&root, &["add", "app.txt"]);
        git(&root, &["commit", "-q", "-m", "base"]);
        root
    }

    fn integration_worktree(root: &Path) -> PathBuf {
        root.join(".git/refine-integration/target")
    }

    fn materialize_integration_worktree(root: &Path) -> PathBuf {
        let worktree = integration_worktree(root);
        git(
            root,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        );
        worktree
    }

    fn last_recovery_event(root: &Path) -> Value {
        let recoveries = FileGitWorktreeService::new(root)
            .git_path(INTEGRATED_TARGET_RECOVERIES)
            .unwrap();
        serde_json::from_str(
            fs::read_to_string(&recoveries)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap()
    }

    /// The exact marker shape the deployed fleet builds wrote while
    /// integration porcelain still ran in the shared checkout: no `workspace`
    /// field.
    fn write_legacy_marker(root: &Path, goal_id: &str) -> PathBuf {
        let marker = FileGitWorktreeService::new(root)
            .git_path(INTEGRATED_TARGET_TRANSACTION)
            .unwrap();
        let fields = serde_json::json!({
            "goal_id": goal_id,
            "round_idx": 0,
            "branch": "main",
            "commit": git_stdout(root, &["rev-parse", "HEAD"]),
            "started_at": "2026-08-16T00:00:00Z"
        });
        fs::write(&marker, fields.to_string()).unwrap();
        marker
    }

    #[test]
    fn completed_integrated_target_transaction_removes_its_marker() {
        let root = repository("finish");
        let marker = FileGitWorktreeService::new(&root)
            .git_path(INTEGRATED_TARGET_TRANSACTION)
            .unwrap();
        open_integrated_target_transaction(&root, "GOAL1", 0).unwrap();
        assert!(marker.exists());
        let recorded: Value = serde_json::from_str(&fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(recorded["workspace"], "integration_worktree");

        finish_integrated_target_transaction(&root).unwrap();

        assert!(!marker.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transaction_opens_despite_untracked_user_files_in_the_shared_checkout() {
        let root = repository("untracked");
        fs::write(root.join("scratch-notes.txt"), "human scratch file\n").unwrap();

        open_integrated_target_transaction(&root, "GOAL1", 0).unwrap();
        finish_integrated_target_transaction(&root).unwrap();

        // The untracked file was neither integration-blocking nor touched.
        assert_eq!(
            fs::read_to_string(root.join("scratch-notes.txt")).unwrap(),
            "human scratch file\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transaction_opens_despite_tracked_and_untracked_dirt_in_the_shared_checkout() {
        // The production regression this whole migration exists for: human
        // dirt of any kind in the shared checkout must neither block the lane
        // nor be touched by it, because integration porcelain runs only in the
        // detached integration worktree.
        let root = repository("any-dirt");
        fs::write(root.join("app.txt"), "uncommitted human edit\n").unwrap();
        fs::write(root.join("scratch-notes.txt"), "human scratch file\n").unwrap();

        open_integrated_target_transaction(&root, "GOAL1", 0).unwrap();
        finish_integrated_target_transaction(&root).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap(),
            "uncommitted human edit\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("scratch-notes.txt")).unwrap(),
            "human scratch file\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn next_transaction_recreates_the_interrupted_integration_worktree_without_touching_the_checkout()
     {
        let root = repository("worktree-recovery");
        // Human dirt that must survive recovery byte-identical: with a
        // new-format marker there is nothing to quarantine in the checkout.
        fs::write(root.join("app.txt"), "human edit in flight\n").unwrap();
        open_integrated_target_transaction(&root, "GOAL1", 0).unwrap();
        let worktree = materialize_integration_worktree(&root);
        fs::write(worktree.join("residue.txt"), "refine merge residue\n").unwrap();

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert!(
            !worktree.exists(),
            "interrupted integration worktree must be torn down for recreation"
        );
        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap(),
            "human edit in flight\n"
        );
        let event = last_recovery_event(&root);
        assert_eq!(event["workspace"], "integration_worktree");
        assert_eq!(event["interrupted"]["goal_id"], "GOAL1");
        assert_eq!(event["next_owner"]["goal_id"], "GOAL2");
        assert!(
            event["integration_worktree_removed"].as_str().is_some(),
            "{event}"
        );
        assert!(
            event["stash_commit"].is_null(),
            "no stash for a Refine-owned tree"
        );
        assert!(event["checkout_sync"].is_null());
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn next_transaction_quarantines_residue_owned_by_a_legacy_interrupted_goal() {
        let root = repository("legacy-recovery");
        fs::write(root.join("app.txt"), "interrupted mutation\n").unwrap();
        git(&root, &["add", "app.txt"]);
        write_legacy_marker(&root, "GOAL1");

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(fs::read_to_string(root.join("app.txt")).unwrap(), "base\n");
        let status = FileGitWorktreeService::new(&root).inspect("").unwrap();
        assert!(!status.dirty_user_changes);
        let event = last_recovery_event(&root);
        assert_eq!(event["workspace"], "shared_checkout");
        assert_eq!(event["interrupted"]["goal_id"], "GOAL1");
        assert_eq!(event["next_owner"]["goal_id"], "GOAL2");
        assert!(event["stash_commit"].as_str().is_some());
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_marker_recovery_never_attributes_untracked_files_to_the_lane() {
        // Deployed legacy builds acquired the lane with a user's untracked
        // files present and never recorded them, so recovery quarantines only
        // tracked changes — exactly what those builds themselves did — and
        // must not sweep any untracked file into the stash.
        let root = repository("legacy-untracked-preserved");
        fs::write(root.join("app.txt"), "interrupted mutation\n").unwrap();
        fs::write(root.join("scratch-notes.txt"), "human scratch file\n").unwrap();
        write_legacy_marker(&root, "GOAL1");

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(fs::read_to_string(root.join("app.txt")).unwrap(), "base\n");
        assert_eq!(
            fs::read_to_string(root.join("scratch-notes.txt")).unwrap(),
            "human scratch file\n"
        );
        let event = last_recovery_event(&root);
        assert!(event["stash_commit"].as_str().is_some());
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_marker_with_only_untracked_files_stashes_nothing() {
        let root = repository("legacy-untracked-only");
        fs::write(root.join("scratch-notes.txt"), "human scratch file\n").unwrap();
        write_legacy_marker(&root, "GOAL1");

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("scratch-notes.txt")).unwrap(),
            "human scratch file\n"
        );
        let event = last_recovery_event(&root);
        assert!(event["stash_commit"].is_null());
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_recovery_handles_renames_and_preserves_special_named_untracked_files() {
        let root = repository("legacy-pathspec-recovery");
        // A staged rename (both sides tracked), plus untracked files with a
        // space-bearing name and a glob-shaped name: the rename must be
        // quarantined via literal paths while every untracked file stays put.
        git(&root, &["mv", "app.txt", "app2.txt"]);
        fs::write(root.join("New Document.txt"), "human scratch file\n").unwrap();
        fs::write(root.join("foo[1].txt"), "human scratch file\n").unwrap();
        fs::write(root.join("foo1.txt"), "user file\n").unwrap();
        write_legacy_marker(&root, "GOAL1");

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(fs::read_to_string(root.join("app.txt")).unwrap(), "base\n");
        assert!(!root.join("app2.txt").exists());
        assert_eq!(
            fs::read_to_string(root.join("New Document.txt")).unwrap(),
            "human scratch file\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("foo[1].txt")).unwrap(),
            "human scratch file\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("foo1.txt")).unwrap(),
            "user file\n"
        );
        let event = last_recovery_event(&root);
        assert!(event["stash_commit"].as_str().is_some());
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_purges_a_half_created_integration_worktree() {
        let root = repository("broken-worktree-recovery");
        open_integrated_target_transaction(&root, "GOAL1", 0).unwrap();
        // A crash mid `worktree add`: the directory exists but no `.git`
        // link was written, so `git worktree remove --force` refuses it.
        let worktree = integration_worktree(&root);
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join("partial.txt"), "partial checkout\n").unwrap();

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert!(
            !integration_worktree(&root).exists(),
            "recovery must purge a worktree with a broken registration"
        );
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// The state between the ref CAS and the checkout sync: the branch ref at
    /// `to`, the index and working tree still at `from`.
    fn open_sync_window(root: &Path) -> (String, String) {
        let from = git_stdout(root, &["rev-parse", "HEAD"]);
        fs::write(root.join("app.txt"), "integrated\n").unwrap();
        git(root, &["add", "app.txt"]);
        git(root, &["commit", "-q", "-m", "integrated"]);
        let to = git_stdout(root, &["rev-parse", "HEAD"]);
        git(root, &["reset", "-q", "--hard", &from]);
        git(root, &["update-ref", "refs/heads/main", &to]);
        (from, to)
    }

    fn write_marker_with_sync_window(root: &Path, from: &str, to: &str) {
        let marker = FileGitWorktreeService::new(root)
            .git_path(INTEGRATED_TARGET_TRANSACTION)
            .unwrap();
        fs::write(
            &marker,
            serde_json::json!({
                "goal_id": "GOAL1",
                "round_idx": 0,
                "started_at": "2026-08-16T00:00:00Z",
                "workspace": "integration_worktree",
                "checkout_sync": {"branch": "main", "from": from, "to": to}
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn interrupted_checkout_sync_window_is_replayed_into_the_checkout() {
        let root = repository("sync-window-replay");
        let (from, to) = open_sync_window(&root);
        write_marker_with_sync_window(&root, &from, &to);

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap(),
            "integrated\n",
            "recovery must replay the interrupted sync"
        );
        assert!(git_stdout(&root, &["status", "--porcelain"]).is_empty());
        let event = last_recovery_event(&root);
        assert_eq!(event["workspace"], "integration_worktree");
        assert_eq!(event["checkout_sync"]["resolution"], "synced");
        assert_eq!(event["checkout_sync"]["window"]["from"], from);
        assert_eq!(event["checkout_sync"]["window"]["to"], to);
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_window_replay_respects_a_human_commit_made_after_the_crash() {
        let root = repository("sync-window-human-commit");
        let (from, to) = open_sync_window(&root);
        // While the lane was dead, the human committed the staged-reverse
        // state the desync displayed — deliberately reverting the
        // integration. Replaying the recorded window verbatim would silently
        // re-stage the integration delta on top of that revert; syncing
        // toward the branch's current tip must leave the checkout alone.
        git(
            &root,
            &["commit", "-q", "-am", "human revert of the desync"],
        );
        let human_commit = git_stdout(&root, &["rev-parse", "HEAD"]);
        write_marker_with_sync_window(&root, &from, &to);

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(fs::read_to_string(root.join("app.txt")).unwrap(), "base\n");
        assert!(git_stdout(&root, &["status", "--porcelain"]).is_empty());
        assert_eq!(git_stdout(&root, &["rev-parse", "HEAD"]), human_commit);
        let event = last_recovery_event(&root);
        assert_eq!(event["checkout_sync"]["resolution"], "synced");
        assert_eq!(event["checkout_sync"]["current_tip"], human_commit);
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_window_whose_cas_never_happened_replays_as_a_noop() {
        // The window opens before the compare-and-swap, so a death in between
        // leaves a recorded window while the ref never moved. Replay against
        // the current tip must not force the never-installed `to` content.
        let root = repository("sync-window-no-cas");
        let (from, to) = open_sync_window(&root);
        git(&root, &["update-ref", "refs/heads/main", &from]);
        write_marker_with_sync_window(&root, &from, &to);

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(fs::read_to_string(root.join("app.txt")).unwrap(), "base\n");
        assert!(git_stdout(&root, &["status", "--porcelain"]).is_empty());
        assert_eq!(git_stdout(&root, &["rev-parse", "main"]), from);
        let event = last_recovery_event(&root);
        assert_eq!(event["checkout_sync"]["resolution"], "synced");
        assert_eq!(event["checkout_sync"]["current_tip"], from);
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn colliding_interrupted_checkout_sync_leaves_the_pending_sync_marker() {
        let root = repository("sync-window-collision");
        let (from, to) = open_sync_window(&root);
        // A human edit collides with the pending delta, so the re-attempted
        // sync must refuse and leave the durable pending-sync marker instead.
        fs::write(root.join("app.txt"), "human edit in flight\n").unwrap();
        write_marker_with_sync_window(&root, &from, &to);

        open_integrated_target_transaction(&root, "GOAL2", 1).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap(),
            "human edit in flight\n"
        );
        let event = last_recovery_event(&root);
        assert_eq!(event["checkout_sync"]["resolution"], "pending_sync_marker");
        let pending = root.join(".git/refine-checkout-sync-pending.json");
        assert!(pending.exists(), "the pending-sync marker must survive");
        finish_integrated_target_transaction(&root).unwrap();

        // Once the colliding edit is gone, the next open repairs the sync.
        git(&root, &["checkout", "--", "app.txt"]);
        open_integrated_target_transaction(&root, "GOAL3", 2).unwrap();
        assert!(!pending.exists(), "open must repair the pending sync");
        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap(),
            "integrated\n"
        );
        finish_integrated_target_transaction(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finish_removes_the_marker_despite_checkout_dirt_and_logs_worktree_residue() {
        let root = repository("finish-residue");
        let marker = FileGitWorktreeService::new(&root)
            .git_path(INTEGRATED_TARGET_TRANSACTION)
            .unwrap();
        open_integrated_target_transaction(&root, "GOAL1", 0).unwrap();
        fs::write(root.join("app.txt"), "human edit in flight\n").unwrap();
        let worktree = materialize_integration_worktree(&root);
        fs::write(worktree.join("residue.txt"), "refine residue\n").unwrap();

        finish_integrated_target_transaction(&root).unwrap();

        // Shared-checkout dirt is the user's business; integration-worktree
        // residue is surfaced log-only because the next integration recreates
        // the tree. Neither holds the marker open.
        assert!(!marker.exists());
        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap(),
            "human edit in flight\n"
        );
        assert!(worktree.join("residue.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
