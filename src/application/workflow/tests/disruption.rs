//! Interrupted execution preserves the current step and reusable work. The
//! daemon restores continuation through ordinary admission; interruption alone
//! is not a failed Skill verdict. Each fixture verifies that repeated recovery
//! retains its authored request, candidate, and partially completed evidence.

#![cfg(unix)]

use super::*;

use crate::application::workflow::engine::context::WorkflowContext;
use crate::application::workflow::phases::implementation_planning::run_planning_skills;
use crate::application::workflow::phases::quality::{FileQualityService, QualitySettingsPatch};
use crate::infrastructure::git::worktrees::FileGitWorktreeService;
use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationChecklistItem, ImplementationPlan,
    ImplementationPlanArtifact, ImplementationPlanBinding, ImplementationPlanPhase,
    ImplementationPlanState, ProposedImplementationPlan,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const GOAL: &str = "GOAL1";
const PROMPT: &str = "Implement the disruption fixture change";
const BRANCH: &str = "refine/GOAL1/round-1";
const SEEDED_QUALITY_OPERATION: &str = "OP-QUALITY-SEEDED";
const SEEDED_QUALITY_AGENT_REPORT: &str =
    "Seeded Quality agent report persisted by the interrupted attempt.";

/// A Goal in a real Git target repository, with a smoke-ai provider whose
/// Quality and Governance gates pass and whose implementation invocation makes
/// a deterministic edit — enough to carry any resumed step through to Review.
struct DisruptionFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    runtime_root: PathBuf,
    smoke_ai: PathBuf,
    work_items: FileWorkItemService,
    worktree: PathBuf,
    base: String,
}

