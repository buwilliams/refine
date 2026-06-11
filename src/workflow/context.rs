use std::path::{Path, PathBuf};

use crate::model::JsonObject;
use crate::model::workflow::GapStatus;
use crate::process::subprocess::workflow_subprocess_metadata;
use crate::process::supervisor::errors::RefineResult;

#[derive(Clone, Debug)]
pub struct WorkflowContext {
    pub runtime_root: PathBuf,
    pub durable_root: PathBuf,
    pub active_node_id: String,
    pub execution_id: Option<String>,
    pub round_idx: Option<usize>,
}

impl WorkflowContext {
    pub fn new(
        runtime_root: impl Into<PathBuf>,
        durable_root: impl Into<PathBuf>,
        active_node_id: impl Into<String>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            durable_root: durable_root.into(),
            active_node_id: active_node_id.into(),
            execution_id: None,
            round_idx: None,
        }
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub fn durable_root(&self) -> &Path {
        &self.durable_root
    }

    pub fn request_transition(
        &mut self,
        _gap_id: &str,
        _from: GapStatus,
        _to: GapStatus,
    ) -> RefineResult<()> {
        Ok(())
    }

    pub fn workflow_process_metadata(
        &self,
        gap_id: &str,
        workflow_state: &str,
        behavior: &str,
    ) -> JsonObject {
        workflow_subprocess_metadata(
            self.execution_id.as_deref().unwrap_or(gap_id),
            gap_id,
            workflow_state,
            behavior,
            self.round_idx,
        )
    }
}
