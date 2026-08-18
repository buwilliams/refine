use super::*;

use std::sync::mpsc;
use std::time::Duration;

use crate::tools::git::resolve::{
    ResolutionRequest, ResolverOutcome, ScriptedResolver, ScriptedStep,
};
use crate::tools::git::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::product::work_items::WorkflowAttemptAuthority;
use crate::workflow::context::WorkflowContext;

struct RefreshFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    worktree: PathBuf,
    work_items: FileWorkItemService,
    base: String,
    candidate: String,
    target: String,
    authority: WorkflowAttemptAuthority,
}

impl RefreshFixture {
    fn new(conflict: bool) -> Self {
        Self::new_with_retry(conflict, None)
    }

    fn new_with_retry(conflict: bool, automatic_retry_attempt: Option<u32>) -> Self {
        let temp_root = unique_temp_dir("workflow-candidate-refresh");
        let target_root = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        let worktree = temp_root.join("candidate");
        fs::create_dir_all(&target_root).unwrap();
        git(&target_root, &["init", "-b", "main"]).unwrap();
        git(
            &target_root,
            &["config", "user.email", "refine-test@example.invalid"],
        )
        .unwrap();
        git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
        fs::write(target_root.join("app.txt"), "base\n").unwrap();
        git(&target_root, &["add", "app.txt"]).unwrap();
        git(&target_root, &["commit", "-m", "base"]).unwrap();
        let base = git_output(&target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        git(
            &target_root,
            &[
                "worktree",
                "add",
                "-b",
                "refine/GOAL1/round-1",
                worktree.to_str().unwrap(),
            ],
        )
        .unwrap();
        if conflict {
            fs::write(worktree.join("app.txt"), "candidate\n").unwrap();
            git(&worktree, &["add", "app.txt"]).unwrap();
        } else {
            fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
            git(&worktree, &["add", "candidate.txt"]).unwrap();
        }
        git(&worktree, &["commit", "-m", "candidate"]).unwrap();
        let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        if conflict {
            fs::write(target_root.join("app.txt"), "target\n").unwrap();
            git(&target_root, &["add", "app.txt"]).unwrap();
        } else {
            fs::write(target_root.join("sibling.txt"), "sibling\n").unwrap();
            git(&target_root, &["add", "sibling.txt"]).unwrap();
        }
        git(&target_root, &["commit", "-m", "advance target"]).unwrap();
        let target = git_output(&target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let refine_dir = test_refine_dir(&target_root);
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Refresh candidate", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Buddy", "Implement")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        if let Some(attempt) = automatic_retry_attempt {
            work_items
                .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
                .unwrap();
            work_items
                .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
                .unwrap();
            work_items
                .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
                .unwrap();
            work_items
                .queue_quality_recovery_summary(
                    "GOAL1",
                    0,
                    None,
                    attempt,
                    "seed shared automatic retry lineage",
                    "Continue the recovery lineage.",
                )
                .unwrap();
        }
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs(
                "GOAL1",
                "refine/GOAL1/round-1",
                "main",
                &base,
                Some(&candidate),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Governance)
            .unwrap();
        let (round_idx, revision, request) = work_items.authored_goal_commitment("GOAL1").unwrap();
        let authority = work_items
            .claim_workflow_attempt(
                "GOAL1",
                GoalStatus::Governance,
                round_idx,
                revision,
                &request,
            )
            .unwrap();
        Self {
            temp_root,
            target_root,
            runtime_root,
            worktree,
            work_items,
            base,
            candidate,
            target,
            authority,
        }
    }

    fn context(&self) -> WorkflowContext<'_> {
        let mut context = WorkflowContext::new(
            &self.runtime_root,
            &self.target_root,
            "GOAL1".to_string(),
            "default".to_string(),
            "smoke-ai".to_string(),
            self.authority.round_idx,
            self.authority,
            Default::default(),
            self.work_items.clone(),
        );
        context.branch = Some("refine/GOAL1/round-1".to_string());
        context.worktree_path = Some(self.worktree.display().to_string());
        context.commit = Some(self.candidate.clone());
        context
    }
}

impl Drop for RefreshFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

