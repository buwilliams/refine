//! Pure evidence assessment. Execution status does not decide workflow permission.
use crate::application::events::{EventInvocation, InvocationState};
use crate::model::automation::BindingMode;
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
