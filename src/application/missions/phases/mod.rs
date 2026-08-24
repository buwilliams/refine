//! Mission workflow phases: investigation, Goal materialization and wave
//! admission, contribution settlement, reconciliation orchestration,
//! synthesis, Quality, Governance, and consolidation.
//!
//! Each phase is a short, fenced transition or a one-shot agent operation
//! whose typed output the engine applies deterministically. Phase evidence is
//! durable; agent text can never define workflow transitions or publication
//! authority.

pub mod consolidation;
pub mod distribution;
pub mod execution;
pub mod governance;
pub mod investigation;
pub mod planning;
pub mod quality;
pub mod reconcile;
pub mod synthesis;

#[cfg(test)]
mod tests;

use serde_json::{Value, json};

use crate::application::agent_io::prompts::{PromptEngine, PromptTemplate};
use crate::application::missions::service::FileMissionService;
use crate::error::{RefineError, RefineResult};
use crate::model::mission::Mission;

/// Render one Mission prompt template, converting template errors into
/// Refine errors: a broken template is a programming fault, not a phase
/// outcome.
pub(crate) fn render_prompt(
    template: PromptTemplate,
    variables: &[(&str, &str)],
) -> RefineResult<String> {
    PromptEngine::render(template, variables).map_err(|error| {
        RefineError::InvalidInput(format!("invalid Mission prompt template: {error}"))
    })
}

/// Record one phase's evidence under the current Round's phase_evidence map.
pub fn write_phase_evidence(
    service: &FileMissionService,
    mission_id: &str,
    stage: &str,
    evidence: Value,
) -> RefineResult<Mission> {
    write_nested_phase_evidence(service, mission_id, stage, None, evidence)
}

/// Record one phase's evidence under a per-wave key of the stage entry, so a
/// repeated stage (distribution per wave) appends keyed history instead of
/// overwriting its own receipts.
pub fn write_wave_phase_evidence(
    service: &FileMissionService,
    mission_id: &str,
    stage: &str,
    wave: usize,
    evidence: Value,
) -> RefineResult<Mission> {
    write_nested_phase_evidence(service, mission_id, stage, Some(wave), evidence)
}

fn write_nested_phase_evidence(
    service: &FileMissionService,
    mission_id: &str,
    stage: &str,
    wave: Option<usize>,
    evidence: Value,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    let round_number = mission.current_round.ok_or_else(|| {
        RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
    })?;
    let mut value = service.show_mission_value(mission_id)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RefineError::Serialization("Mission is not a JSON object".to_string()))?;
    let rounds = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| RefineError::Serialization("Mission has no rounds array".to_string()))?;
    let round = rounds
        .iter_mut()
        .find(|round| round.get("number").and_then(Value::as_u64) == Some(round_number as u64))
        .ok_or_else(|| {
            RefineError::NotFound(format!("Mission Round {round_number} was not found"))
        })?;
    let round_object = round.as_object_mut().ok_or_else(|| {
        RefineError::Serialization("MissionRound is not a JSON object".to_string())
    })?;
    let mut phase_evidence = round_object
        .get("phase_evidence")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    match wave {
        None => {
            phase_evidence.insert(stage.to_string(), evidence);
        }
        Some(wave) => {
            let mut stage_entry = phase_evidence
                .get(stage)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            stage_entry.insert(wave.to_string(), evidence);
            phase_evidence.insert(stage.to_string(), Value::Object(stage_entry));
        }
    }
    round_object.insert("phase_evidence".to_string(), Value::Object(phase_evidence));
    let written = crate::application::missions::persistence::write_mission_atomically(
        &service.refine_dir,
        mission_id,
        &value,
    )?;
    crate::application::missions::persistence::parse_mission(&written)
}

/// The current Round of a Mission, failing closed when absent.
pub fn current_round(mission: &Mission) -> RefineResult<&crate::model::mission::MissionRound> {
    let number = mission.current_round.unwrap_or(0);
    mission
        .rounds
        .iter()
        .find(|round| round.number == number)
        .ok_or_else(|| {
            RefineError::InvalidInput(format!("Mission {} has no Round {number}", mission.id))
        })
}

