//! Always-on candidate base refresh at the Implement → Quality boundary.
//!
//! `base_commit` is pinned once at Todo → Plan. Without a refresh here, every
//! Goal that integrates while this one implements is only discovered by the
//! Governance-time refresh — one late rebase after a full Quality run. These
//! tests pin the boundary's contract: free when the target has not moved,
//! repinned (with Quality's stale evidence cleared) when it has, and the exact
//! same resolve-in-place / fenced-recovery policy stage 3 shipped when the
//! rebase conflicts.

#![cfg(unix)]

use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::application::persistence_sync::resolution::{
    ResolutionRequest, ResolverOutcome, ScriptedResolver, ScriptedStep,
};
use crate::application::work_items::WorkflowAttemptAuthority;
use crate::application::workflow::engine::behaviors::contract::WorkflowAdvanceOutcome;
use crate::application::workflow::engine::behaviors::refresh_candidate_at_quality_boundary_with_resolver;
use crate::application::workflow::engine::context::WorkflowContext;
use crate::application::workflow::phases::quality::{FileQualityService, QualitySettingsPatch};
use serde_json::Value;

const GOAL: &str = "GOAL1";
const BRANCH: &str = "refine/GOAL1/round-1";

/// A Goal exactly where the Implement → Quality transition leaves it: status
/// Quality, the Round branch committed to its candidate in a managed worktree,
/// and `base_commit` still pinned at the commit Todo → Plan chose.
struct BoundaryFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    smoke_ai: PathBuf,
    work_items: FileWorkItemService,
    worktree: PathBuf,
    base: String,
    candidate: String,
}

