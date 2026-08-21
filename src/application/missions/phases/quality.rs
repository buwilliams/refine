//! Mission Quality: the combined outcome is evaluated, not each child Goal
//! in isolation.
//!
//! Deterministic checks run first (criteria coverage, required-Goal
//! evidence, artifact reference validity); then a fresh read-only agent
//! supplies the holistic judgment. A failed judgment fails the phase
//! attempt retryably without failing the Round; the recorded evidence is
//! what Review later judges. See `docs/mission-spec.md` ("Quality").

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::agent_io::structured_output::contract::Contract;
use crate::application::missions::agent_phase::{PhaseRun, run_agent_phase};
use crate::application::missions::contracts::{MissionQualityJudgment, judged_criterion_outcome};
use crate::application::missions::service::FileMissionService;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::AgentProviderService;
use crate::model::mission::{CriterionOutcome, Mission};
use crate::model::workflow::GoalStatus;

use super::execution::mission_bound_goals;
use super::{current_round, write_phase_evidence};

/// The deterministic check results presented to the judgment agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeterministicChecks {
    pub criteria_total: usize,
    pub criteria_judged: usize,
    pub required_goal_failures: usize,
    pub required_goal_gaps: usize,
    pub artifact_digest_invalid: Vec<String>,
    pub passed: bool,
}

/// Run the deterministic pre-checks over the candidate Outcome.
pub fn deterministic_checks(
    mission: &Mission,
    work_items: &FileWorkItemService,
) -> RefineResult<DeterministicChecks> {
    let round = current_round(mission)?;
    let Some(outcome) = round.outcome.as_ref() else {
        return Err(RefineError::Conflict(format!(
            "Mission {} has no candidate Outcome to check",
            mission.id
        )));
    };
    let mut checks = DeterministicChecks {
        criteria_total: round.request.criteria.len(),
        criteria_judged: outcome.criteria_results.len(),
        ..Default::default()
    };
    for goal in mission_bound_goals(&work_items.refine_dir, &mission.id)? {
        let spec_required = round
            .plan
            .as_ref()
            .map(|plan| {
                plan.waves.iter().any(|wave| {
                    wave.goal_specs
                        .iter()
                        .any(|spec| spec.mission_goal_key == goal.mission_goal_key && spec.required)
                })
            })
            .unwrap_or(false);
        if !spec_required {
            continue;
        }
        match goal.status {
            GoalStatus::Done | GoalStatus::Review => {}
            GoalStatus::Failed | GoalStatus::Cancelled => checks.required_goal_failures += 1,
            _ => checks.required_goal_gaps += 1,
        }
    }
    for artifact in &outcome.artifact_refs {
        let valid = artifact
            .sha256
            .as_deref()
            .map(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .unwrap_or(false);
        if !valid {
            checks.artifact_digest_invalid.push(artifact.key.clone());
        }
    }
    checks.passed = checks.criteria_judged >= checks.criteria_total
        && checks.required_goal_failures == 0
        && checks.required_goal_gaps == 0
        && checks.artifact_digest_invalid.is_empty();
    Ok(checks)
}

/// Render the Mission Quality judgment prompt.
pub fn quality_prompt(mission: &Mission, checks: &DeterministicChecks) -> RefineResult<String> {
    let round = current_round(mission)?;
    let Some(outcome) = round.outcome.as_ref() else {
        return Err(RefineError::Conflict("no candidate Outcome".to_string()));
    };
    let criteria_checks = json!({
        "criteria": round.request.criteria,
        "judged_results": outcome.criteria_results,
        "deterministic": checks,
    })
    .to_string();
    let goal_evidence = json!(outcome.goal_evidence_refs).to_string();
    super::render_prompt(
        PromptTemplate::MissionQuality,
        &[
            ("criteria_checks", &criteria_checks),
            ("goal_evidence", &goal_evidence),
            ("contract", &MissionQualityJudgment::contract_json()),
        ],
    )
}

/// Run Mission Quality: deterministic checks, then the holistic judgment
/// agent. Pass advances to Governance.
pub fn run_mission_quality(
    service: &FileMissionService,
    work_items: &FileWorkItemService,
    provider: &dyn AgentProviderService,
    provider_name: &str,
    runtime_root: &Path,
    target_root: &Path,
    mission_id: &str,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    if mission.status != crate::model::mission::MissionStatus::Quality {
        return Err(RefineError::Conflict(format!(
            "Mission {} is in {}; quality requires the Quality phase",
            mission.id,
            mission.status.as_str()
        )));
    }
    let checks = deterministic_checks(&mission, work_items)?;
    if !checks.passed {
        let evidence = json!({
            "stage": "quality",
            "deterministic": checks,
            "outcome": "failed_deterministic",
        });
        write_phase_evidence(service, mission_id, "quality", evidence)?;
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} failed deterministic Quality checks: {} of {} criteria judged, {} required failures, {} required gaps, {} invalid artifact digests",
            checks.criteria_judged,
            checks.criteria_total,
            checks.required_goal_failures,
            checks.required_goal_gaps,
            checks.artifact_digest_invalid.len()
        )));
    }

    let round_number = mission.current_round.unwrap_or(0);
    let prompt = quality_prompt(&mission, &checks)?;
    let run: PhaseRun<MissionQualityJudgment> = run_agent_phase(
        provider,
        runtime_root,
        mission_id,
        round_number,
        "quality",
        provider_name,
        &prompt,
        Some(target_root),
    )?;
    let judgment = run.output;

    let evidence = json!({
        "operation_id": run.operation_id,
        "stage": "quality",
        "deterministic": checks,
        "ok": judgment.ok,
        "summary": judgment.summary,
        "findings": judgment.findings,
        "criteria_results": judgment.criteria_results,
    });
    let mission = write_phase_evidence(service, mission_id, "quality", evidence)?;
    if !judgment.ok {
        // A failed judgment is a retryable stage failure, not a failed
        // Round: the Mission stays in Quality with durable evidence.
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} holistic Quality judgment failed: {}",
            judgment.summary
        )));
    }
    service.transition_mission(
        mission_id,
        crate::model::mission::MissionStatus::Governance,
        Some(mission.revision),
    )
}

/// Map one judged criterion string to its model outcome, defaulting to
/// Unmet for unknown spellings.
pub fn criterion_outcome_of(result: &str) -> CriterionOutcome {
    judged_criterion_outcome(result).unwrap_or(CriterionOutcome::Unmet)
}