fn attempt_concurrent_refresh(
    target_root: PathBuf,
    runtime_root: PathBuf,
    worktree: PathBuf,
    work_items: FileWorkItemService,
    candidate: String,
    authority: WorkflowAttemptAuthority,
    barrier: std::sync::Arc<std::sync::Barrier>,
) -> RefineResult<CandidateRefreshOutcome> {
    let mut context = WorkflowContext::new(
        &runtime_root,
        &target_root,
        "GOAL1".to_string(),
        "default".to_string(),
        "smoke-ai".to_string(),
        0,
        authority,
        Default::default(),
        work_items,
    );
    context.branch = Some("refine/GOAL1/round-1".to_string());
    context.worktree_path = Some(worktree.display().to_string());
    context.commit = Some(candidate);
    barrier.wait();
    refresh_candidate_for_target_advancement(&mut context, 5)
}

#[test]
fn target_advancement_rebases_a_provable_delta_and_records_both_identities() {
    let fixture = RefreshFixture::new(false);
    let mut context = fixture.context();
    let outcome = refresh_candidate_for_target_advancement(&mut context, 5).unwrap();
    let CandidateRefreshOutcome::Refreshed {
        replacement_candidate,
        ..
    } = outcome
    else {
        panic!("expected candidate refresh")
    };
    assert_ne!(replacement_candidate, fixture.candidate);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["base_commit"], fixture.target);
    assert_eq!(detail["candidate_commit"], replacement_candidate);
    assert_eq!(
        detail["rounds"][0]["workflow_candidate_refresh"]["original_base_commit"],
        fixture.base
    );
    assert_eq!(
        detail["rounds"][0]["workflow_candidate_refresh"]["original_candidate_commit"],
        fixture.candidate
    );
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        replacement_candidate
    );
    assert!(
        !git_output(
            &fixture.target_root,
            &["cat-file", "-t", &fixture.candidate]
        )
        .trim()
        .is_empty()
    );
}

#[test]
fn repository_lease_serializes_competing_target_refreshes() {
    let fixture = RefreshFixture::new(false);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let spawn = |barrier: std::sync::Arc<std::sync::Barrier>| {
        let target_root = fixture.target_root.clone();
        let runtime_root = fixture.runtime_root.clone();
        let worktree = fixture.worktree.clone();
        let work_items = fixture.work_items.clone();
        let candidate = fixture.candidate.clone();
        let authority = fixture.authority;
        std::thread::spawn(move || {
            attempt_concurrent_refresh(
                target_root,
                runtime_root,
                worktree,
                work_items,
                candidate,
                authority,
                barrier,
            )
        })
    };
    let first = spawn(barrier.clone());
    let second = spawn(barrier);
    let outcomes = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Ok(CandidateRefreshOutcome::Refreshed { .. })))
            .count(),
        1
    );
    let rejected = outcomes
        .iter()
        .find_map(|outcome| outcome.as_ref().err())
        .unwrap();
    assert!(
        rejected
            .to_string()
            .contains("hydrated candidate identity changed")
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    let replacement = detail["candidate_commit"].as_str().unwrap();
    assert_ne!(replacement, fixture.candidate);
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(
        detail["rounds"][0]["workflow_candidate_refresh"]["original_candidate_commit"],
        fixture.candidate
    );
    assert_eq!(
        git_output(&fixture.worktree, &["status", "--porcelain"]),
        ""
    );
}

#[test]
fn authority_loss_after_rebase_restores_the_original_candidate_branch() {
    let fixture = RefreshFixture::new(false);
    let mut context = fixture.context();
    let worktree_git =
        FileGitWorktreeService::with_runtime_root(&fixture.worktree, &fixture.runtime_root);
    let rebase = worktree_git.rebase("main").unwrap();
    assert!(rebase.ok, "{rebase:?}");
    let replacement = git_output(&fixture.worktree, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    assert_ne!(replacement, fixture.candidate);
    fixture.work_items.cancel_goal_summary("GOAL1").unwrap();

    let error = crate::workflow::candidate_refresh::persist_candidate_refresh_or_restore(
        &mut context,
        &worktree_git,
        "refine/GOAL1/round-1",
        fixture.worktree.to_str().unwrap(),
        &fixture.base,
        &fixture.candidate,
        &fixture.target,
        &replacement,
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no longer authorizes governance work"),
        "{error}"
    );
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    assert_eq!(
        fixture.work_items.show_goal_detail("GOAL1").unwrap()["candidate_commit"],
        fixture.candidate
    );
}

#[test]
fn refresh_conflict_aborts_and_queues_one_fenced_recovery_round() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    // Resolver disabled (`workflow_conflict_resolution` off): exactly the
    // pre-resolver behavior.
    let outcome = refresh_candidate_with_resolver(&mut context, 5, None).unwrap();
    assert!(matches!(
        outcome,
        CandidateRefreshOutcome::RecoveryQueued { .. }
    ));
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "todo");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["reason"],
        "candidate refresh conflicted"
    );
    assert_eq!(
        detail["rounds"][1]["automatic_retry"]["kind"],
        "integration"
    );
    assert_eq!(detail["rounds"][1]["automatic_retry"]["attempt"], 1);
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["original_candidate_commit"],
        fixture.candidate
    );
    assert!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["rebase"]["conflicts"]
            .as_array()
            .is_some_and(|conflicts| conflicts.iter().any(|path| path == "app.txt"))
    );
}