impl BoundaryFixture {
    /// `overlapping` decides whether the candidate edits the same file a later
    /// target advance will edit — the difference between a clean rebase and a
    /// conflicted one.
    fn new(prefix: &str, overlapping: bool) -> Self {
        let temp_root = unique_temp_dir(prefix);
        let target_root = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
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
        let worktree = target_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
        fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        git(
            &target_root,
            &["worktree", "add", "-b", BRANCH, worktree.to_str().unwrap()],
        )
        .unwrap();
        if overlapping {
            fs::write(worktree.join("app.txt"), "candidate\n").unwrap();
        } else {
            fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
        }
        git(&worktree, &["add", "-A"]).unwrap();
        git(&worktree, &["commit", "-m", "candidate"]).unwrap();
        let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"Post-implementation Quality evaluation\"*)\n\
               printf '%s\\n' '{\"ok\":true,\"summary\":\"The candidate passes.\",\"results\":[{\"test\":\"Candidate works\",\"status\":\"passed\",\"evidence\":\"Verified.\",\"command\":\"true\"}]}'\n\
               ;;\n\
             *\"Post-implementation governance review\"*)\n\
               printf '%s\\n' '{\"status\":\"passed\",\"message\":\"Compliant.\"}'\n\
               ;;\n\
             *)\n\
               printf '%s\\n' 'smoke-ai goal-agent response'\n\
               ;;\n\
             esac\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }

        let refine_dir = test_refine_dir(&target_root);
        FileSettingsService::new(&refine_dir)
            .update(&json!({"agent_cli": "smoke-ai"}))
            .unwrap();
        FileQualityService::new(&refine_dir)
            .save_settings(QualitySettingsPatch {
                tests: Some(vec!["Candidate works".to_string()]),
                ..QualitySettingsPatch::default()
            })
            .unwrap();
        FileGovernanceService::new(&refine_dir)
            .save(&json!({
                "product": "A small app.",
                "constitution": "Keep the app healthy.",
                "rules": [{"id": "rule-1", "text": "Keep the app healthy.", "source": "manual"}]
            }))
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Refresh at the Quality boundary", Some(GOAL))
            .unwrap();
        work_items
            .append_goal_round_summary(GOAL, "Reporter", "Implement the candidate")
            .unwrap();
        work_items
            .transition_goal_status(GOAL, GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status(GOAL, GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs(GOAL, BRANCH, "main", &base, Some(&candidate))
            .unwrap();
        work_items
            .advance_automated_goal_status(GOAL, GoalStatus::Implement)
            .unwrap();
        work_items
            .advance_automated_goal_status(GOAL, GoalStatus::Quality)
            .unwrap();
        Self {
            temp_root,
            target_root,
            runtime_root,
            smoke_ai,
            work_items,
            worktree,
            base,
            candidate,
        }
    }

    /// Another Goal integrated while this one implemented.
    fn advance_target(&self, path: &str, contents: &str) -> String {
        fs::write(self.target_root.join(path), contents).unwrap();
        git(&self.target_root, &["add", path]).unwrap();
        git(&self.target_root, &["commit", "-m", "advance target"]).unwrap();
        git_output(&self.target_root, &["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }

    /// The target moved and already contains this candidate — the case rung 1
    /// answers with a single ancestry classification.
    fn advance_target_past_the_candidate(&self) -> String {
        git(&self.target_root, &["merge", "--ff-only", BRANCH]).unwrap();
        self.advance_target("sibling.txt", "sibling\n")
    }

    /// Seed the durable Quality evidence of an earlier attempt against the old
    /// base, so a refresh that failed to clear it would be visible.
    fn seed_stale_quality_evidence(&self) {
        self.work_items
            .update_goal_round_evaluation_summary(
                GOAL,
                0,
                &json!({
                    "quality_state": "passed",
                    "quality_candidate_commit": self.candidate,
                    "quality_checked_at": "2026-08-17T00:01:00Z",
                    "quality_details": {"operation_id": "OP-QUALITY-STALE"}
                }),
            )
            .unwrap();
    }

    fn claim(&self) -> WorkflowAttemptAuthority {
        self.claim_at(GoalStatus::Quality)
    }

    fn claim_at(&self, status: GoalStatus) -> WorkflowAttemptAuthority {
        let (round_idx, revision, request) =
            self.work_items.authored_goal_commitment(GOAL).unwrap();
        self.work_items
            .claim_workflow_attempt(GOAL, status, round_idx, revision, &request)
            .unwrap()
    }

    fn context(&self, authority: WorkflowAttemptAuthority) -> WorkflowContext<'_> {
        self.context_at(authority, &self.candidate)
    }

    fn context_at(
        &self,
        authority: WorkflowAttemptAuthority,
        candidate: &str,
    ) -> WorkflowContext<'_> {
        let mut context = WorkflowContext::new(
            &self.runtime_root,
            &self.target_root,
            GOAL.to_string(),
            "default".to_string(),
            "smoke-ai".to_string(),
            authority.round_idx,
            authority,
            Default::default(),
            self.work_items.clone(),
        );
        context.branch = Some(BRANCH.to_string());
        context.worktree_path = Some(self.worktree.display().to_string());
        context.commit = Some(candidate.to_string());
        context
    }

    fn detail(&self) -> Value {
        self.work_items.show_goal_detail(GOAL).unwrap()
    }

    /// Everything a refresh would have to disturb to do any work at all.
    fn checkout_trace(&self) -> (String, String, String, String) {
        (
            git_output(&self.worktree, &["rev-parse", "HEAD"]),
            git_output(&self.worktree, &["status", "--porcelain"]),
            git_output(
                &self.target_root,
                &["reflog", "show", "--format=%H", BRANCH],
            ),
            git_output(&self.target_root, &["rev-list", "--all", "--count"]),
        )
    }
}

impl Drop for BoundaryFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

fn completing_edit(edit: impl FnMut(&ResolutionRequest<'_>) + Send + 'static) -> ScriptedStep {
    let mut edit = edit;
    Box::new(move |request| {
        edit(request);
        Ok(ResolverOutcome::Completed)
    })
}

/// A one-answer resolver that counts how many times it was asked — so a test
/// can assert it was reached exactly once, or never reached at all.
fn counting_resolver(calls: &Arc<AtomicU32>, outcome: ResolverOutcome) -> ScriptedResolver {
    let counted = Arc::clone(calls);
    ScriptedResolver::new(vec![Box::new(move |_request: &ResolutionRequest<'_>| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(outcome.clone())
    }) as ScriptedStep])
}

/// Rung 0: the target is exactly the pinned base. The ladder answers before it
/// can touch anything — no rebase, no worktree churn, no repin, no resolver.
#[test]
fn an_unmoved_target_is_a_no_op_before_quality() {
    let fixture = BoundaryFixture::new("quality-refresh-unmoved", false);
    let authority = fixture.claim();
    let mut context = fixture.context(authority);
    let before = fixture.checkout_trace();
    let calls = Arc::new(AtomicU32::new(0));
    let resolver = counting_resolver(&calls, ResolverOutcome::Unavailable);

    let outcome =
        refresh_candidate_at_quality_boundary_with_resolver(&mut context, Some(&resolver)).unwrap();

    assert!(outcome.is_none(), "Quality must simply continue");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.checkout_trace(), before);
    let detail = fixture.detail();
    assert_eq!(detail["status"], "quality");
    assert_eq!(detail["base_commit"], fixture.base);
    assert_eq!(detail["candidate_commit"], fixture.candidate);
    assert!(
        detail["rounds"][0]["workflow_candidate_refresh"].is_null(),
        "nothing was repinned: {detail:#}"
    );
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(context.commit.as_deref(), Some(fixture.candidate.as_str()));
}

/// Rung 1: the target moved but already contains the candidate, so one
/// ancestry classification decides it — still nothing to rebase.
#[test]
fn a_moved_target_that_already_contains_the_candidate_is_a_no_op() {
    let fixture = BoundaryFixture::new("quality-refresh-contained", false);
    let target = fixture.advance_target_past_the_candidate();
    assert_ne!(target, fixture.base);
    let authority = fixture.claim();
    let mut context = fixture.context(authority);
    let before = fixture.checkout_trace();
    let calls = Arc::new(AtomicU32::new(0));
    let resolver = counting_resolver(&calls, ResolverOutcome::Unavailable);

    let outcome =
        refresh_candidate_at_quality_boundary_with_resolver(&mut context, Some(&resolver)).unwrap();

    assert!(outcome.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.checkout_trace(), before);
    let detail = fixture.detail();
    assert_eq!(detail["candidate_commit"], fixture.candidate);
    assert!(detail["rounds"][0]["workflow_candidate_refresh"].is_null());
}

/// The moved target and the candidate touch different files: the rebase is
/// clean, the Goal is repinned onto the advanced target, and the Round's
/// Quality evidence from the old base is cleared so the gate about to run
/// cannot reuse it.
#[test]
fn a_clean_refresh_repins_the_base_and_clears_stale_quality_evidence() {
    let fixture = BoundaryFixture::new("quality-refresh-clean", false);
    fixture.seed_stale_quality_evidence();
    let target = fixture.advance_target("sibling.txt", "sibling\n");
    let authority = fixture.claim();
    let mut context = fixture.context(authority);

    let outcome = refresh_candidate_at_quality_boundary_with_resolver(&mut context, None).unwrap();

    assert!(outcome.is_none(), "Quality must run on the refreshed base");
    let detail = fixture.detail();
    let replacement = detail["candidate_commit"].as_str().unwrap().to_string();
    assert_ne!(replacement, fixture.candidate);
    assert_eq!(detail["base_commit"], target);
    assert_eq!(context.commit.as_deref(), Some(replacement.as_str()));
    // Quality reads the Round's proof before it re-runs anything; the refresh
    // must leave nothing there that was proved against the old base.
    for cleared in [
        "quality_state",
        "quality_candidate_commit",
        "quality_checked_at",
        "quality_details",
    ] {
        assert!(
            detail["rounds"][0][cleared].is_null(),
            "{cleared} survived the repin: {detail:#}"
        );
    }
    let refresh = &detail["rounds"][0]["workflow_candidate_refresh"];
    assert_eq!(refresh["authorizing_status"], "quality");
    assert_eq!(refresh["original_base_commit"], fixture.base);
    assert_eq!(refresh["original_candidate_commit"], fixture.candidate);
    assert_eq!(refresh["replacement_base_commit"], target);
    assert_eq!(
        refresh["previous_gate_evidence"]["quality_details"]["operation_id"], "OP-QUALITY-STALE",
        "the superseded proof is retained, not destroyed: {refresh:#}"
    );
    // The checkout Quality will build is the replacement: the target's change
    // and the implementation's change together.
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        replacement
    );
    let files = git_output(
        &fixture.worktree,
        &["ls-tree", "-r", "--name-only", &replacement],
    );
    assert!(files.contains("sibling.txt"), "{files}");
    assert!(files.contains("candidate.txt"), "{files}");
    assert_eq!(
        git_output(&fixture.worktree, &["merge-base", &target, &replacement]).trim(),
        target
    );
}

/// A conflicted rebase is resolved in place, unlocked, exactly as stage 3
/// shipped — and the replacement candidate simply proceeds into Quality
/// instead of costing a re-implementation Round.
#[test]
fn a_conflicted_refresh_resolves_in_place_and_the_replacement_proceeds_to_quality() {
    let fixture = BoundaryFixture::new("quality-refresh-conflict", true);
    let target = fixture.advance_target("app.txt", "target\n");
    let authority = fixture.claim();
    let mut context = fixture.context(authority);
    let resolver = ScriptedResolver::new(vec![completing_edit(|request| {
        fs::write(
            request.workspace_dir.join("app.txt"),
            "candidate and target\n",
        )
        .unwrap();
    })]);

    let outcome =
        refresh_candidate_at_quality_boundary_with_resolver(&mut context, Some(&resolver)).unwrap();

    assert!(outcome.is_none(), "the resolved candidate continues");
    let detail = fixture.detail();
    // No recovery Round: the Goal is still the same Round in Quality.
    assert_eq!(detail["status"], "quality");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    let replacement = detail["candidate_commit"].as_str().unwrap().to_string();
    assert_ne!(replacement, fixture.candidate);
    assert_eq!(detail["base_commit"], target);
    let refresh = &detail["rounds"][0]["workflow_candidate_refresh"];
    assert_eq!(refresh["authorizing_status"], "quality");
    assert_eq!(refresh["conflict_resolution"]["outcome"], "resolved");
    assert!(
        refresh["conflict_resolution"]["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file == "app.txt"))
    );
    assert_eq!(
        git_output(
            &fixture.worktree,
            &["show", &format!("{replacement}:app.txt")]
        ),
        "candidate and target\n"
    );
    assert_eq!(context.commit.as_deref(), Some(replacement.as_str()));
}

/// No resolver available: the boundary takes exactly the pre-resolver
/// fallback — abort, restore the exact recorded candidate, and queue the
/// fenced integration recovery Round on the shared budget. The pass ends
/// there; Quality never runs on a candidate that no longer applies.
#[test]
fn an_unavailable_resolver_falls_back_to_the_fenced_recovery_round() {
    let fixture = BoundaryFixture::new("quality-refresh-unavailable", true);
    fixture.advance_target("app.txt", "target\n");
    let authority = fixture.claim();
    let mut context = fixture.context(authority);
    let calls = Arc::new(AtomicU32::new(0));
    let resolver = counting_resolver(&calls, ResolverOutcome::Unavailable);

    let outcome =
        refresh_candidate_at_quality_boundary_with_resolver(&mut context, Some(&resolver)).unwrap();

    let Some(WorkflowAdvanceOutcome::Completed { final_status, .. }) = outcome else {
        panic!("expected the pass to end on the recovery Round, got {outcome:?}");
    };
    assert_eq!(final_status, GoalStatus::Todo);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        git_output(&fixture.worktree, &["rev-parse", "HEAD"]).trim(),
        fixture.candidate,
        "the exact recorded candidate is restored"
    );
    let detail = fixture.detail();
    assert_eq!(detail["status"], "todo");
    assert_eq!(detail["base_commit"], fixture.base);
    assert_eq!(detail["candidate_commit"], fixture.candidate);
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["reason"],
        "candidate refresh conflicted"
    );
    assert_eq!(
        detail["rounds"][1]["automatic_retry"]["kind"],
        "integration"
    );
    assert_eq!(
        detail["rounds"][0]["workflow_recovery"]["retained_evidence"]["conflict_resolution"]["outcome"],
        "unavailable"
    );
}

