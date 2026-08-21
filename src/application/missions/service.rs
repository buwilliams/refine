use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;

use crate::error::{RefineError, RefineResult};
use crate::model::mission::{
    Mission, MissionCriteriaSummary, MissionIndexProjection, MissionRollup, MissionStatus,
    mission_status_transition,
};

use super::persistence::*;

/// Serializes mutations of one Mission against other writers of that Mission.
pub trait MissionService {
    fn create_mission(&self, mission: Mission) -> RefineResult<Mission>;
    fn list_missions(&self) -> RefineResult<Vec<Mission>>;
    fn show_mission(&self, mission_id: &str) -> RefineResult<Mission>;
    fn update_mission(&self, mission: Mission) -> RefineResult<Mission>;
}

#[derive(Clone, Debug)]
pub struct FileMissionService {
    pub refine_dir: PathBuf,
}

impl FileMissionService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub(crate) fn now_timestamp() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    /// Create a new Draft Mission.
    pub fn create_mission(
        &self,
        name: &str,
        intent: &str,
        reporter: Option<&str>,
        coordinator_node_id: Option<&str>,
        id: Option<&str>,
    ) -> RefineResult<Mission> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RefineError::InvalidInput(
                "Mission name is required".to_string(),
            ));
        }
        let intent = intent.trim();
        if intent.is_empty() {
            return Err(RefineError::InvalidInput(
                "Mission intent is required".to_string(),
            ));
        }
        let mission_id = id
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_mission_id);
        if mission_id.len() < 3 {
            return Err(RefineError::InvalidInput(
                "Mission id must be at least three characters".to_string(),
            ));
        }
        let path = mission_json_path(&self.refine_dir, &mission_id);
        if path.exists() {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} already exists"
            )));
        }
        let now = Self::now_timestamp();
        let value = new_mission_value(
            &mission_id,
            name,
            intent,
            reporter,
            coordinator_node_id,
            &now,
        );
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Read one Mission by id.
    pub fn show_mission(&self, mission_id: &str) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let value = read_mission_value(&self.refine_dir, &mission_id)?.ok_or_else(|| {
            RefineError::NotFound(format!(
                "Mission {mission_id} was not found in refine state"
            ))
        })?;
        parse_mission(&value)
    }

    /// Read one Mission's raw record value.
    pub fn show_mission_value(&self, mission_id: &str) -> RefineResult<Value> {
        let mission_id = mission_id.trim().to_uppercase();
        read_mission_value(&self.refine_dir, &mission_id)?.ok_or_else(|| {
            RefineError::NotFound(format!(
                "Mission {mission_id} was not found in refine state"
            ))
        })
    }

    /// List every Mission record.
    pub fn list_missions(&self) -> RefineResult<Vec<Mission>> {
        let mut missions = Vec::new();
        for path in collect_mission_paths(&self.refine_dir)? {
            let value = read_json(&path)?;
            if let Ok(mission) = parse_mission(&value) {
                missions.push(mission);
            }
        }
        missions.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(missions)
    }

    /// List Mission index projections.
    pub fn list_mission_projections(&self) -> RefineResult<Vec<MissionIndexProjection>> {
        let mut projections = self
            .list_missions()?
            .into_iter()
            .map(|mission| self.project_mission(&mission))
            .collect::<Vec<_>>();
        projections.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(projections)
    }

    /// Derive the index projection for one Mission.
    pub fn project_mission(&self, mission: &Mission) -> MissionIndexProjection {
        let current_round = mission.current_round;
        let current_wave = current_round
            .and_then(|round| mission.rounds.iter().find(|r| r.number == round))
            .and_then(|round| round.plan.as_ref())
            .and_then(|plan| plan.waves.first())
            .map(|wave| wave.number);
        let criteria_summary = mission_criteria_summary(mission);
        let outcome_available = mission.rounds.iter().any(|round| round.outcome.is_some());
        MissionIndexProjection {
            id: mission.id.clone(),
            name: mission.name.clone(),
            status: mission.status.clone(),
            reporter: mission.reporter.clone(),
            assignee: mission.assignee.clone(),
            coordinator_node_id: mission.coordinator_node_id.clone(),
            current_round,
            current_wave,
            criteria_summary,
            outcome_available,
            created: mission.created.clone(),
            updated: mission.updated.clone(),
            json_path: mission_json_path(&self.refine_dir, &mission.id)
                .strip_prefix(&self.refine_dir)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default(),
        }
    }

    /// Derive the rollup of contained Goals for one Mission.
    pub fn mission_rollup(
        &self,
        mission_id: &str,
        goals: &[crate::model::goal::GoalIndexProjection],
    ) -> MissionRollup {
        let contained: Vec<_> = goals
            .iter()
            .filter(|goal| {
                goal.mission
                    .as_ref()
                    .is_some_and(|binding| binding.mission_id == mission_id)
            })
            .collect();
        let mut rollup = MissionRollup {
            goal_count: contained.len(),
            ..MissionRollup::default()
        };
        for goal in contained {
            use crate::model::workflow::GoalStatus;
            match goal.status {
                GoalStatus::Done => rollup.done_count += 1,
                GoalStatus::Failed => {
                    rollup.failed_count += 1;
                    rollup.required_failures += 1;
                }
                GoalStatus::Cancelled => rollup.cancelled_count += 1,
                GoalStatus::Plan
                | GoalStatus::Implement
                | GoalStatus::Quality
                | GoalStatus::Governance
                | GoalStatus::Review => rollup.active_count += 1,
                GoalStatus::Backlog | GoalStatus::Todo => {}
            }
        }
        rollup
    }

    /// Transition a Mission to a new status, enforcing the status policy.
    pub fn transition_mission(
        &self,
        mission_id: &str,
        target: MissionStatus,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        let current_status = value
            .get("status")
            .and_then(Value::as_str)
            .and_then(MissionStatus::parse_wire)
            .unwrap_or(MissionStatus::Draft);
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        if !mission_status_transition(&current_status, &target) {
            return Err(RefineError::InvalidInput(format!(
                "Mission transition {} -> {} is not allowed",
                current_status.as_str(),
                target.as_str()
            )));
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        object.insert(
            "status".to_string(),
            Value::String(target.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }

    /// Edit the editable Draft frame (name, intent, criteria, artifact contract).
    pub fn edit_mission_frame(
        &self,
        mission_id: &str,
        name: Option<&str>,
        intent: Option<&str>,
        success_criteria: Option<&Value>,
        artifact_contract: Option<&Value>,
        observed_revision: Option<u64>,
    ) -> RefineResult<Mission> {
        let mission_id = mission_id.trim().to_uppercase();
        let mut value = self.show_mission_value(&mission_id)?;
        if let Some(observed) = observed_revision {
            let current_revision = mission_revision(&value);
            if current_revision != observed {
                return Err(RefineError::Conflict(format!(
                    "Mission {mission_id} changed after it was read (expected revision {observed}, current revision {current_revision})"
                )));
            }
        }
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Mission {mission_id} is not a JSON object"))
        })?;
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                return Err(RefineError::InvalidInput(
                    "Mission name cannot be empty".to_string(),
                ));
            }
            object.insert("name".to_string(), Value::String(name.to_string()));
        }
        if let Some(intent) = intent {
            object.insert("intent".to_string(), Value::String(intent.to_string()));
        }
        if let Some(criteria) = success_criteria {
            validate_criteria(criteria)?;
            object.insert("success_criteria".to_string(), criteria.clone());
        }
        if let Some(contract) = artifact_contract {
            validate_artifact_contract(contract)?;
            object.insert("artifact_contract".to_string(), contract.clone());
        }
        object.insert("updated".to_string(), Value::String(Self::now_timestamp()));
        let written = write_mission_atomically(&self.refine_dir, &mission_id, &value)?;
        parse_mission(&written)
    }
}

