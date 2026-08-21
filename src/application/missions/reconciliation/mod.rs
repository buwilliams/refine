//! Mission reconciliation: the only behavior that turns parallel Goal
//! evidence into canonical Mission context.
//!
//! Everything else in the Mission design is mechanics; reconciliation is
//! where the system judges what it has learned. This module makes three
//! commitments from `docs/mission-reconciliation.md`:
//!
//! 1. **Lineage is a derived closure, not a stored graph.** Assertion state
//!    is computed from immutable snapshot history on demand, like every
//!    other projection in Refine.
//! 2. **Determinism has an honest boundary.** The verifier registry proves
//!    provenance and machine-checkable claims, never truth; the
//!    auto-promotion policy is sized to exactly what it can prove.
//! 3. **The reducer is itself criticized.** Reduction drafts pass through
//!    adversarial criticism before publication, and dissent is preserved as
//!    evidence.
//!
//! The deterministic steps live here; the agent steps (reduction, criticism,
//! revision) run as one-shot managed processes whose typed output contracts
//! are defined by the `engine` module. Agent text can never define workflow
//! transitions or publication authority.

pub mod capsule;
pub mod engine;
pub mod ledger;
pub mod settlement;
pub mod verify;

pub use capsule::{
    CapsuleInclusion, CapsuleManifest, DEFAULT_CAPSULE_ASSERTION_BUDGET, compile_capsule_manifest,
    compile_capsule_manifest_with_budget, manifest_digest,
};
pub use engine::{
    AGENT_CALLS_PER_ATTEMPT, AppliedReduction, ArtifactPromotion, ClaimedContribution,
    CorrectionMandate, CorrectionProvenance, CriticismReport, CriticismVerdict,
    CriticismVerdictEntry, DEFAULT_DECISION_VOLUME_THRESHOLD, DraftedAssertion,
    DraftedContradiction, DraftedRejection, OpenedAttempt, ReconciliationInput, ReductionDraft,
    VerifiedAttempt, VerifiedCandidate, VerifiedFinding, apply_correction, apply_reduction,
    compute_snapshot_digest, next_attempt_id, open_attempt, verify_claims,
};
pub use ledger::{
    AffectedGoalRound, AffectedGoalSpec, AssertionState, CapsuleBinding, InvalidatedBecause,
    InvalidationReport, assertions_through, compute_invalidation, derive_assertion_states,
};
pub use settlement::{
    ClaimClass, GoalWaveState, WaveGoalStatus, WaveSettlement, classify_against_receipt,
    evaluate_wave_settlement, next_claim_set, pre_synthesis_sweep_needed,
};
pub use verify::{
    EnvelopeRejection, EvidenceRef, FindingVerification, VerificationContext, candidate_ref,
    finding_ref, run_verifier, validate_contribution_envelope, verify_contribution_findings,
    verify_finding,
};

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::application::missions::FileMissionService;
    use crate::model::mission::{
        ArtifactObligation, AssertionKind, Finding, MissionPlan, MissionSnapshot, MissionWave,
    };

    fn temp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-reconcile-{}-{nanos}", std::process::id()))
    }

    fn service_with_mission() -> (FileMissionService, String) {
        let dir = temp_dir();
        let service = FileMissionService::new(&dir);
        let mission = service
            .create_mission(
                "Modernize auth",
                "Modernize the authentication flow",
                Some("Buddy"),
                None,
                None,
            )
            .unwrap();
        // Frame with criteria, charter obligations, a round, an approved
        // plan, and the investigation snapshot.
        let criteria = serde_json::json!([
            {"id": "crit:tokens", "description": "token invariants documented"}
        ]);
        let contract = serde_json::json!([
            {"key": "interface-contract", "kind": "contract", "purpose": "pin the interface"}
        ]);
        let mission = service
            .edit_mission_frame(
                &mission.id,
                None,
                None,
                Some(&criteria),
                Some(&contract),
                None,
            )
            .unwrap();
        let mission = service
            .append_round(&mission.id, "Buddy", "begin round 1", None)
            .unwrap();
        let plan = MissionPlan {
            charter_digest: None,
            summary: "one wave".to_string(),
            assumptions: vec![],
            risks: vec![],
            criteria_coverage: vec!["crit:tokens".to_string()],
            waves: vec![MissionWave {
                number: 1,
                purpose: "document the auth surface".to_string(),
                goal_specs: vec![crate::model::mission::MissionGoalSpec {
                    mission_goal_key: "k1".to_string(),
                    name: "Document tokens".to_string(),
                    prompt: "document the token invariant".to_string(),
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
            artifact_obligations: vec![ArtifactObligation {
                key: "interface-contract".to_string(),
                kind: "contract".to_string(),
                purpose: "pin the interface".to_string(),
                required: true,
                validation_policy: None,
                consumers: vec![],
            }],
            criticism: None,
            resolutions: vec![],
            effective_digest: Some("plan-1".to_string()),
        };
        let mission = service
            .approve_plan(
                &mission.id,
                plan,
                "Buddy",
                "looks right",
                Some(mission.revision),
            )
            .unwrap();
        let snapshot = MissionSnapshot {
            version: 1,
            parent_version: None,
            target_head: Some("head0".to_string()),
            plan_digest: Some("plan-1".to_string()),
            artifact_refs: vec![],
            input_refs: vec![],
            consumed_contribution_refs: vec![],
            knowledge_index: vec![],
            corrects_snapshot: None,
            digest: None,
            created: String::new(),
        };
        let mission = service
            .publish_snapshot(&mission.id, snapshot, None)
            .unwrap();
        assert_eq!(mission.rounds[0].snapshots.len(), 1);
        (service, mission.id.clone())
    }

    fn verified_claim(digest: &str, claim_text: &str) -> engine::ClaimedContribution {
        engine::ClaimedContribution {
            goal_id: "G1".to_string(),
            goal_round: 1,
            mission_goal_key: "k1".to_string(),
            digest: digest.to_string(),
            eligible: true,
            in_review: true,
            contribution: crate::model::mission::GoalContribution {
                bound_context_digest: None,
                criteria_evidence: vec![],
                findings: vec![Finding {
                    claim: claim_text.to_string(),
                    evidence: vec!["commit:c1".to_string()],
                }],
                challenged_assumptions: vec![],
                artifact_candidates: vec![],
                suggested_followups: vec![],
                downstream_invalidations: vec![],
                digest: Some(digest.to_string()),
            },
        }
    }

    fn context() -> VerificationContext {
        let mut context = VerificationContext {
            target_head: Some("head1".to_string()),
            ..Default::default()
        };
        context.reachable_commits.insert("c1".to_string());
        context
    }

    #[test]
    fn full_reconciliation_publishes_snapshot_and_receipt_durably() {
        let (service, mission_id) = service_with_mission();
        let mission = service.show_mission(&mission_id).unwrap();
        let input = engine::ReconciliationInput {
            wave: Some(1),
            claims: vec![verified_claim("digest-1", "tokens rotate hourly")],
            verification: context(),
            budgets: Default::default(),
            decision_volume_threshold: None,
            correction: None,
        };
        let opened = engine::open_attempt(&mission, &input).unwrap();
        let verified = engine::verify_claims(&opened);
        let applied = engine::apply_reduction(
            &verified,
            &engine::ReductionDraft::default(),
            &engine::CriticismReport::default(),
        )
        .unwrap();
        let published = service
            .publish_reconciliation(&mission_id, &applied, Some(mission.revision))
            .unwrap();

        let round = &published.rounds[0];
        assert_eq!(round.snapshots.len(), 2);
        assert_eq!(round.snapshots[1].version, 2);
        assert_eq!(round.snapshots[1].knowledge_index.len(), 1);
        assert!(round.snapshots[1].digest.is_some());
        assert_eq!(round.reconciliation_receipts.len(), 1);
        let receipt = &round.reconciliation_receipts[0];
        assert_eq!(
            receipt.attempt,
            format!("mission:{}:round:1:reconcile:1:1", mission_id)
        );
        assert_eq!(receipt.claim_set, vec!["digest-1".to_string()]);
        assert_eq!(receipt.next_snapshot, 2);
        assert!(!receipt.accepted.is_empty());

        // Read-back: the durable record round-trips through the typed model.
        let reread = service.show_mission(&mission_id).unwrap();
        assert_eq!(
            reread.rounds[0].reconciliation_receipts[0].attempt,
            receipt.attempt
        );
        let invalidation =
            ledger::compute_invalidation(&reread, &std::collections::BTreeSet::new(), &[]);
        assert!(invalidation.invalidated.is_empty());
        std::fs::remove_dir_all(service.refine_dir).unwrap();
    }

    #[test]
    fn stale_attempt_fence_rejects_competing_publication() {
        let (service, mission_id) = service_with_mission();
        let mission = service.show_mission(&mission_id).unwrap();
        let input = engine::ReconciliationInput {
            wave: Some(1),
            claims: vec![verified_claim("digest-1", "tokens rotate hourly")],
            verification: context(),
            budgets: Default::default(),
            decision_volume_threshold: None,
            correction: None,
        };
        let opened = engine::open_attempt(&mission, &input).unwrap();
        let verified = engine::verify_claims(&opened);
        let applied = engine::apply_reduction(
            &verified,
            &engine::ReductionDraft::default(),
            &engine::CriticismReport::default(),
        )
        .unwrap();
        // First attempt publishes.
        service
            .publish_reconciliation(&mission_id, &applied, Some(mission.revision))
            .unwrap();
        // A replay of the same attempt identity is stale: the next expected
        // attempt number moved on.
        let err = service
            .publish_reconciliation(&mission_id, &applied, None)
            .unwrap_err();
        assert!(err.to_string().contains("stale"));
        std::fs::remove_dir_all(service.refine_dir).unwrap();
    }

    #[test]
    fn late_contribution_carries_to_the_next_boundary() {
        let (service, mission_id) = service_with_mission();
        let mission = service.show_mission(&mission_id).unwrap();
        let input = engine::ReconciliationInput {
            wave: Some(1),
            claims: vec![verified_claim("digest-1", "tokens rotate hourly")],
            verification: context(),
            budgets: Default::default(),
            decision_volume_threshold: None,
            correction: None,
        };
        let opened = engine::open_attempt(&mission, &input).unwrap();
        let verified = engine::verify_claims(&opened);
        let applied = engine::apply_reduction(
            &verified,
            &engine::ReductionDraft::default(),
            &engine::CriticismReport::default(),
        )
        .unwrap();
        let published = service
            .publish_reconciliation(&mission_id, &applied, Some(mission.revision))
            .unwrap();
        let receipt = &published.rounds[0].reconciliation_receipts[0];

        // A straggler contribution settles after the claim window closed.
        assert_eq!(
            settlement::classify_against_receipt(receipt, "digest-2"),
            settlement::ClaimClass::Late
        );
        // It enters the next claim set unconditionally, together with any
        // deferred findings' contributions.
        let next = settlement::next_claim_set(
            &published.rounds[0].reconciliation_receipts,
            &["digest-1".to_string(), "digest-2".to_string()],
        );
        assert_eq!(next, vec!["digest-2".to_string()]);
        // With all waves settled and evidence remaining, the pre-Synthesis
        // sweep is mandatory.
        assert!(settlement::pre_synthesis_sweep_needed(
            &published.rounds[0].reconciliation_receipts,
            &["digest-2".to_string()],
            true
        ));
        std::fs::remove_dir_all(service.refine_dir).unwrap();
    }

    #[test]
    fn correction_invalidates_and_flags_affected_capsules() {
        let (service, mission_id) = service_with_mission();
        let mission = service.show_mission(&mission_id).unwrap();
        let input = engine::ReconciliationInput {
            wave: Some(1),
            claims: vec![verified_claim("digest-1", "tokens rotate hourly")],
            verification: context(),
            budgets: Default::default(),
            decision_volume_threshold: None,
            correction: None,
        };
        let opened = engine::open_attempt(&mission, &input).unwrap();
        let verified = engine::verify_claims(&opened);
        let applied = engine::apply_reduction(
            &verified,
            &engine::ReductionDraft::default(),
            &engine::CriticismReport::default(),
        )
        .unwrap();
        let published = service
            .publish_reconciliation(&mission_id, &applied, Some(mission.revision))
            .unwrap();
        let accepted_id = published.rounds[0].snapshots[1].knowledge_index[0]
            .assertion_id
            .clone();

        // The Goal leaves Review: its contribution source is invalidated.
        let mut sources = std::collections::BTreeSet::new();
        sources.insert("contribution:G1/1".to_string());
        let before = ledger::compute_invalidation(&published, &sources, &[]);
        assert!(before.invalidated.contains_key(&accepted_id));

        // A correction snapshot repairs the ledger by appending.
        let mut correction = crate::model::mission::KnowledgeAssertion {
            assertion_id: String::new(),
            kind: AssertionKind::Fact,
            authority: crate::model::mission::ArtifactAuthority::Evidence,
            provenance: None,
            qualified: None,
            supersedes: vec![],
            corrects: vec![accepted_id.clone()],
            derived_from: vec![],
            scope: Some("tokens rotate daily, not hourly".to_string()),
            scope_refs: vec![],
            evidence_refs: vec!["commit:c1".to_string()],
            supersedable: true,
            members: vec![],
            resolution: None,
            resolved_by: None,
        };
        correction.assertion_id = "a-correction".to_string();
        let mandate = engine::CorrectionMandate {
            provenance: engine::CorrectionProvenance::SourceInvalidated,
            reason: "Goal G1 left Review".to_string(),
            corrections: vec![engine::DraftedAssertion {
                draft_id: "c1".to_string(),
                assertion: correction,
                evidence_coverage: vec![],
                unverified_extent: false,
            }],
        };
        let criticism = engine::CriticismReport {
            verdicts: vec![engine::CriticismVerdictEntry {
                target: "c1".to_string(),
                verdict: engine::CriticismVerdict::Confirmed,
                note: "pinned evidence".to_string(),
            }],
            notes: String::new(),
        };
        let corrected = engine::apply_correction(
            &published,
            &mandate,
            &criticism,
            &context(),
            Default::default(),
        )
        .unwrap();
        let after = service
            .publish_reconciliation(&mission_id, &corrected, Some(published.revision))
            .unwrap();

        // The GoalRound whose capsule included the invalidated assertion is
        // affected; the ledger records the correction.
        let capsules = vec![ledger::CapsuleBinding {
            goal_id: "G1".to_string(),
            goal_round: 1,
            mission_goal_key: "k1".to_string(),
            assertion_ids: vec![accepted_id.clone()],
        }];
        let report = ledger::compute_invalidation(&after, &sources, &capsules);
        assert_eq!(report.affected_goal_rounds.len(), 1);
        assert_eq!(
            report.affected_goal_rounds[0].invalidated_assertions,
            vec![accepted_id]
        );
        // The spec whose criterion the invalidated assertion scoped to is
        // blocked from future admission.
        assert_eq!(report.affected_specs.len(), 1);
        assert_eq!(report.affected_specs[0].mission_goal_key, "k1");
        std::fs::remove_dir_all(service.refine_dir).unwrap();
    }

    #[test]
    fn capsule_manifest_pins_exactly_what_the_goalround_observed() {
        let (service, mission_id) = service_with_mission();
        let mission = service.show_mission(&mission_id).unwrap();
        let input = engine::ReconciliationInput {
            wave: Some(1),
            claims: vec![verified_claim("digest-1", "tokens rotate hourly")],
            verification: context(),
            budgets: Default::default(),
            decision_volume_threshold: None,
            correction: None,
        };
        let opened = engine::open_attempt(&mission, &input).unwrap();
        let verified = engine::verify_claims(&opened);
        let applied = engine::apply_reduction(
            &verified,
            &engine::ReductionDraft::default(),
            &engine::CriticismReport::default(),
        )
        .unwrap();
        let published = service
            .publish_reconciliation(&mission_id, &applied, Some(mission.revision))
            .unwrap();
        let snapshot = &published.rounds[0].snapshots[1];

        let capsule = service
            .compile_context_capsule(&published, snapshot, "k1")
            .unwrap();
        let digest = capsule["capsule_manifest_digest"].as_str().unwrap();
        assert!(digest.starts_with("sha256:"), "manifest digest is set");
        assert_eq!(
            capsule["capsule_manifest"]["digest"].as_str().unwrap(),
            digest,
            "the embedded manifest carries the same digest"
        );
        // The manifest embedded in the capsule lists exactly the included
        // assertion with its reason.
        let manifest = &capsule["capsule_manifest"];
        let inclusions = manifest["assertions"].as_array().unwrap();
        assert_eq!(inclusions.len(), 1);
        assert!(
            inclusions[0]["reason"]
                .as_str()
                .unwrap()
                .starts_with("active:crit:tokens")
        );
        assert_eq!(
            capsule["assertions"].as_array().unwrap().len(),
            1,
            "capsule renders the included assertion body"
        );
        std::fs::remove_dir_all(service.refine_dir).unwrap();
    }

    #[test]
    fn capsule_renders_assertions_from_the_whole_snapshot_chain() {
        let (service, mission_id) = service_with_mission();
        let mut mission = service.show_mission(&mission_id).unwrap();
        let mut revision = mission.revision;
        // Wave 1 publishes snapshot 2 with one accepted assertion.
        let input = engine::ReconciliationInput {
            wave: Some(1),
            claims: vec![verified_claim("digest-1", "tokens rotate hourly")],
            verification: context(),
            budgets: Default::default(),
            decision_volume_threshold: None,
            correction: None,
        };
        let opened = engine::open_attempt(&mission, &input).unwrap();
        let verified = engine::verify_claims(&opened);
        let applied = engine::apply_reduction(
            &verified,
            &engine::ReductionDraft::default(),
            &engine::CriticismReport::default(),
        )
        .unwrap();
        mission = service
            .publish_reconciliation(&mission_id, &applied, Some(revision))
            .unwrap();
        revision = mission.revision;
        // Wave 2 publishes snapshot 3 with a second accepted assertion.
        let input = engine::ReconciliationInput {
            wave: Some(2),
            claims: vec![verified_claim("digest-2", "refresh tokens live 15 minutes")],
            verification: context(),
            budgets: Default::default(),
            decision_volume_threshold: None,
            correction: None,
        };
        let opened = engine::open_attempt(&mission, &input).unwrap();
        let verified = engine::verify_claims(&opened);
        let applied = engine::apply_reduction(
            &verified,
            &engine::ReductionDraft::default(),
            &engine::CriticismReport::default(),
        )
        .unwrap();
        mission = service
            .publish_reconciliation(&mission_id, &applied, Some(revision))
            .unwrap();

        // The latest snapshot's own knowledge_index holds only the wave-2
        // assertion; the wave-1 assertion lives in the earlier snapshot of
        // the same chain. The capsule must render both: the manifest and the
        // rendered bodies may never disagree.
        let latest = mission.rounds[0].snapshots.last().unwrap().clone();
        assert_eq!(
            latest.knowledge_index.len(),
            1,
            "snapshot 3 carries only its own accepted assertion"
        );
        let capsule = service
            .compile_context_capsule(&mission, &latest, "k1")
            .unwrap();
        let manifest_ids: Vec<&str> = capsule["capsule_manifest"]["assertions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|inclusion| inclusion["id"].as_str())
            .collect();
        assert_eq!(
            manifest_ids.len(),
            2,
            "the manifest accumulates the whole chain"
        );
        assert_eq!(
            capsule["assertions"].as_array().unwrap().len(),
            manifest_ids.len(),
            "every manifest-included assertion renders a body, including assertions accepted by earlier snapshots"
        );
        std::fs::remove_dir_all(service.refine_dir).unwrap();
    }
}