/// The target moves again while the resolver is running unlocked. The refresh
/// aborts and surfaces `TargetAdvanced`; the boundary's bounded loop re-derives
/// against the moved tip rather than publishing a replacement built on a stale
/// one.
#[test]
fn a_target_tip_moved_mid_refresh_retries_boundedly_before_quality() {
    let fixture = BoundaryFixture::new("quality-refresh-race", true);
    let first_target = fixture.advance_target("app.txt", "target\n");
    let authority = fixture.claim();
    let mut context = fixture.context(authority);
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let advance_root = fixture.target_root.clone();
    let resolver = ScriptedResolver::new(vec![
        completing_edit(move |request| {
            counted.fetch_add(1, Ordering::SeqCst);
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
        }),
        {
            let counted = Arc::clone(&calls);
            completing_edit(move |request| {
                counted.fetch_add(1, Ordering::SeqCst);
                fs::write(
                    request.workspace_dir.join("app.txt"),
                    "candidate and target v2\n",
                )
                .unwrap();
            })
        },
    ]);

    let outcome =
        refresh_candidate_at_quality_boundary_with_resolver(&mut context, Some(&resolver)).unwrap();

    assert!(
        outcome.is_none(),
        "the retried refresh continues to Quality"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2, "one retry, not a loop");
    let moved_target = git_output(&fixture.target_root, &["rev-parse", "main"])
        .trim()
        .to_string();
    assert_ne!(moved_target, first_target);
    let detail = fixture.detail();
    assert_eq!(detail["status"], "quality");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(detail["base_commit"], moved_target);
    let replacement = detail["candidate_commit"].as_str().unwrap().to_string();
    assert_eq!(
        git_output(
            &fixture.worktree,
            &["show", &format!("{replacement}:app.txt")]
        ),
        "candidate and target v2\n"
    );
    assert_eq!(
        git_output(
            &fixture.worktree,
            &["merge-base", &moved_target, &replacement]
        )
        .trim(),
        moved_target
    );
}

