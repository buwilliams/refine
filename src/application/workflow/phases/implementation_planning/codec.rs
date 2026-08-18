use std::collections::BTreeSet;

use crate::application::agent_io::structured_output::{Contract, StructuredOutputError};
use crate::error::RefineResult;
use crate::model::goal::{ImplementationCriticism, ProposedImplementationPlan};

pub(crate) fn decode_plan(output: &str) -> RefineResult<ProposedImplementationPlan> {
    Ok(ProposedImplementationPlan::decode(output)?)
}

pub(crate) fn decode_criticism(output: &str) -> RefineResult<ImplementationCriticism> {
    Ok(ImplementationCriticism::decode(output)?)
}

pub(crate) fn validate_revised_plan(
    plan: &ProposedImplementationPlan,
    criticism: &ImplementationCriticism,
) -> RefineResult<()> {
    plan.validate()?;
    let resolved = plan
        .criticism_resolutions
        .iter()
        .map(|resolution| resolution.criticism_id.as_str())
        .collect::<BTreeSet<_>>();
    let material = criticism
        .findings
        .iter()
        .filter(|finding| finding.material)
        .map(|finding| finding.id.as_str())
        .collect::<BTreeSet<_>>();
    let unrelated = resolved.difference(&material).copied().collect::<Vec<_>>();
    if !unrelated.is_empty() {
        return Err(StructuredOutputError::validation(
            ProposedImplementationPlan::LABEL,
            format!(
                "revised implementation plan resolved unknown or non-material criticism: {}",
                unrelated.join(", ")
            ),
        )
        .into());
    }
    let unresolved = criticism
        .findings
        .iter()
        .filter(|finding| finding.material && !resolved.contains(finding.id.as_str()))
        .map(|finding| finding.id.clone())
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return Err(StructuredOutputError::validation(
            ProposedImplementationPlan::LABEL,
            format!(
                "revised implementation plan did not resolve material criticism: {}",
                unresolved.join(", ")
            ),
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_io::contracts::implementation_planning::{
        MAX_ITEM_CHARS, MAX_SUMMARY_CHARS,
    };

    #[test]
    fn plan_decoding_accepts_completion_signal_wrappers_and_stringified_results() {
        let wrapped = decode_plan(
            r#"{"state":"completed","message":"done","planning_result":{"summary":"Change the boundary","checklist":[{"id":"P1","description":"Implement the required behavior."}]}}"#,
        )
        .unwrap();
        assert_eq!(wrapped.checklist[0].id, "P1");

        let stringified = decode_plan(
            r#"{"planning_result":"{\"summary\":\"Change the boundary\",\"checklist\":[{\"id\":\"P1\",\"description\":\"Implement the required behavior.\"}]}"}"#,
        )
        .unwrap();
        assert_eq!(stringified, wrapped);
    }

    #[test]
    fn plans_require_compact_one_line_items_but_allow_as_many_as_needed() {
        let oversized = format!(
            r#"{{"summary":"Do it","checklist":[{{"id":"P1","description":"{}"}}]}}"#,
            "x".repeat(MAX_ITEM_CHARS + 1)
        );
        assert!(decode_plan(&oversized).is_err());

        let many = r#"{"summary":"Do it","checklist":[{"id":"P1","description":"One"},{"id":"P2","description":"Two"},{"id":"P3","description":"Three"},{"id":"P4","description":"Four"},{"id":"P5","description":"Five"}]}"#;
        assert_eq!(decode_plan(many).unwrap().checklist.len(), 5);

        let multiline = r#"{"summary":"Do it","checklist":[{"id":"P1","description":"One\nTwo"}]}"#;
        assert!(decode_plan(multiline).is_err());

        let verbose_shape = r#"{"summary":"Do it","checklist":[{"id":"P1","description":"One","verification":["cargo test"]}]}"#;
        assert!(decode_plan(verbose_shape).is_err());
    }

    #[test]
    fn plan_summaries_allow_paragraphs_beyond_the_former_600_char_limit() {
        let formerly_oversized = format!(
            r#"{{"summary":"{}","checklist":[{{"id":"P1","description":"Change it"}}]}}"#,
            "x".repeat(805)
        );
        assert!(decode_plan(&formerly_oversized).is_ok());

        let oversized = format!(
            r#"{{"summary":"{}","checklist":[{{"id":"P1","description":"Change it"}}]}}"#,
            "x".repeat(MAX_SUMMARY_CHARS + 1)
        );
        assert!(decode_plan(&oversized).is_err());
    }

    #[test]
    fn criticism_details_are_guided_toward_conciseness_without_a_tiny_limit() {
        let formerly_oversized = serde_json::json!({
            "summary": "Material gap",
            "findings": [{
                "id": "C1",
                "material": true,
                "description": "x".repeat(307),
                "recommendation": "Correct the material gap."
            }]
        });
        assert!(decode_criticism(&formerly_oversized.to_string()).is_ok());

        let oversized = serde_json::json!({
            "summary": "Material gap",
            "findings": [{
                "id": "C1",
                "material": true,
                "description": "x".repeat(MAX_ITEM_CHARS + 1),
                "recommendation": "Correct the material gap."
            }]
        });
        assert!(decode_criticism(&oversized.to_string()).is_err());
    }

    #[test]
    fn revised_plan_must_resolve_every_material_criticism() {
        let plan = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}]}"#,
        )
        .unwrap();
        let criticism = decode_criticism(
            r#"{"summary":"Gap","findings":[{"id":"C1","material":true,"description":"Missing failure path","recommendation":"Add it"}]}"#,
        )
        .unwrap();
        assert!(validate_revised_plan(&plan, &criticism).is_err());
    }

    fn material_criticism() -> ImplementationCriticism {
        decode_criticism(
            r#"{"summary":"Gap","findings":[{"id":"C1","material":true,"description":"Missing failure path","recommendation":"Add it"}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn revised_plans_normalize_only_supported_resolution_identifier_spellings() {
        for field in ["criticism_id", "criticismId", "finding_id", "id"] {
            let output = format!(
                r#"{{"state":"completed","planning_result":{{"summary":"Do it","checklist":[{{"id":"P1","description":"Change it"}}],"criticism_resolutions":[{{"{field}":"C1","resolution":"Added the failure path"}}]}}}}"#
            );
            let plan = decode_plan(&output).unwrap();
            assert_eq!(plan.criticism_resolutions[0].criticism_id, "C1");
            validate_revised_plan(&plan, &material_criticism()).unwrap();
        }
    }

    #[test]
    fn revised_plans_reject_malformed_ambiguous_duplicate_unknown_and_incomplete_resolutions() {
        let malformed = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(malformed.contains("invalid JSON"));

        let ambiguous = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1","id":"C1","resolution":"Added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(ambiguous.contains("ambiguous identifier fields"));

        let unsupported_alias = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticismID":"C1","resolution":"Added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(unsupported_alias.contains("unknown field `criticismID`"));

        let duplicate = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1","resolution":"Added it"},{"criticismId":"C1","resolution":"Also added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(duplicate.contains("repeats criticism resolution C1"));

        let unknown = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C2","resolution":"Added it"}]}"#,
        )
        .unwrap();
        assert!(
            validate_revised_plan(&unknown, &material_criticism())
                .unwrap_err()
                .to_string()
                .contains("unknown or non-material criticism: C2")
        );

        let incomplete = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(incomplete.contains("field `criticism_resolutions[0]`"));
        assert!(incomplete.contains("missing field `resolution`"));

        let missing_id = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"resolution":"Added it"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(missing_id.contains("field `criticism_resolutions[0]`"));
        assert!(missing_id.contains("missing field `criticism_id`"));

        let structurally_different = decode_plan(
            r#"{"summary":"Do it","checklist":[{"id":"P1","description":"Change it"}],"criticism_resolutions":[{"criticism_id":"C1","resolution":"Added it","details":"extra shape"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(structurally_different.contains("unknown field `details`"));

        let over_nested = format!("{}0{}", "[".repeat(40), "]".repeat(40));
        assert!(
            decode_plan(&over_nested)
                .unwrap_err()
                .to_string()
                .contains("maximum JSON nesting depth")
        );
    }
}