impl MissionService for FileMissionService {
    fn create_mission(&self, mission: Mission) -> RefineResult<Mission> {
        self.create_mission(
            &mission.name,
            &mission.intent,
            mission.reporter.as_deref(),
            mission.coordinator_node_id.as_deref(),
            Some(&mission.id),
        )
    }

    fn list_missions(&self) -> RefineResult<Vec<Mission>> {
        self.list_missions()
    }

    fn show_mission(&self, mission_id: &str) -> RefineResult<Mission> {
        self.show_mission(mission_id)
    }

    fn update_mission(&self, mission: Mission) -> RefineResult<Mission> {
        let value = serde_json::to_value(&mission).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Mission: {error}"))
        })?;
        write_mission_atomically(&self.refine_dir, &mission.id, &value)?;
        Ok(mission)
    }
}

fn collect_mission_paths(refine_dir: &Path) -> RefineResult<Vec<PathBuf>> {
    let root = refine_dir.join("missions");
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    collect_mission_paths_inner(&root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_mission_paths_inner(root: &Path, files: &mut Vec<PathBuf>) -> RefineResult<()> {
    for entry in fs::read_dir(root).map_err(|error| {
        RefineError::Io(format!(
            "failed to read directory {}: {error}",
            root.display()
        ))
    })? {
        let entry = entry
            .map_err(|error| RefineError::Io(format!("failed to read directory entry: {error}")))?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!("failed to stat {}: {error}", path.display()))
        })?;
        if metadata.is_dir() {
            collect_mission_paths_inner(&path, files)?;
        } else if metadata.is_file()
            && path.file_name().and_then(|name| name.to_str()) == Some("mission.json")
        {
            files.push(path);
        }
    }
    Ok(())
}

