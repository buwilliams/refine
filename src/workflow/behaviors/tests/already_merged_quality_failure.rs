use super::*;
use crate::process::subprocess::workflow_subprocess_metadata;
use crate::process::supervisor::operations::OperationHandle;
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::host::quality::{FileQualityService, QualitySettingsPatch};
use crate::tools::product::work_items::{FileWorkItemService, WorkflowAttemptAuthority};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn newly_failed_quality_is_settled_before_the_resolver_can_rerun_it() {
    let fixture = AlreadyMergedQualityFixture::new("direct");
    let _provider_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _provider_override = SmokeAiOverride::new(&fixture.smoke_ai);
    let mut context = fixture.context();

    let first = WorkflowQuality.advance(&mut context).unwrap();
    assert_failed_outcome(first);
    assert_eq!(fixture.invocation_count(), 1);
    let first_settlement = fixture.assert_failed_settlement();

    let repeated = WorkflowQuality.advance(&mut context).unwrap();
    assert_failed_outcome(repeated);
    assert_eq!(fixture.invocation_count(), 1);
    assert_eq!(fixture.assert_failed_settlement(), first_settlement);
    assert_eq!(fixture.operations().len(), 1);
}

#[test]
fn persisted_failed_quality_is_terminal_and_restart_recoverable() {
    let fixture = AlreadyMergedQualityFixture::new("restart");
    let _provider_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _provider_override = SmokeAiOverride::new(&fixture.smoke_ai);
    let first = fixture.run_quality_once();
    assert!(!first.result.ok);
    assert_eq!(first.operation.state, OperationState::Failed);
    assert_eq!(fixture.invocation_count(), 1);

    // This is the restart window: the first result and terminal operation are durable, while the
    // originating workflow attempt still owns a Goal in Quality and has not settled it as Failed.
    let before_recovery = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(before_recovery["status"], "quality");
    assert_eq!(before_recovery["rounds"][0]["quality_state"], "failed");
    let first_details = before_recovery["rounds"][0]["quality_details"].clone();
    assert_first_failure_details(&first_details, &fixture.candidate, &first.operation.id);

    let mut context = fixture.context();
    let recovered = WorkflowQuality.advance(&mut context).unwrap();
    assert_failed_outcome(recovered);
    assert_eq!(fixture.invocation_count(), 1);
    let reconciliation = fixture.assert_failed_settlement();
    assert_eq!(reconciliation["quality_operation_id"], first.operation.id);
    assert_eq!(
        reconciliation["quality_proof"],
        first_details["quality_proof"]
    );
    assert_eq!(reconciliation["quality_failure"], first_details);

    let repeated = WorkflowQuality.advance(&mut context).unwrap();
    assert_failed_outcome(repeated);
    assert_eq!(fixture.invocation_count(), 1);
    assert_eq!(fixture.assert_failed_settlement(), reconciliation);
    assert_eq!(fixture.operations().len(), 1);
}

#[test]
fn superseded_authority_cannot_settle_a_persisted_failed_quality_result() {
    let fixture = AlreadyMergedQualityFixture::new("superseded");
    let _provider_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _provider_override = SmokeAiOverride::new(&fixture.smoke_ai);
    let first = fixture.run_quality_once();
    assert!(!first.result.ok);
    assert_eq!(fixture.invocation_count(), 1);
    fixture.work_items.cancel_goal_summary("GOAL1").unwrap();
    let mut context = fixture.context();

    let error = WorkflowQuality.advance(&mut context).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed from quality to cancelled")
    );
    assert_eq!(fixture.invocation_count(), 1);
    assert_eq!(fixture.operations().len(), 1);
    let detail = fixture.work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["status"], "cancelled");
    assert_eq!(
        detail["rounds"][0]["workflow_reconciliation"]["state"],
        "detected"
    );
    assert!(
        detail["rounds"][0]["workflow_reconciliation"]
            .get("category")
            .is_none()
    );
}

struct AlreadyMergedQualityFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    smoke_ai: PathBuf,
    invocation_count: PathBuf,
    work_items: FileWorkItemService,
    branch: String,
    candidate: String,
    integration: Value,
    round_idx: usize,
    authority: WorkflowAttemptAuthority,
}

