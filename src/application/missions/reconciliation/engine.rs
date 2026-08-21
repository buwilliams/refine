//! The fenced reconciliation attempt: claim, verify, reduce, criticize,
//! revise, publish.
//!
//! The deterministic steps (claim, envelope verification, claim
//! verification, publication) live here. The agent steps (reduction draft,
//! adversarial criticism, revision) run as one-shot managed processes; this
//! module defines their typed output contracts and applies them with the
//! auto-promotion policy, so agent text can never define workflow transitions
//! or publication authority.
//!
//! See `docs/mission-reconciliation.md` ("The reconciliation loop",
//! "Auto-promotion policy", "Decision traffic", and "Correction snapshots").

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as Sha256Digest, Sha256};

use crate::error::{RefineError, RefineResult};
use crate::model::mission::{
    ArtifactAuthority, ArtifactRef, AssertionKind, BudgetReport, ContradictionResolution,
    DecisionRequest, GoalContribution, KnowledgeAssertion, Mission, MissionSnapshot,
    ReconciliationBudgets, ReconciliationReceipt, VerifierResult,
};

use super::verify::{
    self, EnvelopeRejection, FindingVerification, VerificationContext, candidate_ref,
};

/// How many human interrupts a typical wave boundary should produce before
/// the receipt records a plan-quality signal.
pub const DEFAULT_DECISION_VOLUME_THRESHOLD: usize = 2;

/// The number of agent calls one full reduction attempt consumes: draft,
/// criticism, revision.
pub const AGENT_CALLS_PER_ATTEMPT: usize = 3;

/// One contribution claimed by a reconciliation attempt.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClaimedContribution {
    pub goal_id: String,
    pub goal_round: usize,
    pub mission_goal_key: String,
    /// The contribution digest; must match the GoalRound's settled
    /// contribution digest.
    pub digest: String,
    /// The Goal reached Review with valid integration, Quality, and
    /// Governance evidence, or is terminal with valid evidence. Ineligible
    /// claims fail closed.
    pub eligible: bool,
    /// The Goal is still in Review rather than Done.
    pub in_review: bool,
    pub contribution: GoalContribution,
}

/// The frozen inputs of one reconciliation attempt.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReconciliationInput {
    /// The wave being reconciled; `None` for the pre-Synthesis sweep.
    pub wave: Option<usize>,
    pub claims: Vec<ClaimedContribution>,
    pub verification: VerificationContext,
    #[serde(default)]
    pub budgets: ReconciliationBudgets,
    #[serde(default)]
    pub decision_volume_threshold: Option<usize>,
    /// Set for correction attempts, which may carry an empty claim set.
    #[serde(default)]
    pub correction: Option<String>,
}

impl ReconciliationInput {
    fn decision_threshold(&self) -> usize {
        self.decision_volume_threshold
            .unwrap_or(DEFAULT_DECISION_VOLUME_THRESHOLD)
    }
}

/// The opened, fenced attempt: claim set, parent snapshot, and attempt
/// identity frozen before verification begins.
#[derive(Clone, Debug)]
pub struct OpenedAttempt {
    pub attempt_id: String,
    pub mission: Mission,
    pub wave: Option<usize>,
    pub parent_snapshot: usize,
    pub claims: Vec<ClaimedContribution>,
    pub verification: VerificationContext,
    pub budgets: ReconciliationBudgets,
    pub decision_volume_threshold: usize,
    pub approved_obligation_keys: BTreeSet<String>,
}

/// The next attempt identity for one Mission round and wave. Attempt
/// identities increase monotonically within the Round.
pub fn next_attempt_id(mission: &Mission, wave: Option<usize>) -> String {
    let round = mission.current_round.unwrap_or(0);
    let stage = wave
        .map(|wave| wave.to_string())
        .unwrap_or_else(|| "sweep".to_string());
    let attempts = mission
        .rounds
        .iter()
        .find(|candidate| candidate.number == round)
        .map(|current| {
            current
                .reconciliation_receipts
                .iter()
                .filter(|receipt| receipt.wave == wave)
                .count()
        })
        .unwrap_or(0);
    format!(
        "mission:{}:round:{}:reconcile:{}:{}",
        mission.id,
        round,
        stage,
        attempts + 1
    )
}

/// Open a fenced reconciliation attempt: freeze the claim set, parent
/// snapshot, budgets, and attempt identity.
///
/// Fails closed when the Mission is terminal, has no current Round or parent
/// snapshot, or claims an ineligible contribution or a duplicate digest.
/// Correction attempts may carry an empty claim set.
pub fn open_attempt(mission: &Mission, input: &ReconciliationInput) -> RefineResult<OpenedAttempt> {
    if mission.status.is_terminal() {
        return Err(RefineError::InvalidInput(format!(
            "Mission {} is terminal; reconciliation requires a new Round",
            mission.id
        )));
    }
    let round_number = mission.current_round.ok_or_else(|| {
        RefineError::InvalidInput(format!("Mission {} has no current Round", mission.id))
    })?;
    let round = mission
        .rounds
        .iter()
        .find(|candidate| candidate.number == round_number)
        .ok_or_else(|| {
            RefineError::InvalidInput(format!(
                "Mission {} Round {round_number} was not found",
                mission.id
            ))
        })?;
    let parent_snapshot = round.snapshots.last().map(|s| s.version).ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "Mission {} Round {round_number} has no parent snapshot to reconcile",
            mission.id
        ))
    })?;
    if input.claims.is_empty() && input.correction.is_none() {
        return Err(RefineError::InvalidInput(
            "reconciliation requires at least one claimed contribution or a correction mandate"
                .to_string(),
        ));
    }
    let mut seen_digests = BTreeSet::new();
    let mut seen_keys = BTreeSet::new();
    for claim in &input.claims {
        if !claim.eligible {
            return Err(RefineError::InvalidInput(format!(
                "contribution {} of Goal {} is not eligible for reconciliation",
                claim.digest, claim.goal_id
            )));
        }
        if !seen_digests.insert(claim.digest.clone()) {
            return Err(RefineError::InvalidInput(format!(
                "duplicate contribution digest {} in the claim set",
                claim.digest
            )));
        }
        let key = (claim.goal_id.as_str(), claim.goal_round);
        if !seen_keys.insert(key) {
            return Err(RefineError::InvalidInput(format!(
                "duplicate contribution for Goal {} Round {}",
                claim.goal_id, claim.goal_round
            )));
        }
    }
    let mut approved_obligation_keys = BTreeSet::new();
    for obligation in &round.request.artifact_obligations {
        approved_obligation_keys.insert(obligation.key.clone());
    }
    if let Some(plan) = &round.plan {
        for obligation in &plan.artifact_obligations {
            approved_obligation_keys.insert(obligation.key.clone());
        }
    }
    Ok(OpenedAttempt {
        attempt_id: next_attempt_id(mission, input.wave),
        mission: mission.clone(),
        wave: input.wave,
        parent_snapshot,
        claims: input.claims.clone(),
        verification: input.verification.clone(),
        budgets: input.budgets,
        decision_volume_threshold: input.decision_threshold(),
        approved_obligation_keys,
    })
}

/// One finding after deterministic verification.
#[derive(Clone, Debug)]
pub struct VerifiedFinding {
    pub finding_ref: String,
    pub claim: ClaimedContribution,
    pub claim_text: String,
    pub evidence: Vec<String>,
    pub verification: FindingVerification,
}

/// One artifact candidate after envelope validation.
#[derive(Clone, Debug)]
pub struct VerifiedCandidate {
    pub candidate_ref: String,
    pub obligation_key: String,
}

/// The attempt after tiers 1 and 2: envelope rejections and per-finding
/// verification results, in deterministic claim order.
#[derive(Clone, Debug)]
pub struct VerifiedAttempt {
    pub opened: OpenedAttempt,
    pub envelope_rejections: Vec<EnvelopeRejection>,
    pub findings: Vec<VerifiedFinding>,
    pub candidates: Vec<VerifiedCandidate>,
    pub verifier_results: Vec<VerifierResult>,
}

