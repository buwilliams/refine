//! Port-local facts emitted by the scheduling thread. No health or restart policy here.
use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SchedulerObservation {
    pub runtime_root: PathBuf,
    pub process_id: String,
    pub pid: u32,
    pub os_identity: String,
    pub incarnation: String,
    pub target_root: Option<PathBuf>,
    pub node_id: Option<String>,
    pub sequence: u64,
    pub tick_ms: i64,
    pub completed_cycle_ms: Option<i64>,
    pub active_attempts: BTreeSet<String>,
    pub failure: Option<String>,
    #[serde(default)]
    pub retry_delays: std::collections::BTreeMap<String, i64>,
}

pub fn workflow_incarnation(process: &ManagedProcess) -> Option<String> {
    serde_json::from_str::<Value>(process.details.as_deref()?)
        .ok()?
        .get("workflow_incarnation")?
        .as_str()
        .map(str::to_string)
}
pub fn is_workflow_worker(process: &ManagedProcess) -> bool {
    process.owner == ProcessOwner::Runner
        && serde_json::from_str::<Value>(process.details.as_deref().unwrap_or(""))
            .ok()
            .is_some_and(|v| v["worker_kind"] == "workflow")
}
pub fn scheduler_observation_path(root: &Path, incarnation: &str) -> RefineResult<PathBuf> {
    uuid::Uuid::parse_str(incarnation)
        .map_err(|_| RefineError::Conflict("invalid workflow incarnation".into()))?;
    Ok(root
        .join("workflow-health")
        .join(format!("{incarnation}.json")))
}
impl SchedulerObservation {
    pub fn write(&self, root: &Path) -> RefineResult<()> {
        let path = scheduler_observation_path(root, &self.incarnation)?;
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| RefineError::Io(e.to_string()))?;
        write_json_atomically_transient(
            &path,
            &serde_json::to_vec(self).map_err(|e| RefineError::Serialization(e.to_string()))?,
            "scheduler observation",
        )
    }
    pub fn read(root: &Path, incarnation: &str) -> RefineResult<Self> {
        let path = scheduler_observation_path(root, incarnation)?;
        let bytes = fs::read(path).map_err(|e| RefineError::Io(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| RefineError::Serialization(e.to_string()))
    }
}

pub fn current_os_identity(pid: u32) -> RefineResult<Option<String>> {
    os_process_identity(pid)
}
