//! Reconciliation orchestration: the fenced engine (already deterministic)
//! wired to real agent phases and Goal state.
//!
//! The claim set comes from settled contributions of the wave's Goals. The
//! reduction and adversarial-criticism agents run as one-shot durable
//! operations; the engine — not the agents — assigns ids, enforces the
//! auto-promotion policy, and publishes the next snapshot. See
//! `docs/mission-reconciliation.md` ("The reconciliation loop").

use std::path::Path;

use serde_json::json;

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::agent_io::structured_output::contract::Contract;
use crate::application::missions::agent_phase::{PhaseRun, run_agent_phase};
use crate::application::missions::reconciliation::engine::CriticismReport as CriticismContract;
use crate::application::missions::reconciliation::engine::{
    self, ClaimedContribution, ReconciliationInput, ReductionDraft,
};
use crate::application::missions::reconciliation::settlement::{
    self, WaveGoalStatus, evaluate_wave_settlement, pre_synthesis_sweep_needed,
};
use crate::application::missions::reconciliation::verify::VerificationContext;
use crate::application::missions::reconciliation::verify::finding_ref;
use crate::application::missions::service::FileMissionService;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::AgentProviderService;
use crate::infrastructure::git::repository::FileGitRepository;
use crate::model::mission::Mission;

use super::execution::{approved_wave, observe_wave_goals};
use super::{current_round, ledger_summary, write_phase_evidence};

/// Collect the eligible claims of one wave: every Mission-bound Goal of the
/// wave whose settled contribution exists. Contributions whose Goal is in
/// Review carry `in_review = true` so accepted assertions stay
/// `goal_review_pending`.
pub fn collect_claims(
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: Option<usize>,
) -> RefineResult<Vec<ClaimedContribution>> {
    let mission_service = FileMissionService::new(&work_items.refine_dir);
    let mission = mission_service.show_mission(mission_id)?;
    let round = current_round(&mission)?;
    let specs = wave
        .map(|wave| {
            let approved = approved_wave(&mission, wave)?;
            Ok::<_, RefineError>(approved.goal_specs.clone())
        })
        .transpose()?
        .unwrap_or_default();
    let spec_keys: std::collections::BTreeSet<String> = specs
        .iter()
        .map(|spec| spec.mission_goal_key.clone())
        .collect();
    // The pre-Synthesis sweep claims every remaining contribution.
    let claimed_digests: std::collections::BTreeSet<String> = round
        .reconciliation_receipts
        .iter()
        .flat_map(|receipt| receipt.claim_set.iter().cloned())
        .collect();

    let mut claims = Vec::new();
    for goal in super::execution::mission_bound_goals(&work_items.refine_dir, mission_id)? {
        if let Some(_wave) = wave
            && !spec_keys.contains(&goal.mission_goal_key)
        {
            continue;
        }
        let Some((contribution, goal_round)) =
            work_items.goal_mission_contribution(&goal.goal_id)?
        else {
            continue;
        };
        let Some(digest) = contribution.digest.clone() else {
            continue;
        };
        if claimed_digests.contains(&digest) {
            continue;
        }
        let in_review = goal.status == crate::model::workflow::GoalStatus::Review;
        claims.push(ClaimedContribution {
            goal_id: goal.goal_id.clone(),
            goal_round: goal_round + 1,
            mission_goal_key: goal.mission_goal_key.clone(),
            digest,
            eligible: true,
            in_review,
            contribution,
        });
    }
    claims.sort_by(|a, b| {
        a.goal_id
            .cmp(&b.goal_id)
            .then(a.goal_round.cmp(&b.goal_round))
    });
    Ok(claims)
}

/// Build the verification context for one attempt: target head pinned from
/// the parent snapshot, repository facts enriched, staged candidate bytes
/// verified.
pub fn build_verification_context(
    mission: &Mission,
    refine_dir: &Path,
    claims: &[ClaimedContribution],
    target_root: &Path,
    runtime_root: &Path,
) -> RefineResult<VerificationContext> {
    let round = current_round(mission)?;
    let parent = round.snapshots.last();
    let mut context = VerificationContext {
        target_head: parent.and_then(|snapshot| snapshot.target_head.clone()),
        ..Default::default()
    };
    if context.target_head.is_none() {
        let repo = FileGitRepository::new(target_root, runtime_root);
        context.target_head = super::super::verification_sources::current_target_head(&repo);
    }
    if let Some(head) = context.target_head.clone() {
        let repo = FileGitRepository::new(target_root, runtime_root);
        if repo
            .git(&["rev-parse", "--verify", &format!("{head}^{{commit}}")])
            .map(|output| output.success)
            .unwrap_or(false)
        {
            super::super::verification_sources::enrich_from_repository(
                &mut context,
                &repo,
                claims,
            )?;
        }
    }
    super::super::verification_sources::verify_staged_candidate_bytes(
        &mut context,
        refine_dir,
        &mission.id,
        claims,
    )?;
    Ok(context)
}

/// Render the reduction prompt from the claim set and parent ledger.
pub fn reduction_prompt(mission: &Mission, claims: &[ClaimedContribution]) -> RefineResult<String> {
    let claims_json = serde_json::to_string_pretty(&json!(
        claims
            .iter()
            .map(|claim| json!({
                "finding_refs": claim.contribution.findings.iter().enumerate()
                    .map(|(index, _)| finding_ref(&claim.goal_id, claim.goal_round, index))
                    .collect::<Vec<_>>(),
                "findings": claim.contribution.findings,
                "in_review": claim.in_review,
            }))
            .collect::<Vec<_>>()
    ))
    .map_err(|error| RefineError::Serialization(format!("failed to encode claims: {error}")))?;
    let ledger = ledger_summary(mission)?;
    super::render_prompt(
        PromptTemplate::MissionReduction,
        &[
            ("intent", mission.intent.as_str()),
            ("claims", &claims_json),
            ("ledger", &ledger),
            ("contract", &ReductionDraft::contract_json()),
        ],
    )
}