/// Run tier-1 envelope validation and tier-2 claim verification over the
/// claim set. Pure and deterministic: the same claim set and context yield
/// byte-identical results.
pub fn verify_claims(opened: &OpenedAttempt) -> VerifiedAttempt {
    let mut attempt = VerifiedAttempt {
        opened: opened.clone(),
        envelope_rejections: Vec::new(),
        findings: Vec::new(),
        candidates: Vec::new(),
        verifier_results: Vec::new(),
    };
    let mut rejected_refs = BTreeSet::new();
    for claim in &opened.claims {
        for rejection in verify::validate_contribution_envelope(
            &claim.goal_id,
            claim.goal_round,
            &claim.contribution,
            &opened.approved_obligation_keys,
        ) {
            rejected_refs.insert(rejection.finding_ref.clone());
            attempt.envelope_rejections.push(rejection);
        }
    }
    for claim in &opened.claims {
        let findings = verify::verify_contribution_findings(
            &claim.goal_id,
            claim.goal_round,
            &claim.contribution,
            &opened.verification,
        );
        for (index, (reference, verification, results)) in findings.into_iter().enumerate() {
            attempt.verifier_results.extend(results);
            if rejected_refs.contains(&reference) {
                continue;
            }
            attempt.findings.push(VerifiedFinding {
                finding_ref: reference,
                claim: claim.clone(),
                claim_text: claim.contribution.findings[index].claim.clone(),
                evidence: claim.contribution.findings[index].evidence.clone(),
                verification,
            });
        }
        for candidate in &claim.contribution.artifact_candidates {
            let reference =
                candidate_ref(&claim.goal_id, claim.goal_round, &candidate.obligation_key);
            if rejected_refs.contains(&reference) {
                continue;
            }
            attempt.candidates.push(VerifiedCandidate {
                candidate_ref: reference,
                obligation_key: candidate.obligation_key.clone(),
            });
        }
    }
    attempt
}

/// A drafted acceptance from the reduction agent. The assertion id is
/// assigned deterministically by the engine; agent text cannot mint ids.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DraftedAssertion {
    /// Agent-assigned draft identity used by criticism verdicts.
    pub draft_id: String,
    pub assertion: KnowledgeAssertion,
    /// The finding refs and evidence refs that cover the claim. Model
    /// authority requires non-empty coverage.
    #[serde(default)]
    pub evidence_coverage: Vec<String>,
    /// A universal negative without a named verifier; publishes as Model
    /// qualified `unverified_extent`.
    #[serde(default)]
    pub unverified_extent: bool,
}

/// A drafted rejection with reasoning.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DraftedRejection {
    pub finding_ref: String,
    pub reason: String,
}

/// A drafted contradiction between accepted assertions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DraftedContradiction {
    /// Assertion ids in the existing ledger or accepted by this attempt.
    pub members: Vec<String>,
    /// Evidence, ScopeSplit, or Superseded require a machine-checkable
    /// basis; anything else stays Open.
    pub resolution: Option<ContradictionResolution>,
    /// A tier-2-verified finding ref or an accepted assertion id.
    pub resolution_basis: Option<String>,
}

/// A drafted artifact promotion from a verified candidate.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ArtifactPromotion {
    pub candidate_ref: String,
    pub artifact: ArtifactRef,
}

/// The reduction agent's typed output contract.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ReductionDraft {
    #[serde(default)]
    pub accepts: Vec<DraftedAssertion>,
    #[serde(default)]
    pub rejects: Vec<DraftedRejection>,
    #[serde(default)]
    pub contradictions: Vec<DraftedContradiction>,
    #[serde(default)]
    pub artifact_promotions: Vec<ArtifactPromotion>,
    #[serde(default)]
    pub spec_amendments: Vec<String>,
    #[serde(default)]
    pub followups: Vec<String>,
}

/// One adversarial-criticism verdict.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CriticismVerdict {
    Confirmed,
    Contested,
    InsufficientEvidence,
}

/// The criticism agent's typed output contract. `notes` is preserved
/// verbatim as first-class evidence.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CriticismReport {
    #[serde(default)]
    pub verdicts: Vec<CriticismVerdictEntry>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CriticismVerdictEntry {
    /// A `draft_id`, a finding ref, or a rejection finding ref.
    pub target: String,
    pub verdict: CriticismVerdict,
    #[serde(default)]
    pub note: String,
}

/// The applied, revised reduction ready for publication: the next snapshot
/// and the immutable receipt.
#[derive(Clone, Debug)]
pub struct AppliedReduction {
    pub attempt_id: String,
    pub wave: Option<usize>,
    pub parent_snapshot: usize,
    pub snapshot: MissionSnapshot,
    pub receipt: ReconciliationReceipt,
}

struct DecisionRequestDraft {
    summary: String,
    choices: Vec<String>,
    load_bearing: bool,
}

