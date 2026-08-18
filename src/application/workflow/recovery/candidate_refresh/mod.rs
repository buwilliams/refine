//! Candidate refresh onto an advanced target, with resolve-in-place for
//! conflicted rebases.
//!
//! One implementation serves both stage boundaries that refresh. The
//! Implement→Quality boundary refreshes so Quality judges the candidate
//! against the target it will actually merge into, while the implementing
//! agent's context is still fresh; Governance refreshes again immediately
//! before integrating, and after an earlier refresh that second pass is
//! normally `Unchanged`. `authority_status` is the only difference between the
//! two callers — the workflow status the Goal is authorized under while the
//! refresh runs and persists.
//!
//! The repository lock never contains agent work. The refresh runs as two
//! short holds around an unlocked middle, the same discipline Governance uses
//! for its quality/governance verdicts:
//!
//! - *Hold 1* (seconds): provenance checks and the rebase attempt. A clean
//!   rebase persists inside this hold exactly as before. A conflicted rebase
//!   with a resolver available ends the hold with the conflicted worktree
//!   preserved (no abort yet).
//! - *Unlocked* (minutes): the resolver edits the conflicted rebase workspace
//!   in place, gated by the no-markers check and a scope check that rejects
//!   edits outside the conflicted files, within the shared
//!   [`RESOLUTION_ATTEMPT_LIMIT`] budget.
//! - *Hold 2* (seconds): prove the target tip is unchanged, `git rebase
//!   --continue` (never skip), run the existing linearity/provenance gate on
//!   the final delta, and persist. A moved tip aborts the rebase and surfaces
//!   as [`RefineError::TargetAdvanced`] so the caller's bounded
//!   refresh-again loop retries; every resolution failure falls back to
//!   exactly today's abort + fenced recovery Round.
//!
//! Crash-only: the rebase state itself is the durable workspace, and the
//! candidate branch only moves when the rebase completes. A crash at any
//! point leaves a stopped rebase the next attempt's provenance checks reject;
//! that attempt aborts it — restoring the exact recorded candidate — before
//! taking the existing recovery-Round fallback, so no checkout stays wedged
//! mid-pick waiting for something else to clean it up.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::application::persistence_sync::resolution::{
    ConflictResolver, ConflictedPath, InstalledAgentResolver, RESOLUTION_ATTEMPT_LIMIT,
    ResolutionRequest, ResolverOutcome, candidate_conflict_context, paths_with_conflict_markers,
};
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::with_repository_git_lock;
use crate::infrastructure::git::worktrees::{
    FileGitWorktreeService, GitWorktreeService, MergeResult,
};
use crate::model::JsonObject;
use crate::model::workflow::GoalStatus;

use crate::application::workflow::engine::context::WorkflowContext;
use crate::application::workflow::engine::{agent_idle_timeout, setting_string};

#[derive(Clone, Debug)]
pub(crate) enum CandidateRefreshOutcome {
    Unchanged {
        /// The target tip the refresh observed, so integration can prove the
        /// target has not advanced between the refresh and the merge.
        target_commit: String,
    },
    Refreshed {
        original_candidate: String,
        replacement_candidate: String,
        target_commit: String,
        evidence: Value,
    },
    RecoveryQueued {
        reason: String,
        evidence: Value,
    },
    RecoveryExhausted {
        reason: String,
        evidence: Value,
    },
}

/// The node setting gating resolve-in-place for conflicted candidate
/// refreshes. On by default; off restores the unconditional recovery-Round
/// fallback.
pub(crate) fn workflow_conflict_resolution_enabled(settings: &JsonObject) -> bool {
    setting_string(settings, "workflow_conflict_resolution", "on") != "off"
}