#[test]
fn refresh_conflict_records_explicit_exhaustion_without_a_successor_round() {
    let fixture = RefreshFixture::new_with_retry(true, Some(5));
    let mut context = fixture.context();
    let outcome = refresh_candidate_with_resolver(&mut context, 5, None).unwrap();
    assert!(matches!(
        outcome,
        CandidateRefreshOutcome::RecoveryExhausted { .. }
    ));
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(
        detail["rounds"][1]["workflow_recovery"]["state"],
        "exhausted"
    );
    assert_eq!(detail["rounds"][1]["workflow_recovery"]["attempt"], 5);
    assert_eq!(
        detail["rounds"][1]["failure_category"],
        "integration_retry_exhausted"
    );
    assert_eq!(
        detail["rounds"][1]["workflow_recovery"]["retained_evidence"]["original_candidate_commit"],
        fixture.candidate
    );
}

#[test]
fn integration_merge_conflict_queues_one_fenced_recovery_round_with_the_conflicted_paths() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    let outcome = crate::workflow::behaviors::queue_integration_merge_conflict_recovery(
        &mut context,
        vec!["app.txt".to_string()],
        "candidate integration failed: CONFLICT (content): Merge conflict in app.txt".to_string(),
        5,
    )
    .unwrap();
    let crate::workflow::behavior::WorkflowAdvanceOutcome::Completed { final_status, .. } = outcome
    else {
        panic!("expected a completed outcome");
    };
    assert_eq!(final_status, GoalStatus::Todo);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "todo");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["reason"],
        "candidate integration merge conflicted"
    );
    assert_eq!(
        detail["rounds"][1]["automatic_retry"]["kind"],
        "integration"
    );
    assert_eq!(detail["rounds"][1]["automatic_retry"]["attempt"], 1);
    // The conflicted paths survive into the recovery Round evidence exactly
    // as the rebase path retains `rebase.conflicts`.
    assert!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["merge"]["conflicts"]
            .as_array()
            .is_some_and(|conflicts| conflicts.iter().any(|path| path == "app.txt"))
    );
}

#[test]
fn integration_merge_conflict_exhaustion_fails_with_integration_retry_exhausted() {
    let fixture = RefreshFixture::new_with_retry(true, Some(5));
    let mut context = fixture.context();
    let outcome = crate::workflow::behaviors::queue_integration_merge_conflict_recovery(
        &mut context,
        vec!["app.txt".to_string()],
        "candidate integration failed: CONFLICT (content): Merge conflict in app.txt".to_string(),
        5,
    )
    .unwrap();
    let crate::workflow::behavior::WorkflowAdvanceOutcome::Completed { final_status, .. } = outcome
    else {
        panic!("expected a completed outcome");
    };
    assert_eq!(final_status, GoalStatus::Failed);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "failed");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(
        detail["rounds"][1]["workflow_recovery"]["state"],
        "exhausted"
    );
    assert_eq!(
        detail["rounds"][1]["failure_category"],
        "integration_retry_exhausted"
    );
    assert!(
        detail["rounds"][1]["workflow_recovery"]["retained_evidence"]["merge"]["conflicts"]
            .as_array()
            .is_some_and(|conflicts| conflicts.iter().any(|path| path == "app.txt"))
    );
}

