use super::already_merged_quality_failure::{
    SmokeAiOverride, behavior_test_git, behavior_test_temp_dir,
};
use super::*;
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::host::quality::{FileQualityService, QualitySettingsPatch};
use crate::tools::product::work_items::{FileWorkItemService, WorkflowAttemptAuthority};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const UNRESOLVABLE_CANDIDATE: &str = "0123456789abcdef0123456789abcdef01234567";
const SMOKE_AI_CORRECTION_FIXTURE_REPORT: &str =
    "Smoke AI Quality fixture reviewed the candidate and retained existing tests.";

#[test]
fn resume_with_a_durable_quality_proof_transitions_without_any_provider_invocation() {
    let fixture = QualityResumeFixture::new("proof-skip", None);
    let _provider_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _provider_override = SmokeAiOverride::new(&fixture.smoke_ai);
    fixture.seed_quality_proof(&fixture.candidate);
    let mut context = fixture.context();

    let outcome = WorkflowQuality.advance(&mut context).unwrap();
    let WorkflowAdvanceOutcome::Transition { from, to, reason } = outcome else {
        panic!("expected a Quality transition")
    };
    assert_eq!(from, GoalStatus::Quality);
    assert_eq!(to, GoalStatus::Governance);
    assert_eq!(
        reason,
        "Reused durable Quality proof from an interrupted attempt"
    );
    assert_eq!(fixture.invocation_count(), 0);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "governance");
    // The durable proof itself is untouched.
    assert_eq!(
        detail["rounds"][0]["quality_details"]["operation_id"],
        "OP-QUALITY-SEEDED"
    );
}

#[test]
fn resume_with_a_proof_for_another_candidate_reruns_the_whole_quality_phase() {
    let fixture = QualityResumeFixture::new("proof-mismatch", None);
    let _provider_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _provider_override = SmokeAiOverride::new(&fixture.smoke_ai);
    fixture.seed_quality_proof(&fixture.base);
    let mut context = fixture.context();

    let outcome = WorkflowQuality.advance(&mut context).unwrap();
    let WorkflowAdvanceOutcome::Transition { to, reason, .. } = outcome else {
        panic!("expected a Quality transition")
    };
    assert_eq!(to, GoalStatus::Governance);
    assert_eq!(reason, "Quality checks passed");
    // The gate ran against the real candidate; the correction agent re-ran too.
    assert_eq!(fixture.invocation_count(), 1);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["quality_agent_report"],
        SMOKE_AI_CORRECTION_FIXTURE_REPORT
    );
    assert_eq!(
        detail["rounds"][0]["quality_candidate_commit"],
        fixture.candidate
    );
}

#[test]
fn resume_with_an_unresolvable_proof_candidate_reruns_the_whole_quality_phase() {
    let fixture = QualityResumeFixture::new("proof-unresolvable", Some(UNRESOLVABLE_CANDIDATE));
    let _provider_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _provider_override = SmokeAiOverride::new(&fixture.smoke_ai);
    fixture.seed_quality_proof(UNRESOLVABLE_CANDIDATE);
    let mut context = fixture.context();
    context.commit = Some(UNRESOLVABLE_CANDIDATE.to_string());

    let outcome = WorkflowQuality.advance(&mut context).unwrap();
    let WorkflowAdvanceOutcome::Transition { to, reason, .. } = outcome else {
        panic!("expected a Quality transition")
    };
    assert_eq!(to, GoalStatus::Governance);
    assert_eq!(reason, "Quality checks passed");
    assert_eq!(fixture.invocation_count(), 1);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    // The full re-run re-derived the candidate from the retained worktree.
    assert_eq!(
        detail["rounds"][0]["quality_candidate_commit"],
        fixture.candidate
    );
    assert_eq!(
        detail["rounds"][0]["quality_agent_report"],
        SMOKE_AI_CORRECTION_FIXTURE_REPORT
    );
}

#[test]
fn resume_with_persisted_correction_evidence_skips_the_agent_and_reruns_only_the_gate() {
    let fixture = QualityResumeFixture::new("agent-skip", None);
    let _provider_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _provider_override = SmokeAiOverride::new(&fixture.smoke_ai);
    fixture
        .work_items
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({
                "quality_agent_report": "Persisted correction report.",
                "quality_candidate_commit": fixture.candidate
            }),
        )
        .unwrap();
    let mut context = fixture.context();

    let outcome = WorkflowQuality.advance(&mut context).unwrap();
    let WorkflowAdvanceOutcome::Transition { to, reason, .. } = outcome else {
        panic!("expected a Quality transition")
    };
    assert_eq!(to, GoalStatus::Governance);
    assert_eq!(reason, "Quality checks passed");
    // Exactly one provider invocation: the gate. A re-run correction agent would
    // have replaced the persisted report with the smoke-ai fixture text.
    assert_eq!(fixture.invocation_count(), 1);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        detail["rounds"][0]["quality_agent_report"],
        "Persisted correction report."
    );
    assert_eq!(detail["rounds"][0]["quality_state"], "passed");
    assert_eq!(
        detail["rounds"][0]["quality_candidate_commit"],
        fixture.candidate
    );
}