/// The resolver a conflicted refresh calls out to: the workflow's own provider
/// identity and idle budget, matching every other agent invocation this Goal
/// makes. Not installed maps to `Unavailable`, and `workflow_conflict_resolution
/// = off` maps to `None`; both fall back exactly to pre-resolver behavior.
pub(crate) fn workflow_conflict_resolver(
    ctx: &WorkflowContext<'_>,
) -> Option<InstalledAgentResolver> {
    workflow_conflict_resolution_enabled(&ctx.settings).then(|| InstalledAgentResolver {
        provider: ctx.provider.clone(),
        agents_runtime_root: ctx.runtime_root.join("agents"),
        stall_timeout_seconds: agent_idle_timeout(&ctx.settings).map(|timeout| timeout.as_secs()),
    })
}

pub(crate) fn refresh_candidate_for_target_advancement(
    ctx: &mut WorkflowContext<'_>,
    authority_status: GoalStatus,
    max_automatic_round_retries: u32,
) -> RefineResult<CandidateRefreshOutcome> {
    let resolver = workflow_conflict_resolver(ctx);
    refresh_candidate_with_resolver(
        ctx,
        authority_status,
        max_automatic_round_retries,
        resolver
            .as_ref()
            .map(|resolver| resolver as &dyn ConflictResolver),
    )
}

/// Refresh with an explicit resolver (or none). Manages its own repository
/// lock holds — callers must not wrap this in `with_repository_git_lock`.
pub(crate) fn refresh_candidate_with_resolver(
    ctx: &mut WorkflowContext<'_>,
    authority_status: GoalStatus,
    max_automatic_round_retries: u32,
    resolver: Option<&dyn ConflictResolver>,
) -> RefineResult<CandidateRefreshOutcome> {
    let target_root = ctx.target_root.to_path_buf();
    let step = with_repository_git_lock(&target_root, || {
        locked_refresh_attempt(
            ctx,
            &authority_status,
            max_automatic_round_retries,
            resolver.is_some(),
        )
    })?;
    match step {
        LockedRefreshStep::Done(outcome) => Ok(outcome),
        LockedRefreshStep::Conflicted(pending) => {
            let resolver = resolver
                .expect("a conflicted refresh is only preserved when a resolver is available");
            resolve_conflicted_refresh(
                ctx,
                &authority_status,
                max_automatic_round_retries,
                resolver,
                *pending,
            )
        }
    }
}

enum LockedRefreshStep {
    Done(CandidateRefreshOutcome),
    /// Hold 1 ended with the conflicted rebase preserved for unlocked
    /// resolution.
    Conflicted(Box<ConflictedRefresh>),
}

/// Everything hold 1 proved about the conflicted rebase, carried across the
/// unlocked resolution window.
struct ConflictedRefresh {
    original_base: String,
    original_candidate: String,
    branch: String,
    worktree: String,
    target_branch: String,
    target_commit: String,
    rebase: MergeResult,
}