/// A compact JSON summary of the derived knowledge ledger for agent
/// prompts: the assertions of the current Round's chain through the latest
/// snapshot, each with its derived state, so the reduction and criticism
/// agents see exactly what the Mission currently accepts.
pub fn ledger_summary(mission: &Mission) -> RefineResult<String> {
    let round = current_round(mission)?;
    let latest = round
        .snapshots
        .last()
        .map(|snapshot| snapshot.version)
        .unwrap_or(0);
    let entries: Vec<Value> =
        crate::application::missions::reconciliation::ledger::assertions_through(
            mission,
            latest,
            &std::collections::BTreeSet::new(),
        )
        .into_iter()
        .map(|(assertion, state)| {
            json!({
                "assertion_id": assertion.assertion_id,
                "kind": assertion.kind.as_str(),
                "authority": assertion.authority.as_str(),
                "state": state_as_str(state),
                "claim": assertion.scope,
                "scope_refs": assertion.scope_refs,
                "qualified": assertion.qualified,
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries)
        .map_err(|error| RefineError::Serialization(format!("failed to encode ledger: {error}")))
}

fn state_as_str(
    state: crate::application::missions::reconciliation::ledger::AssertionState,
) -> &'static str {
    use crate::application::missions::reconciliation::ledger::AssertionState;
    match state {
        AssertionState::Active => "active",
        AssertionState::Superseded => "superseded",
        AssertionState::Invalidated => "invalidated",
        AssertionState::Contested => "contested",
    }
}

/// A compact JSON summary of one provider phase for evidence records.
pub fn phase_summary(operation_id: &str, extra: Value) -> Value {
    json!({
        "operation_id": operation_id,
        "extra": extra,
    })
}

/// Mark one stage attempt as a retryable stage failure. The Mission stays in
/// its current nonterminal phase; the engine stops re-attempting until a
/// user-authorized retry clears the marker. Existing evidence is preserved
/// and augmented, never replaced.
pub fn mark_stage_failed(
    service: &FileMissionService,
    mission_id: &str,
    stage: &str,
    message: &str,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    let round_number = mission.current_round.ok_or_else(|| {
        RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
    })?;
    let mut value = service.show_mission_value(mission_id)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RefineError::Serialization("Mission is not a JSON object".to_string()))?;
    let rounds = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| RefineError::Serialization("Mission has no rounds array".to_string()))?;
    let round = rounds
        .iter_mut()
        .find(|round| round.get("number").and_then(Value::as_u64) == Some(round_number as u64))
        .ok_or_else(|| {
            RefineError::NotFound(format!("Mission Round {round_number} was not found"))
        })?;
    let round_object = round.as_object_mut().ok_or_else(|| {
        RefineError::Serialization("MissionRound is not a JSON object".to_string())
    })?;
    let mut phase_evidence = round_object
        .get("phase_evidence")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut stage_entry = phase_evidence
        .get(stage)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    stage_entry.insert("stage_failed".to_string(), Value::Bool(true));
    stage_entry.insert("retry_authorized".to_string(), Value::Bool(false));
    stage_entry.insert("last_error".to_string(), Value::String(message.to_string()));
    stage_entry.insert(
        "failed_at".to_string(),
        Value::String(crate::application::missions::service::FileMissionService::now_timestamp()),
    );
    phase_evidence.insert(stage.to_string(), Value::Object(stage_entry));
    round_object.insert("phase_evidence".to_string(), Value::Object(phase_evidence));
    let written = crate::application::missions::persistence::write_mission_atomically(
        &service.refine_dir,
        mission_id,
        &value,
    )?;
    crate::application::missions::persistence::parse_mission(&written)
}

/// Clear one stage's failure state so the engine may re-attempt it. Only a
/// user-authorized retry reaches this; the engine itself never retries a
/// failed stage.
pub fn authorize_stage_retry(
    service: &FileMissionService,
    mission_id: &str,
    stage: &str,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    let round_number = mission.current_round.ok_or_else(|| {
        RefineError::InvalidInput(format!("Mission {mission_id} has no current Round"))
    })?;
    let failed = mission
        .rounds
        .iter()
        .find(|round| round.number == round_number)
        .and_then(|round| round.phase_evidence.get(stage))
        .and_then(Value::as_object)
        .is_some_and(|evidence| {
            evidence
                .get("stage_failed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
    if !failed {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} has no retryable stage failure for {stage}"
        )));
    }
    let mut value = service.show_mission_value(mission_id)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RefineError::Serialization("Mission is not a JSON object".to_string()))?;
    let rounds = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| RefineError::Serialization("Mission has no rounds array".to_string()))?;
    let round = rounds
        .iter_mut()
        .find(|round| round.get("number").and_then(Value::as_u64) == Some(round_number as u64))
        .ok_or_else(|| {
            RefineError::NotFound(format!("Mission Round {round_number} was not found"))
        })?;
    let round_object = round.as_object_mut().ok_or_else(|| {
        RefineError::Serialization("MissionRound is not a JSON object".to_string())
    })?;
    let mut phase_evidence = round_object
        .get("phase_evidence")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(stage_entry) = phase_evidence.get_mut(stage).and_then(Value::as_object_mut) {
        stage_entry.insert("retry_authorized".to_string(), Value::Bool(true));
        stage_entry.insert(
            "retry_authorized_at".to_string(),
            Value::String(
                crate::application::missions::service::FileMissionService::now_timestamp(),
            ),
        );
    }
    round_object.insert("phase_evidence".to_string(), Value::Object(phase_evidence));
    let written = crate::application::missions::persistence::write_mission_atomically(
        &service.refine_dir,
        mission_id,
        &value,
    )?;
    crate::application::missions::persistence::parse_mission(&written)
}