/// Apply the reduction draft and adversarial criticism deterministically:
/// the engine performs the revision step, enforces the auto-promotion
/// policy, preserves dissent verbatim, batches and ranks decision requests
/// against the decision budget, and builds the next snapshot and receipt.
///
/// The agent budget is consumed by the draft, criticism, and revision calls;
/// an exhausted agent budget publishes a partial reconciliation with every
/// finding deferred and no evidence discarded.
pub fn apply_reduction(
    verified: &VerifiedAttempt,
    draft: &ReductionDraft,
    criticism: &CriticismReport,
) -> RefineResult<AppliedReduction> {
    let opened = &verified.opened;
    let agent_exhausted =
        opened.budgets.agent.limit != 0 && opened.budgets.agent.limit < AGENT_CALLS_PER_ATTEMPT;

    let mut accepted: Vec<KnowledgeAssertion> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    let mut deferred: Vec<String> = Vec::new();
    let mut contested: Vec<String> = Vec::new();
    let mut dissent: Vec<String> = Vec::new();
    let mut decision_requests: Vec<DecisionRequestDraft> = Vec::new();
    let mut artifact_refs: Vec<ArtifactRef> = parent_artifact_refs(opened);

    let verdict_for = |target: &str| -> Option<CriticismVerdictEntry> {
        criticism
            .verdicts
            .iter()
            .find(|entry| entry.target == target)
            .cloned()
    };
    let finding_by_ref = |reference: &str| -> Option<&VerifiedFinding> {
        verified
            .findings
            .iter()
            .find(|finding| finding.finding_ref == reference)
    };
    let spec_links = spec_links_of(&opened.mission);
    let load_bearing = |assertion: &KnowledgeAssertion| -> bool {
        assertion
            .scope_refs
            .iter()
            .any(|link| spec_links.contains(link))
    };

    // Track which findings the draft addressed, so untouched tier-2-verified
    // findings auto-promote.
    let mut addressed: BTreeSet<String> = BTreeSet::new();
    for rejection in &draft.rejects {
        addressed.insert(rejection.finding_ref.clone());
    }

    if agent_exhausted {
        // Publish partial: nothing is consumed, everything carries.
        for finding in &verified.findings {
            deferred.push(finding.finding_ref.clone());
        }
        for rejection in &verified.envelope_rejections {
            deferred.push(rejection.finding_ref.clone());
        }
        return assemble(
            opened,
            verified,
            accepted,
            rejected,
            deferred,
            contested,
            dissent,
            rank_decisions(decision_requests, opened),
            artifact_refs,
            criticism_ref_of(criticism),
            true,
        );
    }

    // Tier-1 envelope rejections are durable diagnostics.
    for rejection in &verified.envelope_rejections {
        rejected.push(format!("{}: {}", rejection.finding_ref, rejection.reason));
    }

    // Drafted rejections: a criticism that contests the rejection means it
    // discarded evidence rather than reasoning, so the finding defers
    // instead of dying.
    for rejection in &draft.rejects {
        if finding_by_ref(&rejection.finding_ref).is_none() {
            return Err(RefineError::InvalidInput(format!(
                "drafted rejection targets unknown finding {}",
                rejection.finding_ref
            )));
        }
        match verdict_for(&rejection.finding_ref) {
            Some(entry) if entry.verdict == CriticismVerdict::Contested => {
                deferred.push(rejection.finding_ref.clone());
                dissent.push(format!(
                    "{}: rejection contested: {}",
                    rejection.finding_ref, entry.note
                ));
            }
            _ => rejected.push(format!("{}: {}", rejection.finding_ref, rejection.reason)),
        }
    }

    // Drafted acceptances under the auto-promotion policy.
    let mut draft_ids = BTreeSet::new();
    for drafted in &draft.accepts {
        if !draft_ids.insert(drafted.draft_id.clone()) {
            return Err(RefineError::InvalidInput(format!(
                "duplicate draft id {}",
                drafted.draft_id
            )));
        }
        let mut assertion = drafted.assertion.clone();
        match assertion.authority {
            ArtifactAuthority::Decision | ArtifactAuthority::Directive => {
                // Never auto: the human gate owns the promotion.
                decision_requests.push(DecisionRequestDraft {
                    summary: format!(
                        "promote assertion to {} authority: {}",
                        assertion.authority.as_str(),
                        assertion.scope.as_deref().unwrap_or("unnamed")
                    ),
                    choices: vec!["approve".to_string(), "reject".to_string()],
                    load_bearing: load_bearing(&assertion),
                });
                continue;
            }
            ArtifactAuthority::Model => {
                if assertion.evidence_refs.is_empty() && drafted.evidence_coverage.is_empty() {
                    deferred.push(format!(
                        "{}: model assertion lacks source coverage",
                        drafted.draft_id
                    ));
                    continue;
                }
                if drafted.unverified_extent {
                    assertion
                        .qualified
                        .get_or_insert_with(|| "unverified_extent".to_string());
                }
            }
            ArtifactAuthority::Evidence => {}
        }
        // The criticism gate: only confirmed acceptances publish.
        match verdict_for(&drafted.draft_id) {
            Some(entry) if entry.verdict == CriticismVerdict::Confirmed => {}
            Some(entry) => {
                contested.push(drafted.draft_id.clone());
                dissent.push(format!(
                    "{}: {}: {}",
                    drafted.draft_id,
                    match entry.verdict {
                        CriticismVerdict::Contested => "contested",
                        CriticismVerdict::InsufficientEvidence => "insufficient evidence",
                        CriticismVerdict::Confirmed => "confirmed",
                    },
                    entry.note
                ));
                if load_bearing(&assertion) {
                    decision_requests.push(DecisionRequestDraft {
                        summary: format!(
                            "contested load-bearing acceptance: {}",
                            assertion.scope.as_deref().unwrap_or("unnamed")
                        ),
                        choices: vec!["accept".to_string(), "reject".to_string()],
                        load_bearing: true,
                    });
                }
                continue;
            }
            None => {
                // No verdict: insufficient independent confirmation.
                contested.push(drafted.draft_id.clone());
                continue;
            }
        }
        if let Some(basis) = assertion.provenance.as_deref()
            && let Some(finding) = finding_by_ref(basis)
        {
            addressed.insert(finding.finding_ref.clone());
        }
        for coverage in &drafted.evidence_coverage {
            if let Some(finding) = finding_by_ref(coverage) {
                addressed.insert(finding.finding_ref.clone());
            }
        }
        accepted.push(assertion);
    }

    // Auto-promotion and deferral for findings the draft left untouched.
    for finding in &verified.findings {
        if addressed.contains(&finding.finding_ref) {
            continue;
        }
        match finding.verification {
            FindingVerification::Verified => {
                accepted.push(auto_promoted_assertion(opened, finding)?);
            }
            FindingVerification::Unverified => deferred.push(format!(
                "{}: verifier failure routes to judgment",
                finding.finding_ref
            )),
            FindingVerification::NotVerifiable => deferred.push(format!(
                "{}: no registered verifier applies; carries to judgment",
                finding.finding_ref
            )),
        }
    }

    // Artifact promotions.
    for promotion in &draft.artifact_promotions {
        if !verified
            .candidates
            .iter()
            .any(|candidate| candidate.candidate_ref == promotion.candidate_ref)
        {
            return Err(RefineError::InvalidInput(format!(
                "artifact promotion targets unknown candidate {}",
                promotion.candidate_ref
            )));
        }
        let digest = promotion.artifact.sha256.clone().unwrap_or_default();
        if !opened.verification.matched_digests.contains(&digest) {
            deferred.push(format!(
                "{}: staged bytes did not match digest {}",
                promotion.candidate_ref, digest
            ));
            continue;
        }
        match promotion.artifact.authority {
            ArtifactAuthority::Decision | ArtifactAuthority::Directive => {
                decision_requests.push(DecisionRequestDraft {
                    summary: format!(
                        "promote artifact {} to {} authority",
                        promotion.artifact.key,
                        promotion.artifact.authority.as_str()
                    ),
                    choices: vec!["approve".to_string(), "reject".to_string()],
                    load_bearing: spec_links.contains(&promotion.artifact.key),
                });
            }
            ArtifactAuthority::Model if promotion.artifact.applicability.is_none() => {
                deferred.push(format!(
                    "{}: model artifact lacks applicability",
                    promotion.candidate_ref
                ));
            }
            _ => {
                // A newer snapshot selects the newer file: promotion of the
                // same key replaces the parent's selection.
                artifact_refs.retain(|existing| existing.key != promotion.artifact.key);
                artifact_refs.push(promotion.artifact.clone());
            }
        }
    }

    // Contradictions: members must exist, and resolution needs a
    // machine-checkable basis; otherwise the contradiction stays open and
    // escalates when it touches accepted assertions.
    let ledger_ids = ledger_assertion_ids(opened);
    let mut accepted_ids: BTreeSet<String> = accepted
        .iter()
        .map(|assertion| assertion.assertion_id.clone())
        .collect();
    for drafted in &draft.contradictions {
        if drafted.members.len() < 2 {
            return Err(RefineError::InvalidInput(
                "a contradiction needs at least two members".to_string(),
            ));
        }
        for member in &drafted.members {
            if !ledger_ids.contains(member) && !accepted_ids.contains(member) {
                return Err(RefineError::InvalidInput(format!(
                    "contradiction member {member} is not a known assertion"
                )));
            }
        }
        let proposed = drafted.resolution.unwrap_or(ContradictionResolution::Open);
        let basis_ok = proposed == ContradictionResolution::Open
            || drafted.resolution_basis.as_deref().is_some_and(|basis| {
                accepted_ids.contains(basis)
                    || ledger_ids.contains(basis)
                    || finding_by_ref(basis).is_some_and(|finding| {
                        finding.verification == FindingVerification::Verified
                    })
            });
        let resolution = if basis_ok {
            proposed
        } else {
            ContradictionResolution::Open
        };
        let mut contradiction = KnowledgeAssertion {
            assertion_id: String::new(),
            kind: AssertionKind::Contradiction,
            authority: ArtifactAuthority::Evidence,
            provenance: None,
            qualified: None,
            supersedes: Vec::new(),
            corrects: Vec::new(),
            derived_from: Vec::new(),
            scope: None,
            scope_refs: Vec::new(),
            evidence_refs: Vec::new(),
            supersedable: true,
            members: drafted.members.clone(),
            resolution: Some(resolution),
            resolved_by: drafted.resolution_basis.clone(),
        };
        contradiction.assertion_id = deterministic_assertion_id(opened, &contradiction)?;
        if resolution == ContradictionResolution::Open
            && drafted
                .members
                .iter()
                .any(|member| accepted_ids.contains(member) || ledger_ids.contains(member))
        {
            decision_requests.push(DecisionRequestDraft {
                summary: format!("open contradiction between {}", drafted.members.join(", ")),
                choices: vec![
                    "accept-first".to_string(),
                    "accept-second".to_string(),
                    "reject-both".to_string(),
                ],
                load_bearing: true,
            });
        }
        accepted_ids.insert(contradiction.assertion_id.clone());
        accepted.push(contradiction);
    }

    assemble(
        opened,
        verified,
        accepted,
        rejected,
        deferred,
        contested,
        dissent,
        rank_decisions(decision_requests, opened),
        artifact_refs,
        criticism_ref_of(criticism),
        false,
    )
}