fn locked_refresh_attempt(
    ctx: &mut WorkflowContext<'_>,
    authority_status: &GoalStatus,
    max_automatic_round_retries: u32,
    resolver_available: bool,
) -> RefineResult<LockedRefreshStep> {
    ctx.revalidate_authority(authority_status.clone())?;
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let branch = required(&detail, "branch_name", &ctx.goal_id)?;
    let original_base = required(&detail, "base_commit", &ctx.goal_id)?;
    let original_candidate = required(&detail, "candidate_commit", &ctx.goal_id)?;
    let target_branch = required(&detail, "target_branch", &ctx.goal_id)?;
    let worktree = ctx.require_worktree_path()?.to_string();
    if ctx.require_branch()? != branch || ctx.require_commit()? != original_candidate {
        return Err(RefineError::Conflict(format!(
            "Goal {} hydrated candidate identity changed before the {} candidate refresh",
            ctx.goal_id,
            authority_status.as_str()
        )));
    }

    let target_git = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root);
    let worktree_git = FileGitWorktreeService::with_runtime_root(&worktree, ctx.runtime_root);
    let resolved_base = target_git.resolve_commit(&original_base);
    let resolved_candidate = target_git.resolve_commit(&original_candidate);
    let target_commit = target_git.resolve_commit(&target_branch)?;
    if resolved_base
        .as_ref()
        .is_ok_and(|resolved| resolved == &target_commit)
    {
        return Ok(LockedRefreshStep::Done(
            CandidateRefreshOutcome::Unchanged { target_commit },
        ));
    }
    if resolved_candidate
        .as_ref()
        .is_ok_and(|resolved| resolved == &original_candidate)
        && target_git.commit_is_ancestor(&original_candidate, &target_commit)?
    {
        return Ok(LockedRefreshStep::Done(
            CandidateRefreshOutcome::Unchanged { target_commit },
        ));
    }

    let head = worktree_git.head_ref()?;
    let status = worktree_git.inspect("")?;
    let provenance_error = if !resolved_base
        .as_ref()
        .is_ok_and(|resolved| resolved == &original_base)
    {
        Some("recorded base does not resolve to its exact durable commit")
    } else if !resolved_candidate
        .as_ref()
        .is_ok_and(|resolved| resolved == &original_candidate)
    {
        Some("recorded candidate does not resolve to its exact durable commit")
    } else if !target_git.commit_is_ancestor(&original_base, &original_candidate)? {
        Some("recorded base is not an ancestor of the exact candidate")
    } else if !target_git.commit_is_ancestor(&original_base, &target_commit)? {
        Some("advanced target does not descend from the recorded base")
    } else if !target_git.commit_range_is_linear(&original_base, &original_candidate)? {
        Some("base-to-candidate delta contains merge commits and has no implicit replay mainline")
    } else if head.branch.as_deref() != Some(branch.as_str())
        || head.commit.as_deref() != Some(original_candidate.as_str())
    {
        Some("clean candidate branch no longer names the exact recorded candidate")
    } else if !status.is_pristine() {
        Some("candidate checkout is dirty")
    } else {
        None
    };
    if let Some(reason) = provenance_error {
        // A crash during unlocked resolution leaves the rebase stopped
        // mid-pick, which is one of the states these checks reject. The
        // candidate branch itself never moved — Git advances it only when the
        // rebase completes — so aborting loses nothing and hands the recovery
        // Round a clean checkout instead of a wedged one.
        let interrupted = worktree_git.operation_in_progress()?;
        let recovery = interrupted
            .then(|| worktree_git.recover())
            .transpose()?
            .map(|recovery| json!(recovery));
        let restored = interrupted.then(|| worktree_git.head_ref()).transpose()?;
        return queue_recovery(
            ctx,
            authority_status,
            max_automatic_round_retries,
            reason,
            json!({
                "original_base_commit": original_base,
                "original_candidate_commit": original_candidate,
                "branch": branch,
                "worktree": worktree,
                "target_branch": target_branch,
                "target_commit": target_commit,
                "candidate_head": head,
                "candidate_status": status,
                "rebase_abort": recovery,
                "restored_candidate_head": restored
            }),
        )
        .map(LockedRefreshStep::Done);
    }

    ctx.revalidate_authority(authority_status.clone())?;
    let rebase = worktree_git.rebase(&target_branch)?;
    if !rebase.ok {
        if resolver_available && !rebase.conflicts.is_empty() {
            // Resolve in place before re-implementing: the hold ends with
            // the conflicted rebase preserved as the resolution workspace.
            return Ok(LockedRefreshStep::Conflicted(Box::new(ConflictedRefresh {
                original_base,
                original_candidate,
                branch,
                worktree,
                target_branch,
                target_commit,
                rebase,
            })));
        }
        let recovery = worktree_git.recover()?;
        let restored = worktree_git.head_ref()?;
        return queue_recovery(
            ctx,
            authority_status,
            max_automatic_round_retries,
            "candidate refresh conflicted",
            json!({
                "original_base_commit": original_base,
                "original_candidate_commit": original_candidate,
                "branch": branch,
                "worktree": worktree,
                "target_branch": target_branch,
                "target_commit": target_commit,
                "rebase": rebase,
                "rebase_abort": recovery,
                "restored_candidate_head": restored
            }),
        )
        .map(LockedRefreshStep::Done);
    }
    finalize_refreshed_candidate(
        ctx,
        authority_status,
        &worktree_git,
        &branch,
        &worktree,
        &original_base,
        &original_candidate,
        &target_commit,
        None,
    )
    .map(LockedRefreshStep::Done)
}

