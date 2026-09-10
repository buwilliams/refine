use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::FileEventService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::model::automation::*;

pub use super::records::{EventInvocation, InvocationContext, InvocationState, PinnedBinding};

pub use super::context::goal_context;
use super::parameters::field;
pub use super::parameters::resolve_parameters;

pub fn stable_id(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

mod blocking;
mod lifecycle;
mod preparation;
mod runner;
mod workspace;
pub(crate) use blocking::BlockingInvocation;
pub use lifecycle::LifecycleWorkspace;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod workspace_tests;

pub fn aggregate_state(invocation: &EventInvocation) -> InvocationState {
    let mut failed = false;
    for binding in invocation
        .bindings
        .iter()
        .filter(|b| b.binding.mode != BindingMode::Context)
    {
        match invocation
            .results
            .get(&binding.binding.id)
            .map(|r| r.outcome.as_str())
        {
            Some("success") => {}
            Some("failure") => failed = true,
            _ => return InvocationState::Error,
        }
    }
    if failed {
        InvocationState::Failed
    } else {
        InvocationState::Succeeded
    }
}

impl EventInvocation {
    /// Keep historical recorded state intact while exposing the outcome of all actual executions.
    pub fn execution_state(&self) -> InvocationState {
        if !self.state.terminal()
            || self.state == InvocationState::Cancelled
            || (self.state == InvocationState::Error
                && self.results.values().all(|r| r.outcome != "error"))
        {
            return self.state.clone();
        }
        aggregate_state(self)
    }

    pub(crate) fn execution_error(&self) -> RefineError {
        let message = self
            .error
            .clone()
            .unwrap_or_else(|| format!("Event {} execution failed", self.event.name));
        if self
            .results
            .values()
            .any(|r| r.artifacts["fault_kind"] == "output_contract")
        {
            RefineError::Serialization(message)
        } else {
            RefineError::Degraded(message)
        }
    }
}
