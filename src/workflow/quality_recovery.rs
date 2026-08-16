use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};
use crate::structured_output::{
    Contract, DecodeOptions, Selection, StructuredOutputError, select_value,
};
use crate::tools::host::quality::QualityCheckResult;

use super::json_object;

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
    let investigation_contract = QualityRecoveryVerdict::contract_json();
    Ok(render(
        PromptTemplate::GoalWorkflowQualityRecovery,
        &[
            ("goal_id", goal_id),
            ("round_number", &round_number),
            ("worktree_path", worktree_path),
            ("context_json", &context_json),
            ("quality_agent_report", quality_agent_report),
            ("quality_json", &quality_json),
            ("investigation_contract", &investigation_contract),
        ],
    ))
}

/// The canonical investigation shape shown to and required from the agent.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct QualityRecoveryVerdict {
    recovery_analysis: String,
    recovery_round_prompt: String,
}

impl Contract for QualityRecoveryVerdict {
    const LABEL: &'static str = "Quality recovery investigation JSON";

    fn example() -> Self {
        QualityRecoveryVerdict {
            recovery_analysis: "concise evidence-based cause and required correction".to_string(),
            recovery_round_prompt: "complete actionable request for the next implementation Round"
                .to_string(),
        }
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        for (key, value) in [
            ("recovery_analysis", &self.recovery_analysis),
            ("recovery_round_prompt", &self.recovery_round_prompt),
        ] {
            if value.trim().is_empty() {
                return Err(StructuredOutputError::validation(
                    Self::LABEL,
                    format!("Quality recovery investigation omitted {key}"),
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn parse_quality_recovery_provider_output(
    output: &str,
) -> RefineResult<QualityRecoveryInvestigation> {
    let shaped: &dyn Fn(&Value) -> bool = &|value| {
        value.get("recovery_analysis").is_some() || value.get("recovery_round_prompt").is_some()
    };
    let value = select_value(
        output,
        &DecodeOptions::new(QualityRecoveryVerdict::LABEL)
            .selecting(Selection::LastMatching(shaped)),
    )?;
    let verdict: QualityRecoveryVerdict =
        serde_path_to_error::deserialize(value.clone()).map_err(|error| {
            StructuredOutputError::schema(
                QualityRecoveryVerdict::LABEL,
                Some(error.path().to_string()).filter(|path| !path.is_empty()),
                error.inner().to_string(),
            )
        })?;
    verdict.validate()?;
    Ok(QualityRecoveryInvestigation {
        analysis: verdict.recovery_analysis.trim().to_string(),
        round_prompt: verdict.recovery_round_prompt.trim().to_string(),
        details: json_object(json!({
            "phase": "quality_recovery",
            "raw_output": output,
            "verdict": value
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_recovery_contract_example_roundtrips() {
        crate::structured_output::assert_contract_roundtrip::<QualityRecoveryVerdict>();
    }

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