/// Persist a completed rebase as the replacement candidate. Shared by the
/// clean-rebase path (no resolution note) and the resolved-conflict path.
#[allow(clippy::too_many_arguments)]
fn finalize_refreshed_candidate(
    ctx: &mut WorkflowContext<'_>,
    authority_status: &GoalStatus,
    worktree_git: &FileGitWorktreeService,
    branch: &str,
    worktree: &str,
    original_base: &str,
    original_candidate: &str,
    target_commit: &str,
    conflict_resolution: Option<Value>,
) -> RefineResult<CandidateRefreshOutcome> {
    let replacement = worktree_git.head_ref()?.commit.ok_or_else(|| {
        RefineError::Conflict(format!(
            "Goal {} refreshed candidate branch has no commit",
            ctx.goal_id
        ))
    })?;
    let refreshed_status = worktree_git.inspect("")?;
    if !refreshed_status.is_pristine() {
        return Err(RefineError::Conflict(format!(
            "Goal {} candidate refresh left a dirty worktree; replacement evidence was not persisted",
            ctx.goal_id
        )));
    }
    let evidence = persist_candidate_refresh_or_restore(
        ctx,
        authority_status,
        worktree_git,
        branch,
        worktree,
        original_base,
        original_candidate,
        target_commit,
        &replacement,
        conflict_resolution,
    )?;
    ctx.commit = Some(replacement.clone());
    Ok(CandidateRefreshOutcome::Refreshed {
        original_candidate: original_candidate.to_string(),
        replacement_candidate: replacement,
        target_commit: target_commit.to_string(),
        evidence,
    })
}

enum ContinueStep {
    Finished(CandidateRefreshOutcome),
    /// The target tip moved during unlocked resolution; the rebase was
    /// aborted under the hold.
    TipMoved {
        current: String,
    },
    /// The rebase stopped on the next conflicted pick.
    NextStop(MergeResult),
    /// `rebase --continue` failed without a further conflict (for example an
    /// emptied pick, which Refine never skips past).
    Broken(MergeResult),
}

