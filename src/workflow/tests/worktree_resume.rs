use super::*;

use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::quality::{FileQualityService, QualitySettingsPatch};
use serde_json::Value;

/// A Goal durably in Quality with its Round branch, candidate commit, and managed worktree
/// intact — exactly the state an interrupted attempt leaves behind.
struct QualityResumeFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    smoke_ai: PathBuf,
    work_items: FileWorkItemService,
    worktree: PathBuf,
    candidate: String,
}

impl QualityResumeFixture {
    fn new(prefix: &str) -> Self {
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
            &[
                "worktree",
                "add",
                "-b",
                "refine/GOAL1/round-1",
                worktree.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::write(worktree.join("candidate.txt"), "candidate\n").unwrap();
        git(&worktree, &["add", "candidate.txt"]).unwrap();
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
            .create_goal_summary("Resume Quality", Some("GOAL1"))
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
        Self {
            temp_root,
            target_root,
            runtime_root,
            smoke_ai,
            work_items,
            worktree,
            candidate,
        }
    }
}

impl Drop for QualityResumeFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

#[test]
fn resume_rebuilds_a_pruned_candidate_worktree_and_finishes_the_round() {
    let fixture = QualityResumeFixture::new("workflow-worktree-resume");
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _smoke_ai_path_guard = SmokeAiGuard::set(&fixture.smoke_ai);

    // External cleanup removed the scratch directory but left the registration:
    // the branch and candidate commit — the durable contract — are intact.
    fs::remove_dir_all(&fixture.worktree).unwrap();

    let workflow = WorkflowEngine::with_target_root(&fixture.runtime_root, &fixture.target_root);
    let results = workflow.execute_work().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].final_status, "review");
    assert_eq!(
        fixture
            .work_items
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .status,
        GoalStatus::Review
    );
    assert!(fixture.worktree.exists());
    assert_eq!(
        git_output(
            &fixture.target_root,
            &["rev-parse", "refs/heads/refine/GOAL1/round-1"]
        )
        .trim(),
        fixture.candidate
    );
    let operations = FileOperationRegistry::new(&fixture.runtime_root)
        .recover()
        .unwrap();
    let handoff = operations
        .iter()
        .find(|operation| {
            operation.request.get("kind").and_then(Value::as_str)
                == Some("workflow_candidate_handoff")
        })
        .expect("candidate handoff was re-registered on resume");
    assert_eq!(handoff.state, OperationState::Succeeded);
    assert_eq!(handoff.result["disposition"], "candidate_integrated");
    let registered = PathBuf::from(handoff.request["worktree_path"].as_str().unwrap());
    assert_eq!(
        fs::canonicalize(&registered).unwrap(),
        fs::canonicalize(&fixture.worktree).unwrap()
    );
}

#[test]
fn resume_reuses_a_durable_quality_proof_and_skips_straight_to_governance() {
    let fixture = QualityResumeFixture::new("workflow-quality-proof-resume");
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _smoke_ai_path_guard = SmokeAiGuard::set(&fixture.smoke_ai);

    // The interrupted attempt already recorded a durable passed Quality proof for the
    // exact candidate before it died; only the transition to Governance was lost.
    fixture
        .work_items
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({
                "quality_state": "passed",
                "quality_candidate_commit": fixture.candidate,
                "quality_checked_at": "2026-08-15T00:01:00Z",
                "quality_details": {
                    "operation_id": "OP-QUALITY-SEEDED",
                    "candidate_commit": fixture.candidate,
                    "source_candidate_commit": fixture.candidate,
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
    fs::remove_dir_all(&fixture.worktree).unwrap();

    let workflow = WorkflowEngine::with_target_root(&fixture.runtime_root, &fixture.target_root);
    let results = workflow.execute_work().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].final_status, "review");
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    // The seeded proof was reused: the Quality gate never regenerated it.
    assert_eq!(
        detail["rounds"][0]["quality_details"]["operation_id"],
        "OP-QUALITY-SEEDED"
    );
    assert_eq!(
        detail["rounds"][0]["quality_candidate_commit"],
        fixture.candidate
    );
    assert!(fixture.worktree.exists());
}

#[test]
fn resume_fails_closed_when_the_round_branch_no_longer_contains_the_candidate() {
    let fixture = QualityResumeFixture::new("workflow-worktree-diverged");
    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _smoke_ai_path_guard = SmokeAiGuard::set(&fixture.smoke_ai);

    fs::remove_dir_all(&fixture.worktree).unwrap();
    git(&fixture.target_root, &["worktree", "prune"]).unwrap();
    fs::write(fixture.target_root.join("unrelated.txt"), "unrelated\n").unwrap();
    git(&fixture.target_root, &["add", "unrelated.txt"]).unwrap();
    git(&fixture.target_root, &["commit", "-m", "unrelated"]).unwrap();
    let unrelated = git_output(&fixture.target_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    git(
        &fixture.target_root,
        &["branch", "-f", "refine/GOAL1/round-1", &unrelated],
    )
    .unwrap();

    let workflow = WorkflowEngine::with_target_root(&fixture.runtime_root, &fixture.target_root);
    let error = workflow.execute_work().unwrap_err();
    assert!(
        error.to_string().contains("existing work was preserved"),
        "{error}"
    );
    assert_eq!(
        fixture
            .work_items
            .show_goal_summary("GOAL1")
            .unwrap()
            .goal
            .status,
        GoalStatus::Failed
    );
    assert_eq!(
        git_output(
            &fixture.target_root,
            &["rev-parse", "refs/heads/refine/GOAL1/round-1"]
        )
        .trim(),
        unrelated
    );
}
