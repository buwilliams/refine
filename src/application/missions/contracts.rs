//! Mission agent-output contracts.
//!
//! Every Mission phase output is decoded through the shared structured-output
//! subsystem: the serde type is the single source of truth, the prompt shows
//! the same JSON the decoder targets, and bounded repair uses the shared
//! repair prompt. Agent text can never define workflow transitions or
//! publication authority; the engine applies these typed results
//! deterministically.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::application::agent_io::structured_output::contract::Contract;
use crate::error::StructuredOutputError;

use super::reconciliation::engine::{CriticismReport, ReductionDraft};
use crate::model::mission::{
    ArtifactAuthority, ArtifactRef, AssertionKind, ContradictionResolution, CriterionOutcome,
    KnowledgeAssertion,
};

fn example_assertion() -> KnowledgeAssertion {
    KnowledgeAssertion {
        assertion_id: "engine-assigned".to_string(),
        kind: AssertionKind::Fact,
        authority: ArtifactAuthority::Evidence,
        provenance: Some("contribution:G1/1".to_string()),
        qualified: None,
        supersedes: vec![],
        corrects: vec![],
        derived_from: vec![],
        scope: Some("one sentence the Mission now accepts".to_string()),
        scope_refs: vec!["crit:id or mission goal key".to_string()],
        evidence_refs: vec!["commit:<sha> or path:<path>@<sha>".to_string()],
        supersedable: true,
        members: vec![],
        resolution: None,
        resolved_by: None,
    }
}

fn example_artifact_promotion() -> super::reconciliation::engine::ArtifactPromotion {
    super::reconciliation::engine::ArtifactPromotion {
        candidate_ref: "contribution:G1/1/candidate/<obligation-key>".to_string(),
        artifact: ArtifactRef {
            key: "obligation-key".to_string(),
            title: "Artifact title".to_string(),
            kind: "model".to_string(),
            authority: ArtifactAuthority::Model,
            path: "missions/<shard>/<id>/artifacts/<key>/<sha256>.md".to_string(),
            media_type: Some("text/markdown".to_string()),
            size: 1024,
            sha256: Some("64 hex chars".to_string()),
            provenance: Some("contribution:G1/1".to_string()),
            applicability: Some("where this artifact applies".to_string()),
        },
    }
}

/// The investigation agent's typed output: the initial claims the Mission
/// proposes to accept before any wave has run.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct InvestigationOutput {
    #[serde(default)]
    pub accepts: Vec<super::reconciliation::engine::DraftedAssertion>,
    #[serde(default)]
    pub contradictions: Vec<super::reconciliation::engine::DraftedContradiction>,
    #[serde(default)]
    pub artifact_promotions: Vec<super::reconciliation::engine::ArtifactPromotion>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}

impl Contract for InvestigationOutput {
    const LABEL: &'static str = "Mission investigation JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["investigation", "result"];

    fn example() -> Self {
        Self {
            accepts: vec![super::reconciliation::engine::DraftedAssertion {
                draft_id: "d1".to_string(),
                assertion: example_assertion(),
                evidence_coverage: vec!["contribution:G1/1/finding/0".to_string()],
                unverified_extent: false,
            }],
            contradictions: Vec::new(),
            artifact_promotions: vec![example_artifact_promotion()],
            open_questions: vec!["what cannot be determined from the repository yet".to_string()],
        }
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        for drafted in &self.accepts {
            if drafted.draft_id.trim().is_empty() {
                return Err(StructuredOutputError::validation(
                    Self::LABEL,
                    "every drafted assertion requires a draft_id",
                ));
            }
        }
        Ok(())
    }
}

impl Contract for ReductionDraft {
    const LABEL: &'static str = "Mission reduction JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["reduction", "result"];