/// Render the criticism prompt from the draft, claim set, and ledger. The
/// critic sees the draft artifact, not the drafter's reasoning.
pub fn criticism_prompt(
    mission: &Mission,
    draft: &ReductionDraft,
    claims: &[ClaimedContribution],
) -> RefineResult<String> {
    let draft_json = serde_json::to_string_pretty(draft)
        .map_err(|error| RefineError::Serialization(format!("failed to encode draft: {error}")))?;
    let claims_json = serde_json::to_string_pretty(&json!(
        claims
            .iter()
            .map(|claim| json!({
                "findings": claim.contribution.findings,
                "in_review": claim.in_review,
            }))
            .collect::<Vec<_>>()
    ))
    .map_err(|error| RefineError::Serialization(format!("failed to encode claims: {error}")))?;
    let ledger = ledger_summary(mission)?;
    super::render_prompt(
        PromptTemplate::MissionCriticism,
        &[
            ("draft", &draft_json),
            ("claims", &claims_json),
            ("ledger", &ledger),
            ("contract", &CriticismContract::contract_json()),
        ],
    )
}

/// Run one full reconciliation attempt of a wave boundary (or the
/// pre-Synthesis sweep when `wave` is `None`): claim, verify, reduce,
/// criticize, publish.
#[allow(clippy::too_many_arguments)]
pub fn run_reconciliation(
    service: &FileMissionService,
    work_items: &FileWorkItemService,
    provider: &dyn AgentProviderService,
    provider_name: &str,
    runtime_root: &Path,
    target_root: &Path,
    mission_id: &str,
    wave: Option<usize>,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    let round_number = mission.current_round.unwrap_or(0);
    let claims = collect_claims(work_items, mission_id, wave)?;
    if claims.is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "Mission {mission_id} has no unclaimed eligible contributions for this boundary"
        )));
    }
    let verification = build_verification_context(
        &mission,
        &service.refine_dir,
        &claims,
        target_root,
        runtime_root,
    )?;

    let input = ReconciliationInput {
        wave,
        claims: claims.clone(),
        verification: verification.clone(),
        budgets: Default::default(),
        decision_volume_threshold: None,
        correction: None,
    };
    let opened = engine::open_attempt(&mission, &input)?;
    let verified = engine::verify_claims(&opened);

    let reduction_prompt = reduction_prompt(&mission, &claims)?;
    let reduction_run: PhaseRun<ReductionDraft> = run_agent_phase(
        provider,
        runtime_root,
        mission_id,
        round_number,
        &wave_stage(wave),
        provider_name,
        &reduction_prompt,
        Some(target_root),
    )?;
    let criticism_prompt = criticism_prompt(&mission, &reduction_run.output, &claims)?;
    let criticism_run: PhaseRun<CriticismContract> = run_agent_phase(
        provider,
        runtime_root,
        mission_id,
        round_number,
        &format!("{}-criticism", wave_stage(wave)),
        provider_name,
        &criticism_prompt,
        Some(target_root),
    )?;

    let applied = engine::apply_reduction(&verified, &reduction_run.output, &criticism_run.output)?;
    let published = service.publish_reconciliation(mission_id, &applied, Some(mission.revision))?;

    let evidence = json!({
        "operation_id": reduction_run.operation_id,
        "criticism_operation_id": criticism_run.operation_id,
        "stage": wave_stage(wave),
        "wave": wave,
        "attempt": applied.attempt_id,
        "accepted": applied.receipt.accepted.len(),
        "deferred": applied.receipt.deferred.len(),
        "contested": applied.receipt.contested.len(),
        "decision_requests": applied.receipt.decision_requests.len(),
    });
    write_phase_evidence(
        service,
        mission_id,
        &format!("reconcile-{}", wave_stage(wave)),
        evidence,
    )?;
    Ok(published)
}

fn wave_stage(wave: Option<usize>) -> String {
    wave.map(|wave| format!("wave-{wave}"))
        .unwrap_or_else(|| "sweep".to_string())
}

/// Evaluate the settlement of one wave from Goal state.
pub fn wave_settlement(
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: usize,
    optional_wait_exceeded: &[String],
) -> RefineResult<settlement::WaveSettlement> {
    let observations = observe_wave_goals(work_items, mission_id, wave, optional_wait_exceeded)?;
    let statuses: Vec<WaveGoalStatus> = observations
        .into_iter()
        .map(|observation| WaveGoalStatus {
            mission_goal_key: observation.mission_goal_key,
            required: observation.required,
            state: observation.state,
        })
        .collect();
    Ok(evaluate_wave_settlement(&statuses))
}

/// Whether the pre-Synthesis sweep is required before Synthesize.
pub fn sweep_needed(work_items: &FileWorkItemService, mission_id: &str) -> RefineResult<bool> {
    let service = FileMissionService::new(&work_items.refine_dir);
    let mission = service.show_mission(mission_id)?;
    let round = current_round(&mission)?;
    let eligible: Vec<String> = collect_claims(work_items, mission_id, None)?
        .into_iter()
        .map(|claim| claim.digest)
        .collect();
    Ok(pre_synthesis_sweep_needed(
        &round.reconciliation_receipts,
        &eligible,
        true,
    ))
}