/// Governance remains the last line. When the target moves again between the
/// two boundaries, the Governance-time refresh does its ordinary work on the
/// already-refreshed candidate — and the repin it publishes carries the
/// earlier one rather than overwriting it out of the Round.
#[test]
fn a_later_target_move_still_refreshes_at_governance_and_retains_the_earlier_repin() {
    let fixture = BoundaryFixture::new("quality-refresh-then-governance", false);
    let first_target = fixture.advance_target("sibling.txt", "sibling\n");
    let quality_authority = fixture.claim();
    let mut context = fixture.context(quality_authority);
    assert!(
        refresh_candidate_at_quality_boundary_with_resolver(&mut context, None)
            .unwrap()
            .is_none()
    );
    let after_quality = fixture.detail();
    assert_eq!(after_quality["base_commit"], first_target);
    let quality_replacement = after_quality["candidate_commit"]
        .as_str()
        .unwrap()
        .to_string();

    // A second Goal integrates between the two boundaries.
    let second_target = fixture.advance_target("second.txt", "second\n");
    fixture
        .work_items
        .advance_automated_goal_status(GOAL, GoalStatus::Governance)
        .unwrap();
    let governance_authority = fixture.claim_at(GoalStatus::Governance);
    let mut context = fixture.context_at(governance_authority, &quality_replacement);

    let outcome =
        refresh_candidate_for_target_advancement(&mut context, GoalStatus::Governance, 5).unwrap();

    let CandidateRefreshOutcome::Refreshed {
        original_candidate,
        replacement_candidate,
        target_commit,
        ..
    } = outcome
    else {
        panic!("Governance must still refresh onto a target that moved after Quality");
    };
    assert_eq!(original_candidate, quality_replacement);
    assert_eq!(target_commit, second_target);
    let detail = fixture.detail();
    assert_eq!(detail["base_commit"], second_target);
    assert_eq!(detail["candidate_commit"], replacement_candidate);
    let refresh = &detail["rounds"][0]["workflow_candidate_refresh"];
    assert_eq!(refresh["authorizing_status"], "governance");
    // The history is flat and capped: what each superseded refresh moved,
    // never a nested copy of the refresh itself, because a Round's refresh
    // count is unbounded and the Goal record is synchronized state.
    let superseded = refresh["superseded_refresh"].as_array().unwrap();
    assert_eq!(superseded.len(), 1, "{refresh:#}");
    assert_eq!(superseded[0]["authorizing_status"], "quality");
    assert_eq!(
        superseded[0]["original_candidate_commit"],
        fixture.candidate
    );
    assert_eq!(superseded[0]["replacement_base_commit"], first_target);
    assert!(
        superseded[0]["previous_gate_evidence"].is_null(),
        "the gate evidence a superseded refresh cleared belongs to that refresh: {refresh:#}"
    );
}

