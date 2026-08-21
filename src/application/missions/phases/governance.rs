//! Mission Governance: the system-level verdict at the exact pinned tuple
//! `(target_head, final_snapshot_digest, candidate_outcome_manifest_digest)`.
//!
//! A changed target head, snapshot, or manifest invalidates the verdict;
//! the verdict is recorded with the tuple it judged. Target-app constitution
//! and rules outrank every Goal request. See `docs/mission-spec.md`
//! ("Governance").

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::agent_io::structured_output::contract::Contract;
use crate::application::missions::agent_phase::{PhaseRun, run_agent_phase};
use crate::application::missions::contracts::MissionGovernanceVerdict;
use crate::application::missions::service::FileMissionService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::AgentProviderService;
use crate::model::mission::Mission;

use super::current_round;
use super::write_phase_evidence;

/// The exact tuple one Governance verdict is bound to. A changed tuple
/// invalidates the verdict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GovernanceTuple {
    pub target_head: Option<String>,
    pub final_snapshot_digest: Option<String>,
    pub outcome_manifest_digest: Option<String>,
}

impl GovernanceTuple {
    pub fn of(mission: &Mission) -> RefineResult<Self> {
        let round = current_round(mission)?;
        let snapshot_digest = round
            .snapshots
            .last()
            .and_then(|snapshot| snapshot.digest.clone());
        Ok(Self {
            target_head: round
                .snapshots
                .last()
                .and_then(|snapshot| snapshot.target_head.clone()),
            final_snapshot_digest: snapshot_digest,
            outcome_manifest_digest: round.outcome.as_ref().and_then(|outcome| {
                outcome.manifest_digest.clone().or_else(|| {
                    round
                        .outcome
                        .as_ref()
                        .and_then(|o| o.manifest_digest.clone())
                })
            }),
        })
    }

    pub fn render(&self) -> String {
        json!(self).to_string()
    }
}

/// Render the Mission Governance verdict prompt.
pub fn governance_prompt(mission: &Mission) -> RefineResult<String> {
    let round = current_round(mission)?;
    let tuple = GovernanceTuple::of(mission)?;
    let charter = json!({
        "intent": round.request.intent,
        "criteria": round.request.criteria,
    })
    .to_string();
    let evidence = json!({
        "outcome": round.outcome,
        "reconciliation_receipts": round.reconciliation_receipts.len(),
        "phase_evidence_keys": round.phase_evidence.keys().collect::<Vec<_>>(),
    })
    .to_string();
    super::render_prompt(
        PromptTemplate::MissionGovernance,
        &[
            ("tuple", &tuple.render()),
            ("charter", &charter),
            ("evidence", &evidence),
            ("contract", &MissionGovernanceVerdict::contract_json()),
        ],
    )
}

/// Run Mission Governance at the current pinned tuple. A passed verdict
/// advances to Review; a failed verdict fails the Round with its recorded
/// recovery analysis, and only a new Round may resume the Mission.
pub fn run_mission_governance(
    service: &FileMissionService,
    provider: &dyn AgentProviderService,
    provider_name: &str,
    runtime_root: &Path,
    target_root: &Path,
    mission_id: &str,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    if mission.status != crate::model::mission::MissionStatus::Governance {
        return Err(RefineError::Conflict(format!(
            "Mission {} is in {}; governance requires the Governance phase",
            mission.id,
            mission.status.as_str()
        )));
    }
    let round_number = mission.current_round.unwrap_or(0);
    let prompt = governance_prompt(&mission)?;
    let run: PhaseRun<MissionGovernanceVerdict> = run_agent_phase(
        provider,
        runtime_root,
        mission_id,
        round_number,
        "governance",
        provider_name,
        &prompt,
        Some(target_root),
    )?;
    let verdict = run.output;

    let evidence = json!({
        "operation_id": run.operation_id,
        "stage": "governance",
        "tuple": GovernanceTuple::of(&mission)?,
        "status": verdict.status,
        "message": verdict.message,
        "violations": verdict.violations,
        "recovery_analysis": verdict.recovery_analysis,
    });
    let mission = write_phase_evidence(service, mission_id, "governance", evidence)?;
    if !verdict.passed() {
        let round = current_round(&mission)?;
        let reason = if verdict.message.trim().is_empty() {
            format!(
                "governance verdict failed with {} violation(s)",
                verdict.violations.len()
            )
        } else {
            verdict.message.clone()
        };
        return service.fail_round(mission_id, &round.number.to_string(), &reason);
    }
    service.transition_mission(
        mission_id,
        crate::model::mission::MissionStatus::Review,
        Some(mission.revision),
    )
}
