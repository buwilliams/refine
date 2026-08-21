//! The Investigate phase: one fresh agent investigates the target app and
//! bound inputs; Application validates and promotes the initial
//! MissionSnapshot.
//!
//! Investigation is observational. The agent may read the target app, but it
//! may not change the application worktree or live Refine state; its output
//! lands as typed structured output, and only the deterministic engine turns
//! it into the initial snapshot. See `docs/mission-spec.md` ("Investigate").

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::json;

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::agent_io::structured_output::contract::Contract;
use crate::application::missions::agent_phase::{PhaseRun, run_agent_phase};
use crate::application::missions::contracts::InvestigationOutput;
use crate::application::missions::reconciliation::engine::compute_snapshot_digest;
use crate::application::missions::service::FileMissionService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::AgentProviderService;
use crate::model::mission::{Mission, MissionSnapshot};

use super::write_phase_evidence;

/// Render the investigation prompt from the frozen Round charter.
pub fn investigation_prompt(mission: &Mission) -> RefineResult<String> {
    let round = mission
        .rounds
        .iter()
        .find(|round| round.number == mission.current_round.unwrap_or(0))
        .ok_or_else(|| {
            RefineError::InvalidInput(format!(
                "Mission {} has no current Round to investigate",
                mission.id
            ))
        })?;
    let criteria = round
        .request
        .criteria
        .iter()
        .map(|criterion| format!("- {}: {}", criterion.id, criterion.description))
        .collect::<Vec<_>>()
        .join("\n");
    super::render_prompt(
        PromptTemplate::MissionInvestigation,
        &[
            ("intent", round.request.intent.as_str()),
            ("criteria", &criteria),
            ("contract", &InvestigationOutput::contract_json()),
        ],
    )
}

/// Run the investigation agent and promote the validated initial snapshot.
///
/// The initial snapshot is version 1 of the current Round: accepted
/// assertions carry deterministic ids, artifact promotions replace nothing
/// (there is no parent), and unbound candidates defer rather than promote.
pub fn run_investigation(
    service: &FileMissionService,
    provider: &dyn AgentProviderService,
    provider_name: &str,
    runtime_root: &Path,
    target_root: &Path,
    mission_id: &str,
    verification: &crate::application::missions::reconciliation::verify::VerificationContext,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    if mission.status != crate::model::mission::MissionStatus::Investigate {
        return Err(RefineError::Conflict(format!(
            "Mission {} is in {}; investigation requires the Investigate phase",
            mission.id,
            mission.status.as_str()
        )));
    }
    let round_number = mission.current_round.unwrap_or(0);
    let round = mission
        .rounds
        .iter()
        .find(|round| round.number == round_number)
        .ok_or_else(|| {
            RefineError::InvalidInput(format!("Mission {mission_id} has no Round {round_number}"))
        })?;
    if round.snapshots.iter().any(|snapshot| snapshot.version >= 1) {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} Round {round_number} already has an investigation snapshot"
        )));
    }

    let prompt = investigation_prompt(&mission)?;
    let run: PhaseRun<InvestigationOutput> = run_agent_phase(
        provider,
        runtime_root,
        mission_id,
        round_number,
        "investigation",
        provider_name,
        &prompt,
        Some(target_root),
    )?;
    let output = run.output;

    // Promote artifact candidates whose staged bytes matched their digest;
    // everything else defers as durable diagnostic evidence.
    let mut artifact_refs = Vec::new();
    let mut deferred = Vec::new();
    for promotion in &output.artifact_promotions {
        let digest = promotion.artifact.sha256.clone().unwrap_or_default();
        if verification.matched_digests.contains(&digest) {
            artifact_refs.push(promotion.artifact.clone());
        } else {
            deferred.push(format!(
                "{}: investigation artifact bytes were not verified against digest {digest}",
                promotion.candidate_ref
            ));
        }
    }

    // Deterministic assertion ids for the initial ledger, failing closed on
    // any duplicate content.
    let mut seen = BTreeSet::new();
    let mut knowledge_index = Vec::new();
    for drafted in &output.accepts {
        let mut assertion = drafted.assertion.clone();
        assertion.assertion_id =
            crate::application::missions::reconciliation::engine::deterministic_assertion_id_for(
                mission_id,
                round_number,
                0,
                &assertion,
            )?;
        if !seen.insert(assertion.assertion_id.clone()) {
            return Err(RefineError::InvalidInput(format!(
                "investigation produced duplicate assertion content for draft {}",
                drafted.draft_id
            )));
        }
        knowledge_index.push(assertion);
    }

    let mut snapshot = MissionSnapshot {
        version: 1,
        parent_version: None,
        target_head: verification.target_head.clone(),
        plan_digest: None,
        artifact_refs,
        input_refs: round
            .input_bindings
            .iter()
            .map(|binding| binding.source_mission_id.clone())
            .collect(),
        consumed_contribution_refs: Vec::new(),
        knowledge_index,
        corrects_snapshot: None,
        digest: None,
        created: String::new(),
    };
    snapshot.digest = Some(compute_snapshot_digest(&snapshot));

    let _mission = service.publish_snapshot(mission_id, snapshot, Some(mission.revision))?;
    let evidence = json!({
        "operation_id": run.operation_id,
        "stage": "investigation",
        "attempts": run.attempts,
        "accepted": output.accepts.len(),
        "open_questions": output.open_questions,
        "deferred_artifacts": deferred,
    });
    let mission = write_phase_evidence(service, mission_id, "investigation", evidence)?;
    service.transition_mission(
        mission_id,
        crate::model::mission::MissionStatus::Plan,
        Some(mission.revision),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mission::{MissionCriterion, MissionRound, MissionRoundRequest};

    fn mission_with_charter() -> Mission {
        let mut mission = Mission {
            id: "MTEST".to_string(),
            name: "Test".to_string(),
            intent: "Modernize auth".to_string(),
            status: crate::model::mission::MissionStatus::Investigate,
            reporter: None,
            assignee: None,
            coordinator_node_id: None,
            success_criteria: vec![MissionCriterion {
                id: "crit:tokens".to_string(),
                description: "token invariants documented".to_string(),
            }],
            artifact_contract: vec![],
            current_round: Some(1),
            revision: 0,
            rounds: vec![MissionRound {
                number: 1,
                request: MissionRoundRequest {
                    intent: "Modernize auth".to_string(),
                    constraints: vec![],
                    criteria: vec![MissionCriterion {
                        id: "crit:tokens".to_string(),
                        description: "token invariants documented".to_string(),
                    }],
                    artifact_obligations: vec![],
                    authorizing_request: "go".to_string(),
                    charter_digest: None,
                },
                input_bindings: vec![],
                plan: None,
                plan_amendments: vec![],
                snapshots: vec![],
                reconciliation_receipts: vec![],
                phase_evidence: Default::default(),
                review: None,
                outcome: None,
                outcome_publication: None,
                failure: None,
                created: String::new(),
                updated: String::new(),
            }],
            created: String::new(),
            updated: String::new(),
        };
        mission.success_criteria.clear();
        mission
    }

    #[test]
    fn investigation_prompt_binds_charter_and_contract() {
        let mission = mission_with_charter();
        let prompt = investigation_prompt(&mission).unwrap();
        assert!(prompt.contains("Modernize auth"));
        assert!(prompt.contains("crit:tokens"));
        assert!(prompt.contains("Mission investigation JSON"));
    }
}