fn read_json(path: &Path) -> RefineResult<Value> {
    let bytes = fs::read(path)
        .map_err(|error| RefineError::Io(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
    })
}

fn new_mission_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut value = (now.as_millis() << 64)
        ^ ((now.subsec_nanos() as u128) << 32)
        ^ ((std::process::id() as u128) << 16)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let mut chars = [b'0'; 26];
    for idx in (0..26).rev() {
        chars[idx] = ALPHABET[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).unwrap()
}

fn mission_criteria_summary(mission: &Mission) -> MissionCriteriaSummary {
    let mut summary = MissionCriteriaSummary {
        total: mission.success_criteria.len(),
        ..MissionCriteriaSummary::default()
    };
    if let Some(round) = mission
        .current_round
        .and_then(|round| mission.rounds.iter().find(|r| r.number == round))
        && let Some(review) = &round.review
    {
        for result in &review.criteria_results {
            match result.result {
                crate::model::mission::CriterionOutcome::Met => summary.met += 1,
                crate::model::mission::CriterionOutcome::Partial => summary.partial += 1,
                crate::model::mission::CriterionOutcome::Unmet => summary.unmet += 1,
                crate::model::mission::CriterionOutcome::Contradicted => summary.contradicted += 1,
                crate::model::mission::CriterionOutcome::Waived => summary.waived += 1,
            }
        }
    }
    summary
}

fn validate_criteria(value: &Value) -> RefineResult<()> {
    let criteria = value.as_array().ok_or_else(|| {
        RefineError::InvalidInput("success_criteria must be an array".to_string())
    })?;
    let mut seen = std::collections::BTreeSet::new();
    for criterion in criteria {
        let id = criterion.get("id").and_then(Value::as_str).ok_or_else(|| {
            RefineError::InvalidInput("each criterion requires a string id".to_string())
        })?;
        if !seen.insert(id.to_string()) {
            return Err(RefineError::InvalidInput(format!(
                "duplicate criterion id {id}"
            )));
        }
    }
    Ok(())
}

fn validate_artifact_contract(value: &Value) -> RefineResult<()> {
    let contract = value.as_array().ok_or_else(|| {
        RefineError::InvalidInput("artifact_contract must be an array".to_string())
    })?;
    let mut seen = std::collections::BTreeSet::new();
    for obligation in contract {
        let key = obligation
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RefineError::InvalidInput(
                    "each artifact obligation requires a string key".to_string(),
                )
            })?;
        if !seen.insert(key.to_string()) {
            return Err(RefineError::InvalidInput(format!(
                "duplicate artifact key {key}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-mission-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn create_and_show_mission() {
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
        assert_eq!(mission.status, MissionStatus::Draft);
        // Mutations return the authoritative read-back: the revision that
        // was durably written, not the pre-write value.
        assert_eq!(mission.revision, 1);

        let shown = service.show_mission(&mission.id).unwrap();
        assert_eq!(shown.name, "Modernize auth");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn transition_enforces_policy() {
        let dir = temp_dir();
        let service = FileMissionService::new(&dir);
        let mission = service
            .create_mission("M", "intent", None, None, None)
            .unwrap();
        let err = service
            .transition_mission(&mission.id, MissionStatus::Execute, None)
            .unwrap_err();
        assert!(err.to_string().contains("not allowed"));

        let investigated = service
            .transition_mission(&mission.id, MissionStatus::Investigate, None)
            .unwrap();
        assert_eq!(investigated.status, MissionStatus::Investigate);
        assert_eq!(investigated.revision, 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_revision_is_rejected() {
        let dir = temp_dir();
        let service = FileMissionService::new(&dir);
        let mission = service
            .create_mission("M", "intent", None, None, None)
            .unwrap();
        service
            .transition_mission(&mission.id, MissionStatus::Investigate, None)
            .unwrap();
        let err = service
            .transition_mission(&mission.id, MissionStatus::Plan, Some(0))
            .unwrap_err();
        assert!(err.to_string().contains("changed after it was read"));
        fs::remove_dir_all(dir).unwrap();
    }
}
