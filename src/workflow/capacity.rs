use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::workflow::{WorkflowPolicy, now_timestamp};

pub const AGENT_CAPACITY_STATE_FILE: &str = "agent-capacity-state.json";
const AGENT_CAPACITY_LOCK_FILE: &str = ".agent-capacity.lock";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCapacityLease {
    pub owner_id: String,
    pub role: String,
    pub node_id: String,
    pub provider: String,
    pub target_app_id: String,
    pub holder_pid: u32,
    pub acquired_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCapacityState {
    #[serde(default)]
    pub leases: Vec<AgentCapacityLease>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCapacityRequest {
    pub owner_id: String,
    pub role: String,
    pub node_id: String,
    pub provider: String,
    pub target_app_id: String,
}

#[derive(Clone, Debug)]
pub struct AgentCapacityService {
    runtime_root: PathBuf,
}

impl AgentCapacityService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub fn try_acquire(
        &self,
        policy: &WorkflowPolicy,
        request: AgentCapacityRequest,
    ) -> RefineResult<bool> {
        let _guard = self.acquire_lock()?;
        let mut state = self.load_state()?;
        let pruned = prune_dead_leases(&mut state);
        if let Some(existing) = state
            .leases
            .iter()
            .find(|lease| lease.owner_id == request.owner_id)
        {
            let held_by_this_process = existing.holder_pid == std::process::id();
            if pruned {
                self.write_state(&state)?;
            }
            return Ok(held_by_this_process);
        }
        if !capacity_available(&state, policy, &request) {
            if pruned {
                self.write_state(&state)?;
            }
            return Ok(false);
        }
        state.leases.push(AgentCapacityLease {
            owner_id: request.owner_id,
            role: request.role,
            node_id: request.node_id,
            provider: request.provider,
            target_app_id: request.target_app_id,
            holder_pid: std::process::id(),
            acquired_at: now_timestamp(),
        });
        self.write_state(&state)?;
        Ok(true)
    }

    pub fn release(&self, owner_id: &str) -> RefineResult<bool> {
        let _guard = self.acquire_lock()?;
        let mut state = self.load_state()?;
        let before = state.leases.len();
        state.leases.retain(|lease| lease.owner_id != owner_id);
        let changed = prune_dead_leases(&mut state) || state.leases.len() != before;
        if changed {
            self.write_state(&state)?;
        }
        Ok(state.leases.len() != before)
    }

    pub fn snapshot(&self) -> RefineResult<AgentCapacityState> {
        let _guard = self.acquire_lock()?;
        let mut state = self.load_state()?;
        if prune_dead_leases(&mut state) {
            self.write_state(&state)?;
        }
        Ok(state)
    }

    fn load_state(&self) -> RefineResult<AgentCapacityState> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(AgentCapacityState::default());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read agent capacity state {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse agent capacity state {}: {error}",
                path.display()
            ))
        })
    }

    fn write_state(&self, state: &AgentCapacityState) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create agent capacity directory {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!("failed to encode agent capacity state: {error}"))
        })?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = self.runtime_root.join(format!(
            ".agent-capacity-{}-{sequence}.tmp",
            std::process::id()
        ));
        fs::write(&temp, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write agent capacity state {}: {error}",
                temp.display()
            ))
        })?;
        fs::rename(&temp, self.state_path()).map_err(|error| {
            RefineError::Io(format!("failed to publish agent capacity state: {error}"))
        })
    }

    fn state_path(&self) -> PathBuf {
        self.runtime_root.join(AGENT_CAPACITY_STATE_FILE)
    }

    fn acquire_lock(&self) -> RefineResult<AgentCapacityLock> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create agent capacity directory {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let path = self.runtime_root.join(AGENT_CAPACITY_LOCK_FILE);
        for _ in 0..500 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(AgentCapacityLock { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(30));
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to lock agent capacity state {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        Err(RefineError::Conflict(
            "agent capacity state is busy; retry shortly".to_string(),
        ))
    }
}

fn capacity_available(
    state: &AgentCapacityState,
    policy: &WorkflowPolicy,
    request: &AgentCapacityRequest,
) -> bool {
    let mut by_node = BTreeMap::<&str, usize>::new();
    let mut by_provider = BTreeMap::<&str, usize>::new();
    let mut by_target_app = BTreeMap::<&str, usize>::new();
    for lease in &state.leases {
        *by_node.entry(&lease.node_id).or_default() += 1;
        *by_provider.entry(&lease.provider).or_default() += 1;
        *by_target_app.entry(&lease.target_app_id).or_default() += 1;
    }
    state.leases.len() < policy.global_limit
        && by_node.get(request.node_id.as_str()).copied().unwrap_or(0) < policy.per_node_limit
        && by_provider
            .get(request.provider.as_str())
            .copied()
            .unwrap_or(0)
            < policy.per_provider_limit
        && by_target_app
            .get(request.target_app_id.as_str())
            .copied()
            .unwrap_or(0)
            < policy.per_target_app_limit
}

fn prune_dead_leases(state: &mut AgentCapacityState) -> bool {
    let before = state.leases.len();
    state
        .leases
        .retain(|lease| process_is_alive(lease.holder_pid));
    state.leases.len() != before
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

struct AgentCapacityLock {
    path: PathBuf,
}

impl Drop for AgentCapacityLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(owner_id: &str, role: &str) -> AgentCapacityRequest {
        AgentCapacityRequest {
            owner_id: owner_id.to_string(),
            role: role.to_string(),
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            target_app_id: "target".to_string(),
        }
    }

    fn policy(limit: usize) -> WorkflowPolicy {
        WorkflowPolicy {
            global_limit: limit,
            per_node_limit: limit,
            per_provider_limit: limit,
            per_target_app_limit: limit,
            active_node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            target_app_id: "target".to_string(),
        }
    }

    #[test]
    fn shared_capacity_serializes_roles_at_one_and_allows_two() {
        let root = std::env::temp_dir().join(format!(
            "refine-capacity-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let capacity = AgentCapacityService::new(&root);
        assert!(
            capacity
                .try_acquire(&policy(1), request("goal", "workflow"))
                .unwrap()
        );
        assert!(
            !capacity
                .try_acquire(&policy(1), request("supervisor", "supervisor"))
                .unwrap()
        );
        assert!(capacity.release("goal").unwrap());
        assert!(
            capacity
                .try_acquire(&policy(1), request("supervisor", "supervisor"))
                .unwrap()
        );
        assert!(capacity.release("supervisor").unwrap());
        assert!(
            capacity
                .try_acquire(&policy(2), request("goal", "workflow"))
                .unwrap()
        );
        assert!(
            capacity
                .try_acquire(&policy(2), request("supervisor", "supervisor"))
                .unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn acquisition_is_idempotent_and_release_recovers_the_slot() {
        let root = std::env::temp_dir().join(format!(
            "refine-capacity-recovery-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let capacity = AgentCapacityService::new(&root);
        assert!(
            capacity
                .try_acquire(&policy(1), request("supervisor", "supervisor"))
                .unwrap()
        );
        assert!(
            capacity
                .try_acquire(&policy(1), request("supervisor", "supervisor"))
                .unwrap()
        );
        assert_eq!(capacity.snapshot().unwrap().leases.len(), 1);
        assert!(capacity.release("supervisor").unwrap());
        assert!(
            capacity
                .try_acquire(&policy(1), request("goal", "workflow"))
                .unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dead_holder_lease_is_reclaimed_after_restart() {
        let root = std::env::temp_dir().join(format!(
            "refine-capacity-dead-holder-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let capacity = AgentCapacityService::new(&root);
        capacity
            .write_state(&AgentCapacityState {
                leases: vec![AgentCapacityLease {
                    owner_id: "abandoned-supervisor".to_string(),
                    role: "supervisor".to_string(),
                    node_id: "default".to_string(),
                    provider: "smoke-ai".to_string(),
                    target_app_id: "target".to_string(),
                    holder_pid: u32::MAX,
                    acquired_at: now_timestamp(),
                }],
            })
            .unwrap();

        assert!(
            capacity
                .try_acquire(&policy(1), request("goal", "workflow"))
                .unwrap()
        );
        let snapshot = capacity.snapshot().unwrap();
        assert_eq!(snapshot.leases.len(), 1);
        assert_eq!(snapshot.leases[0].owner_id, "goal");
        fs::remove_dir_all(root).unwrap();
    }
}
