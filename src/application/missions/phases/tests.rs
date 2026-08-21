//! End-to-end Mission lifecycle tests with a stub agent provider.
//!
//! The full loop: investigation → plan approval → wave materialization and
//! admission → contribution settlement → reconciliation → synthesis →
//! quality → governance → review approval → consolidation with the
//! two-commit Git read-back.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::application::missions::runner::MissionWorkflowEngine;
use crate::application::missions::service::FileMissionService;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{
    AgentProviderService, ProviderCapability, ProviderInvocation,
};
use crate::infrastructure::storage::project_layout::refine_dir_for_target_root;
use crate::model::mission::{GoalContribution, MissionPlan, MissionWave};
use crate::model::mission::{MissionGoalSpec, MissionStatus};

/// A stub provider that answers each Mission phase prompt with a valid
/// contract instance. The reduction draft is empty so verified findings
/// auto-promote; the criticism report is empty so nothing is contested.
struct StubMissionProvider {
    target_head: String,
}

impl AgentProviderService for StubMissionProvider {
    fn detect(&self) -> RefineResult<Vec<ProviderCapability>> {
        Ok(vec![ProviderCapability {
            name: "stub".to_string(),
            display_name: "Stub Mission Provider".to_string(),
            binary: "stub".to_string(),
            installed: true,
            path: None,
            supports_resume: false,
            supports_direct_api: false,
            supports_cli: true,
            output_format: "plain".to_string(),
            prompt_transport: Default::default(),
        }])
    }

    fn configure(&self, _provider: &str) -> RefineResult<()> {
        Ok(())
    }

    fn authenticate(&self, _provider: &str) -> RefineResult<()> {
        Ok(())
    }

    fn invoke(&self, invocation: ProviderInvocation) -> RefineResult<String> {
        let head = &self.target_head;
        let output = if invocation.prompt.contains("Mission investigation JSON") {
            json!({
                "accepts": [{
                    "draft_id": "d1",
                    "assertion": {
                        "assertion_id": "",
                        "kind": "fact",
                        "authority": "evidence",
                        "provenance": null,
                        "qualified": null,
                        "supersedes": [],
                        "corrects": [],
                        "derived_from": [],
                        "scope": "the auth service exists",
                        "scope_refs": ["crit:tokens"],
                        "evidence_refs": [format!("path:README.md@{head}")],
                        "supersedable": true,
                        "members": [],
                        "resolution": null,
                        "resolved_by": null
                    },
                    "evidence_coverage": [],
                    "unverified_extent": false
                }],
                "contradictions": [],
                "artifact_promotions": [],
                "open_questions": ["what rotation policy applies"]
            })
        } else if invocation.prompt.contains("Mission reduction JSON") {
            json!({
                "accepts": [],
                "rejects": [],
                "contradictions": [],
                "artifact_promotions": [],
                "spec_amendments": [],
                "followups": []
            })
        } else if invocation.prompt.contains("Mission criticism JSON") {
            json!({"verdicts": [], "notes": "stub criticism: no counter-case found"})
        } else if invocation.prompt.contains("Mission synthesis JSON") {
            json!({
                "summary": "auth documented",
                "criteria_results": [{
                    "criterion_id": "crit:tokens",
                    "result": "met",
                    "evidence": ["assertion from wave 1"]
                }],
                "artifact_promotions": [],
                "residual_risks": []
            })
        } else if invocation.prompt.contains("Mission quality JSON") {
            json!({
                "ok": true,
                "summary": "combined outcome holds",
                "findings": [],
                "criteria_results": []
            })
        } else if invocation.prompt.contains("Mission governance JSON") {
            json!({
                "status": "passed",
                "message": "no system-level violations",
                "violations": [],
                "recovery_analysis": "",
                "recovery_round_prompt": ""
            })
        } else {
            return Err(RefineError::InvalidInput(
                "stub provider received an unknown Mission prompt".to_string(),
            ));
        };
        Ok(output.to_string())
    }