#[test]
fn integration_recovery_uses_the_next_shared_automatic_retry_attempt() {
    let fixture = RefreshFixture::new_with_retry(true, Some(4));
    let mut context = fixture.context();
    let outcome = refresh_candidate_with_resolver(&mut context, 5, None).unwrap();
    assert!(matches!(
        outcome,
        CandidateRefreshOutcome::RecoveryQueued { .. }
    ));
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    // The claimed trailing Round was itself an unworked automation-appended
    // recovery Round, so the queued recovery reuses it in place instead of
    // growing an unbounded chain of inert Rounds; the shared attempt counter
    // still advances and lineage keeps naming the worked source Round.
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(
        detail["rounds"][1]["automatic_retry"]["kind"],
        "integration"
    );
    assert_eq!(detail["rounds"][1]["automatic_retry"]["attempt"], 5);
    assert_eq!(detail["rounds"][1]["automatic_retry"]["source_round"], 1);
}

fn completing_edit(edit: impl FnMut(&ResolutionRequest<'_>) + Send + 'static) -> ScriptedStep {
    let mut edit = edit;
    Box::new(move |request| {
        edit(request);
        Ok(ResolverOutcome::Completed)
    })
}

#[test]
fn workflow_conflict_resolution_defaults_on_and_honors_off() {
    let mut settings = crate::model::JsonObject::new();
    assert!(workflow_conflict_resolution_enabled(&settings));
    settings.insert("workflow_conflict_resolution".to_string(), json!("off"));
    assert!(!workflow_conflict_resolution_enabled(&settings));
    settings.insert("workflow_conflict_resolution".to_string(), json!("on"));
    assert!(workflow_conflict_resolution_enabled(&settings));
}

#[test]
fn conflicted_refresh_resolves_in_place_without_holding_the_repository_lock() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    let probe_root = fixture.target_root.clone();
    let resolver = ScriptedResolver::new(vec![completing_edit(move |request| {
        // The resolver must run outside the repository lock: a probe thread
        // acquiring it has to succeed while the resolver is in flight.
        let (sender, receiver) = mpsc::channel();
        let lock_root = probe_root.clone();
        std::thread::spawn(move || {
            let acquired = with_repository_git_lock(&lock_root, || Ok(())).is_ok();
            let _ = sender.send(acquired);
        });
        assert!(
            receiver
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or(false),
            "repository lock was held during resolver invocation"
        );
        // The markers carry the merge base as well as both sides, whatever
        // the user's own `merge.conflictStyle` is: a resolver cannot tell
        // which side changed what from two versions alone.
        let conflicted = fs::read_to_string(request.workspace_dir.join("app.txt")).unwrap();
        assert!(
            conflicted.contains("|||||||") && conflicted.contains("base\n"),
            "the conflicted file must show the base section: {conflicted}"
        );
        fs::write(
            request.workspace_dir.join("app.txt"),
            "candidate and target\n",
        )
        .unwrap();
    })]);

    let outcome = refresh_candidate_with_resolver(&mut context, 5, Some(&resolver)).unwrap();

    let CandidateRefreshOutcome::Refreshed {
        replacement_candidate,
        evidence,
        ..
    } = outcome
    else {
        panic!("expected a resolved refresh, got a recovery outcome");
    };
    assert_ne!(replacement_candidate, fixture.candidate);
    // No recovery Round was queued; the note lives in the refresh evidence.
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "governance");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(detail["candidate_commit"], replacement_candidate);
    let resolution = &detail["rounds"][0]["workflow_candidate_refresh"]["conflict_resolution"];
    assert_eq!(resolution["outcome"], "resolved");
    assert!(
        resolution["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file == "app.txt"))
    );
    assert_eq!(
        resolution["attempts"].as_array().unwrap().len(),
        1,
        "{resolution}"
    );
    assert_eq!(resolution["attempts"][0]["outcome"], "accepted");
    assert_eq!(evidence["conflict_resolution"]["outcome"], "resolved");
    assert_eq!(
        git_output(&fixture.worktree, &["status", "--porcelain"]),
        ""
    );
    assert_eq!(
        git_output(
            &fixture.worktree,
            &["show", &format!("{replacement_candidate}:app.txt")]
        ),
        "candidate and target\n"
    );
    // The resolved delta still descends linearly from the advanced target.
    assert_eq!(
        git_output(
            &fixture.worktree,
            &["merge-base", &fixture.target, &replacement_candidate]
        )
        .trim(),
        fixture.target
    );
}

