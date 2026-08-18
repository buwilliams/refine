use super::*;

use crate::infrastructure::git::worktrees::GitRemoteRefDeleteOutcome;
use crate::model::goal::GoalIndexProjection;
use crate::model::workflow::GoalStatus;

#[derive(Clone, Debug, Default)]
struct BranchCandidate {
    local_present: bool,
    local_commit: Option<String>,
    remote_commit: Option<String>,
}

pub(super) fn cleanup_goal_round_branches(
    git: &FileGitWorktreeService,
    goals: &BTreeMap<String, GoalIndexProjection>,
    active_ownership: &ActiveWorktreeOwnership,
    now: DateTime<Utc>,
    options: WorktreeCleanupOptions,
    remote: &str,
    target_branch: &str,
) -> RefineResult<Vec<WorktreeBranchCleanupEntry>> {
    let linked_worktrees = git.list_linked_worktrees()?;
    let checked_out = linked_worktrees
        .iter()
        .filter_map(|worktree| worktree.branch.clone())
        .collect::<BTreeSet<_>>();
    let actively_owned_branches = linked_worktrees
        .iter()
        .filter(|worktree| {
            active_ownership
                .paths
                .iter()
                .any(|active| same_or_descendant(active, &worktree.path))
        })
        .filter_map(|worktree| worktree.branch.clone())
        .collect::<BTreeSet<_>>();
    let mut candidates = BTreeMap::<String, BranchCandidate>::new();
    let mut local_errors = BTreeMap::new();
    for branch in git.list_refine_owned_branches()? {
        candidates.entry(branch.clone()).or_default().local_present = true;
        match git.resolve_commit(&branch) {
            Ok(commit) => candidates.entry(branch).or_default().local_commit = Some(commit),
            Err(error) => {
                local_errors.insert(branch.clone(), error.to_string());
                candidates.entry(branch).or_default();
            }
        }
    }

    let snapshot = match git.remote_refine_ref_snapshot(remote, target_branch) {
        Ok(snapshot) => snapshot,
        Err(RefineError::NotFound(_)) => {
            return Ok(candidates
                .into_iter()
                .map(|(branch, candidate)| {
                    preserved_entry(
                        branch,
                        candidate.local_present,
                        false,
                        "configured_remote_missing",
                    )
                })
                .collect());
        }
        Err(error) => {
            return Ok(candidates
                .into_iter()
                .map(|(branch, candidate)| {
                    inspection_failure_entry(
                        branch,
                        candidate.local_present,
                        false,
                        "remote_inventory_failed",
                        error.to_string(),
                    )
                })
                .collect());
        }
    };
    for remote_ref in &snapshot.refine_branches {
        candidates
            .entry(remote_ref.branch.clone())
            .or_default()
            .remote_commit = Some(remote_ref.commit.clone());
    }
    let Some(target_commit) = snapshot.target_commit.as_deref() else {
        return Ok(candidates
            .into_iter()
            .map(|(branch, candidate)| {
                inspection_failure_entry(
                    branch,
                    candidate.local_present,
                    candidate.remote_commit.is_some(),
                    "remote_target_missing",
                    format!("configured remote target {remote}/{target_branch} was not found"),
                )
            })
            .collect());
    };

    let mut fetch_commits = snapshot
        .refine_branches
        .iter()
        .map(|remote_ref| remote_ref.commit.clone())
        .collect::<Vec<_>>();
    fetch_commits.push(target_commit.to_string());
    if let Err(error) = git.fetch_exact_commits(remote, &fetch_commits) {
        return Ok(candidates
            .into_iter()
            .map(|(branch, candidate)| {
                inspection_failure_entry(
                    branch,
                    candidate.local_present,
                    candidate.remote_commit.is_some(),
                    "branch_inspection_failed",
                    error.to_string(),
                )
            })
            .collect());
    }

    let mut entries = Vec::new();
    for (branch, candidate) in candidates {
        let mut entry = classify_candidate(
            git,
            &branch,
            &candidate,
            goals,
            active_ownership,
            &checked_out,
            &actively_owned_branches,
            now,
            options.older_than_seconds,
            target_commit,
        );
        if let Some(error) = local_errors.get(&branch) {
            entry.local_reason = Some("local_inspection_failed".to_string());
            if entry.remote_present {
                entry.remote_reason = Some("local_inspection_failed".to_string());
            }
            entry.error = Some(error.clone());
            entry.local_eligible = false;
            entry.remote_eligible = false;
            refresh_aggregate(&mut entry);
        }
        if options.apply && entry.eligible {
            apply_retirement(
                git,
                &candidate,
                &mut entry,
                remote,
                target_branch,
                target_commit,
            );
        }
        entries.push(entry);
    }
    Ok(entries)
}

