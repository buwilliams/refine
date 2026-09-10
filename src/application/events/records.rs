//! Durable occurrence records and recoverable history indexes.
use super::FileEventService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::infrastructure::storage::automation::{read_json, write_json};
use crate::model::automation::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InvocationContext {
    pub node_id: String,
    pub target_root: PathBuf,
    pub cwd: PathBuf,
    pub provider: String,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub round_idx: Option<usize>,
    #[serde(default)]
    pub workflow_revision: Option<u64>,
    #[serde(default)]
    pub candidate_commit: Option<String>,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Error,
    Cancelled,
}

impl InvocationState {
    pub fn terminal(&self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PinnedBinding {
    pub binding: Binding,
    pub skill: Skill,
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EventInvocation {
    pub id: String,
    pub event: EventDefinition,
    pub config_revision: u64,
    pub context: InvocationContext,
    pub bindings: Vec<PinnedBinding>,
    pub state: InvocationState,
    pub results: BTreeMap<String, SkillResult>,
    pub attempts: Vec<Value>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub action_applied: bool,
}

impl FileEventService {
    pub fn invocation_path(&self, id: &str) -> RefineResult<PathBuf> {
        if !valid_id(id) {
            return Err(RefineError::InvalidInput("invalid invocation ID".into()));
        }
        Ok(self
            .refine_dir
            .join("automation/invocations")
            .join(format!("{id}.json")))
    }
    pub fn invocation(&self, id: &str) -> RefineResult<EventInvocation> {
        read_json(&self.invocation_path(id)?)
    }
    pub fn save_invocation(&self, invocation: &EventInvocation) -> RefineResult<()> {
        with_record_lock(
            &self.refine_dir,
            &format!("event-{}", invocation.id),
            || {
                let path = self.invocation_path(&invocation.id)?;
                if path.exists() {
                    let current = self.invocation(&invocation.id)?;
                    if current.state == InvocationState::Cancelled
                        && invocation.state != InvocationState::Cancelled
                    {
                        return Err(RefineError::Conflict(
                            "Event invocation was cancelled".into(),
                        ));
                    }
                    if current.state.terminal() && !invocation.state.terminal() {
                        return Err(RefineError::Conflict(
                            "Event invocation is already settled".into(),
                        ));
                    }
                }
                let journal = self
                    .refine_dir
                    .join("automation/index-updates")
                    .join(&invocation.context.node_id)
                    .join(format!("{}.json", invocation.id));
                write_json(&journal, &json!({"id": invocation.id}))?;
                write_json(&path, invocation)?;
                self.write_invocation_indexes(invocation)?;
                remove_if_present(&journal)
            },
        )
    }

    fn write_invocation_indexes(&self, invocation: &EventInvocation) -> RefineResult<()> {
        let history = self.refine_dir.join("automation/history").join(format!(
            "{}-{}.json",
            invocation.created_at.replace(':', "-"),
            invocation.id
        ));
        write_json(
            &history,
            &json!({"id":invocation.id, "event":{"id":invocation.event.id,"name":invocation.event.name,"kind":invocation.event.kind,"source":invocation.event.source},"config_revision":invocation.config_revision,"state":invocation.state,"created_at":invocation.created_at,"completed_at":invocation.completed_at,"goal_id":invocation.context.goal_id,"node_id":invocation.context.node_id,"error":invocation.error,"result_count":invocation.results.len(),"gate":invocation.gate_assessment(),"execution_state":invocation.execution_state()}),
        )?;
        if let Some(goal_id) = &invocation.context.goal_id {
            if !valid_id(goal_id) {
                return Err(RefineError::InvalidInput("invalid Event Goal ID".into()));
            }
            let goal_history = self
                .refine_dir
                .join("automation/goal-history")
                .join(goal_id)
                .join(history.file_name().expect("history name"));
            let summary: Value = read_json(&history)?;
            write_json(&goal_history, &summary)?;
        }
        let pending = self
            .refine_dir
            .join("automation/pending")
            .join(&invocation.context.node_id)
            .join(format!("{}.json", invocation.id));
        if invocation.state.terminal() && !invocation.success_action_ready() {
            remove_if_present(&pending)?;
            if self.runtime_root.is_some() {
                self.record_wait(&invocation.id, None)?;
            }
        } else {
            write_json(
                &pending,
                &json!({"id": invocation.id, "node_id": invocation.context.node_id}),
            )?;
        }
        Ok(())
    }

    /// Recover only interrupted index updates, never scan historical invocation blobs.
    pub(crate) fn repair_invocation_indexes(&self, node: Option<&str>) -> RefineResult<()> {
        let root = self.refine_dir.join("automation/index-updates");
        if !root.exists() {
            return Ok(());
        }
        let directories = if let Some(node) = node {
            vec![root.join(node)]
        } else {
            std::fs::read_dir(&root)
                .map_err(|e| RefineError::Io(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| RefineError::Io(e.to_string()))?
                .into_iter()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        };
        let mut remaining = 128;
        for directory in directories {
            if !directory.exists() {
                continue;
            }
            for entry in std::fs::read_dir(directory)
                .map_err(|e| RefineError::Io(e.to_string()))?
                .take(remaining)
            {
                let journal = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
                if journal.extension().and_then(|v| v.to_str()) != Some("json") {
                    continue;
                }
                let id = journal
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| {
                        RefineError::Serialization("missing invocation index ID".into())
                    })?;
                self.invocation_path(id)?;
                with_record_lock(&self.refine_dir, &format!("event-{id}"), || {
                    if !journal.exists() {
                        return Ok(());
                    }
                    if self.invocation_path(id)?.exists() {
                        self.write_invocation_indexes(&self.invocation(id)?)?;
                    }
                    remove_if_present(&journal)
                })?;
                remaining -= 1;
            }
            if remaining == 0 {
                break;
            }
        }
        Ok(())
    }

    pub fn invocations(&self, offset: usize, limit: usize) -> RefineResult<Value> {
        self.repair_invocation_indexes(None)?;
        self.read_invocation_history(self.refine_dir.join("automation/history"), offset, limit)
    }

    pub fn goal_invocations(
        &self,
        goal_id: &str,
        offset: usize,
        limit: usize,
    ) -> RefineResult<Value> {
        if !valid_id(goal_id) {
            return Err(RefineError::InvalidInput("invalid Goal ID".into()));
        }
        self.repair_invocation_indexes(None)?;
        self.read_invocation_history(
            self.refine_dir
                .join("automation/goal-history")
                .join(goal_id),
            offset,
            limit,
        )
    }

    fn read_invocation_history(
        &self,
        directory: PathBuf,
        offset: usize,
        limit: usize,
    ) -> RefineResult<Value> {
        if !directory.exists() {
            return Ok(json!({"items": [], "offset": offset, "total": 0}));
        }
        let mut paths: Vec<_> = std::fs::read_dir(directory)
            .map_err(|e| RefineError::Io(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| RefineError::Io(e.to_string()))?
            .into_iter()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort_by(|a, b| b.cmp(a));
        let total = paths.len();
        let mut items = paths
            .iter()
            .skip(offset)
            .take(limit.clamp(1, 100))
            .map(|p| read_json::<Value>(p))
            .collect::<RefineResult<Vec<_>>>()?;
        for item in &mut items {
            if item.get("execution_state").is_none()
                && let Some(id) = item["id"].as_str()
                && let Ok(invocation) = self.invocation(id)
            {
                item["execution_state"] = json!(invocation.execution_state());
                item["gate"] = json!(invocation.gate_assessment());
            }
            self.decorate_wait(item);
        }
        Ok(json!({"items": items, "offset": offset, "total": total}))
    }
}

fn remove_if_present(path: &std::path::Path) -> RefineResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RefineError::Io(e.to_string())),
    }
}
