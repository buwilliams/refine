use crate::prompts::{PromptTemplate, render};
use crate::structured_output::RepairDirective;

/// Render the shared structured-output repair prompt: the original phase
/// contract, the exact rejection diagnostic, the retained invalid response,
/// and the canonical contract JSON to correct against.
pub fn repair_prompt(
    original_prompt: &str,
    output_label: &str,
    contract_json: &str,
    directive: &RepairDirective,
) -> String {
    render(
        PromptTemplate::StructuredOutputRepair,
        &[
            ("original_prompt", original_prompt),
            ("output_label", output_label),
            ("attempt", &directive.attempt.to_string()),
            ("max_repairs", &directive.max_repairs.to_string()),
            ("contract_json", contract_json),
            ("diagnostics", &directive.diagnostics),
            ("raw_output", &directive.raw_output),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_prompt_carries_diagnostics_contract_and_retained_response() {
        let prompt = repair_prompt(
            "Original phase contract",
            "revised implementation plan",
            r#"{"summary":"..."}"#,
            &RepairDirective {
                attempt: 1,
                max_repairs: 2,
                diagnostics: "missing criticism resolution C1".to_string(),
                raw_output: "{not json}".to_string(),
            },
        );
        assert!(prompt.contains("Original phase contract"));
        assert!(prompt.contains("Repair 1/2"));
        assert!(prompt.contains("revised implementation plan"));
        assert!(prompt.contains(r#"{"summary":"..."}"#));
        assert!(prompt.contains("missing criticism resolution C1"));
        assert!(prompt.contains("{not json}"));
    }
}
