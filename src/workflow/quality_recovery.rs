use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::quality::QualityCheckResult;

use super::{json_object, json_object_candidates};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct QualityRecoveryInvestigation {
    pub(super) analysis: String,
    pub(super) round_prompt: String,
    pub(super) details: JsonObject,
}

pub(super) fn quality_recovery_prompt(
    goal_id: &str,
    round_idx: usize,
    worktree_path: &str,
    context: &Value,
    quality_agent_report: &str,
    quality: &QualityCheckResult,
) -> RefineResult<String> {
    let round_number = (round_idx + 1).to_string();
    let context_json = serde_json::to_string_pretty(context).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode Quality recovery context: {error}"
        ))
    })?;
    let quality_json = serde_json::to_string_pretty(quality).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Quality recovery result: {error}"))
    })?;
    Ok(render(
        PromptTemplate::GoalWorkflowQualityRecovery,
        &[
            ("goal_id", goal_id),
            ("round_number", &round_number),
            ("worktree_path", worktree_path),
            ("context_json", &context_json),
            ("quality_agent_report", quality_agent_report),
            ("quality_json", &quality_json),
        ],
    ))
}

pub(super) fn parse_quality_recovery_provider_output(
    output: &str,
) -> RefineResult<QualityRecoveryInvestigation> {
    let value = json_object_candidates(output)
        .into_iter()
        .rev()
        .find(|value| {
            value.get("recovery_analysis").is_some() || value.get("recovery_round_prompt").is_some()
        })
        .ok_or_else(|| {
            RefineError::Serialization(
                "Quality recovery investigation did not return the required JSON object"
                    .to_string(),
            )
        })?;
    let analysis = required_string(&value, "recovery_analysis")?;
    let round_prompt = required_string(&value, "recovery_round_prompt")?;
    Ok(QualityRecoveryInvestigation {
        analysis,
        round_prompt,
        details: json_object(json!({
            "phase": "quality_recovery",
            "raw_output": output,
            "verdict": value
        })),
    })
}

fn required_string(value: &Value, key: &str) -> RefineResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            RefineError::Serialization(format!("Quality recovery investigation omitted {key}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_recovery_reads_the_last_structured_investigation() {
        let output = "Reviewed code containing { braces.\n\
            {\"recovery_analysis\":\"The candidate broke the parser.\",\
             \"recovery_round_prompt\":\"Restore parser behavior and add the failing regression test.\"}";
        let recovery = parse_quality_recovery_provider_output(output).unwrap();
        assert_eq!(recovery.analysis, "The candidate broke the parser.");
        assert!(recovery.round_prompt.starts_with("Restore parser behavior"));
    }

    #[test]
    fn quality_recovery_requires_both_fields() {
        let error = parse_quality_recovery_provider_output(
            "{\"recovery_analysis\":\"The candidate broke the parser.\"}",
        )
        .unwrap_err();
        assert!(error.to_string().contains("recovery_round_prompt"));
    }
}