/// The refresh slot is bounded: a Round that repins many times keeps the most
/// recent superseded refreshes and drops the rest, so a Goal record that is
/// republished and three-way merged whole on every sync cannot grow without
/// limit.
#[test]
fn repeated_refreshes_keep_a_bounded_flat_history() {
    let fixture = BoundaryFixture::new("quality-refresh-bounded-history", false);
    let mut expected_bases = Vec::new();
    for index in 0..6 {
        let target = fixture.advance_target(&format!("sibling-{index}.txt"), "sibling\n");
        expected_bases.push(target);
        let authority = fixture.claim();
        let candidate = fixture.detail()["candidate_commit"]
            .as_str()
            .unwrap()
            .to_string();
        let mut context = fixture.context_at(authority, &candidate);
        assert!(
            refresh_candidate_at_quality_boundary_with_resolver(&mut context, None)
                .unwrap()
                .is_none()
        );
    }

    let detail = fixture.detail();
    let refresh = &detail["rounds"][0]["workflow_candidate_refresh"];
    assert_eq!(refresh["replacement_base_commit"], expected_bases[5]);
    let superseded = refresh["superseded_refresh"].as_array().unwrap();
    assert_eq!(superseded.len(), 3, "{refresh:#}");
    // The oldest are dropped, the most recent kept, and none of them nests
    // another history inside itself.
    for (offset, entry) in superseded.iter().enumerate() {
        assert_eq!(entry["replacement_base_commit"], expected_bases[2 + offset]);
        assert!(entry["superseded_refresh"].is_null(), "{entry:#}");
        assert!(entry["previous_gate_evidence"].is_null(), "{entry:#}");
    }
}

