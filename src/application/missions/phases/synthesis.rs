//! The Synthesize phase: one fresh synthesis agent receives only pinned
//! Mission inputs and produces the candidate Outcome; Application validates
//! and promotes it into the candidate final snapshot and the Outcome
//! manifest.
//!
//! Synthesis cannot repair target-app code directly; a discovered code gap
//! is an unmet criterion, and the Round's judgment follows from evidence.
//! See `docs/mission-spec.md` ("Synthesize").

use std::path::Path;

use serde_json::{Value, json};

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::agent_io::structured_output::contract::Contract;
use crate::application::missions::agent_phase::{PhaseRun, run_agent_phase};
use crate::application::missions::contracts::{SynthesisOutput, judged_criterion_outcome};
use crate::application::missions::reconciliation::engine::compute_snapshot_digest;
use crate::application::missions::service::FileMissionService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::AgentProviderService;
use crate::model::mission::{CriterionResult, Mission, MissionSnapshot, OutcomeManifest};

use super::{current_round, write_phase_evidence};

/// Render the synthesis prompt from the pinned Mission inputs.
pub fn synthesis_prompt(mission: &Mission) -> RefineResult<String> {
    let round = current_round(mission)?;
    let snapshot = round.snapshots.last().ok_or_else(|| {
        RefineError::Conflict(format!(
            "Mission {} has no execution snapshot to synthesize from",
            mission.id
        ))
    })?;
    let charter = json!({
        "intent": round.request.intent,
        "criteria": round.request.criteria,
        "artifact_obligations": round.request.artifact_obligations,
    })
    .to_string();
    let goal_evidence = json!(
        round
            .snapshots
            .iter()
            .flat_map(|snapshot| snapshot.consumed_contribution_refs.iter())
            .collect::<Vec<_>>()
    )
    .to_string();
    super::render_prompt(
        PromptTemplate::MissionSynthesis,
        &[
            ("charter", &charter),
            (
                "snapshot",
                &serde_json::to_string(snapshot).unwrap_or_default(),
            ),
            ("goal_evidence", &goal_evidence),
            ("contract", &SynthesisOutput::contract_json()),
        ],
    )
}

/// Run the synthesis agent, promote the candidate final snapshot, and settle
/// the candidate Outcome manifest on the Round.
pub fn run_synthesis(
    service: &FileMissionService,
    provider: &dyn AgentProviderService,
    provider_name: &str,
    runtime_root: &Path,
    target_root: &Path,
    mission_id: &str,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    if mission.status != crate::model::mission::MissionStatus::Synthesize {
        return Err(RefineError::Conflict(format!(
            "Mission {} is in {}; synthesis requires the Synthesize phase",
            mission.id,
            mission.status.as_str()
        )));
    }
    let round_number = mission.current_round.unwrap_or(0);
    let prompt = synthesis_prompt(&mission)?;
    let run: PhaseRun<SynthesisOutput> = run_agent_phase(
        provider,
        runtime_root,
        mission_id,
        round_number,
        "synthesis",
        provider_name,
        &prompt,
        Some(target_root),
    )?;
    let output = run.output;

    let round = current_round(&mission)?;
    let parent = round.snapshots.last().ok_or_else(|| {
        RefineError::Conflict(format!("Mission {mission_id} has no parent snapshot"))
    })?;

    // The candidate final snapshot: parent artifacts plus promoted synthesis
    // artifacts (newer file of the same key replaces the parent selection),
    // with the accumulated knowledge chain unchanged.
    let mut artifact_refs = parent.artifact_refs.clone();
    for promotion in &output.artifact_promotions {
        artifact_refs.retain(|existing| existing.key != promotion.artifact.key);
        artifact_refs.push(promotion.artifact.clone());
    }
    let mut snapshot = MissionSnapshot {
        version: parent.version + 1,
        parent_version: Some(parent.version),
        target_head: parent.target_head.clone(),
        plan_digest: parent.plan_digest.clone(),
        artifact_refs,
        input_refs: parent.input_refs.clone(),
        consumed_contribution_refs: Vec::new(),
        knowledge_index: Vec::new(),
        corrects_snapshot: None,
        digest: None,
        created: String::new(),
    };
    snapshot.digest = Some(compute_snapshot_digest(&snapshot));
    let _mission = service.publish_snapshot(mission_id, snapshot, Some(mission.revision))?;
    let mission = service.show_mission(mission_id)?;
    let final_snapshot = current_round(&mission)?
        .snapshots
        .last()
        .map(|snapshot| snapshot.version)
        .ok_or_else(|| RefineError::Conflict("final snapshot missing".to_string()))?;

    let criteria_results = output
        .criteria_results
        .iter()
        .map(|judged| {
            let result = judged_criterion_outcome(&judged.result)
                .unwrap_or(crate::model::mission::CriterionOutcome::Unmet);
            CriterionResult {
                criterion_id: judged.criterion_id.clone(),
                result,
                evidence: judged.evidence.clone(),
            }
        })
        .collect();
    let mut manifest = OutcomeManifest {
        mission_id: mission.id.clone(),
        mission_round: round_number,
        charter_digest: round.request.charter_digest.clone(),
        final_snapshot: Some(final_snapshot),
        criteria_results,
        artifact_refs: current_round(&mission)?
            .snapshots
            .last()
            .map(|snapshot| snapshot.artifact_refs.clone())
            .unwrap_or_default(),
        goal_evidence_refs: current_round(&mission)?
            .snapshots
            .iter()
            .flat_map(|snapshot| snapshot.consumed_contribution_refs.iter().cloned())
            .collect(),
        target_commit_refs: Vec::new(),
        input_bindings: round.input_bindings.clone(),
        manifest_digest: None,
        approved_at: None,
        approved_by: None,
    };
    let digest = compute_snapshot_digest_bytes_for(&manifest)?;
    manifest.manifest_digest = Some(digest);
    let _mission = service.settle_outcome(mission_id, manifest, Some(mission.revision))?;

    let evidence = json!({
        "operation_id": run.operation_id,
        "stage": "synthesis",
        "summary": output.summary,
        "residual_risks": output.residual_risks,
        "criteria_judged": output.criteria_results.len(),
    });
    let mission = write_phase_evidence(service, mission_id, "synthesis", evidence)?;
    service.transition_mission(
        mission_id,
        crate::model::mission::MissionStatus::Quality,
        Some(mission.revision),
    )
}

fn compute_snapshot_digest_bytes_for(manifest: &OutcomeManifest) -> RefineResult<String> {
    let mut value = serde_json::to_value(manifest).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Outcome manifest: {error}"))
    })?;
    if let Some(object) = value.as_object_mut() {
        object.insert("manifest_digest".to_string(), Value::Null);
        object.insert("approved_at".to_string(), Value::Null);
        object.insert("approved_by".to_string(), Value::Null);
    }
    Ok(
        crate::application::missions::reconciliation::engine::compute_snapshot_digest_bytes(
            value.to_string().as_bytes(),
        ),
    )
}