/// Rank and batch decision requests: load-bearing first, then draft order.
/// The budget shapes presentation, never truth: load-bearing requests cannot
/// be deferred.
fn rank_decisions(
    drafts: Vec<DecisionRequestDraft>,
    opened: &OpenedAttempt,
) -> Vec<DecisionRequest> {
    let mut ranked: Vec<(usize, DecisionRequestDraft)> =
        drafts.into_iter().enumerate().collect::<Vec<_>>();
    ranked.sort_by(|a, b| match (a.1.load_bearing, b.1.load_bearing) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });
    let limit = opened.budgets.decision.limit;
    ranked
        .into_iter()
        .enumerate()
        .map(|(rank, (position, request))| {
            let over_budget = limit != 0 && rank >= limit;
            DecisionRequest {
                id: format!("d-{}", position + 1),
                group: opened.wave.map(|wave| format!("wave-{wave}")),
                summary: request.summary,
                choices: request.choices,
                load_bearing: request.load_bearing,
                rank: rank + 1,
                deferred: over_budget && !request.load_bearing,
            }
        })
        .collect()
}

/// Build the next snapshot and receipt from the revised reduction.
#[allow(clippy::too_many_arguments)]
fn assemble(
    opened: &OpenedAttempt,
    verified: &VerifiedAttempt,
    mut accepted: Vec<KnowledgeAssertion>,
    rejected: Vec<String>,
    deferred: Vec<String>,
    contested: Vec<String>,
    dissent: Vec<String>,
    decision_requests: Vec<DecisionRequest>,
    artifact_refs: Vec<ArtifactRef>,
    criticism_ref: Option<String>,
    partial: bool,
) -> RefineResult<AppliedReduction> {
    // Assign deterministic ids and fail closed on any collision with the
    // existing ledger.
    let mut ledger_ids = ledger_assertion_ids(opened);
    for assertion in &mut accepted {
        if assertion.assertion_id.is_empty() {
            assertion.assertion_id = deterministic_assertion_id(opened, assertion)?;
        }
        if !ledger_ids.insert(assertion.assertion_id.clone()) {
            return Err(RefineError::InvalidInput(format!(
                "assertion id {} collides with the existing ledger",
                assertion.assertion_id
            )));
        }
    }

    let version = opened.parent_snapshot + 1;
    let claim_set: Vec<String> = opened
        .claims
        .iter()
        .map(|claim| claim.digest.clone())
        .collect();
    let current_round = opened.mission.current_round.unwrap_or(0);
    let plan_digest = opened
        .mission
        .rounds
        .iter()
        .find(|round| round.number == current_round)
        .and_then(|round| round.plan.as_ref())
        .and_then(|plan| plan.effective_digest.clone());
    let mut snapshot = MissionSnapshot {
        version,
        parent_version: Some(opened.parent_snapshot),
        target_head: opened.verification.target_head.clone(),
        plan_digest: plan_digest.clone(),
        artifact_refs,
        input_refs: parent_snapshot_of(opened)
            .map(|snapshot| snapshot.input_refs.clone())
            .unwrap_or_default(),
        consumed_contribution_refs: claim_set.clone(),
        knowledge_index: accepted,
        corrects_snapshot: None,
        digest: None,
        created: String::new(),
    };
    snapshot.digest = Some(compute_snapshot_digest(&snapshot));

    let decision_volume = decision_requests.len();
    let plan_quality = if decision_volume > opened.decision_volume_threshold {
        Some(format!(
            "decision volume {decision_volume} exceeds threshold {}; review the wave decomposition",
            opened.decision_volume_threshold
        ))
    } else {
        None
    };
    let budgets = ReconciliationBudgets {
        repair: opened.budgets.repair,
        agent: BudgetReport {
            limit: opened.budgets.agent.limit,
            used: if partial {
                opened.budgets.agent.limit.min(AGENT_CALLS_PER_ATTEMPT)
            } else {
                AGENT_CALLS_PER_ATTEMPT
            },
        },
        decision: BudgetReport {
            limit: opened.budgets.decision.limit,
            used: decision_volume,
        },
        publication: opened.budgets.publication,
    };
    let receipt = ReconciliationReceipt {
        attempt: opened.attempt_id.clone(),
        parent_snapshot: opened.parent_snapshot,
        next_snapshot: version,
        wave: opened.wave,
        claim_set,
        verifier_results: verified.verifier_results.clone(),
        accepted: snapshot
            .knowledge_index
            .iter()
            .map(|assertion| assertion.assertion_id.clone())
            .collect(),
        rejected,
        deferred,
        contested,
        dissent,
        criticism_ref,
        decision_requests,
        budgets,
        plan_quality,
        correction: None,
        created: String::new(),
    };
    Ok(AppliedReduction {
        attempt_id: opened.attempt_id.clone(),
        wave: opened.wave,
        parent_snapshot: opened.parent_snapshot,
        snapshot,
        receipt,
    })
}

/// The provenance of a correction snapshot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionProvenance {
    ReductionError,
    SourceInvalidated,
    ChallengeAccepted,
}

impl CorrectionProvenance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReductionError => "reduction_error",
            Self::SourceInvalidated => "source_invalidated",
            Self::ChallengeAccepted => "challenge_accepted",
        }
    }
}

/// Why a correction snapshot is being appended.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CorrectionMandate {
    pub provenance: CorrectionProvenance,
    pub reason: String,
    /// Correcting assertions; each carries `supersedes`/`corrects` ids.
    pub corrections: Vec<DraftedAssertion>,
}

/// Apply a correction mandate: an ordinary reconciliation attempt with a
/// small or empty contribution set and a correction mandate. Because
/// snapshots are immutable, reduction errors are repaired by appending,
/// never editing.
///
/// The criticism gate applies to corrections exactly as it does to wave
/// reconciliations; the caller supplies the criticism report of the
/// correction attempt.
pub fn apply_correction(
    mission: &Mission,
    mandate: &CorrectionMandate,
    criticism: &CriticismReport,
    verification: &VerificationContext,
    budgets: ReconciliationBudgets,
) -> RefineResult<AppliedReduction> {
    let input = ReconciliationInput {
        wave: None,
        claims: Vec::new(),
        verification: verification.clone(),
        budgets,
        decision_volume_threshold: None,
        correction: Some(format!(
            "{}: {}",
            mandate.provenance.as_str(),
            mandate.reason
        )),
    };
    let opened = open_attempt(mission, &input)?;
    let verified = verify_claims(&opened);
    let draft = ReductionDraft {
        accepts: mandate.corrections.clone(),
        ..Default::default()
    };
    let mut applied = apply_reduction(&verified, &draft, criticism)?;
    applied.snapshot.corrects_snapshot = Some(applied.parent_snapshot);
    applied.receipt.correction = Some(format!(
        "{}: {}",
        mandate.provenance.as_str(),
        mandate.reason
    ));
    Ok(applied)
}