/// Unlocked resolver attempts over the preserved conflicted rebase, with a
/// short repository-lock hold around every `rebase --continue`. One budget of
/// [`RESOLUTION_ATTEMPT_LIMIT`] resolver invocations covers rejected outputs
/// and further conflicted stops alike; each stop's conflicted picks count as
/// one attempt.
fn resolve_conflicted_refresh(
    ctx: &mut WorkflowContext<'_>,
    authority_status: &GoalStatus,
    max_automatic_round_retries: u32,
    resolver: &dyn ConflictResolver,
    pending: ConflictedRefresh,
) -> RefineResult<CandidateRefreshOutcome> {
    let target_root = ctx.target_root.to_path_buf();
    let worktree_git =
        FileGitWorktreeService::with_runtime_root(&pending.worktree, ctx.runtime_root);
    let target_git = FileGitWorktreeService::with_runtime_root(&target_root, ctx.runtime_root);
    let workspace = PathBuf::from(&pending.worktree);

    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let goal_prompt = format!(
        "{}\n\n{}",
        detail.get("name").and_then(Value::as_str).unwrap_or(""),
        round_prompt(&detail, 0)
    )
    .trim()
    .to_string();
    let round_intent = round_prompt(&detail, ctx.round_idx);
    let other_reports = conflicting_goal_reports(ctx, &target_git, &pending);
    let ancestry = format!(
        "rebasing candidate branch {} (base {}) onto {} at {}",
        pending.branch, pending.original_base, pending.target_branch, pending.target_commit
    );

    let mut attempts: Vec<Value> = Vec::new();
    let mut resolved_files: BTreeSet<String> = BTreeSet::new();
    let mut stop_conflicts: Vec<String> = pending.rebase.conflicts.clone();
    // What the stopped rebase already touches on its own: the pick's merged
    // hunks are staged and its conflicts unmerged. Anything that appears
    // beyond this and the conflicted files is the resolver's own edit.
    let mut stop_baseline = changed_paths(&worktree_git)?;
    let mut feedback: Option<String> = None;
    let mut invocation: u32 = 0;
    loop {
        if invocation >= RESOLUTION_ATTEMPT_LIMIT {
            let question = resolution_question(ctx, &pending, &stop_conflicts);
            return abort_conflicted_refresh_and_queue(
                ctx,
                authority_status,
                max_automatic_round_retries,
                &pending,
                json!({
                    "outcome": "needs_decision",
                    "attempts": attempts,
                    "question": question
                }),
            );
        }
        invocation += 1;
        let conflicts = stop_conflicts
            .iter()
            .map(|path| ConflictedPath {
                path: path.clone(),
                summary: format!("conflicted while rebasing onto {}", pending.target_branch),
            })
            .collect::<Vec<_>>();
        let context = candidate_conflict_context(
            &goal_prompt,
            &round_intent,
            &stop_conflicts.join("\n"),
            &other_reports,
        );
        let request = ResolutionRequest {
            workspace_dir: &workspace,
            conflicts: &conflicts,
            sides: &[],
            ancestry: &ancestry,
            context: &context,
            attempt: invocation,
            feedback: feedback.as_deref(),
        };
        let outcome = match resolver.resolve(&request) {
            Ok(outcome) => outcome,
            Err(error) => {
                // The rebase is still stopped mid-pick. An error that is not
                // the resolver's verdict must not leave the goal's checkout
                // wedged there for the next attempt to reject.
                let _ = with_repository_git_lock(&target_root, || worktree_git.recover());
                return Err(error);
            }
        };
        match outcome {
            ResolverOutcome::Unavailable => {
                attempts.push(json!({"attempt": invocation, "outcome": "unavailable"}));
                return abort_conflicted_refresh_and_queue(
                    ctx,
                    authority_status,
                    max_automatic_round_retries,
                    &pending,
                    json!({"outcome": "unavailable", "attempts": attempts}),
                );
            }
            // The resolver read both intents and cannot choose: escalate with
            // its own question instead of spending the rest of the budget.
            ResolverOutcome::NeedsDecision { question } => {
                attempts.push(json!({
                    "attempt": invocation,
                    "outcome": "needs_decision",
                    "question": question,
                    "files": stop_conflicts
                }));
                return abort_conflicted_refresh_and_queue(
                    ctx,
                    authority_status,
                    max_automatic_round_retries,
                    &pending,
                    json!({
                        "outcome": "needs_decision",
                        "attempts": attempts,
                        "question": question
                    }),
                );
            }
            ResolverOutcome::Failed(reason) => {
                attempts.push(json!({
                    "attempt": invocation,
                    "outcome": "failed",
                    "reason": reason,
                    "files": stop_conflicts
                }));
                feedback = Some(format!("the previous resolution attempt failed: {reason}"));
                continue;
            }
            ResolverOutcome::Completed => {}
        }
        let marked = paths_with_conflict_markers(&workspace, &stop_conflicts)?;
        if !marked.is_empty() {
            let why = format!("conflict markers remain in {}", marked.join(", "));
            attempts.push(json!({
                "attempt": invocation,
                "outcome": "rejected",
                "reason": why,
                "files": stop_conflicts
            }));
            feedback = Some(why);
            continue;
        }
        // Scope is an acceptance gate, not an honor system: `rebase
        // --continue` commits the whole index, so an edit outside the
        // conflicted files would ride into the replacement candidate under a
        // resolution note that never mentions it.
        let out_of_scope = changed_paths(&worktree_git)?
            .into_iter()
            .filter(|path| !stop_baseline.contains(path) && !stop_conflicts.contains(path))
            .collect::<Vec<_>>();
        if !out_of_scope.is_empty() {
            let why = format!(
                "changes outside the conflicted files: {}",
                out_of_scope.join(", ")
            );
            attempts.push(json!({
                "attempt": invocation,
                "outcome": "rejected",
                "reason": why,
                "files": stop_conflicts,
                "out_of_scope_files": out_of_scope
            }));
            feedback = Some(format!(
                "{why}. Resolve only the conflicted files and make no other edits."
            ));
            continue;
        }
        attempts.push(json!({
            "attempt": invocation,
            "outcome": "accepted",
            "files": stop_conflicts
        }));
        resolved_files.extend(stop_conflicts.iter().cloned());
        feedback = None;

        let continue_step = with_repository_git_lock(&target_root, || {
            let current_target = target_git.resolve_commit(&pending.target_branch)?;
            if current_target != pending.target_commit {
                worktree_git.recover()?;
                return Ok(ContinueStep::TipMoved {
                    current: current_target,
                });
            }
            let continued = worktree_git.rebase_continue(&stop_conflicts)?;
            if !continued.ok {
                if continued.conflicts.is_empty() {
                    return Ok(ContinueStep::Broken(continued));
                }
                return Ok(ContinueStep::NextStop(continued));
            }
            // The existing linearity/provenance gate, now over the final
            // resolved delta.
            let replacement = worktree_git.head_ref()?.commit.ok_or_else(|| {
                RefineError::Conflict(format!(
                    "Goal {} resolved candidate branch has no commit",
                    ctx.goal_id
                ))
            })?;
            if !target_git.commit_is_ancestor(&pending.target_commit, &replacement)?
                || !worktree_git.commit_range_is_linear(&pending.target_commit, &replacement)?
            {
                worktree_git.reset_hard_to(&pending.original_candidate)?;
                return Ok(ContinueStep::Broken(MergeResult {
                    ok: false,
                    conflicts: Vec::new(),
                    message: Some(
                        "resolved rebase left a non-linear or disconnected target-to-candidate delta"
                            .to_string(),
                    ),
                }));
            }
            let note = json!({
                "outcome": "resolved",
                "attempts": attempts.clone(),
                "files": resolved_files.iter().cloned().collect::<Vec<_>>()
            });
            finalize_refreshed_candidate(
                ctx,
                authority_status,
                &worktree_git,
                &pending.branch,
                &pending.worktree,
                &pending.original_base,
                &pending.original_candidate,
                &pending.target_commit,
                Some(note),
            )
            .map(ContinueStep::Finished)
        })?;
        match continue_step {
            ContinueStep::Finished(outcome) => return Ok(outcome),
            ContinueStep::TipMoved { current } => {
                // Same race as the pre-merge tip check: surface it as
                // TargetAdvanced so the caller's bounded refresh loop retries
                // against the moved tip (and falls back after its budget).
                return Err(RefineError::TargetAdvanced {
                    reference: pending.target_branch.clone(),
                    expected: pending.target_commit.clone(),
                    current,
                });
            }
            ContinueStep::NextStop(result) => {
                stop_conflicts = result.conflicts;
                stop_baseline = changed_paths(&worktree_git)?;
            }
            ContinueStep::Broken(result) => {
                attempts.push(json!({
                    "outcome": "continue_failed",
                    "reason": result.message
                }));
                return abort_conflicted_refresh_and_queue(
                    ctx,
                    authority_status,
                    max_automatic_round_retries,
                    &pending,
                    json!({"outcome": "failed", "attempts": attempts}),
                );
            }
        }
    }
}

