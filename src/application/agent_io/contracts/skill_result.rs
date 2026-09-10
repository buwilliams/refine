//! Skill envelopes compose the same typed artifacts used by their validators.
use crate::application::agent_io::structured_output::Contract;
use crate::error::{RefineError, RefineResult};
use crate::model::automation::SkillResult;
use serde_json::{Value, json};

pub(crate) fn result_contract(invocation_id: &str, binding_id: &str, role: &str) -> Value {
    let artifacts = match role {
        "plan" => {
            json!({"plan": crate::model::goal::ProposedImplementationPlan::example()})
        }
        "implement" => {
            json!({"implementation_evidence": crate::model::goal::ImplementationExecutionEvidence::example()})
        }
        "quality" => {
            json!({"tests": [{"test": "Observable requirement", "command": "non-interactive command whose exit 0 means pass", "status": "passed", "evidence": "Observed result"}]})
        }
        "governance" => {
            json!({"violations": [], "recovery_analysis": null, "recovery_round_prompt": null})
        }
        _ => json!({}),
    };
    json!({"invocation_id": invocation_id, "binding_id": binding_id, "role": role, "outcome": "success", "summary": "What happened", "evidence": ["Observed supporting evidence"], "artifacts": artifacts})
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
            summary: "Task completed".into(),
            evidence: vec!["Observed evidence".into()],
            artifacts: json!({}),
        }
    }
}

pub(crate) fn validate_artifacts(result: &SkillResult) -> RefineResult<()> {
    use crate::application::agent_io::structured_output::Contract;
    use crate::model::goal::{ImplementationExecutionEvidence, ProposedImplementationPlan};
    if result.outcome != "success" {
        return Ok(());
    }
    match result.role.as_str() {
        "plan" => {
            ProposedImplementationPlan::decode(&result.artifacts["plan"].to_string())
                .map_err(|e| RefineError::Serialization(e.to_string()))?;
        }
        "implement" => {
            ImplementationExecutionEvidence::decode(
                &result.artifacts["implementation_evidence"].to_string(),
            )
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
        }
        "quality" => {
            for test in result.artifacts["tests"].as_array().into_iter().flatten() {
                for key in ["test", "command"] {
                    if test
                        .get(key)
                        .and_then(Value::as_str)
                        .is_none_or(|v| v.trim().is_empty())
                    {
                        return Err(RefineError::Serialization(format!(
                            "artifacts.tests requires a nonempty {key}"
                        )));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}