fn auto_promoted_assertion(
    opened: &OpenedAttempt,
    finding: &VerifiedFinding,
) -> RefineResult<KnowledgeAssertion> {
    let mut assertion = KnowledgeAssertion {
        assertion_id: String::new(),
        kind: AssertionKind::Fact,
        authority: ArtifactAuthority::Evidence,
        provenance: Some(format!(
            "contribution:{}/{}",
            finding.claim.goal_id, finding.claim.goal_round
        )),
        qualified: None,
        supersedes: Vec::new(),
        corrects: Vec::new(),
        derived_from: Vec::new(),
        scope: Some(finding.claim_text.clone()),
        scope_refs: spec_scope_refs(&opened.mission, &finding.claim.mission_goal_key),
        evidence_refs: finding.evidence.clone(),
        supersedable: true,
        members: Vec::new(),
        resolution: None,
        resolved_by: None,
    };
    if finding.claim.in_review {
        assertion
            .qualified
            .get_or_insert_with(|| "goal_review_pending".to_string());
    }
    assertion.assertion_id = deterministic_assertion_id(opened, &assertion)?;
    Ok(assertion)
}

fn spec_scope_refs(mission: &Mission, mission_goal_key: &str) -> Vec<String> {
    let mut refs = vec![mission_goal_key.to_string()];
    for spec in mission
        .rounds
        .iter()
        .flat_map(|round| round.plan.iter())
        .flat_map(|plan| plan.waves.iter())
        .flat_map(|wave| wave.goal_specs.iter())
        .filter(|spec| spec.mission_goal_key == mission_goal_key)
    {
        refs.extend(spec.criterion_ids.iter().cloned());
    }
    refs.sort();
    refs.dedup();
    refs
}

fn spec_links_of(mission: &Mission) -> BTreeSet<String> {
    mission
        .rounds
        .iter()
        .flat_map(|round| round.plan.iter())
        .flat_map(|plan| plan.waves.iter())
        .flat_map(|wave| wave.goal_specs.iter())
        .flat_map(|spec| {
            std::iter::once(spec.mission_goal_key.clone())
                .chain(spec.criterion_ids.iter().cloned())
                .chain(spec.input_artifact_keys.iter().cloned())
                .chain(spec.output_artifact_keys.iter().cloned())
        })
        .collect()
}

fn ledger_assertion_ids(opened: &OpenedAttempt) -> BTreeSet<String> {
    opened
        .mission
        .rounds
        .iter()
        .flat_map(|round| round.snapshots.iter())
        .flat_map(|snapshot| snapshot.knowledge_index.iter())
        .map(|assertion| assertion.assertion_id.clone())
        .collect()
}

fn parent_snapshot_of(opened: &OpenedAttempt) -> Option<&MissionSnapshot> {
    let current_round = opened.mission.current_round.unwrap_or(0);
    opened
        .mission
        .rounds
        .iter()
        .find(|round| round.number == current_round)
        .and_then(|round| round.snapshots.last())
}

fn parent_artifact_refs(opened: &OpenedAttempt) -> Vec<ArtifactRef> {
    parent_snapshot_of(opened)
        .map(|snapshot| snapshot.artifact_refs.clone())
        .unwrap_or_default()
}

fn criticism_ref_of(criticism: &CriticismReport) -> Option<String> {
    if criticism.verdicts.is_empty() && criticism.notes.is_empty() {
        None
    } else {
        let encoded = serde_json::to_vec(criticism).unwrap_or_default();
        Some(format!("sha256:{}", hex_digest(&encoded)))
    }
}

/// Deterministic assertion identity: a content hash of the assertion, the
/// Mission, and the parent snapshot. The same claim set, parent snapshot,
/// target head, and plan digest yield identical ids.
fn deterministic_assertion_id(
    opened: &OpenedAttempt,
    assertion: &KnowledgeAssertion,
) -> RefineResult<String> {
    deterministic_assertion_id_for(
        &opened.mission.id,
        opened.mission.current_round.unwrap_or(0),
        opened.parent_snapshot,
        assertion,
    )
}

/// The standalone deterministic assertion identity used outside a fenced
/// reconciliation attempt (for example the investigation snapshot), so ids
/// derive the same way everywhere: content, Mission, Round, and parent
/// snapshot.
pub fn deterministic_assertion_id_for(
    mission_id: &str,
    round: usize,
    parent_snapshot: usize,
    assertion: &KnowledgeAssertion,
) -> RefineResult<String> {
    let mut value = serde_json::to_value(assertion).map_err(|error| {
        RefineError::Serialization(format!("failed to encode assertion: {error}"))
    })?;
    if let Some(object) = value.as_object_mut() {
        object.insert("assertion_id".to_string(), Value::String(String::new()));
        object.insert("mission".to_string(), Value::String(mission_id.to_string()));
        object.insert("round".to_string(), Value::from(round));
        object.insert("parent_snapshot".to_string(), Value::from(parent_snapshot));
    }
    Ok(format!(
        "a{}",
        &hex_digest(value.to_string().as_bytes())[..24]
    ))
}

