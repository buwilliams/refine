use serde_json::json;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::{
    FileGitWorktreeService, GitRemoteRefDeleteOutcome, trimmed_command_text, validate_branch_name,
    validate_commitish, validate_head_branch_ref,
};

impl FileGitWorktreeService {
    /// Compare-and-swap advance of a fully qualified branch ref.
    ///
    /// Unlike `branch -f`, `update-ref` moves a branch even while it is
    /// checked out in another worktree, which is what lets a detached
    /// integration worktree advance the target branch of the shared human
    /// checkout. Losing the race maps to `TargetAdvanced` naming the current
    /// commit so callers can rejoin their refresh loop.
    pub fn update_ref_cas(
        &self,
        reference: &str,
        to_commit: &str,
        expected_old: &str,
    ) -> RefineResult<()> {
        validate_head_branch_ref(reference)?;
        validate_commitish(to_commit)?;
        validate_commitish(expected_old)?;
        let output = self.git_raw(&["update-ref", reference, to_commit, expected_old])?;
        if output.success {
            return self.audit(
                "update_ref_cas",
                "ok",
                json!({
                    "reference": reference,
                    "to_commit": to_commit,
                    "expected_old": expected_old,
                    "exact_sha_fence": true
                }),
            );
        }
        let message = trimmed_command_text(&output);
        let expected = self.resolve_commit(expected_old)?;
        if let Ok(current) = self.resolve_commit(reference)
            && current != expected
        {
            let _ = self.audit(
                "update_ref_cas",
                "conflict",
                json!({"reference": reference, "expected_old": expected, "current": current}),
            );
            return Err(RefineError::TargetAdvanced {
                reference: reference.to_string(),
                expected,
                current,
            });
        }
        Err(RefineError::Conflict(message))
    }

    /// Delete only the exact Refine-owned remote ref previously inspected.
    ///
    /// The force-with-lease is a compare-and-delete fence, not a history
    /// rewrite: if another process advances the branch, Git rejects deletion.
    pub fn delete_remote_branch_if_matches(
        &self,
        remote: &str,
        branch: &str,
        expected_commit: &str,
    ) -> RefineResult<()> {
        validate_branch_name(branch)?;
        validate_commitish(expected_commit)?;
        let reference = format!("refs/heads/{branch}");
        let lease = format!("--force-with-lease={reference}:{expected_commit}");
        let deletion = format!(":{reference}");
        self.git_output(&["push", &lease, remote, &deletion])?;
        self.audit(
            "remote_branch_delete",
            "ok",
            json!({
                "remote": remote,
                "branch": branch,
                "expected_commit": expected_commit,
                "exact_sha_fence": true
            }),
        )
    }

    /// Atomically compare both advertised refs and delete only the branch.
    ///
    /// The target refspec is intentionally a no-op when the snapshot still
    /// matches. If the target moved, it becomes a rejected stale lease instead
    /// of silently validating ancestry against an obsolete target.
    pub fn delete_remote_branch_if_snapshot_matches(
        &self,
        remote: &str,
        branch: &str,
        expected_branch_commit: &str,
        target_branch: &str,
        expected_target_commit: &str,
    ) -> RefineResult<GitRemoteRefDeleteOutcome> {
        validate_branch_name(branch)?;
        validate_branch_name(target_branch)?;
        validate_commitish(expected_branch_commit)?;
        validate_commitish(expected_target_commit)?;
        if branch == target_branch {
            return Err(RefineError::InvalidInput(
                "cleanup branch and merge target must be distinct".to_string(),
            ));
        }
        let snapshot = self.remote_refine_ref_snapshot(remote, target_branch)?;
        if snapshot.target_commit.as_deref() != Some(expected_target_commit) {
            return Ok(GitRemoteRefDeleteOutcome::TargetChanged);
        }
        if snapshot
            .refine_branches
            .iter()
            .find(|candidate| candidate.branch == branch)
            .map(|candidate| candidate.commit.as_str())
            != Some(expected_branch_commit)
        {
            return Ok(GitRemoteRefDeleteOutcome::BranchChanged);
        }

        let branch_ref = format!("refs/heads/{branch}");
        let target_ref = format!("refs/heads/{target_branch}");
        let branch_lease = format!("--force-with-lease={branch_ref}:{expected_branch_commit}");
        let target_lease = format!("--force-with-lease={target_ref}:{expected_target_commit}");
        let deletion = format!(":{branch_ref}");
        let target_noop = format!("{expected_target_commit}:{target_ref}");
        let output = self.git_raw(&[
            "push",
            "--atomic",
            &branch_lease,
            &target_lease,
            remote,
            &deletion,
            &target_noop,
        ])?;
        if output.success {
            self.audit(
                "remote_branch_delete",
                "ok",
                json!({
                    "remote": remote,
                    "branch": branch,
                    "expected_commit": expected_branch_commit,
                    "target_branch": target_branch,
                    "expected_target_commit": expected_target_commit,
                    "exact_sha_fence": true,
                    "atomic_target_fence": true
                }),
            )?;
            return Ok(GitRemoteRefDeleteOutcome::Deleted);
        }

        let message = trimmed_command_text(&output);
        let current = self.remote_refine_ref_snapshot(remote, target_branch)?;
        if current.target_commit.as_deref() != Some(expected_target_commit) {
            return Ok(GitRemoteRefDeleteOutcome::TargetChanged);
        }
        if current
            .refine_branches
            .iter()
            .find(|candidate| candidate.branch == branch)
            .map(|candidate| candidate.commit.as_str())
            != Some(expected_branch_commit)
        {
            return Ok(GitRemoteRefDeleteOutcome::BranchChanged);
        }
        let lower = message.to_ascii_lowercase();
        if lower.contains("does not support --atomic")
            || lower.contains("does not support atomic")
            || lower.contains("atomic push is not supported")
        {
            return Ok(GitRemoteRefDeleteOutcome::AtomicUnsupported);
        }
        Err(RefineError::Conflict(message))
    }
}
