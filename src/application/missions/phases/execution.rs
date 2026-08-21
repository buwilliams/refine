//! The Execute phase: Goal materialization from the approved plan and wave
//! admission.
//!
//! Plan approval is one authorization: it permits the Mission engine to
//! materialize Goals idempotently and distribute eligible work according to
//! the approved plan. Materialization is keyed by `(mission_id,
//! mission_goal_key)` so a crash after creation recovers by scanning the
//! stable key instead of creating duplicates. Admission compiles each Goal's
//! capsule from the required snapshot, pins it onto the GoalRound, and moves
//! the Goal to Todo through ordinary work-item behavior. See
//! `docs/mission-spec.md` ("Execute").

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::application::missions::reconciliation::capsule::CapsuleManifest;
use crate::application::missions::service::FileMissionService;
use crate::application::work_items::FileWorkItemService;
use crate::application::work_items::{FeatureGoalPlacement, GoalAuthoringRequest};
use crate::error::{RefineError, RefineResult};
use crate::model::mission::{GoalRoundMissionContext, Mission, MissionWave};
use crate::model::workflow::GoalStatus;

use super::current_round;

/// One materialized or reused Goal of a wave.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterializedGoal {
    pub mission_goal_key: String,
    pub goal_id: String,
    pub created: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MaterializationReport {
    pub wave: usize,
    pub goals: Vec<MaterializedGoal>,
}

/// The approved wave of a Mission Round, failing closed when the plan has
/// not been approved (no plan, or no recorded approval evidence).
pub fn approved_wave(mission: &Mission, wave: usize) -> RefineResult<&MissionWave> {
    let round = current_round(mission)?;
    let plan = round.plan.as_ref().ok_or_else(|| {
        RefineError::Conflict(format!(
            "Mission {} Round {} has no plan",
            mission.id, round.number
        ))
    })?;
    if round
        .phase_evidence
        .get("plan_approval")
        .map(|approval| approval.is_null())
        .unwrap_or(true)
    {
        return Err(RefineError::Conflict(format!(
            "Mission {} Round {} plan has not been approved",
            mission.id, round.number
        )));
    }
    plan.waves
        .iter()
        .find(|candidate| candidate.number == wave)
        .ok_or_else(|| {
            RefineError::NotFound(format!(
                "Mission {} Round {} has no wave {wave}",
                mission.id, round.number
            ))
        })
}

/// Materialize the Goals of one approved wave. Idempotent: existing bindings
/// are reused, duplicates are a conflict, and creation happens through the
/// ordinary Goal authoring behavior.
pub fn materialize_wave_goals(
    mission_service: &FileMissionService,
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: usize,
) -> RefineResult<MaterializationReport> {
    let mission = mission_service.show_mission(mission_id)?;
    let approved = approved_wave(&mission, wave)?;
    let specs = approved.goal_specs.clone();
    let reporter = mission
        .reporter
        .clone()
        .unwrap_or_else(|| format!("mission:{mission_id}"));

    let mut bindings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for goal in mission_bound_goals(&work_items.refine_dir, mission_id)? {
        bindings
            .entry(goal.mission_goal_key.clone())
            .or_default()
            .push(goal.goal_id.clone());
    }

    let mut report = MaterializationReport {
        wave,
        goals: Vec::new(),
    };
    for spec in &specs {
        let existing = bindings
            .get(&spec.mission_goal_key)
            .cloned()
            .unwrap_or_default();
        match existing.len() {
            1 => {
                report.goals.push(MaterializedGoal {
                    mission_goal_key: spec.mission_goal_key.clone(),
                    goal_id: existing[0].clone(),
                    created: false,
                });
            }
            0 => {
                let request = GoalAuthoringRequest {
                    id: None,
                    goal_id: None,
                    name: Some(spec.name.clone()),
                    prompt: spec.prompt.clone(),
                    reporter: reporter.clone(),
                    assignee: None,
                    priority: "medium".to_string(),
                    feature_id: spec.feature_id.clone(),
                    placement: FeatureGoalPlacement::Unordered,
                    duplicate_decision: String::new(),
                };
                let created = work_items.author_goal(request)?;
                let goal_id = created
                    .goal
                    .as_ref()
                    .map(|goal| goal.id.clone())
                    .ok_or_else(|| {
                        RefineError::Conflict(format!(
                            "Goal authoring for Mission {mission_id} key {} returned no Goal",
                            spec.mission_goal_key
                        ))
                    })?;
                work_items.bind_goal_to_mission(&goal_id, mission_id, &spec.mission_goal_key)?;
                report.goals.push(MaterializedGoal {
                    mission_goal_key: spec.mission_goal_key.clone(),
                    goal_id,
                    created: true,
                });
            }
            _ => {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} goal key {} matches {} Goals; the stable key must be unique",
                    spec.mission_goal_key,
                    existing.len()
                )));
            }
        }
    }
    Ok(report)
}