    fn resume(&self, _provider: &str, _session_id: &str) -> RefineResult<String> {
        Err(RefineError::InvalidInput(
            "stub provider does not resume sessions".to_string(),
        ))
    }

    fn diagnose(&self, _provider: &str) -> RefineResult<Vec<String>> {
        Ok(Vec::new())
    }
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "refine-mission-phase-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git must be runnable");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git must be runnable");
    assert!(output.status.success(), "git {} failed", args.join(" "));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Force one Goal into Review with the evidence settlement requires. The
/// Mission engine tests drive Mission phases; Goal workflow execution is
/// covered by the Goal workflow's own suites.
fn force_goal_review_ready(refine_dir: &Path, goal_id: &str) {
    let goal_id = goal_id.to_uppercase();
    let path = refine_dir
        .join("goals")
        .join(&goal_id[..2])
        .join(&goal_id[2..])
        .join("goal.json");
    let bytes = std::fs::read(&path).unwrap();
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    let object = value.as_object_mut().unwrap();
    object.insert("status".to_string(), json!("review"));
    let rounds = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .expect("goal has rounds");
    let round = rounds.last_mut().unwrap();
    let round = round.as_object_mut().unwrap();
    round.insert("quality_state".to_string(), json!("passed"));
    round.insert("rule_state".to_string(), json!("passed"));
    round.insert(
        "workflow_integration".to_string(),
        json!({"state": "integrated"}),
    );
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn test_plan() -> MissionPlan {
    MissionPlan {
        charter_digest: None,
        summary: "one wave documenting the auth surface".to_string(),
        assumptions: vec![],
        risks: vec![],
        criteria_coverage: vec!["crit:tokens".to_string()],
        waves: vec![MissionWave {
            number: 1,
            purpose: "document tokens".to_string(),
            goal_specs: vec![MissionGoalSpec {
                mission_goal_key: "k1".to_string(),
                name: "Document tokens".to_string(),
                prompt: "Document the token invariant of the auth service".to_string(),
                role: None,
                required: true,
                criterion_ids: vec!["crit:tokens".to_string()],
                input_artifact_keys: vec![],
                output_artifact_keys: vec![],
                expected_findings: vec![],
                feature_id: None,
                feature_order: None,
                preferred_node: None,
            }],
            required_snapshot: None,
            completion_condition: None,
        }],
        artifact_obligations: vec![],
        criticism: None,
        resolutions: vec![],
        effective_digest: Some("plan-digest-1".to_string()),
    }
}

#[test]
fn mission_engine_runs_the_full_lifecycle_to_done() {
    let root = unique_temp_dir("lifecycle");
    let target_root = root.join("target");
    let runtime_root = root.join("runtime");
    std::fs::create_dir_all(&target_root).unwrap();
    std::fs::create_dir_all(&runtime_root).unwrap();
    git(&target_root, &["init", "-q"]);
    git(&target_root, &["config", "user.email", "test@refine"]);
    git(&target_root, &["config", "user.name", "refine test"]);
    std::fs::write(target_root.join("README.md"), "auth service\n").unwrap();
    git(&target_root, &["add", "README.md"]);
    git(&target_root, &["commit", "-q", "-m", "initial"]);
    let head = git_stdout(&target_root, &["rev-parse", "HEAD"]);
    git(
        &target_root,
        &["commit", "-q", "--allow-empty", "-m", "second"],
    );
    let later_head = git_stdout(&target_root, &["rev-parse", "HEAD"]);

    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    let service = FileMissionService::new(&refine_dir);
    let work_items = FileWorkItemService::new(&refine_dir);
    let provider = Arc::new(StubMissionProvider {
        target_head: head.clone(),
    });
    let engine = MissionWorkflowEngine::new(&runtime_root, &target_root)
        .with_provider(provider.clone(), "stub");

    // Frame the Mission and start it.
    let mission = service
        .create_mission(
            "Modernize auth",
            "Modernize the authentication flow",
            Some("Buddy"),
            None,
            None,
        )
        .unwrap();
    let criteria = json!([{"id": "crit:tokens", "description": "token invariants documented"}]);
    service
        .edit_mission_frame(&mission.id, None, None, Some(&criteria), None, None)
        .unwrap();
    service
        .append_round(&mission.id, "Buddy", "begin round 1", None)
        .unwrap();
    service
        .transition_mission(&mission.id, MissionStatus::Investigate, None)
        .unwrap();

    // Investigation publishes snapshot 1 and moves to Plan.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert_eq!(
        outcome.as_deref(),
        Some("investigation published the initial snapshot")
    );
    let mission = service.show_mission(&mission.id).unwrap();
    assert_eq!(mission.status, MissionStatus::Plan);
    let round = &mission.rounds[0];
    assert_eq!(round.snapshots.len(), 1);
    assert_eq!(round.snapshots[0].knowledge_index.len(), 1);
    assert!(round.phase_evidence.get("investigation").is_some());

    // Plan approval is the human gate.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert_eq!(outcome, None, "unapproved plan waits");
    let mission = service
        .approve_plan(&mission.id, test_plan(), "Buddy", "looks right", None)
        .unwrap();

    // Execute: materialize and admit wave 1. The first step enters Execute;
    // the second materializes and admits the wave Goals.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert!(outcome.unwrap().contains("Execute begins"));
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    let detail = outcome.unwrap_or_default();
    assert!(
        detail.contains("materialized 1"),
        "wave goals materialize; engine said: {detail}"
    );
    let mission = service.show_mission(&mission.id).unwrap();
    assert_eq!(mission.status, MissionStatus::Execute);

    let goals = super::execution::mission_bound_goals(&refine_dir, &mission.id).unwrap();
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0].mission_goal_key, "k1");
    assert_eq!(goals[0].status, crate::model::workflow::GoalStatus::Todo);
    let goal_id = goals[0].goal_id.clone();

    // The admitted GoalRound carries the pinned capsule; the capsule is a
    // member of the pinned agent context once the Goal workflow pins it.
    let detail = work_items.show_goal_detail(&goal_id).unwrap();
    let round = &detail["rounds"][0];
    assert!(round["mission_context"]["capsule_manifest_digest"].is_string());
    assert!(round["mission_capsule"]["capsule_manifest"]["digest"].is_string());

    // The wave is not settled while the Goal is pending.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert_eq!(outcome, None, "unsettled wave waits");

    // The Goal reaches Review with valid evidence and settles a contribution.
    force_goal_review_ready(&refine_dir, &goal_id);
    let contribution = GoalContribution {
        bound_context_digest: None,
        criteria_evidence: vec!["crit:tokens".to_string()],
        findings: vec![crate::model::mission::Finding {
            claim: "the auth service rotates tokens".to_string(),
            evidence: vec![format!("commit:{head}")],
        }],
        challenged_assumptions: vec![],
        artifact_candidates: vec![],
        suggested_followups: vec![],
        downstream_invalidations: vec![],
        digest: None,
    };
    work_items
        .settle_goal_mission_contribution(&goal_id, contribution)
        .unwrap();

    // Wave settled: reconciliation runs the reduction and criticism agents
    // and publishes the next snapshot with the auto-promoted fact.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert!(outcome.unwrap().contains("wave 1 reconciled"));
    let mission = service.show_mission(&mission.id).unwrap();
    let round = &mission.rounds[0];
    assert_eq!(round.snapshots.len(), 2);
    assert_eq!(round.snapshots[1].knowledge_index.len(), 1);
    let assertion = &round.snapshots[1].knowledge_index[0];
    assert_eq!(assertion.authority.as_str(), "evidence");
    assert_eq!(assertion.qualified.as_deref(), Some("goal_review_pending"));
    assert_eq!(round.reconciliation_receipts.len(), 1);
    assert!(!round.reconciliation_receipts[0].claim_set.is_empty());

    // No claims remain: the engine moves to Synthesize.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert!(outcome.unwrap().contains("Synthesize begins"));

    // Synthesis settles the candidate Outcome.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert!(outcome.unwrap().contains("candidate Outcome"));
    let mission = service.show_mission(&mission.id).unwrap();
    assert_eq!(mission.status, MissionStatus::Quality);
    let round = &mission.rounds[0];
    let manifest = round.outcome.as_ref().expect("candidate Outcome");
    assert_eq!(manifest.criteria_results.len(), 1);
    assert_eq!(
        manifest.criteria_results[0].result.as_str(),
        crate::model::mission::CriterionOutcome::Met.as_str()
    );
    assert!(manifest.manifest_digest.is_some());

    // Quality passes deterministic checks and the holistic judgment.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert!(outcome.unwrap().contains("quality passed"));
    assert_eq!(
        service.show_mission(&mission.id).unwrap().status,
        MissionStatus::Governance
    );

    // Governance passes and awaits human review.
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert!(outcome.unwrap().contains("Review awaits"));
    assert_eq!(
        service.show_mission(&mission.id).unwrap().status,
        MissionStatus::Review
    );
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert_eq!(outcome, None, "Review waits for the human gate");

    // Review approval authorizes consolidation; the engine performs the
    // two-commit read-back and exposes Done.
    service
        .transition_mission(&mission.id, MissionStatus::Consolidate, None)
        .unwrap();
    let outcome = engine.evaluate_one(&service, &mission.id).unwrap();
    assert!(outcome.unwrap().contains("consolidated"));
    let mission = service.show_mission(&mission.id).unwrap();
    assert_eq!(mission.status, MissionStatus::Done);

    let round = &mission.rounds[0];
    let publication = round
        .outcome_publication
        .as_ref()
        .expect("publication receipt");
    assert!(publication.outcome_state_commit.is_some());
    assert!(publication.verified_path_digests.len() == 1);

    // The manifest bytes are durable in live state and match the digest.
    let manifest_path = refine_dir
        .join("missions")
        .join(&mission.id[..2])
        .join(&mission.id[2..])
        .join("outcomes")
        .join("1")
        .join("manifest.json");
    assert!(manifest_path.exists(), "Outcome manifest is durable");
    let written: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(
        written["manifest_digest"],
        manifest.manifest_digest.clone().unwrap()
    );

    let _ = later_head;
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn mission_contribution_settlement_requires_review_evidence() {
    let root = unique_temp_dir("settlement");
    let target_root = root.join("target");
    std::fs::create_dir_all(&target_root).unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);

    let created = work_items
        .author_goal(crate::application::work_items::GoalAuthoringRequest {
            id: None,
            goal_id: None,
            name: Some("Standalone".to_string()),
            prompt: "do a thing".to_string(),
            reporter: "Buddy".to_string(),
            assignee: None,
            priority: "medium".to_string(),
            feature_id: None,
            placement: crate::application::work_items::FeatureGoalPlacement::Unordered,
            duplicate_decision: String::new(),
        })
        .unwrap();
    let goal_id = created.goal.as_ref().unwrap().id.clone();

    let err = work_items
        .settle_goal_mission_contribution(
            &goal_id,
            GoalContribution {
                bound_context_digest: None,
                criteria_evidence: vec![],
                findings: vec![],
                challenged_assumptions: vec![],
                artifact_candidates: vec![],
                suggested_followups: vec![],
                downstream_invalidations: vec![],
                digest: None,
            },
        )
        .unwrap_err();
    // A standalone Goal in Backlog never settles: no Review, no Mission
    // context, no evidence.
    assert!(err.to_string().to_lowercase().contains("review"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unverified_evidence_defers_rather_than_promoting() {
    // A contribution citing an unreachable commit fails its tier-2 verifier;
    // the finding defers to judgment and never auto-promotes.
    let root = unique_temp_dir("unverified");
    let target_root = root.join("target");
    let runtime_root = root.join("runtime");
    std::fs::create_dir_all(&target_root).unwrap();
    std::fs::create_dir_all(&runtime_root).unwrap();
    git(&target_root, &["init", "-q"]);
    git(&target_root, &["config", "user.email", "test@refine"]);
    git(&target_root, &["config", "user.name", "refine test"]);
    std::fs::write(target_root.join("README.md"), "x\n").unwrap();
    git(&target_root, &["add", "README.md"]);
    git(&target_root, &["commit", "-q", "-m", "initial"]);
    let head = git_stdout(&target_root, &["rev-parse", "HEAD"]);

    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    let service = FileMissionService::new(&refine_dir);
    let work_items = FileWorkItemService::new(&refine_dir);
    let provider = Arc::new(StubMissionProvider { target_head: head });
    let engine =
        MissionWorkflowEngine::new(&runtime_root, &target_root).with_provider(provider, "stub");

    let mission = service
        .create_mission("M", "intent", Some("Buddy"), None, None)
        .unwrap();
    let criteria = json!([{"id": "crit:tokens", "description": "d"}]);
    service
        .edit_mission_frame(&mission.id, None, None, Some(&criteria), None, None)
        .unwrap();
    service
        .append_round(&mission.id, "Buddy", "go", None)
        .unwrap();
    service
        .transition_mission(&mission.id, MissionStatus::Investigate, None)
        .unwrap();
    engine.evaluate_one(&service, &mission.id).unwrap();
    service
        .approve_plan(&mission.id, test_plan(), "Buddy", "ok", None)
        .unwrap();
    engine.evaluate_one(&service, &mission.id).unwrap();
    engine.evaluate_one(&service, &mission.id).unwrap();

    let goals = super::execution::mission_bound_goals(&refine_dir, &mission.id).unwrap();
    let goal_id = goals[0].goal_id.clone();
    force_goal_review_ready(&refine_dir, &goal_id);
    work_items
        .settle_goal_mission_contribution(
            &goal_id,
            GoalContribution {
                bound_context_digest: None,
                criteria_evidence: vec![],
                findings: vec![crate::model::mission::Finding {
                    claim: "cites a commit that does not exist".to_string(),
                    evidence: vec!["commit:deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string()],
                }],
                challenged_assumptions: vec![],
                artifact_candidates: vec![],
                suggested_followups: vec![],
                downstream_invalidations: vec![],
                digest: None,
            },
        )
        .unwrap();

    engine.evaluate_one(&service, &mission.id).unwrap();
    let mission = service.show_mission(&mission.id).unwrap();
    let round = &mission.rounds[0];
    let receipt = round.reconciliation_receipts.last().unwrap();
    assert!(
        !receipt.deferred.is_empty(),
        "unverified findings defer: {receipt:?}"
    );
    assert!(
        receipt.accepted.is_empty(),
        "nothing auto-promotes from failed verifiers"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn draft_missions_wait_for_the_human_start() {
    let root = unique_temp_dir("draft-waits");
    let target_root = root.join("target");
    std::fs::create_dir_all(&target_root).unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    let service = FileMissionService::new(&refine_dir);
    let mission = service
        .create_mission("M", "intent", None, None, None)
        .unwrap();
    let provider = Arc::new(StubMissionProvider {
        target_head: "x".to_string(),
    });
    let engine = MissionWorkflowEngine::new(root.join("runtime"), &target_root)
        .with_provider(provider, "stub");
    // Draft consumes no agent or fleet capacity; the engine never
    // auto-starts a Mission.
    assert_eq!(engine.evaluate_one(&service, &mission.id).unwrap(), None);
    let _ = std::fs::remove_dir_all(&root);
}