/// Every path the worktree currently reports as changed or untracked.
fn changed_paths(worktree_git: &FileGitWorktreeService) -> RefineResult<BTreeSet<String>> {
    let status = worktree_git.inspect("")?;
    Ok(status
        .dirty_paths
        .into_iter()
        .chain(status.untracked_paths)
        .collect())
}

/// Exactly today's fallback — abort the rebase and queue a fenced recovery
/// Round — with the resolution note (attempts, outcome, question) added to
/// the retained evidence.
fn abort_conflicted_refresh_and_queue(
    ctx: &mut WorkflowContext<'_>,
    authority_status: &GoalStatus,
    max_automatic_round_retries: u32,
    pending: &ConflictedRefresh,
    conflict_resolution: Value,
) -> RefineResult<CandidateRefreshOutcome> {
    let target_root = ctx.target_root.to_path_buf();
    let worktree_git =
        FileGitWorktreeService::with_runtime_root(&pending.worktree, ctx.runtime_root);
    let (recovery, restored) = with_repository_git_lock(&target_root, || {
        let recovery = worktree_git.recover()?;
        let restored = worktree_git.head_ref()?;
        Ok((recovery, restored))
    })?;
    queue_recovery(
        ctx,
        authority_status,
        max_automatic_round_retries,
        "candidate refresh conflicted",
        json!({
            "original_base_commit": pending.original_base,
            "original_candidate_commit": pending.original_candidate,
            "branch": pending.branch,
            "worktree": pending.worktree,
            "target_branch": pending.target_branch,
            "target_commit": pending.target_commit,
            "rebase": pending.rebase,
            "rebase_abort": recovery,
            "restored_candidate_head": restored,
            "conflict_resolution": conflict_resolution
        }),
    )
}