impl DisruptionFixture {
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
        fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"Post-implementation Quality evaluation\"*)\n\
               printf '%s\\n' '{\"ok\":true,\"summary\":\"The candidate passes.\",\"results\":[{\"test\":\"Candidate works\",\"status\":\"passed\",\"evidence\":\"Verified.\",\"command\":\"true\"}]}'\n\
               ;;\n\
             *\"Post-implementation governance review\"*)\n\
               printf '%s\\n' '{\"status\":\"passed\",\"message\":\"Compliant.\",\"violations\":[]}'\n\
               ;;\n\
             *)\n\
               printf '\\n# disruption fixture implementation edit\\n' >> app.txt\n\
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
            .create_goal_summary("Disruption doctrine", Some(GOAL))
            .unwrap();
        work_items
            .append_goal_round_summary(GOAL, "Reporter", PROMPT)
            .unwrap();
        work_items
            .transition_goal_status(GOAL, GoalStatus::Todo)
            .unwrap();
        let worktree = target_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
        Self {
            temp_root,
            target_root,
            runtime_root,
            smoke_ai,
            work_items,
            worktree,
            base,
        }
    }

    fn advance(&self, status: GoalStatus) {
        self.work_items
            .advance_automated_goal_status(GOAL, status)
            .unwrap();
    }

    /// The durable state the Todo behavior leaves behind: status Plan, the
    /// Round branch and worktree materialized at the base, and Git refs
    /// recorded on the Goal.
    fn enter_plan_with_worktree(&self) {
        self.advance(GoalStatus::Plan);
        fs::create_dir_all(self.worktree.parent().unwrap()).unwrap();
        git(
            &self.target_root,
            &[
                "worktree",
                "add",
                "-b",
                BRANCH,
                self.worktree.to_str().unwrap(),
            ],
        )
        .unwrap();
        self.work_items
            .update_goal_git_refs(GOAL, BRANCH, "main", &self.base, None)
            .unwrap();
    }

    /// Pins the Round agent context the same way an interrupted attempt did,
    /// so plan bindings computed before and after the restart agree.
    fn seed_agent_context(&self) -> Value {
        let agent_context = json!({
            "version": 1,
            "governance": {
                "product": "A small app.",
                "constitution": "Keep the app healthy.",
                "rules": [{"id": "rule-1", "text": "Keep the app healthy.", "source": "manual"}],
                "configured": true
            },
            "guidance_candidates": [],
            "goal": {"id": GOAL, "name": "Disruption doctrine", "node_id": "default"},
            "previous_rounds": [],
            "current_round": {"round": 1, "prompt": PROMPT}
        });
        self.work_items
            .update_goal_round_evaluation_summary(GOAL, 0, &json!({"agent_context": agent_context}))
            .unwrap();
        self.detail()["rounds"][0]["agent_context"].clone()
    }

    /// Claims the workflow attempt exactly as the dead process did; the claim
    /// stays durably on the Round after the "force-stop".
    fn claim(&self, status: GoalStatus) -> WorkflowContext<'_> {
        let (round_idx, revision, request) =
            self.work_items.authored_goal_commitment(GOAL).unwrap();
        let authority = self
            .work_items
            .claim_workflow_attempt(GOAL, status, round_idx, revision, &request)
            .unwrap();
        WorkflowContext::new(
            &self.runtime_root,
            &self.target_root,
            GOAL.to_string(),
            "default".to_string(),
            "smoke-ai".to_string(),
            round_idx,
            authority,
            Default::default(),
            self.work_items.clone(),
        )
    }

    /// Runs the real governed planning trio (smoke-ai fixture phases) so the
    /// Round carries a genuine persisted final plan.
    fn run_real_planning(&self) -> WorkflowContext<'_> {
        let mut ctx = self.claim(GoalStatus::Plan);
        ctx.branch = Some(BRANCH.into());
        ctx.worktree_path = Some(self.worktree.display().to_string());
        let goal = self.work_items.show_goal_detail(GOAL).unwrap();
        let agent_context = goal["rounds"][0]["agent_context"].clone();
        run_planning_skills(&ctx, &agent_context, &self.worktree).unwrap();
        ctx
    }

    fn commit_candidate(&self, line: &str) -> String {
        let app = self.worktree.join("app.txt");
        let mut content = fs::read_to_string(&app).unwrap();
        content.push_str(line);
        content.push('\n');
        fs::write(&app, content).unwrap();
        git(&self.worktree, &["add", "-A"]).unwrap();
        git(&self.worktree, &["commit", "-m", "candidate"]).unwrap();
        git_output(&self.worktree, &["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }

    /// A durable passed Quality proof for the exact candidate, as the
    /// interrupted attempt recorded it before dying.
    fn seed_quality_proof(&self, candidate: &str) {
        self.work_items
            .update_goal_round_evaluation_summary(
                GOAL,
                0,
                &json!({
                    "quality_state": "passed",
                    "quality_candidate_commit": candidate,
                    "quality_checked_at": "2026-08-15T00:01:00Z",
                    "quality_details": {
                        "operation_id": SEEDED_QUALITY_OPERATION,
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

    fn assert_restart_preserves_evidence_and_continuation(&self) {
        let before = self.work_items.show_goal_detail(GOAL).unwrap();
        let main = git_output(&self.target_root, &["rev-parse", "main"]);
        let head = git_output(&self.worktree, &["rev-parse", "HEAD"]);
        let work = fs::read(self.worktree.join("app.txt")).unwrap();
        let engine = WorkflowEngine::with_target_root(&self.runtime_root, &self.target_root);
        for reason in [
            "operator force-stopped production",
            "replacement daemon restarted",
        ] {
            engine.recover_interrupted_goals(reason).unwrap();
            assert_eq!(
                self.detail(),
                before,
                "recovery changed the current request or evidence"
            );
            assert_eq!(git_output(&self.target_root, &["rev-parse", "main"]), main);
            assert_eq!(git_output(&self.worktree, &["rev-parse", "HEAD"]), head);
            assert_eq!(fs::read(self.worktree.join("app.txt")).unwrap(), work);
            assert_eq!(
                engine
                    .launchable_goals(&std::collections::BTreeSet::new())
                    .unwrap(),
                vec![GOAL],
                "preserved work must remain available to ordinary workflow admission",
            );
        }
    }
    fn detail(&self) -> Value {
        self.work_items.show_goal_detail(GOAL).unwrap()
    }
}

impl Drop for DisruptionFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

/// Disruption point: after the workflow attempt claim, before the Todo → Plan
/// transition. The claim is durable on the Round; nothing else happened.
/// Recovery leaves this unstarted occurrence available to normal admission.
#[test]
fn force_stop_after_claim_in_todo_preserves_the_unstarted_attempt() {
    let fixture = DisruptionFixture::new("disruption-todo-claim");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    drop(fixture.claim(GoalStatus::Todo));

    let before = fixture.detail();
    let engine = WorkflowEngine::with_target_root(&fixture.runtime_root, &fixture.target_root);
    assert_eq!(
        engine
            .recover_interrupted_goals("worker replaced before work")
            .unwrap(),
        0
    );
    assert_eq!(fixture.detail(), before);
    assert!(!fixture.worktree.exists());
}

/// Disruption point: after the Todo → Plan transition and worktree
/// materialization, before any planning artifact was persisted.
/// Recovery preserves the selection; ordinary admission prepares the Plan work.
#[test]
fn force_stop_after_entering_plan_preserves_plan_entry() {
    let fixture = DisruptionFixture::new("disruption-plan-entry");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    drop(fixture.claim(GoalStatus::Plan));

    fixture.assert_restart_preserves_evidence_and_continuation();
    let engine = WorkflowEngine::with_target_root(&fixture.runtime_root, &fixture.target_root);
    let continued = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = continued.clone();
    crate::application::workflow::engine::test_hooks::install(
        &fixture.runtime_root,
        std::sync::Arc::new(move |_, id, stage, _| {
            if stage == "executing" {
                assert_eq!(id, GOAL);
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Err(RefineError::Conflict(
                    "fixture observed resumed Plan execution".into(),
                ));
            }
            Ok(())
        }),
    );
    let resumed = engine.evaluate_workflow();
    crate::application::workflow::engine::test_hooks::remove(&fixture.runtime_root);
    assert!(
        resumed
            .unwrap_err()
            .to_string()
            .contains("fixture observed resumed Plan execution")
    );
    assert_eq!(continued.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(fixture.detail()["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(fixture.detail()["rounds"][0]["prompt"], PROMPT);
}

/// Disruption point: mid-planning — the proposal artifact is persisted, the
/// criticize phase never ran. Recovery preserves the proposal while ordinary
/// continuation determines which compatible evidence can be reused.
#[test]
fn force_stop_mid_planning_preserves_the_proposal_for_continuation() {
    let fixture = DisruptionFixture::new("disruption-plan-partial");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    let agent_context = fixture.seed_agent_context();
    let context_digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&agent_context).unwrap())
    );
    let observation =
        FileGitWorktreeService::with_runtime_root(&fixture.worktree, &fixture.runtime_root)
            .implementation_planning_observation()
            .unwrap();
    let seeded_started_at = "2026-08-15T00:00:00Z".to_string();
    let plan = ImplementationPlan {
        schema_version: IMPLEMENTATION_PLAN_SCHEMA_VERSION,
        state: ImplementationPlanState::InProgress,
        phase: ImplementationPlanPhase::Plan,
        binding: ImplementationPlanBinding {
            goal_id: GOAL.to_string(),
            round_idx: 0,
            context_version: 1,
            context_digest,
            implementation_branch: BRANCH.to_string(),
            target_branch: "main".to_string(),
            base_commit: fixture.base.clone(),
        },
        started_at: seeded_started_at.clone(),
        phase_started_at: seeded_started_at.clone(),
        updated_at: seeded_started_at.clone(),
        completed_at: None,
        proposal: Some(ImplementationPlanArtifact {
            started_at: seeded_started_at.clone(),
            completed_at: "2026-08-15T00:00:10Z".to_string(),
            git_before: observation.clone(),
            git_after: observation,
            result: ProposedImplementationPlan {
                summary: "Seeded proposal from the interrupted attempt.".to_string(),
                checklist: vec![ImplementationChecklistItem {
                    id: "P1".to_string(),
                    description: "Implement and verify the current Round request.".to_string(),
                    affected_behavior: Vec::new(),
                    governance_rationale: None,
                    verification: Vec::new(),
                }],
                criticism_resolutions: Vec::new(),
            },
        }),
        criticism: None,
        final_plan: None,
        implementation: None,
        failure: None,
        invalid_output_attempts: Vec::new(),
        provider_session_id: None,
        governance_precheck: None,
    };
    fixture
        .work_items
        .seed_legacy_implementation_plan(GOAL, 0, &json!({"implementation_plan":plan}))
        .unwrap();
    drop(fixture.claim(GoalStatus::Plan));

    fixture.assert_restart_preserves_evidence_and_continuation();
}

/// Disruption point: planning finished and the Plan → Implement transition
/// landed, but the implementation agent never launched. Recovery preserves the
/// final plan and keeps implementation available to normal admission.
#[test]
fn force_stop_after_plan_before_implement_preserves_the_final_plan() {
    let fixture = DisruptionFixture::new("disruption-implement-entry");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    fixture.seed_agent_context();
    drop(fixture.run_real_planning());
    fixture.advance(GoalStatus::Implement);
    let planned = fixture.detail()["rounds"][0]["event_results"].clone();
    assert!(!planned.is_null());

    fixture.assert_restart_preserves_evidence_and_continuation();
}

/// Disruption point: mid-implement — the durable phase is `implement`, the
/// worktree is present with the dead agent's uncommitted half-done edit.
/// Recovery retains the worktree and leaves implementation eligible to continue.
#[test]
fn force_stop_mid_implement_with_worktree_preserves_partial_implementation() {
    let fixture = DisruptionFixture::new("disruption-implement-mid");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    fixture.seed_agent_context();
    let mut ctx = fixture.run_real_planning();
    ctx.request_transition(GoalStatus::Plan, GoalStatus::Implement)
        .unwrap();
    drop(ctx);
    // The dead agent's half-finished tracked edit, never committed.
    let app = fixture.worktree.join("app.txt");
    let mut content = fs::read_to_string(&app).unwrap();
    content.push_str("# half-finished agent edit\n");
    fs::write(&app, content).unwrap();

    fixture.assert_restart_preserves_evidence_and_continuation();
}

/// Disruption point: the implementation committed its candidate and recorded
/// it on the Goal, but the Implement → Quality transition was lost. Recovery
/// retains the candidate without repeating implementation.
#[test]
fn force_stop_after_implement_commit_before_quality_preserves_the_candidate() {
    let fixture = DisruptionFixture::new("disruption-implement-committed");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    fixture.seed_agent_context();
    let mut ctx = fixture.run_real_planning();
    ctx.request_transition(GoalStatus::Plan, GoalStatus::Implement)
        .unwrap();
    drop(ctx);
    fixture
        .work_items
        .update_latest_goal_round_implementation_report(
            GOAL,
            "implementation report before disruption",
        )
        .unwrap();
    let seeded_candidate = fixture.commit_candidate("# pre-disruption candidate line");
    fixture
        .work_items
        .update_goal_candidate_commit(GOAL, &seeded_candidate)
        .unwrap();

    fixture.assert_restart_preserves_evidence_and_continuation();
}

/// Disruption point: the Quality correction agent finished and its report was
/// persisted, but the gate never ran. Recovery preserves the report and leaves
/// Quality eligible without manufacturing a gate verdict.
#[test]
fn force_stop_after_quality_agent_evidence_preserves_it_for_continuation() {
    let fixture = DisruptionFixture::new("disruption-quality-agent");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    let candidate = fixture.commit_candidate("# quality-reviewed candidate line");
    fixture
        .work_items
        .update_goal_git_refs(GOAL, BRANCH, "main", &fixture.base, Some(&candidate))
        .unwrap();
    fixture.advance(GoalStatus::Implement);
    fixture.advance(GoalStatus::Quality);
    fixture
        .work_items
        .update_goal_round_evaluation_summary(
            GOAL,
            0,
            &json!({
                "quality_agent_report": SEEDED_QUALITY_AGENT_REPORT,
                "quality_candidate_commit": candidate
            }),
        )
        .unwrap();
    drop(fixture.claim(GoalStatus::Quality));

    fixture.assert_restart_preserves_evidence_and_continuation();
}

/// Disruption point: a durable passed Quality proof exists and the
/// Quality → Governance transition landed, but Governance never started.
/// Recovery preserves the proof and leaves Governance available to normal
/// admission, where evidence applicability is rechecked before integration.
#[test]
fn force_stop_after_quality_proof_before_governance_preserves_the_proof() {
    let fixture = DisruptionFixture::new("disruption-governance-entry");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    let candidate = fixture.commit_candidate("# governed candidate line");
    fixture
        .work_items
        .update_goal_git_refs(GOAL, BRANCH, "main", &fixture.base, Some(&candidate))
        .unwrap();
    fixture.advance(GoalStatus::Implement);
    fixture.advance(GoalStatus::Quality);
    fixture.seed_quality_proof(&candidate);
    fixture.advance(GoalStatus::Governance);
    drop(fixture.claim(GoalStatus::Governance));

    fixture.assert_restart_preserves_evidence_and_continuation();
}

/// Disruption point: mid-Governance with the integrated-target transaction
/// marker written and integration-worktree residue left behind — integration
/// itself was interrupted. Recovery retains transaction facts and leaves the
/// current occurrence available to ordinary reconciliation before integration.
#[test]
fn force_stop_mid_governance_with_transaction_marker_preserves_the_transaction_for_continuation() {
    let fixture = DisruptionFixture::new("disruption-governance-marker");
    let _env = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _path = SmokeAiGuard::set(&fixture.smoke_ai);

    fixture.enter_plan_with_worktree();
    let candidate = fixture.commit_candidate("# integration-interrupted candidate line");
    fixture
        .work_items
        .update_goal_git_refs(GOAL, BRANCH, "main", &fixture.base, Some(&candidate))
        .unwrap();
    fixture.advance(GoalStatus::Implement);
    fixture.advance(GoalStatus::Quality);
    fixture.seed_quality_proof(&candidate);
    fixture.advance(GoalStatus::Governance);
    drop(fixture.claim(GoalStatus::Governance));
    // The interrupted integration: the durable transaction marker is open and
    // the Refine-owned integration worktree holds residue.
    crate::application::workflow::governance::integration::transaction::open_integrated_target_transaction(
        &fixture.target_root,
        GOAL,
        0,
    )
    .unwrap();
    let marker = fixture
        .target_root
        .join(".git/refine-integrated-target-transaction.json");
    assert!(marker.exists());
    let integration_worktree = fixture.target_root.join(".git/refine-integration/target");
    git(
        &fixture.target_root,
        &[
            "worktree",
            "add",
            "--detach",
            integration_worktree.to_str().unwrap(),
        ],
    )
    .unwrap();
    fs::write(
        integration_worktree.join("residue.txt"),
        "refine merge residue\n",
    )
    .unwrap();

    fixture.assert_restart_preserves_evidence_and_continuation();
}
