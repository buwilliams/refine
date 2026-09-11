//! Refine reads the agent's decision; supporting material is optional context.
use crate::model::automation::SkillResult;
use serde_json::{Value, json};

pub(crate) fn result_contract(invocation_id: &str, binding_id: &str, role: &str) -> Value {
    json!({"invocation_id":invocation_id,"binding_id":binding_id,"role":role,
        "outcome":"success","summary":"Your decision and any context useful to the next agent"})
}

impl crate::application::agent_io::structured_output::Contract for SkillResult {
    const LABEL: &'static str = "Skill completion result";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["skill_result", "result"];
    fn example() -> Self {
        Self {
            invocation_id: "invocation".into(),
            binding_id: "binding".into(),
            role: "task".into(),
            outcome: "success".into(),
            summary: String::new(),
            evidence: Vec::new(),
            artifacts: Value::Null,
        }
    }
}