#[allow(clippy::too_many_arguments)]
fn classify_candidate(
    git: &FileGitWorktreeService,
    branch: &str,
    candidate: &BranchCandidate,
    goals: &BTreeMap<String, GoalIndexProjection>,
    active_ownership: &ActiveWorktreeOwnership,
    checked_out: &BTreeSet<String>,
    actively_owned_branches: &BTreeSet<String>,
    now: DateTime<Utc>,
    older_than_seconds: u64,
    target_commit: &str,
) -> WorktreeBranchCleanupEntry {
    let mut entry = WorktreeBranchCleanupEntry {
        branch: branch.to_string(),
        goal_id: None,
        goal_status: None,
        eligible: false,
        local_present: candidate.local_present,
        remote_present: candidate.remote_commit.is_some(),
        local_eligible: false,
        remote_eligible: false,
        local_branch_deleted: false,
        remote_branch_deleted: false,
        reason: "preserved".to_string(),
        local_reason: candidate
            .local_commit
            .as_ref()
            .map(|_| "preserved".to_string()),
        remote_reason: candidate
            .remote_commit
            .as_ref()
            .map(|_| "preserved".to_string()),
        error: None,
    };
    if branch == "refine/state" {
        preserve_both(&mut entry, "protected_state_branch");
        return entry;
    }
    let Some(goal_id) = exact_round_goal_id(branch) else {
        preserve_both(&mut entry, "malformed_branch");
        return entry;
    };
    entry.goal_id = Some(goal_id.to_string());

    if actively_owned_branches.contains(branch) || active_ownership.goal_ids.contains(goal_id) {
        preserve_both(&mut entry, "active_owner");
        return entry;
    }

    let metadata_owners = goals
        .values()
        .filter(|goal| goal.branch_name.as_deref() == Some(branch))
        .map(|goal| goal.id.as_str())
        .filter(|id| *id != goal_id)
        .collect::<BTreeSet<_>>();
    if !metadata_owners.is_empty() {
        preserve_both(&mut entry, "ambiguous_goal");
        return entry;
    }
    let goal = goals.get(goal_id);
    if let Some(goal) = goal {
        entry.goal_status = Some(goal.status.as_str().to_string());
        if !matches!(goal.status, GoalStatus::Done | GoalStatus::Cancelled) {
            preserve_both(&mut entry, "goal_not_terminal");
            return entry;
        }
        if goal_is_too_recent(&goal.updated, now, older_than_seconds) {
            preserve_both(&mut entry, "retention_window");
            return entry;
        }
    }

    entry.local_reason = candidate.local_commit.as_ref().map(|commit| {
        side_eligibility_reason(
            git,
            commit,
            target_commit,
            goal.is_none(),
            now,
            older_than_seconds,
            checked_out.contains(branch),
        )
    });
    entry.remote_reason = candidate.remote_commit.as_ref().map(|commit| {
        side_eligibility_reason(
            git,
            commit,
            target_commit,
            goal.is_none(),
            now,
            older_than_seconds,
            false,
        )
    });
    entry.local_eligible = entry.local_reason.as_deref() == Some("eligible");
    entry.remote_eligible = entry.remote_reason.as_deref() == Some("eligible");
    refresh_aggregate(&mut entry);
    entry
}

fn side_eligibility_reason(
    git: &FileGitWorktreeService,
    commit: &str,
    target_commit: &str,
    missing_goal: bool,
    now: DateTime<Utc>,
    older_than_seconds: u64,
    checked_out: bool,
) -> String {
    if checked_out {
        return "checked_out".to_string();
    }
    if missing_goal {
        match git.commit_timestamp(commit) {
            Ok(updated)
                if now.signed_duration_since(updated).num_seconds()
                    < i64::try_from(older_than_seconds).unwrap_or(i64::MAX) =>
            {
                return "retention_window".to_string();
            }
            Ok(_) => {}
            Err(_) => return "commit_time_inspection_failed".to_string(),
        }
    }
    match git.commit_is_ancestor(commit, target_commit) {
        Ok(true) => "eligible".to_string(),
        Ok(false) => "not_in_remote_target".to_string(),
        Err(_) => "ancestry_inspection_failed".to_string(),
    }
}