/// The domain-terms question retained when the resolution budget is spent.
fn resolution_question(
    ctx: &WorkflowContext<'_>,
    pending: &ConflictedRefresh,
    remaining: &[String],
) -> String {
    format!(
        "Goal {}: rebasing candidate branch {} onto {} ({}) still conflicted in {} after {} resolution attempts. Should the Round intent be re-implemented on the advanced target (the queued recovery Round), or should an operator resolve the rebase by hand on {}?",
        ctx.goal_id,
        pending.branch,
        pending.target_branch,
        pending.target_commit,
        remaining.join(", "),
        RESOLUTION_ATTEMPT_LIMIT,
        pending.branch
    )
}

fn round_prompt(detail: &Value, round_idx: usize) -> String {
    detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(round_idx))
        .and_then(|round| round.get("prompt"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Implementation reports of other Refine goals whose integrated commits are
/// behind the conflicting range, so the resolver sees both intents. Prompt
/// enrichment only: lookup failures degrade to "(none)", never block
/// resolution.
fn conflicting_goal_reports(
    ctx: &WorkflowContext<'_>,
    target_git: &FileGitWorktreeService,
    pending: &ConflictedRefresh,
) -> String {
    let sections = (|| -> RefineResult<Vec<String>> {
        let commits = target_git.commits_in_range_touching(
            &pending.original_base,
            &pending.target_commit,
            &pending.rebase.conflicts,
        )?;
        if commits.is_empty() {
            return Ok(Vec::new());
        }
        let mut sections = Vec::new();
        for summary in ctx.work_items.list_goal_summaries()? {
            if summary.goal.id == ctx.goal_id {
                continue;
            }
            let detail = ctx.work_items.show_goal_detail(&summary.goal.id)?;
            let Some(rounds) = detail.get("rounds").and_then(Value::as_array) else {
                continue;
            };
            for round in rounds.iter().rev() {
                let Some(integration) = round
                    .get("workflow_integration")
                    .filter(|value| !value.is_null())
                else {
                    continue;
                };
                let candidate = integration
                    .get("candidate_commit")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let premerge_target = integration
                    .get("target_commit")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if candidate.is_empty() {
                    continue;
                }
                // A conflicting commit belongs to this goal when it is in the
                // candidate's history but not in the target history the
                // candidate was integrated onto.
                let owns = commits.iter().any(|commit| {
                    commit == candidate
                        || (target_git
                            .commit_is_ancestor(commit, candidate)
                            .unwrap_or(false)
                            && !premerge_target.is_empty()
                            && !target_git
                                .commit_is_ancestor(commit, premerge_target)
                                .unwrap_or(true))
                });
                if owns {
                    if let Some(report) = round
                        .get("implementation_report")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|report| !report.is_empty())
                    {
                        sections.push(format!(
                            "Goal {} — {}:\n{}",
                            summary.goal.id, summary.goal.name, report
                        ));
                    }
                    break;
                }
            }
        }
        Ok(sections)
    })()
    .unwrap_or_default();
    if sections.is_empty() {
        "(none)".to_string()
    } else {
        sections.join("\n\n")
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn persist_candidate_refresh_or_restore(
    ctx: &mut WorkflowContext<'_>,
    authority_status: &GoalStatus,
    worktree_git: &FileGitWorktreeService,
    branch: &str,
    worktree: &str,
    original_base: &str,
    original_candidate: &str,
    target_commit: &str,
    replacement: &str,
    conflict_resolution: Option<Value>,
) -> RefineResult<Value> {
    let persist = (|| {
        ctx.revalidate_authority(authority_status.clone())?;
        ctx.work_items.record_candidate_refresh(
            &ctx.goal_id,
            ctx.attempt_authority,
            authority_status,
            &ctx.node_id,
            branch,
            worktree,
            original_base,
            original_candidate,
            target_commit,
            replacement,
            conflict_resolution,
        )
    })();
    match persist {
        Ok(evidence) => Ok(evidence),
        Err(error) => {
            match restore_unpersisted_refresh(worktree_git, branch, replacement, original_candidate)
            {
                Ok(()) => Err(error),
                Err(rollback) => Err(RefineError::Conflict(format!(
                    "{error}; candidate refresh replacement {replacement} was not persisted and branch {branch} could not be restored to {original_candidate}: {rollback}"
                ))),
            }
        }
    }
}

fn restore_unpersisted_refresh(
    worktree_git: &FileGitWorktreeService,
    branch: &str,
    replacement: &str,
    original_candidate: &str,
) -> RefineResult<()> {
    let head = worktree_git.head_ref()?;
    let status = worktree_git.inspect("")?;
    if head.branch.as_deref() != Some(branch)
        || head.commit.as_deref() != Some(replacement)
        || !status.is_pristine()
    {
        return Err(RefineError::Conflict(
            "the refreshed checkout changed or became dirty after rebase; user work was preserved"
                .to_string(),
        ));
    }
    worktree_git.reset_hard_to(original_candidate)
}

fn queue_recovery(
    ctx: &mut WorkflowContext<'_>,
    authority_status: &GoalStatus,
    max_automatic_round_retries: u32,
    reason: &str,
    evidence: Value,
) -> RefineResult<CandidateRefreshOutcome> {
    let recovery = ctx.work_items.queue_integration_recovery_summary(
        &ctx.goal_id,
        ctx.attempt_authority,
        authority_status,
        &ctx.node_id,
        reason,
        evidence.clone(),
        max_automatic_round_retries,
    )?;
    if recovery.goal.status == GoalStatus::Todo {
        Ok(CandidateRefreshOutcome::RecoveryQueued {
            reason: reason.to_string(),
            evidence,
        })
    } else {
        Ok(CandidateRefreshOutcome::RecoveryExhausted {
            reason: reason.to_string(),
            evidence,
        })
    }
}

fn required(value: &Value, key: &str, goal_id: &str) -> RefineResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RefineError::Conflict(format!("Goal {goal_id} has no recorded {key}")))
}
