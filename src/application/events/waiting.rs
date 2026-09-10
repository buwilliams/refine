//! Waiting is node-local execution information, not synchronized workflow authority.
use super::FileEventService;
use crate::error::RefineResult;
use crate::infrastructure::storage::automation::{read_json, write_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WaitReason {
    Capacity,
    WorkspaceBusy,
    Paused,
}

impl FileEventService {
    fn wait_path(&self, id: &str) -> RefineResult<std::path::PathBuf> {
        self.invocation_path(id)?;
        Ok(self
            .runtime()?
            .join("skill-waits")
            .join(super::execution::stable_id(
                &self.refine_dir.display().to_string(),
            ))
            .join(format!("{id}.json")))
    }
    pub(crate) fn record_wait(&self, id: &str, reason: Option<WaitReason>) -> RefineResult<()> {
        let path = self.wait_path(id)?;
        if let Some(reason) = reason {
            let previous: Option<Value> = read_json(&path).ok();
            if previous
                .as_ref()
                .is_some_and(|v| v["reason"] == json!(reason))
            {
                return Ok(());
            }
            write_json(
                &path,
                &json!({"reason":reason,"since":chrono::Utc::now().to_rfc3339()}),
            )
        } else {
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(crate::error::RefineError::Io(e.to_string())),
            }
        }
    }
    pub fn invocation_view(&self, id: &str) -> RefineResult<Value> {
        let invocation = self.invocation(id)?;
        let mut value = json!(invocation);
        value["execution_state"] = json!(invocation.execution_state());
        value["gate"] = json!(invocation.gate_assessment());
        self.decorate_wait(&mut value);
        Ok(value)
    }
    pub(crate) fn decorate_wait(&self, value: &mut Value) {
        if matches!(value["state"].as_str(), Some("pending" | "running"))
            && let Some(id) = value["id"].as_str()
            && let Ok(path) = self.wait_path(id)
            && let Ok(wait) = read_json::<Value>(&path)
        {
            value["waiting"] = wait;
        }
    }
}