#[test]
fn rejected_resolutions_exhaust_the_budget_into_todays_fallback_with_a_question() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    // Both attempts claim completion without touching the workspace, so the
    // no-markers gate rejects each one.
    let resolver = ScriptedResolver::new(vec![
        completing_edit(|_request| {}),
        completing_edit(|request| {
            assert!(
                request
                    .feedback
                    .is_some_and(|feedback| feedback.contains("conflict markers")),
                "second attempt must carry the gate rejection as feedback"
            );
        }),
    ]);

    let outcome = refresh_candidate_with_resolver(&mut context, 5, Some(&resolver)).unwrap();

    let CandidateRefreshOutcome::RecoveryQueued { reason, evidence } = outcome else {
        panic!("expected today's recovery fallback");
    };
    assert_eq!(reason, "candidate refresh conflicted");
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "todo");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    let retained = &detail["rounds"][0]["workflow_recovery"]["retained_evidence"];
    assert!(
        retained["rebase"]["conflicts"]
            .as_array()
            .is_some_and(|conflicts| conflicts.iter().any(|path| path == "app.txt"))
    );
    assert_eq!(retained["conflict_resolution"]["outcome"], "needs_decision");
    assert_eq!(
        retained["conflict_resolution"]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let question = retained["conflict_resolution"]["question"]
        .as_str()
        .unwrap();
    assert!(question.contains("GOAL1"), "{question}");
    assert!(question.contains("app.txt"), "{question}");
    assert_eq!(evidence["conflict_resolution"]["outcome"], "needs_decision");
}

#[test]
fn an_unavailable_resolver_falls_back_immediately_without_more_attempts() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counted = std::sync::Arc::clone(&calls);
    let resolver = ScriptedResolver::new(vec![Box::new(move |_request: &ResolutionRequest<'_>| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ResolverOutcome::Unavailable)
    }) as ScriptedStep]);

    let outcome = refresh_candidate_with_resolver(&mut context, 5, Some(&resolver)).unwrap();

    assert!(matches!(
        outcome,
        CandidateRefreshOutcome::RecoveryQueued { .. }
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["conflict_resolution"]["outcome"],
        "unavailable"
    );
}

#[test]
fn a_target_tip_moved_during_resolution_retries_boundedly_then_resolves() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    let advance_root = fixture.target_root.clone();
    let resolver = ScriptedResolver::new(vec![completing_edit(move |request| {
        // The target advances while the resolver holds no lock; the
        // continue hold must detect it and abort instead of publishing a
        // replacement based on a stale tip.
        fs::write(advance_root.join("app.txt"), "target v2\n").unwrap();
        git(&advance_root, &["add", "app.txt"]).unwrap();
        git(
            &advance_root,
            &["commit", "-m", "advance during resolution"],
        )
        .unwrap();
        fs::write(
            request.workspace_dir.join("app.txt"),
            "candidate and target\n",
        )
        .unwrap();
    })]);

    let error = refresh_candidate_with_resolver(&mut context, 5, Some(&resolver)).unwrap_err();
    let RefineError::TargetAdvanced {
        expected, current, ..
    } = error
    else {
        panic!("expected TargetAdvanced for the caller's bounded retry loop");
    };
    assert_eq!(expected, fixture.target);
    assert_ne!(current, fixture.target);
    // The rebase was aborted: the candidate branch is restored and no state
    // was persisted, so the caller's refresh loop can simply run again.
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    assert_eq!(
        git_output(&fixture.worktree, &["status", "--porcelain"]),
        ""
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "governance");
    assert_eq!(detail["candidate_commit"], fixture.candidate);

    // The bounded retry (the caller's refresh-again loop) succeeds against
    // the moved tip.
    let retry_resolver = ScriptedResolver::new(vec![completing_edit(|request| {
        fs::write(
            request.workspace_dir.join("app.txt"),
            "candidate and target v2\n",
        )
        .unwrap();
    })]);
    let outcome = refresh_candidate_with_resolver(&mut context, 5, Some(&retry_resolver)).unwrap();
    let CandidateRefreshOutcome::Refreshed {
        replacement_candidate,
        target_commit,
        ..
    } = outcome
    else {
        panic!("expected the retried refresh to resolve");
    };
    assert_eq!(target_commit, current);
    assert_eq!(
        git_output(
            &fixture.worktree,
            &["show", &format!("{replacement_candidate}:app.txt")]
        ),
        "candidate and target v2\n"
    );
}