/// One admitted Goal of a wave, with the reason when admission deferred.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AdmittedGoal {
    pub mission_goal_key: String,
    pub goal_id: String,
    pub admitted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AdmissionReport {
    pub wave: usize,
    pub goals: Vec<AdmittedGoal>,
}

/// Admit one wave: compile each Goal's capsule from the wave's required
/// snapshot, pin it onto the GoalRound, and move the Goal to Todo. A Goal
/// whose capsule is blocked by load-bearing contested knowledge does not
/// admit; the deferral is recorded, never resolved silently.
pub fn admit_wave(
    mission_service: &FileMissionService,
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: usize,
) -> RefineResult<AdmissionReport> {
    let mission = mission_service.show_mission(mission_id)?;
    let approved = approved_wave(&mission, wave)?;
    let specs = approved.goal_specs.clone();
    let round = current_round(&mission)?;
    let required_snapshot = approved.required_snapshot.unwrap_or_else(|| {
        round
            .snapshots
            .last()
            .map(|snapshot| snapshot.version)
            .unwrap_or(1)
    });
    let snapshot = round
        .snapshots
        .iter()
        .find(|candidate| candidate.version == required_snapshot)
        .ok_or_else(|| {
            RefineError::NotFound(format!(
                "Mission {mission_id} has no snapshot {required_snapshot} required by wave {wave}"
            ))
        })?;

    let bindings: BTreeMap<String, String> =
        mission_bound_goals(&work_items.refine_dir, mission_id)?
            .into_iter()
            .map(|goal| (goal.mission_goal_key, goal.goal_id))
            .collect();

    let mut report = AdmissionReport {
        wave,
        goals: Vec::new(),
    };
    for spec in &specs {
        let Some(goal_id) = bindings.get(&spec.mission_goal_key) else {
            report.goals.push(AdmittedGoal {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id: String::new(),
                admitted: false,
                reason: Some("goal was not materialized".to_string()),
            });
            continue;
        };
        let summary = work_items.show_goal_summary(goal_id)?;
        if summary.goal.status != GoalStatus::Backlog {
            report.goals.push(AdmittedGoal {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id: goal_id.clone(),
                admitted: false,
                reason: Some(format!("goal is {}", summary.goal.status.as_str())),
            });
            continue;
        }
        let capsule =
            mission_service.compile_context_capsule(&mission, snapshot, &spec.mission_goal_key)?;
        let manifest: CapsuleManifest = serde_json::from_value(capsule["capsule_manifest"].clone())
            .map_err(|error| {
                RefineError::Serialization(format!("failed to decode capsule manifest: {error}"))
            })?;
        if !manifest.blocking.is_empty() {
            report.goals.push(AdmittedGoal {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id: goal_id.clone(),
                admitted: false,
                reason: Some(format!(
                    "capsule blocked by contested knowledge: {}",
                    manifest.blocking.join(", ")
                )),
            });
            continue;
        }
        let context = GoalRoundMissionContext {
            mission_id: mission.id.clone(),
            mission_round: mission.current_round.unwrap_or(0),
            snapshot_version: snapshot.version,
            snapshot_digest: snapshot.digest.clone(),
            capsule_digest: capsule["capsule_manifest_digest"]
                .as_str()
                .map(str::to_string),
            capsule_manifest_digest: capsule["capsule_manifest_digest"]
                .as_str()
                .map(str::to_string),
        };
        work_items.pin_goal_mission_context(goal_id, &context, &capsule)?;
        work_items.transition_goal_status(goal_id, GoalStatus::Todo)?;
        report.goals.push(AdmittedGoal {
            mission_goal_key: spec.mission_goal_key.clone(),
            goal_id: goal_id.clone(),
            admitted: true,
            reason: None,
        });
    }
    Ok(report)
}

/// One Mission-bound Goal discovered in Goal state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionGoal {
    pub goal_id: String,
    pub mission_goal_key: String,
    pub status: GoalStatus,
}