    fn example() -> Self {
        Self {
            accepts: vec![super::reconciliation::engine::DraftedAssertion {
                draft_id: "d1".to_string(),
                assertion: example_assertion(),
                evidence_coverage: vec!["contribution:G1/1/finding/0".to_string()],
                unverified_extent: false,
            }],
            rejects: vec![super::reconciliation::engine::DraftedRejection {
                finding_ref: "contribution:G1/1/finding/1".to_string(),
                reason: "why the finding is not accepted".to_string(),
            }],
            contradictions: vec![super::reconciliation::engine::DraftedContradiction {
                members: vec!["assertion ids".to_string(), "assertion ids".to_string()],
                resolution: Some(ContradictionResolution::Open),
                resolution_basis: Some("finding ref of the machine-checkable basis".to_string()),
            }],
            artifact_promotions: vec![example_artifact_promotion()],
            spec_amendments: vec![],
            followups: vec![],
        }
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        for drafted in &self.accepts {
            if drafted.draft_id.trim().is_empty() {
                return Err(StructuredOutputError::validation(
                    Self::LABEL,
                    "every drafted assertion requires a draft_id",
                ));
            }
        }
        for rejection in &self.rejects {
            if rejection.finding_ref.trim().is_empty() {
                return Err(StructuredOutputError::validation(
                    Self::LABEL,
                    "every drafted rejection requires a finding_ref",
                ));
            }
        }
        Ok(())
    }
}

impl Contract for CriticismReport {
    const LABEL: &'static str = "Mission criticism JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["criticism", "result"];

    fn example() -> Self {
        use super::reconciliation::engine::{CriticismVerdict, CriticismVerdictEntry};
        Self {
            verdicts: vec![CriticismVerdictEntry {
                target: "d1".to_string(),
                verdict: CriticismVerdict::Confirmed,
                note: "the counter-case attempt and its result".to_string(),
            }],
            notes: "overall assessment preserved verbatim as evidence".to_string(),
        }
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        for entry in &self.verdicts {
            if entry.target.trim().is_empty() {
                return Err(StructuredOutputError::validation(
                    Self::LABEL,
                    "every criticism verdict requires a target",
                ));
            }
        }
        Ok(())
    }
}

/// One judged criterion result from synthesis or quality.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JudgedCriterion {
    pub criterion_id: String,
    pub result: String,
    #[serde(default)]
    pub evidence: Vec<String>,
}

fn parse_criterion_outcome(value: &str) -> Option<CriterionOutcome> {
    match value.trim().to_ascii_lowercase().as_str() {
        "met" => Some(CriterionOutcome::Met),
        "partial" => Some(CriterionOutcome::Partial),
        "unmet" => Some(CriterionOutcome::Unmet),
        "contradicted" => Some(CriterionOutcome::Contradicted),
        "waived" => Some(CriterionOutcome::Waived),
        _ => None,
    }
}

pub(crate) fn judged_criterion_outcome(result: &str) -> Option<CriterionOutcome> {
    parse_criterion_outcome(result)
}

/// The synthesis agent's typed output: the candidate Outcome summary.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct SynthesisOutput {
    pub summary: String,
    #[serde(default)]
    pub criteria_results: Vec<JudgedCriterion>,
    #[serde(default)]
    pub artifact_promotions: Vec<super::reconciliation::engine::ArtifactPromotion>,
    #[serde(default)]
    pub residual_risks: Vec<String>,
}

impl Contract for SynthesisOutput {
    const LABEL: &'static str = "Mission synthesis JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["synthesis", "result"];

    fn example() -> Self {
        Self {
            summary: "plain-language outcome of this Round".to_string(),
            criteria_results: vec![JudgedCriterion {
                criterion_id: "criterion id".to_string(),
                result: "met".to_string(),
                evidence: vec!["goal:<id> round:<n>, commit:<sha>, artifact:<key>".to_string()],
            }],
            artifact_promotions: vec![example_artifact_promotion()],
            residual_risks: vec!["what remains uncertain and why".to_string()],
        }
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        if self.summary.trim().is_empty() {
            return Err(StructuredOutputError::validation(
                Self::LABEL,
                "synthesis requires a summary",
            ));
        }
        for criterion in &self.criteria_results {
            if parse_criterion_outcome(&criterion.result).is_none() {
                return Err(StructuredOutputError::validation(
                    Self::LABEL,
                    "criterion results must be met, partial, unmet, contradicted, or waived",
                ));
            }
        }
        Ok(())
    }
}

/// The holistic Mission Quality judgment.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MissionQualityJudgment {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub criteria_results: Vec<JudgedCriterion>,
}

impl Contract for MissionQualityJudgment {
    const LABEL: &'static str = "Mission quality JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["quality", "result"];