/// The wiring, end to end through the real engine: a Goal resumed in Quality
/// with the target moved under it refreshes first, so the Quality proof that
/// carries it into Governance was taken on the advanced base — and the
/// Governance-time refresh has nothing left to do.
#[test]
fn quality_runs_against_the_refreshed_base_end_to_end() {
    let fixture = BoundaryFixture::new("quality-refresh-end-to-end", false);
    let target = fixture.advance_target("sibling.txt", "sibling\n");
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _smoke_ai_path_guard = SmokeAiGuard::set(&fixture.smoke_ai);

    let workflow = WorkflowEngine::with_target_root(&fixture.runtime_root, &fixture.target_root);
    let results = workflow.execute_work().unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].final_status, "review");
    let detail = fixture.detail();
    let candidate = detail["candidate_commit"].as_str().unwrap().to_string();
    let refresh = &detail["rounds"][0]["workflow_candidate_refresh"];
    assert_eq!(
        refresh["authorizing_status"], "quality",
        "the Implement → Quality boundary did the work, not Governance: {detail:#}"
    );
    assert_eq!(refresh["replacement_base_commit"], target);
    // The refresh cleared the Round's Quality evidence, so a recorded proof can
    // only have been taken after it — against the advanced base.
    assert_eq!(
        detail["rounds"][0]["quality_candidate_commit"], candidate,
        "{detail:#}"
    );
    assert_eq!(detail["rounds"][0]["quality_state"], "passed");
    assert_eq!(
        detail["rounds"][0]["workflow_integration"]["candidate_commit"],
        json!(candidate)
    );
    // Governance's refresh found nothing to do, so it left no second repin.
    assert!(refresh["superseded_refresh"].is_null(), "{refresh:#}");
    let files = git_output(
        &fixture.target_root,
        &["ls-tree", "-r", "--name-only", &candidate],
    );
    assert!(files.contains("sibling.txt"), "{files}");
    assert!(files.contains("candidate.txt"), "{files}");
    git(
        &fixture.target_root,
        &["merge-base", "--is-ancestor", &candidate, "main"],
    )
    .unwrap();
}