/// Scan Goal state for the Goals bound to one Mission. Mission membership is
/// Goal-owned, so this projection walks Goal records — the Mission never
/// keeps a competing member list.
pub fn mission_bound_goals(refine_dir: &Path, mission_id: &str) -> RefineResult<Vec<MissionGoal>> {
    let mission_id = mission_id.trim().to_uppercase();
    let mut goals = Vec::new();
    let root = refine_dir.join("goals");
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("goal.json") {
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                let bound = value
                    .get("mission")
                    .filter(|mission| !mission.is_null())
                    .and_then(|mission| {
                        let id = mission.get("mission_id")?.as_str()?;
                        let key = mission.get("mission_goal_key")?.as_str()?;
                        Some((id.to_uppercase(), key.to_string()))
                    });
                let Some((bound_id, key)) = bound else {
                    continue;
                };
                if bound_id != mission_id {
                    continue;
                }
                let status = value
                    .get("status")
                    .and_then(Value::as_str)
                    .and_then(GoalStatus::parse_wire)
                    .unwrap_or(GoalStatus::Backlog);
                let goal_id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                goals.push(MissionGoal {
                    goal_id,
                    mission_goal_key: key,
                    status,
                });
            }
        }
    }
    goals.sort_by(|a, b| a.goal_id.cmp(&b.goal_id));
    Ok(goals)
}

/// The settlement-relevant state of one Mission-bound Goal.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WaveGoalObservation {
    pub mission_goal_key: String,
    pub goal_id: String,
    pub required: bool,
    pub state: crate::application::missions::reconciliation::settlement::GoalWaveState,
}

/// Observe the wave Goals' settlement state from durable Goal records.
pub fn observe_wave_goals(
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: usize,
    optional_wait_exceeded: &[String],
) -> RefineResult<Vec<WaveGoalObservation>> {
    use crate::application::missions::reconciliation::settlement::GoalWaveState;

    let mission_bound = mission_bound_goals(&work_items.refine_dir, mission_id)?
        .into_iter()
        .map(|goal| (goal.mission_goal_key.clone(), goal))
        .collect::<BTreeMap<_, _>>();
    let mission = FileMissionService::new(&work_items.refine_dir).show_mission(mission_id)?;
    let approved = approved_wave(&mission, wave)?;
    let mut observations = Vec::new();
    for spec in &approved.goal_specs {
        let Some(bound) = mission_bound.get(&spec.mission_goal_key) else {
            // An unmaterialized Goal is pending: a wave is settled only when
            // every planned Goal is terminal, Review-ready, or a
            // wait-exceeded optional Goal.
            observations.push(WaveGoalObservation {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id: String::new(),
                required: spec.required,
                state: GoalWaveState::Pending,
            });
            continue;
        };
        let state = if optional_wait_exceeded.contains(&spec.mission_goal_key) {
            GoalWaveState::WaitExceeded
        } else {
            match bound.status {
                GoalStatus::Done | GoalStatus::Failed | GoalStatus::Cancelled => {
                    GoalWaveState::Terminal
                }
                GoalStatus::Review => {
                    let evidence_valid = goal_evidence_valid(work_items, &bound.goal_id)?;
                    if evidence_valid {
                        GoalWaveState::ReviewReady
                    } else {
                        GoalWaveState::Pending
                    }
                }
                _ => GoalWaveState::Pending,
            }
        };
        observations.push(WaveGoalObservation {
            mission_goal_key: spec.mission_goal_key.clone(),
            goal_id: bound.goal_id.clone(),
            required: spec.required,
            state,
        });
    }
    Ok(observations)
}

/// Whether a Goal in Review holds valid integration, Quality, and
/// Governance evidence on its latest Round.
fn goal_evidence_valid(work_items: &FileWorkItemService, goal_id: &str) -> RefineResult<bool> {
    let detail = work_items.show_goal_detail(goal_id)?;
    let Some(rounds) = detail.get("rounds").and_then(Value::as_array) else {
        return Ok(false);
    };
    let Some(round) = rounds.last() else {
        return Ok(false);
    };
    Ok(
        round.get("quality_state").and_then(Value::as_str) == Some("passed")
            && round.get("rule_state").and_then(Value::as_str) == Some("passed")
            && round
                .get("workflow_integration")
                .map(|integration| !integration.is_null())
                .unwrap_or(false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_bound_goal_scan_ignores_unbound_goals() {
        let dir = std::env::temp_dir().join(format!(
            "refine-mission-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let goal_dir = dir.join("goals").join("GO").join("AL1");
        std::fs::create_dir_all(&goal_dir).unwrap();
        std::fs::write(
            goal_dir.join("goal.json"),
            serde_json::json!({
                "id": "GOAL1",
                "status": "review",
                "mission": {"mission_id": "MTEST", "mission_goal_key": "k1"}
            })
            .to_string(),
        )
        .unwrap();
        let other_dir = dir.join("goals").join("GO").join("AL2");
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::write(
            other_dir.join("goal.json"),
            serde_json::json!({"id": "GOAL2", "status": "todo"}).to_string(),
        )
        .unwrap();
        let goals = mission_bound_goals(&dir, "MTEST").unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].goal_id, "GOAL1");
        assert_eq!(goals[0].status, GoalStatus::Review);
        assert_eq!(goals[0].mission_goal_key, "k1");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
