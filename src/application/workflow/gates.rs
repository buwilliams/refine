//! Pure evidence assessment. Execution status does not decide workflow permission.
use crate::application::agent_io::contracts::skill_result::validate_artifacts;
use crate::application::events::execution::PinnedBinding;
use crate::application::events::{EventInvocation, InvocationState};
use crate::error::{RefineError, RefineResult};
use crate::model::automation::{BindingMode, SkillResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateAssessment {
    Satisfied,
    Missing,
    Finding,
    Fault,
}

impl EventInvocation {
    pub(crate) fn blocking_result(&self, pinned: &PinnedBinding) -> RefineResult<&SkillResult> {
        let invalid = |reason: String| {
            RefineError::Serialization(format!(
                "Skill invocation {} binding {}: {reason} (state {:?}); retained evidence was preserved",
                self.id, pinned.binding.id, self.state
            ))
        };
        let result = self
            .results
            .get(&pinned.binding.id)
            .ok_or_else(|| invalid("missing required completion result".into()))?;
        result
            .validate(&self.id, &pinned.binding.id, &pinned.skill.role)
            .map_err(invalid)?;
        validate_artifacts(result).map_err(|error| invalid(error.to_string()))?;
        Ok(result)
    }

    pub(crate) fn blocking_results(&self) -> RefineResult<Vec<SkillResult>> {
        let bindings = self
            .bindings
            .iter()
            .filter(|b| b.binding.mode == BindingMode::Blocking)
            .collect::<Vec<_>>();
        if bindings.is_empty() {
            return Ok(Vec::new());
        }
        // Validate each retained result even when the execution state reports a fault.
        let results = bindings
            .iter()
            .map(|b| self.blocking_result(b).cloned())
            .collect::<RefineResult<Vec<_>>>()?;
        if matches!(
            self.gate_assessment(),
            GateAssessment::Fault | GateAssessment::Missing
        ) {
            let message = format!(
                "Skill invocation {} required bindings [{}] cannot authorize a gate: {:?}; {}",
                self.id,
                bindings
                    .iter()
                    .map(|b| b.binding.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                self.state,
                self.error
                    .as_deref()
                    .unwrap_or("completion evidence is unavailable")
            );
            return Err(
                if matches!(self.execution_error(), RefineError::Serialization(_)) {
                    RefineError::Serialization(message)
                } else {
                    RefineError::Degraded(message)
                },
            );
        }
        Ok(results)
    }

    pub fn gate_assessment(&self) -> GateAssessment {
        if self.state == InvocationState::Cancelled {
            return GateAssessment::Fault;
        }
        if self.state == InvocationState::Error
            && self.results.values().all(|r| r.outcome != "error")
        {
            return GateAssessment::Fault;
        }
        let mut verdict = GateAssessment::Satisfied;
        for binding in self
            .bindings
            .iter()
            .filter(|b| b.binding.mode == BindingMode::Blocking)
        {
            if self.results.contains_key(&binding.binding.id)
                && self.blocking_result(binding).is_err()
            {
                return GateAssessment::Fault;
            }
            match self
                .results
                .get(&binding.binding.id)
                .map(|r| r.outcome.as_str())
            {
                Some("success") => {}
                Some("failure") => verdict = GateAssessment::Finding,
                Some(_) => return GateAssessment::Fault,
                None if self.state.terminal() => return GateAssessment::Fault,
                None => {
                    if verdict != GateAssessment::Finding {
                        verdict = GateAssessment::Missing;
                    }
                }
            }
        }
        // An admission or parameter failure has no resolved binding evidence.
        if self.bindings.is_empty() && self.state == InvocationState::Error {
            return GateAssessment::Fault;
        }
        verdict
    }

    pub(crate) fn success_action_ready(&self) -> bool {
        self.event.on_success.is_some()
            && !self.action_applied
            && self.state.terminal()
            && if self
                .bindings
                .iter()
                .any(|b| b.binding.mode == BindingMode::Blocking)
            {
                self.gate_assessment() == GateAssessment::Satisfied
            } else {
                self.execution_state() == InvocationState::Succeeded
            }
    }
}