struct QualityResumeFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    smoke_ai: PathBuf,
    invocation_count: PathBuf,
    work_items: FileWorkItemService,
    branch: String,
    worktree: PathBuf,
    base: String,
    candidate: String,
    round_idx: usize,
    authority: WorkflowAttemptAuthority,
}

impl QualityResumeFixture {
    /// A Goal durably in Quality with a healthy candidate worktree, resumed by a fresh
    /// attempt. `recorded_candidate` overrides the Goal's candidate_commit for scenarios
    /// where the recorded value no longer resolves.
    fn new(scenario: &str, recorded_candidate: Option<&str>) -> Self {
        let temp_root = behavior_test_temp_dir(&format!("quality-resume-{scenario}"));
        let target_root = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        let smoke_ai = temp_root.join("smoke-ai");
        let invocation_count = temp_root.join("smoke-ai.count");
        fs::create_dir_all(&target_root).unwrap();
        behavior_test_git(&target_root, &["init", "-b", "main"]);
        behavior_test_git(
            &target_root,
            &["config", "user.email", "refine-test@example.invalid"],
        );
        behavior_test_git(&target_root, &["config", "user.name", "Refine Test"]);
        fs::write(target_root.join("app.txt"), "base\n").unwrap();
        behavior_test_git(&target_root, &["add", "app.txt"]);
        behavior_test_git(&target_root, &["commit", "-m", "base"]);
        let base = behavior_test_git(&target_root, &["rev-parse", "HEAD"]);
        let branch = "refine/GOAL1/round-1".to_string();
        let worktree = temp_root.join("candidate");
        behavior_test_git(
            &target_root,
            &["worktree", "add", "-b", &branch, worktree.to_str().unwrap()],
        );
        fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
        behavior_test_git(&worktree, &["add", "candidate.txt"]);
        behavior_test_git(&worktree, &["commit", "-m", "candidate"]);
        let candidate = behavior_test_git(&worktree, &["rev-parse", "HEAD"]);

        fs::write(
            &smoke_ai,
            r#"#!/bin/sh
count=0
if test -f "$0.count"; then count=$(sed -n '1p' "$0.count"); fi
count=$((count + 1))
printf '%s\n' "$count" > "$0.count"
printf '%s\n' '{"summary":"The candidate passes.","results":[{"test":"Candidate works","status":"passed","evidence":"Verified.","command":"true"}]}'
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();

        let refine_dir =
            crate::tools::host::project_layout::refine_dir_for_target_root(&target_root).unwrap();
        FileQualityService::new(&refine_dir)
            .save_settings(QualitySettingsPatch {
                tests: Some(vec!["Candidate works".to_string()]),
                ..QualitySettingsPatch::default()
            })
            .unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Resume Quality idempotently", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Implement the candidate")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs(
                "GOAL1",
                &branch,
                "main",
                &base,
                Some(recorded_candidate.unwrap_or(candidate.as_str())),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
            .unwrap();
        let (round_idx, revision, request) = work_items.authored_goal_commitment("GOAL1").unwrap();
        let authority = work_items
            .claim_workflow_attempt("GOAL1", GoalStatus::Quality, round_idx, revision, &request)
            .unwrap();
        Self {
            temp_root,
            target_root,
            runtime_root,
            smoke_ai,
            invocation_count,
            work_items,
            branch,
            worktree,
            base,
            candidate,
            round_idx,
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
            self.round_idx,
            self.authority,
            Default::default(),
            self.work_items.clone(),
        );
        context.branch = Some(self.branch.clone());
        context.worktree_path = Some(self.worktree.display().to_string());
        context.agent_cwd = Some(self.worktree.clone());
        context.provider_output = Some("Resumed existing workflow candidate".to_string());
        context.commit = Some(self.candidate.clone());
        context.implementation_changed = true;
        context.start_status = GoalStatus::Quality;
        context
    }

    /// Durable Quality evidence in the exact shape `normalize_quality_proof` accepts.
    fn seed_quality_proof(&self, candidate: &str) {
        self.work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({
                    "quality_state": "passed",
                    "quality_candidate_commit": candidate,
                    "quality_checked_at": "2026-08-15T00:01:00Z",
                    "quality_details": {
                        "operation_id": "OP-QUALITY-SEEDED",
                        "candidate_commit": candidate,
                        "source_candidate_commit": candidate,
                        "evaluation_scope": "isolated_candidate",
                        "results": [{
                            "test": "Candidate works",
                            "status": "passed",
                            "evidence": "Verified.",
                            "command": "true"
                        }]
                    }
                }),
            )
            .unwrap();
    }

    fn invocation_count(&self) -> usize {
        fs::read_to_string(&self.invocation_count)
            .map(|count| count.trim().parse().unwrap())
            .unwrap_or(0)
    }
}

impl Drop for QualityResumeFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}