/// `rebase --continue` commits the whole index, so scope is an acceptance
/// gate rather than an instruction: an edit outside the conflicted files is
/// rejected with feedback and never reaches the replacement candidate.
#[test]
fn an_out_of_scope_edit_is_rejected_before_it_reaches_the_replacement() {
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    let resolver = ScriptedResolver::new(vec![
        completing_edit(|request| {
            fs::write(
                request.workspace_dir.join("app.txt"),
                "candidate and target\n",
            )
            .unwrap();
            fs::write(request.workspace_dir.join("unrelated.txt"), "drive-by\n").unwrap();
        }),
        completing_edit(|request| {
            assert!(
                request
                    .feedback
                    .is_some_and(|feedback| feedback.contains("unrelated.txt")),
                "the rejection must name the out-of-scope file: {:?}",
                request.feedback
            );
            fs::remove_file(request.workspace_dir.join("unrelated.txt")).unwrap();
            fs::write(
                request.workspace_dir.join("app.txt"),
                "candidate and target\n",
            )
            .unwrap();
        }),
    ]);

    let outcome = refresh_candidate_with_resolver(&mut context, 5, Some(&resolver)).unwrap();

    let CandidateRefreshOutcome::Refreshed {
        replacement_candidate,
        ..
    } = outcome
    else {
        panic!("expected the corrected second attempt to resolve");
    };
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    let resolution = &detail["rounds"][0]["workflow_candidate_refresh"]["conflict_resolution"];
    assert_eq!(resolution["attempts"][0]["outcome"], "rejected");
    assert_eq!(
        resolution["attempts"][0]["out_of_scope_files"][0],
        "unrelated.txt"
    );
    assert_eq!(resolution["attempts"][1]["outcome"], "accepted");
    // The replacement carries the resolution and nothing else.
    let files = git_output(
        &fixture.worktree,
        &["ls-tree", "-r", "--name-only", &replacement_candidate],
    );
    assert!(!files.contains("unrelated.txt"), "{files}");
}

/// A resolver that reads both intents and cannot choose escalates with its
/// own question, and is not asked again for the rest of its budget.
#[test]
fn a_declared_decision_escalates_with_the_resolvers_own_question() {
    const QUESTION: &str = "Goal GOAL1 rewrote app.txt for the candidate line while the target rewrote it for \
         another; which line should survive?";
    let fixture = RefreshFixture::new(true);
    let mut context = fixture.context();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let counted = std::sync::Arc::clone(&calls);
    let resolver = ScriptedResolver::new(vec![Box::new(move |_request: &ResolutionRequest<'_>| {
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ResolverOutcome::NeedsDecision {
            question: QUESTION.to_string(),
        })
    }) as ScriptedStep]);

    let outcome = refresh_candidate_with_resolver(&mut context, 5, Some(&resolver)).unwrap();

    assert!(matches!(
        outcome,
        CandidateRefreshOutcome::RecoveryQueued { .. }
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // The rebase was aborted: the exact recorded candidate is restored.
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate
    );
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    let retained = &detail["rounds"][0]["workflow_recovery"]["retained_evidence"];
    assert_eq!(retained["conflict_resolution"]["outcome"], "needs_decision");
    assert_eq!(retained["conflict_resolution"]["question"], QUESTION);
}

/// Crash-only: an interruption during unlocked resolution leaves the rebase
/// stopped mid-pick. The next attempt rejects that checkout on provenance —
/// and must abort the rebase it found rather than hand the recovery Round a
/// wedged worktree.
#[test]
fn an_interrupted_rebase_is_aborted_before_the_recovery_round_is_queued() {
    let fixture = RefreshFixture::new(true);
    // Exactly what a crash between the two holds leaves behind.
    assert!(git(&fixture.worktree, &["rebase", "main"]).is_err());
    let worktree_git =
        FileGitWorktreeService::with_runtime_root(&fixture.worktree, &fixture.runtime_root);
    assert!(worktree_git.operation_in_progress().unwrap());
    let mut context = fixture.context();

    let outcome = refresh_candidate_with_resolver(&mut context, 5, None).unwrap();

    let CandidateRefreshOutcome::RecoveryQueued { evidence, .. } = outcome else {
        panic!("expected the recovery fallback");
    };
    assert!(!worktree_git.operation_in_progress().unwrap());
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate,
        "aborting restores the exact recorded candidate"
    );
    assert_eq!(
        evidence["restored_candidate_head"]["commit"], fixture.candidate,
        "{evidence:#}"
    );
    assert!(!evidence["rebase_abort"].is_null(), "{evidence:#}");
}