fn apply_retirement(
    git: &FileGitWorktreeService,
    candidate: &BranchCandidate,
    entry: &mut WorktreeBranchCleanupEntry,
    remote: &str,
    target_branch: &str,
    target_commit: &str,
) {
    if entry.remote_eligible {
        let remote_commit = candidate
            .remote_commit
            .as_deref()
            .expect("remote eligibility requires a commit");
        match git.delete_remote_branch_if_snapshot_matches(
            remote,
            &entry.branch,
            remote_commit,
            target_branch,
            target_commit,
        ) {
            Ok(GitRemoteRefDeleteOutcome::Deleted) => {
                entry.remote_branch_deleted = true;
                entry.remote_reason = Some("retired".to_string());
            }
            Ok(GitRemoteRefDeleteOutcome::BranchChanged) => {
                entry.remote_eligible = false;
                entry.remote_reason = Some("remote_branch_changed".to_string());
            }
            Ok(GitRemoteRefDeleteOutcome::TargetChanged) => {
                entry.remote_eligible = false;
                entry.local_eligible = false;
                entry.remote_reason = Some("remote_target_changed".to_string());
                if entry.local_present {
                    entry.local_reason = Some("remote_target_changed".to_string());
                }
            }
            Ok(GitRemoteRefDeleteOutcome::AtomicUnsupported) => {
                entry.remote_eligible = false;
                entry.local_eligible = false;
                entry.remote_reason = Some("atomic_delete_unsupported".to_string());
                if entry.local_present {
                    entry.local_reason = Some("atomic_delete_unsupported".to_string());
                }
            }
            Err(error) => {
                entry.remote_eligible = false;
                entry.local_eligible = false;
                entry.remote_reason = Some("remote_delete_failed".to_string());
                entry.error = Some(error.to_string());
            }
        }
    }

    if entry.local_eligible {
        match git.remote_refine_ref_snapshot(remote, target_branch) {
            Ok(snapshot) if snapshot.target_commit.as_deref() == Some(target_commit) => {
                let local_commit = candidate
                    .local_commit
                    .as_deref()
                    .expect("local eligibility requires a commit");
                match git.delete_branch_if_matches(&entry.branch, local_commit) {
                    Ok(()) => {
                        entry.local_branch_deleted = true;
                        entry.local_reason = Some("retired".to_string());
                    }
                    Err(error) => {
                        entry.local_eligible = false;
                        entry.local_reason = Some("local_delete_failed".to_string());
                        entry.error = Some(error.to_string());
                    }
                }
            }
            Ok(_) => {
                entry.local_eligible = false;
                entry.local_reason = Some("remote_target_changed".to_string());
            }
            Err(error) => {
                entry.local_eligible = false;
                entry.local_reason = Some("target_reinspection_failed".to_string());
                entry.error = Some(error.to_string());
            }
        }
    }
    refresh_aggregate(entry);
}

fn exact_round_goal_id(branch: &str) -> Option<&str> {
    let mut parts = branch.split('/');
    if parts.next()? != "refine" {
        return None;
    }
    let goal_id = parts.next().filter(|value| !value.is_empty())?;
    let round = parts.next()?.strip_prefix("round-")?;
    if parts.next().is_some()
        || round.is_empty()
        || !round.bytes().all(|byte| byte.is_ascii_digit())
        || round.parse::<u64>().ok()? == 0
    {
        return None;
    }
    Some(goal_id)
}

fn inspection_failure_entry(
    branch: String,
    local_present: bool,
    remote_present: bool,
    reason: &str,
    error: String,
) -> WorktreeBranchCleanupEntry {
    WorktreeBranchCleanupEntry {
        branch,
        goal_id: None,
        goal_status: None,
        eligible: false,
        local_present,
        remote_present,
        local_eligible: false,
        remote_eligible: false,
        local_branch_deleted: false,
        remote_branch_deleted: false,
        reason: reason.to_string(),
        local_reason: local_present.then(|| reason.to_string()),
        remote_reason: remote_present.then(|| reason.to_string()),
        error: Some(error),
    }
}

fn preserved_entry(
    branch: String,
    local_present: bool,
    remote_present: bool,
    reason: &str,
) -> WorktreeBranchCleanupEntry {
    WorktreeBranchCleanupEntry {
        branch,
        goal_id: None,
        goal_status: None,
        eligible: false,
        local_present,
        remote_present,
        local_eligible: false,
        remote_eligible: false,
        local_branch_deleted: false,
        remote_branch_deleted: false,
        reason: reason.to_string(),
        local_reason: local_present.then(|| reason.to_string()),
        remote_reason: remote_present.then(|| reason.to_string()),
        error: None,
    }
}

fn preserve_both(entry: &mut WorktreeBranchCleanupEntry, reason: &str) {
    if entry.local_present {
        entry.local_reason = Some(reason.to_string());
    }
    if entry.remote_present {
        entry.remote_reason = Some(reason.to_string());
    }
    entry.reason = reason.to_string();
}

fn refresh_aggregate(entry: &mut WorktreeBranchCleanupEntry) {
    entry.eligible = entry.local_eligible || entry.remote_eligible;
    entry.reason = if entry.local_branch_deleted || entry.remote_branch_deleted {
        "retired"
    } else {
        match (entry.local_eligible, entry.remote_eligible) {
            (true, true) => "local_and_remote_eligible",
            (true, false) => "local_eligible",
            (false, true) => "remote_eligible",
            (false, false) => match (
                entry.local_reason.as_deref(),
                entry.remote_reason.as_deref(),
            ) {
                (Some(local), None) => local,
                (None, Some(remote)) => remote,
                (Some(local), Some(remote)) if local == remote => local,
                _ => "preserved",
            },
        }
    }
    .to_string();
}

#[cfg(test)]
mod tests {
    use super::exact_round_goal_id;

    #[test]
    fn exact_round_branch_parser_rejects_state_and_malformed_refs() {
        assert_eq!(exact_round_goal_id("refine/GOAL/round-12"), Some("GOAL"));
        for branch in [
            "refine/state",
            "refine/GOAL",
            "refine/GOAL/round-0",
            "refine/GOAL/round-x",
            "refine/GOAL/round-1/extra",
        ] {
            assert_eq!(exact_round_goal_id(branch), None, "{branch}");
        }
    }
}