impl AlreadyMergedQualityFixture {
    fn new(scenario: &str) -> Self {
        let temp_root =
            behavior_test_temp_dir(&format!("already-merged-quality-fail-closed-{scenario}"));
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
        let remote = temp_root.join("remote.git");
        behavior_test_git(
            &temp_root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        );
        behavior_test_git(
            &target_root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        behavior_test_git(&target_root, &["push", "-u", "origin", "main"]);
        let branch = "refine/GOAL1/round-1".to_string();
        behavior_test_git(&target_root, &["checkout", "-b", &branch]);
        fs::write(target_root.join("candidate.txt"), "candidate\n").unwrap();
        behavior_test_git(&target_root, &["add", "candidate.txt"]);
        behavior_test_git(&target_root, &["commit", "-m", "candidate"]);
        let candidate = behavior_test_git(&target_root, &["rev-parse", "HEAD"]);
        behavior_test_git(&target_root, &["checkout", "main"]);
        behavior_test_git(&target_root, &["merge", "--no-ff", "--no-edit", &candidate]);
        let integrated = behavior_test_git(&target_root, &["rev-parse", "HEAD"]);
        behavior_test_git(&target_root, &["push", "origin", "main"]);

        fs::write(
            &smoke_ai,
            r#"#!/bin/sh
count=0
if test -f "$0.count"; then count=$(sed -n '1p' "$0.count"); fi
count=$((count + 1))
printf '%s\n' "$count" > "$0.count"
if test "$count" -eq 1; then
  printf '%s\n' '{"summary":"first exact-candidate evaluation failed","results":[{"test":"Outcome works","status":"failed","evidence":"first failure","command":"false"}]}'
else
  printf '%s\n' '{"summary":"later evaluation passed","results":[{"test":"Outcome works","status":"passed","evidence":"later pass","command":"true"}]}'
fi
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();

        let refine_dir =
            crate::tools::host::project_layout::refine_dir_for_target_root(&target_root).unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Already merged Quality failure", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Reporter", "Retain the first failed evaluation")
            .unwrap();
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs("GOAL1", &branch, "main", &base, Some(&candidate))
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Implement)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Quality)
            .unwrap();
        let integration = json!({
            "candidate_commit": candidate,
            "target_branch": "main",
            "target_commit": integrated,
            "remote": "origin",
            "pushed": true,
            "integrated_at": "2026-08-15T00:03:00Z",
            "merge": {"ok": true, "conflicts": [], "message": "integrated"}
        });
        work_items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({
                    "rule_state": "passed",
                    "meta_rule_state": "passed",
                    "product_state": "passed",
                    "constitution_state": "passed",
                    "governance_candidate_commit": candidate,
                    "governance_checked_at": "2026-08-15T00:02:00Z",
                    "workflow_integration": integration,
                    "workflow_reconciliation": {
                        "state": "detected",
                        "candidate_commit": candidate,
                        "target_branch": "main",
                        "detected_target_commit": integrated
                    }
                }),
            )
            .unwrap();
        FileQualityService::new(&refine_dir)
            .save_settings(QualitySettingsPatch {
                tests: Some(vec!["Outcome works".to_string()]),
                ..QualitySettingsPatch::default()
            })
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
            candidate,
            integration,
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
        context.provider_output = Some("Reconciled existing integrated candidate".to_string());
        context.commit = Some(self.candidate.clone());
        context.implementation_changed = true;
        context.reconciliation = Some(serde_json::from_value(self.integration.clone()).unwrap());
        context.reconciliation_state = Some("detected".to_string());
        context.start_status = GoalStatus::Quality;
        context
    }

    fn run_quality_once(&self) -> crate::tools::host::quality::QualityOperationResult {
        let mut metadata = workflow_subprocess_metadata(
            "GOAL1",
            "quality",
            "AlreadyMergedQualityRegeneration",
            Some(self.round_idx),
        );
        metadata.insert(
            "workflow_revision".to_string(),
            json!(self.authority.workflow_revision),
        );
        QualityOperationRunner::new(
            crate::tools::host::project_layout::refine_dir_for_target_root(&self.target_root)
                .unwrap(),
            &self.runtime_root,
            &self.target_root,
        )
        .run_goal_checks("GOAL1", "smoke-ai", metadata)
        .unwrap()
    }

    fn invocation_count(&self) -> usize {
        fs::read_to_string(&self.invocation_count)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn operations(&self) -> Vec<OperationHandle> {
        FileOperationRegistry::new(&self.runtime_root)
            .recover()
            .unwrap()
    }

    fn assert_failed_settlement(&self) -> Value {
        let settled = self.work_items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(settled["status"], "failed");
        let round = &settled["rounds"][0];
        assert_eq!(round["quality_state"], "failed");
        assert_eq!(round["failure_category"], "already_merged_quality_failed");
        assert!(round.get("reviewed_at").is_none());
        assert_first_failure_details(
            &round["quality_details"],
            &self.candidate,
            round["quality_details"]["operation_id"].as_str().unwrap(),
        );
        let reconciliation = round["workflow_reconciliation"].clone();
        assert_eq!(reconciliation["state"], "failed");
        assert_eq!(reconciliation["category"], "already_merged_quality_failed");
        assert_eq!(reconciliation["candidate_commit"], self.candidate);
        assert!(reconciliation.get("resolved_at").is_none());
        assert!(reconciliation.get("approval").is_none());
        assert_eq!(reconciliation["quality_failure"], round["quality_details"]);
        reconciliation
    }
}

impl Drop for AlreadyMergedQualityFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

struct SmokeAiOverride(Option<OsString>);

impl SmokeAiOverride {
    fn new(path: &Path) -> Self {
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe { std::env::set_var("REFINE_SMOKE_AI_PATH", path) };
        Self(previous)
    }
}

impl Drop for SmokeAiOverride {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.0.take() {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }
    }
}

fn assert_failed_outcome(outcome: WorkflowAdvanceOutcome) {
    assert!(matches!(
        outcome,
        WorkflowAdvanceOutcome::Completed {
            final_status: GoalStatus::Failed,
            ..
        }
    ));
}

fn assert_first_failure_details(details: &Value, candidate: &str, operation_id: &str) {
    assert_eq!(details["operation_id"], operation_id);
    assert_eq!(details["candidate_commit"], candidate);
    assert_eq!(details["source_candidate_commit"], candidate);
    assert_eq!(details["evaluation_scope"], "isolated_candidate");
    assert_eq!(details["quality_proof_mode"], "regenerated");
    assert_eq!(details["quality_proof"]["state"], "failed");
    assert_eq!(
        details["quality_proof"]["checked_candidate_commit"],
        candidate
    );
    assert_eq!(
        details["quality_proof"]["source_candidate_commit"],
        candidate
    );
    assert_eq!(details["provider_attempts"].as_array().unwrap().len(), 1);
    assert!(
        details["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| !diagnostics.is_empty())
    );
}

fn behavior_test_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn behavior_test_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