/// The snapshot digest: sha256 over the canonical JSON of the snapshot with
/// the digest and creation timestamp excluded, so the digest is a
/// deterministic function of the accepted content.
pub fn compute_snapshot_digest(snapshot: &MissionSnapshot) -> String {
    let mut value = serde_json::to_value(snapshot).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.insert("digest".to_string(), Value::Null);
        object.insert("created".to_string(), Value::String(String::new()));
    }
    format!("sha256:{}", hex_digest(value.to_string().as_bytes()))
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A bare sha256 digest over arbitrary canonical bytes, used for
/// contribution digests and other content identities outside the snapshot
/// shape.
pub fn compute_snapshot_digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mission::{
        ArtifactCandidate, ArtifactObligation, Finding, MissionCriterion, MissionGoalSpec,
        MissionPlan, MissionRound, MissionRoundRequest, MissionWave,
    };

    fn verification_context() -> VerificationContext {
        let mut context = VerificationContext {
            target_head: Some("head1".to_string()),
            ..Default::default()
        };
        context.reachable_commits.insert("c1".to_string());
        context.matched_digests.insert("b".repeat(64));
        context
    }

    fn test_mission() -> Mission {
        let mut mission = Mission {
            id: "MTEST".to_string(),
            name: "Test".to_string(),
            intent: "Modernize auth".to_string(),
            status: crate::model::mission::MissionStatus::Execute,
            reporter: None,
            assignee: None,
            coordinator_node_id: None,
            success_criteria: vec![MissionCriterion {
                id: "crit:tokens".to_string(),
                description: "tokens work".to_string(),
            }],
            artifact_contract: vec![],
            current_round: Some(1),
            revision: 3,
            rounds: Vec::new(),
            created: String::new(),
            updated: String::new(),
        };
        let round = MissionRound {
            number: 1,
            request: MissionRoundRequest {
                intent: "intent".to_string(),
                constraints: vec![],
                criteria: mission.success_criteria.clone(),
                artifact_obligations: vec![ArtifactObligation {
                    key: "interface-contract".to_string(),
                    kind: "contract".to_string(),
                    purpose: "pin the auth interface".to_string(),
                    required: true,
                    validation_policy: None,
                    consumers: vec![],
                }],
                authorizing_request: "go".to_string(),
                charter_digest: None,
            },
            input_bindings: vec![],
            plan: Some(MissionPlan {
                charter_digest: None,
                summary: "summary".to_string(),
                assumptions: vec![],
                risks: vec![],
                criteria_coverage: vec![],
                waves: vec![MissionWave {
                    number: 1,
                    purpose: "wave 1".to_string(),
                    goal_specs: vec![MissionGoalSpec {
                        mission_goal_key: "k1".to_string(),
                        name: "Goal".to_string(),
                        prompt: "prompt".to_string(),
                        role: None,
                        required: true,
                        criterion_ids: vec!["crit:tokens".to_string()],
                        input_artifact_keys: vec![],
                        output_artifact_keys: vec!["interface-contract".to_string()],
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
            }),
            plan_amendments: vec![],
            snapshots: vec![MissionSnapshot {
                version: 1,
                parent_version: None,
                target_head: Some("head0".to_string()),
                plan_digest: Some("plan-digest-1".to_string()),
                artifact_refs: vec![],
                input_refs: vec![],
                consumed_contribution_refs: vec![],
                knowledge_index: vec![],
                corrects_snapshot: None,
                digest: None,
                created: String::new(),
            }],
            reconciliation_receipts: vec![],
            phase_evidence: Default::default(),
            review: None,
            outcome: None,
            outcome_publication: None,
            failure: None,
            created: String::new(),
            updated: String::new(),
        };
        mission.rounds.push(round);
        mission
    }

    fn claim(digest: &str, finding: Finding) -> ClaimedContribution {
        claim_from("G1", digest, finding)
    }

    fn claim_from(goal_id: &str, digest: &str, finding: Finding) -> ClaimedContribution {
        ClaimedContribution {
            goal_id: goal_id.to_string(),
            goal_round: 1,
            mission_goal_key: "k1".to_string(),
            digest: digest.to_string(),
            eligible: true,
            in_review: true,
            contribution: GoalContribution {
                bound_context_digest: None,
                criteria_evidence: vec![],
                findings: vec![finding],
                challenged_assumptions: vec![],
                artifact_candidates: vec![],
                suggested_followups: vec![],
                downstream_invalidations: vec![],
                digest: Some(digest.to_string()),
            },
        }
    }

    fn input(claims: Vec<ClaimedContribution>) -> ReconciliationInput {
        ReconciliationInput {
            wave: Some(1),
            claims,
            verification: verification_context(),
            budgets: ReconciliationBudgets::default(),
            decision_volume_threshold: None,
            correction: None,
        }
    }

    fn open(mission: &Mission, input: &ReconciliationInput) -> OpenedAttempt {
        open_attempt(mission, input).unwrap()
    }

    fn empty_assertion(kind: AssertionKind, authority: ArtifactAuthority) -> KnowledgeAssertion {
        KnowledgeAssertion {
            assertion_id: String::new(),
            kind,
            authority,
            provenance: None,
            qualified: None,
            supersedes: vec![],
            corrects: vec![],
            derived_from: vec![],
            scope: None,
            scope_refs: vec![],
            evidence_refs: vec![],
            supersedable: true,
            members: vec![],
            resolution: None,
            resolved_by: None,
        }
    }

    #[test]
    fn attempt_identity_includes_wave_and_attempt_number() {
        let mission = test_mission();
        assert_eq!(
            next_attempt_id(&mission, Some(1)),
            "mission:MTEST:round:1:reconcile:1:1"
        );
        assert_eq!(
            next_attempt_id(&mission, None),
            "mission:MTEST:round:1:reconcile:sweep:1"
        );
    }

    #[test]
    fn ineligible_and_duplicate_claims_fail_closed() {
        let mission = test_mission();
        let mut ineligible = claim(
            "d1",
            Finding {
                claim: "fact".to_string(),
                evidence: vec![],
            },
        );
        ineligible.eligible = false;
        let err = open_attempt(&mission, &input(vec![ineligible])).unwrap_err();
        assert!(err.to_string().contains("not eligible"));

        let ok = claim(
            "d1",
            Finding {
                claim: "fact".to_string(),
                evidence: vec![],
            },
        );
        let dup = claim(
            "d1",
            Finding {
                claim: "fact".to_string(),
                evidence: vec![],
            },
        );
        let err = open_attempt(&mission, &input(vec![ok, dup])).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn verified_findings_auto_promote_as_evidence() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![claim(
                "d1",
                Finding {
                    claim: "token invariant exists".to_string(),
                    evidence: vec!["commit:c1".to_string()],
                },
            )]),
        );
        let verified = verify_claims(&opened);
        assert_eq!(verified.findings.len(), 1);
        assert_eq!(
            verified.findings[0].verification,
            FindingVerification::Verified
        );

        let applied = apply_reduction(
            &verified,
            &ReductionDraft::default(),
            &CriticismReport::default(),
        )
        .unwrap();
        assert_eq!(applied.snapshot.knowledge_index.len(), 1);
        let assertion = &applied.snapshot.knowledge_index[0];
        assert_eq!(assertion.kind, AssertionKind::Fact);
        assert_eq!(assertion.authority, ArtifactAuthority::Evidence);
        assert_eq!(assertion.qualified.as_deref(), Some("goal_review_pending"));
        assert!(assertion.assertion_id.starts_with('a'));
        assert!(assertion.scope_refs.contains(&"k1".to_string()));
        assert_eq!(applied.receipt.accepted.len(), 1);
        assert_eq!(applied.receipt.claim_set, vec!["d1".to_string()]);
        assert_eq!(applied.snapshot.version, 2);
        assert_eq!(applied.snapshot.parent_version, Some(1));
        assert!(applied.snapshot.digest.is_some());
    }

    #[test]
    fn deterministic_ids_and_digests_are_stable() {
        let mission = test_mission();
        let run = || {
            let opened = open(
                &mission,
                &input(vec![claim(
                    "d1",
                    Finding {
                        claim: "token invariant exists".to_string(),
                        evidence: vec!["commit:c1".to_string()],
                    },
                )]),
            );
            let verified = verify_claims(&opened);
            apply_reduction(
                &verified,
                &ReductionDraft::default(),
                &CriticismReport::default(),
            )
            .unwrap()
        };
        let first = run();
        let second = run();
        assert_eq!(
            first.snapshot.knowledge_index[0].assertion_id,
            second.snapshot.knowledge_index[0].assertion_id
        );
        assert_eq!(first.snapshot.digest, second.snapshot.digest);
        assert_eq!(
            first.receipt.verifier_results,
            second.receipt.verifier_results
        );
    }

    #[test]
    fn unverified_findings_defer_to_judgment() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![claim(
                "d1",
                Finding {
                    claim: "no callers remain".to_string(),
                    evidence: vec!["commit:gone".to_string()],
                },
            )]),
        );
        let verified = verify_claims(&opened);
        let applied = apply_reduction(
            &verified,
            &ReductionDraft::default(),
            &CriticismReport::default(),
        )
        .unwrap();
        assert!(applied.snapshot.knowledge_index.is_empty());
        assert_eq!(applied.receipt.deferred.len(), 1);
        assert!(applied.receipt.deferred[0].contains("verifier failure"));
    }

    #[test]
    fn criticism_blocks_unconfirmed_acceptances_and_preserves_dissent() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![claim(
                "d1",
                Finding {
                    claim: "the model holds".to_string(),
                    evidence: vec![],
                },
            )]),
        );
        let verified = verify_claims(&opened);
        let mut drafted = empty_assertion(AssertionKind::Model, ArtifactAuthority::Model);
        drafted.scope = Some("the auth model".to_string());
        drafted.scope_refs = vec!["crit:tokens".to_string()];
        drafted.evidence_refs = vec!["commit:c1".to_string()];
        let draft = ReductionDraft {
            accepts: vec![DraftedAssertion {
                draft_id: "d1".to_string(),
                assertion: drafted,
                evidence_coverage: vec![],
                unverified_extent: false,
            }],
            ..Default::default()
        };
        let criticism = CriticismReport {
            verdicts: vec![CriticismVerdictEntry {
                target: "d1".to_string(),
                verdict: CriticismVerdict::Contested,
                note: "counter-case: token refresh path".to_string(),
            }],
            notes: "I could not confirm the refresh path".to_string(),
        };
        let applied = apply_reduction(&verified, &draft, &criticism).unwrap();
        assert!(applied.snapshot.knowledge_index.is_empty());
        assert_eq!(applied.receipt.contested, vec!["d1".to_string()]);
        assert_eq!(applied.receipt.dissent.len(), 1);
        assert!(applied.receipt.dissent[0].contains("token refresh path"));
        // Load-bearing contested acceptance raises a blocking decision.
        assert_eq!(applied.receipt.decision_requests.len(), 1);
        assert!(applied.receipt.decision_requests[0].load_bearing);
        assert!(applied.receipt.criticism_ref.is_some());
    }

    #[test]
    fn confirmed_model_acceptance_publishes_with_coverage() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![claim(
                "d1",
                Finding {
                    claim: "the model holds".to_string(),
                    evidence: vec![],
                },
            )]),
        );
        let verified = verify_claims(&opened);
        let mut drafted = empty_assertion(AssertionKind::Model, ArtifactAuthority::Model);
        drafted.evidence_refs = vec!["commit:c1".to_string()];
        let draft = ReductionDraft {
            accepts: vec![DraftedAssertion {
                draft_id: "d1".to_string(),
                assertion: drafted,
                evidence_coverage: vec![],
                unverified_extent: true,
            }],
            ..Default::default()
        };
        let criticism = CriticismReport {
            verdicts: vec![CriticismVerdictEntry {
                target: "d1".to_string(),
                verdict: CriticismVerdict::Confirmed,
                note: "checked".to_string(),
            }],
            notes: String::new(),
        };
        let applied = apply_reduction(&verified, &draft, &criticism).unwrap();
        assert_eq!(applied.snapshot.knowledge_index.len(), 1);
        assert_eq!(
            applied.snapshot.knowledge_index[0].qualified.as_deref(),
            Some("unverified_extent")
        );
    }

    #[test]
    fn model_without_source_coverage_defers() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![claim(
                "d1",
                Finding {
                    claim: "x".to_string(),
                    evidence: vec![],
                },
            )]),
        );
        let verified = verify_claims(&opened);
        let drafted = empty_assertion(AssertionKind::Model, ArtifactAuthority::Model);
        let draft = ReductionDraft {
            accepts: vec![DraftedAssertion {
                draft_id: "d1".to_string(),
                assertion: drafted,
                evidence_coverage: vec![],
                unverified_extent: false,
            }],
            ..Default::default()
        };
        let applied = apply_reduction(&verified, &draft, &CriticismReport::default()).unwrap();
        assert!(applied.snapshot.knowledge_index.is_empty());
        // The model defers for missing coverage and the unaddressed finding
        // carries to judgment: nothing is discarded.
        assert_eq!(applied.receipt.deferred.len(), 2);
        assert!(
            applied
                .receipt
                .deferred
                .iter()
                .any(|entry| entry.contains("source coverage"))
        );
    }

    #[test]
    fn decision_and_directive_authority_never_auto_publish() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![claim(
                "d1",
                Finding {
                    claim: "we should adopt oauth".to_string(),
                    evidence: vec!["commit:c1".to_string()],
                },
            )]),
        );
        let verified = verify_claims(&opened);
        let drafted = |authority: ArtifactAuthority| {
            let mut assertion = empty_assertion(AssertionKind::Fact, authority);
            assertion.scope = Some("adopt oauth".to_string());
            assertion.evidence_refs = vec!["commit:c1".to_string()];
            DraftedAssertion {
                draft_id: "d1".to_string(),
                assertion,
                evidence_coverage: vec![],
                unverified_extent: false,
            }
        };
        for authority in [ArtifactAuthority::Decision, ArtifactAuthority::Directive] {
            let draft = ReductionDraft {
                accepts: vec![drafted(authority)],
                ..Default::default()
            };
            let criticism = CriticismReport {
                verdicts: vec![CriticismVerdictEntry {
                    target: "d1".to_string(),
                    verdict: CriticismVerdict::Confirmed,
                    note: String::new(),
                }],
                notes: String::new(),
            };
            let applied = apply_reduction(&verified, &draft, &criticism).unwrap();
            // No Decision or Directive authority entered the ledger.
            assert!(applied.snapshot.knowledge_index.iter().all(|assertion| {
                assertion.authority != ArtifactAuthority::Decision
                    && assertion.authority != ArtifactAuthority::Directive
            }));
            assert_eq!(applied.receipt.decision_requests.len(), 1);
        }
    }

    #[test]
    fn decision_budget_defers_ranked_non_load_bearing_requests() {
        let mission = test_mission();
        let mut input = input(vec![claim(
            "d1",
            Finding {
                claim: "x".to_string(),
                evidence: vec![],
            },
        )]);
        input.budgets.decision.limit = 1;
        let opened = open(&mission, &input);
        let verified = verify_claims(&opened);
        let drafted = |id: &str, load_bearing: bool| {
            let mut assertion = empty_assertion(AssertionKind::Fact, ArtifactAuthority::Decision);
            assertion.scope = Some(format!("choice {id}"));
            if load_bearing {
                assertion.scope_refs = vec!["crit:tokens".to_string()];
            }
            DraftedAssertion {
                draft_id: id.to_string(),
                assertion,
                evidence_coverage: vec![],
                unverified_extent: false,
            }
        };
        let draft = ReductionDraft {
            accepts: vec![
                drafted("d1", false),
                drafted("d2", true),
                drafted("d3", false),
            ],
            ..Default::default()
        };
        let applied = apply_reduction(&verified, &draft, &CriticismReport::default()).unwrap();
        let requests = &applied.receipt.decision_requests;
        assert_eq!(requests.len(), 3);
        // Load-bearing ranks first and is never deferred.
        assert!(requests[0].load_bearing);
        assert!(!requests[0].deferred);
        assert!(requests.iter().any(|request| request.deferred));
        // Volume above threshold records a plan-quality signal.
        assert!(
            applied
                .receipt
                .plan_quality
                .as_deref()
                .unwrap_or_default()
                .contains("exceeds threshold")
        );
    }

    #[test]
    fn agent_budget_exhaustion_defers_everything_without_loss() {
        let mission = test_mission();
        let mut input = input(vec![claim(
            "d1",
            Finding {
                claim: "fact".to_string(),
                evidence: vec!["commit:c1".to_string()],
            },
        )]);
        input.budgets.agent.limit = 2;
        let opened = open(&mission, &input);
        let verified = verify_claims(&opened);
        let applied = apply_reduction(
            &verified,
            &ReductionDraft::default(),
            &CriticismReport::default(),
        )
        .unwrap();
        assert!(applied.snapshot.knowledge_index.is_empty());
        assert_eq!(
            applied.receipt.deferred,
            vec!["contribution:G1/1/finding/0".to_string()]
        );
        assert_eq!(applied.receipt.budgets.agent.used, 2);
    }

    #[test]
    fn artifact_promotion_requires_matched_digest_and_replaces_key() {
        let mission = test_mission();
        let digest = "b".repeat(64);
        let mut contribution = claim(
            "d1",
            Finding {
                claim: "contract produced".to_string(),
                evidence: vec![],
            },
        );
        contribution.contribution.artifact_candidates = vec![ArtifactCandidate {
            obligation_key: "interface-contract".to_string(),
            kind: "contract".to_string(),
            media_type: Some("text/markdown".to_string()),
            size: 12,
            digest: Some(digest.clone()),
            handoff_ref: None,
            evidence: vec![],
            provenance: None,
            proposed_authority: Some(ArtifactAuthority::Evidence),
        }];
        let opened = open(&mission, &input(vec![contribution]));
        let verified = verify_claims(&opened);
        let artifact = ArtifactRef {
            key: "interface-contract".to_string(),
            title: "Interface contract".to_string(),
            kind: "contract".to_string(),
            authority: ArtifactAuthority::Evidence,
            path: format!("missions/MT/MTEST/artifacts/interface-contract/{digest}.md"),
            media_type: Some("text/markdown".to_string()),
            size: 12,
            sha256: Some(digest.clone()),
            provenance: None,
            applicability: Some("auth surface".to_string()),
        };
        let draft = ReductionDraft {
            artifact_promotions: vec![ArtifactPromotion {
                candidate_ref: "contribution:G1/1/candidate/interface-contract".to_string(),
                artifact,
            }],
            ..Default::default()
        };
        let applied = apply_reduction(&verified, &draft, &CriticismReport::default()).unwrap();
        assert_eq!(applied.snapshot.artifact_refs.len(), 1);
        assert_eq!(applied.snapshot.artifact_refs[0].key, "interface-contract");

        // A digest that never matched defers instead of promoting.
        let mismatch = ArtifactRef {
            sha256: Some("c".repeat(64)),
            ..applied.snapshot.artifact_refs[0].clone()
        };
        let draft = ReductionDraft {
            artifact_promotions: vec![ArtifactPromotion {
                candidate_ref: "contribution:G1/1/candidate/interface-contract".to_string(),
                artifact: mismatch,
            }],
            ..Default::default()
        };
        let deferred = apply_reduction(&verified, &draft, &CriticismReport::default()).unwrap();
        assert!(deferred.snapshot.artifact_refs.is_empty());
        assert!(
            deferred
                .receipt
                .deferred
                .iter()
                .any(|entry| entry.contains("did not match digest"))
        );
    }

    #[test]
    fn contradiction_without_basis_stays_open_and_escalates() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![
                claim_from(
                    "G1",
                    "d1",
                    Finding {
                        claim: "tokens rotate hourly".to_string(),
                        evidence: vec!["commit:c1".to_string()],
                    },
                ),
                claim_from(
                    "G2",
                    "d2",
                    Finding {
                        claim: "tokens never rotate".to_string(),
                        evidence: vec!["commit:c1".to_string()],
                    },
                ),
            ]),
        );
        let verified = verify_claims(&opened);
        // First pass discovers the deterministic ids of the auto-promoted
        // assertions; the second pass contradicts them.
        let first = apply_reduction(
            &verified,
            &ReductionDraft::default(),
            &CriticismReport::default(),
        )
        .unwrap();
        let members: Vec<String> = first
            .snapshot
            .knowledge_index
            .iter()
            .map(|assertion| assertion.assertion_id.clone())
            .collect();
        assert_eq!(members.len(), 2);
        let draft = ReductionDraft {
            contradictions: vec![DraftedContradiction {
                members: members.clone(),
                resolution: Some(ContradictionResolution::Evidence),
                resolution_basis: None,
            }],
            ..Default::default()
        };
        let applied = apply_reduction(&verified, &draft, &CriticismReport::default()).unwrap();
        assert!(
            applied
                .snapshot
                .knowledge_index
                .iter()
                .any(|assertion| assertion.kind == AssertionKind::Contradiction)
        );
        let contradiction = applied
            .snapshot
            .knowledge_index
            .iter()
            .find(|assertion| assertion.kind == AssertionKind::Contradiction)
            .unwrap();
        assert_eq!(
            contradiction.resolution,
            Some(ContradictionResolution::Open)
        );
        assert_eq!(contradiction.members, members);
        // An open contradiction touching accepted assertions escalates.
        assert!(
            applied
                .receipt
                .decision_requests
                .iter()
                .any(|request| request.summary.contains("open contradiction"))
        );
    }

    #[test]
    fn contradiction_with_verified_basis_resolves() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![
                claim_from(
                    "G1",
                    "d1",
                    Finding {
                        claim: "tokens rotate hourly".to_string(),
                        evidence: vec!["commit:c1".to_string()],
                    },
                ),
                claim_from(
                    "G2",
                    "d2",
                    Finding {
                        claim: "tokens never rotate".to_string(),
                        evidence: vec!["commit:c1".to_string()],
                    },
                ),
            ]),
        );
        let verified = verify_claims(&opened);
        let first = apply_reduction(
            &verified,
            &ReductionDraft::default(),
            &CriticismReport::default(),
        )
        .unwrap();
        let members: Vec<String> = first
            .snapshot
            .knowledge_index
            .iter()
            .map(|assertion| assertion.assertion_id.clone())
            .collect();
        let draft = ReductionDraft {
            contradictions: vec![DraftedContradiction {
                members,
                resolution: Some(ContradictionResolution::Evidence),
                resolution_basis: Some("contribution:G1/1/finding/0".to_string()),
            }],
            ..Default::default()
        };
        let applied = apply_reduction(&verified, &draft, &CriticismReport::default()).unwrap();
        let contradiction = applied
            .snapshot
            .knowledge_index
            .iter()
            .find(|assertion| assertion.kind == AssertionKind::Contradiction)
            .unwrap();
        assert_eq!(
            contradiction.resolution,
            Some(ContradictionResolution::Evidence)
        );
        assert!(applied.receipt.decision_requests.is_empty());
    }

    #[test]
    fn unknown_contradiction_members_fail_closed() {
        let mission = test_mission();
        let opened = open(
            &mission,
            &input(vec![claim(
                "d1",
                Finding {
                    claim: "x".to_string(),
                    evidence: vec!["commit:c1".to_string()],
                },
            )]),
        );
        let verified = verify_claims(&opened);
        let draft = ReductionDraft {
            contradictions: vec![DraftedContradiction {
                members: vec!["missing-1".to_string(), "missing-2".to_string()],
                resolution: Some(ContradictionResolution::Open),
                resolution_basis: None,
            }],
            ..Default::default()
        };
        let err = apply_reduction(&verified, &draft, &CriticismReport::default()).unwrap_err();
        assert!(err.to_string().contains("not a known assertion"));
    }

    #[test]
    fn envelope_failures_are_durable_diagnostics() {
        let mission = test_mission();
        let mut contribution = claim(
            "d1",
            Finding {
                claim: "ok".to_string(),
                evidence: vec!["commit:c1".to_string()],
            },
        );
        contribution.contribution.findings.push(Finding {
            claim: "  ".to_string(),
            evidence: vec![],
        });
        let opened = open(&mission, &input(vec![contribution]));
        let verified = verify_claims(&opened);
        assert_eq!(verified.envelope_rejections.len(), 1);
        assert_eq!(verified.findings.len(), 1);
        let applied = apply_reduction(
            &verified,
            &ReductionDraft::default(),
            &CriticismReport::default(),
        )
        .unwrap();
        assert_eq!(applied.receipt.rejected.len(), 1);
        assert!(applied.receipt.rejected[0].contains("empty"));
    }

    #[test]
    fn correction_snapshot_appends_and_marks_what_it_corrects() {
        let mut mission = test_mission();
        let mut seed = empty_assertion(AssertionKind::Fact, ArtifactAuthority::Evidence);
        seed.assertion_id = "a-seed".to_string();
        seed.scope = Some("stale fact".to_string());
        mission.rounds[0].snapshots[0].knowledge_index.push(seed);

        let mut correction = empty_assertion(AssertionKind::Fact, ArtifactAuthority::Evidence);
        correction.corrects = vec!["a-seed".to_string()];
        correction.scope = Some("corrected".to_string());
        let mandate = CorrectionMandate {
            provenance: CorrectionProvenance::SourceInvalidated,
            reason: "Goal G1 left Review".to_string(),
            corrections: vec![DraftedAssertion {
                draft_id: "c1".to_string(),
                assertion: correction,
                evidence_coverage: vec![],
                unverified_extent: false,
            }],
        };
        let criticism = CriticismReport {
            verdicts: vec![CriticismVerdictEntry {
                target: "c1".to_string(),
                verdict: CriticismVerdict::Confirmed,
                note: "verified against the pinned evidence".to_string(),
            }],
            notes: String::new(),
        };
        let applied = apply_correction(
            &mission,
            &mandate,
            &criticism,
            &verification_context(),
            ReconciliationBudgets::default(),
        )
        .unwrap();
        assert_eq!(applied.snapshot.corrects_snapshot, Some(1));
        assert_eq!(applied.snapshot.version, 2);
        assert!(
            applied
                .receipt
                .correction
                .as_deref()
                .unwrap_or_default()
                .contains("source_invalidated")
        );
        assert!(
            applied
                .snapshot
                .knowledge_index
                .iter()
                .any(|assertion| assertion.corrects.contains(&"a-seed".to_string()))
        );
        assert!(applied.receipt.claim_set.is_empty());
    }
}
