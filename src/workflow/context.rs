use std::path::{Path, PathBuf};

use crate::model::workflow::GapStatus;
use crate::tools::supervisor::errors::RefineResult;

#[derive(Clone, Debug)]
pub struct WorkflowContext {
    pub runtime_root: PathBuf,
    pub durable_root: PathBuf,
    pub active_node_id: String,
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
}