    fn example() -> Self {
        Self {
            ok: true,
            summary: "combined-outcome judgment".to_string(),
            findings: vec!["cross-Goal coherence finding with evidence".to_string()],
            criteria_results: vec![JudgedCriterion {
                criterion_id: "criterion id".to_string(),
                result: "met".to_string(),
                evidence: vec!["evidence reference".to_string()],
            }],
        }
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        for judged in &self.criteria_results {
            if parse_criterion_outcome(&judged.result).is_none() {
                return Err(StructuredOutputError::validation(
                    Self::LABEL,
                    "criterion results must be met, partial, unmet, contradicted, or waived",
                ));
            }
        }
        Ok(())
    }
}

/// The Mission Governance verdict at the exact pinned tuple.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MissionGovernanceVerdict {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub violations: Vec<MissionGovernanceViolation>,
    #[serde(default)]
    pub recovery_analysis: String,
    #[serde(default)]
    pub recovery_round_prompt: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MissionGovernanceViolation {
    #[serde(default)]
    pub rule: String,
    #[serde(default)]
    pub message: String,
}

impl MissionGovernanceVerdict {
    pub fn passed(&self) -> bool {
        matches!(
            self.status.trim().to_ascii_lowercase().as_str(),
            "passed" | "pass" | "ok" | "approved"
        )
    }
}

impl Contract for MissionGovernanceVerdict {
    const LABEL: &'static str = "Mission governance JSON";
    const ENVELOPE_FIELDS: &'static [&'static str] = &["governance"];

    fn example() -> Self {
        Self {
            status: "passed|failed".to_string(),
            message: "short human-readable verdict".to_string(),
            violations: vec![MissionGovernanceViolation {
                rule: "rule or constitution concern".to_string(),
                message: "exact system-level effect".to_string(),
            }],
            recovery_analysis: "required when failed".to_string(),
            recovery_round_prompt: "required when failed".to_string(),
        }
    }

    fn normalize(value: &mut Value) -> Result<(), StructuredOutputError> {
        // Accept the common spellings a verdict agent may use; everything is
        // funneled into `status` so `passed()` stays the single decoder.
        let Some(object) = value.as_object_mut() else {
            return Ok(());
        };
        if !object.contains_key("status") {
            for alias in ["verdict", "result"] {
                if let Some(alias_value) = object.remove(alias) {
                    object.insert("status".to_string(), alias_value);
                    break;
                }
            }
        }
        if !object.contains_key("status")
            && let Some(ok) = object.get("ok").and_then(Value::as_bool)
        {
            object.insert(
                "status".to_string(),
                Value::String(if ok { "passed" } else { "failed" }.to_string()),
            );
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), StructuredOutputError> {
        if self.status.trim().is_empty() {
            return Err(StructuredOutputError::validation(
                Self::LABEL,
                "a governance verdict requires a status",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_contracts_round_trip_through_their_own_examples() {
        crate::application::agent_io::structured_output::contract::assert_contract_roundtrip::<
            InvestigationOutput,
        >();
        crate::application::agent_io::structured_output::contract::assert_contract_roundtrip::<
            ReductionDraft,
        >();
        crate::application::agent_io::structured_output::contract::assert_contract_roundtrip::<
            CriticismReport,
        >();
        crate::application::agent_io::structured_output::contract::assert_contract_roundtrip::<
            SynthesisOutput,
        >();
        crate::application::agent_io::structured_output::contract::assert_contract_roundtrip::<
            MissionQualityJudgment,
        >();
        crate::application::agent_io::structured_output::contract::assert_contract_roundtrip::<
            MissionGovernanceVerdict,
        >();
    }

    #[test]
    fn governance_verdict_normalizes_alias_spellings() {
        let aliased = serde_json::json!({
            "verdict": "passed",
            "message": "fine",
            "ok": true
        });
        let encoded = aliased.to_string();
        let decoded = MissionGovernanceVerdict::decode(&encoded).unwrap();
        assert!(decoded.passed());
        assert_eq!(decoded.message, "fine");

        let boolean = serde_json::json!({"ok": false, "message": "no"});
        let decoded = MissionGovernanceVerdict::decode(&boolean.to_string()).unwrap();
        assert!(!decoded.passed());
    }

    #[test]
    fn synthesis_rejects_unknown_criterion_results() {
        let bad = serde_json::json!({
            "summary": "s",
            "criteria_results": [{"criterion_id": "c", "result": "excellent"}]
        });
        assert!(SynthesisOutput::decode(&bad.to_string()).is_err());
    }
}
