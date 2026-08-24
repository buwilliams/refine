//! The Plan phase: proposal, independent criticism, and revision.
//!
//! Planning runs as three one-shot agent phases against the frozen Round
//! charter and the investigation snapshot. The engine validates the revised
//! plan and records it as a draft; human approval of the exact effective
//! digest remains the gate into Execute. Agent text never approves anything.
//! See `docs/mission-spec.md` ("Plan").

use std::path::Path;

use serde_json::json;

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::agent_io::structured_output::contract::Contract;
use crate::application::missions::agent_phase::run_agent_phase;
use crate::application::missions::contracts::{MissionPlanProposal, MissionPlanRevision};
use crate::application::missions::service::FileMissionService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::AgentProviderService;
use crate::model::mission::{Mission, MissionPlan};

use super::current_round;

/// Render the planning proposal prompt from the charter and ledger.
pub fn planning_proposal_prompt(mission: &Mission) -> RefineResult<String> {
    let round = current_round(mission)?;
    let criteria = round
        .request
        .criteria
        .iter()
        .map(|criterion| format!("- {}: {}", criterion.id, criterion.description))
        .collect::<Vec<_>>()
        .join("\n");
    let obligations = if round.request.artifact_obligations.is_empty() {
        "none".to_string()
    } else {
        round
            .request
            .artifact_obligations
            .iter()
            .map(|obligation| {
                format!(
                    "- {} ({}): {}",
                    obligation.key, obligation.kind, obligation.purpose
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    super::render_prompt(
        PromptTemplate::MissionPlanningProposal,
        &[
            ("intent", round.request.intent.as_str()),
            ("criteria", &criteria),
            ("ledger", &super::ledger_summary(mission)?),
            ("obligations", &obligations),
            ("contract", &MissionPlanProposal::contract_json()),
        ],
    )
}

/// Render the plan criticism prompt from the drafted plan.
pub fn plan_criticism_prompt(mission: &Mission, draft: &MissionPlan) -> RefineResult<String> {
    let round = current_round(mission)?;
    let charter = json!({
        "intent": round.request.intent,
        "criteria": round.request.criteria,
        "artifact_obligations": round.request.artifact_obligations,
    })
    .to_string();
    super::render_prompt(
        PromptTemplate::MissionPlanningCriticism,
        &[
            ("draft", &serde_json::to_string(draft).unwrap_or_default()),
            ("charter", &charter),
            ("ledger", &super::ledger_summary(mission)?),
            (
                "contract",
                &crate::application::missions::reconciliation::engine::CriticismReport::contract_json(),
            ),
        ],
    )
}

/// Render the plan revision prompt from the draft and its criticism.
pub fn plan_revision_prompt(
    mission: &Mission,
    draft: &MissionPlan,
    criticism: &crate::application::missions::reconciliation::engine::CriticismReport,
) -> RefineResult<String> {
    let round = current_round(mission)?;
    let charter = json!({
        "intent": round.request.intent,
        "criteria": round.request.criteria,
    })
    .to_string();
    super::render_prompt(
        PromptTemplate::MissionPlanningRevision,
        &[
            ("draft", &serde_json::to_string(draft).unwrap_or_default()),
            (
                "criticism",
                &serde_json::to_string(criticism).unwrap_or_default(),
            ),
            ("charter", &charter),
            ("contract", &MissionPlanRevision::contract_json()),
        ],
    )
}

/// Run the planning trio: proposal, independent criticism, revision. The
/// revised plan is validated and recorded as the Round's draft plan; the
/// Mission stays in Plan until a human approves the exact effective digest.
pub fn run_planning(
    service: &FileMissionService,
    provider: &dyn AgentProviderService,
    provider_name: &str,
    runtime_root: &Path,
    target_root: &Path,
    mission_id: &str,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    if mission.status != crate::model::mission::MissionStatus::Plan {
        return Err(RefineError::Conflict(format!(
            "Mission {} is in {}; planning requires the Plan phase",
            mission.id,
            mission.status.as_str()
        )));
    }
    let round = current_round(&mission)?;
    if round.snapshots.is_empty() {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} has no investigation snapshot to plan against"
        )));
    }
    if round.plan.is_some() {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} Round {} already has a drafted plan",
            round.number
        )));
    }
    let round_number = mission.current_round.unwrap_or(0);

    let proposal_run = run_agent_phase::<MissionPlanProposal>(
        provider,
        runtime_root,
        mission_id,
        round_number,
        "planning-proposal",
        provider_name,
        &planning_proposal_prompt(&mission)?,
        Some(target_root),
    )?;
    let criticism_run =
        run_agent_phase::<crate::application::missions::reconciliation::engine::CriticismReport>(
            provider,
            runtime_root,
            mission_id,
            round_number,
            "planning-criticism",
            provider_name,
            &plan_criticism_prompt(&mission, &proposal_run.output.0)?,
            Some(target_root),
        )?;
    let revision_run = run_agent_phase::<MissionPlanRevision>(
        provider,
        runtime_root,
        mission_id,
        round_number,
        "planning-revision",
        provider_name,
        &plan_revision_prompt(&mission, &proposal_run.output.0, &criticism_run.output)?,
        Some(target_root),
    )?;
    let mut plan = revision_run.output.0;
    plan.criticism = Some(
        serde_json::to_string(&criticism_run.output)
            .unwrap_or_else(|_| criticism_run.output.notes.clone()),
    );

    let mission = service.record_plan(mission_id, plan, Some(mission.revision))?;
    let evidence = json!({
        "proposal_operation_id": proposal_run.operation_id,
        "criticism_operation_id": criticism_run.operation_id,
        "revision_operation_id": revision_run.operation_id,
        "attempts": proposal_run.attempts.len()
            + criticism_run.attempts.len()
            + revision_run.attempts.len(),
        "criticism_verdicts": criticism_run.output.verdicts.len(),
        "plan_digest": current_round(&mission)?
            .plan
            .as_ref()
            .and_then(|plan| plan.effective_digest.clone()),
    });
    super::write_phase_evidence(service, mission_id, "planning", evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mission::{MissionCriterion, MissionRound, MissionRoundRequest};

    fn mission_with_charter() -> Mission {
        Mission {
            id: "MTEST".to_string(),
            name: "Test".to_string(),
            intent: "Modernize auth".to_string(),
            status: crate::model::mission::MissionStatus::Plan,
            reporter: None,
            assignee: None,
            coordinator_node_id: None,
            success_criteria: vec![],
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
                snapshots: vec![MissionSnapshotFixture::snapshot()],
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
        }
    }

    struct MissionSnapshotFixture;

    impl MissionSnapshotFixture {
        fn snapshot() -> crate::model::mission::MissionSnapshot {
            crate::model::mission::MissionSnapshot {
                version: 1,
                parent_version: None,
                target_head: None,
                plan_digest: None,
                artifact_refs: vec![],
                input_refs: vec![],
                consumed_contribution_refs: vec![],
                knowledge_index: vec![],
                corrects_snapshot: None,
                digest: None,
                created: String::new(),
            }
        }
    }

    #[test]
    fn planning_prompts_bind_charter_ledger_and_contract() {
        let mission = mission_with_charter();
        let proposal = planning_proposal_prompt(&mission).unwrap();
        assert!(proposal.contains("Modernize auth"));
        assert!(proposal.contains("crit:tokens"));
        assert!(proposal.contains("Mission planning JSON"));

        let draft = MissionPlan {
            charter_digest: None,
            summary: "one wave".to_string(),
            assumptions: vec![],
            risks: vec![],
            criteria_coverage: vec![],
            waves: vec![],
            artifact_obligations: vec![],
            criticism: None,
            resolutions: vec![],
            effective_digest: None,
        };
        let criticism = plan_criticism_prompt(&mission, &draft).unwrap();
        assert!(criticism.contains("one wave"));
        assert!(criticism.contains("Mission criticism JSON"));
    }
}
