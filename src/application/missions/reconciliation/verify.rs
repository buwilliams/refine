//! Verification tiers and the deterministic verifier registry.
//!
//! Determinism has an honest boundary: deterministic verifiers prove
//! provenance and machine-checkable claims; they never prove that a claim
//! about the system is true. Which verifier applies is declared by the
//! evidence reference shape and the registry, never inferred from claim text.
//!
//! See `docs/mission-reconciliation.md` ("Verification tiers" and
//! "Auto-promotion policy").

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::mission::{GoalContribution, VerifierOutcome, VerifierResult};

/// The maximum accepted artifact-candidate size for envelope validation.
pub const MAX_CANDIDATE_BYTES: u64 = 8 * 1024 * 1024;

/// One structured evidence reference parsed from a finding's evidence list.
///
/// Wire syntax is positional and exact:
/// `commit:<id>`, `path:<path>@<commit>`, `quote:<text>@<commit>`,
/// `test:<name>`, `digest:<sha256>`. Anything else is not verifiable at
/// tier 2 and routes to judgment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EvidenceRef {
    Commit { id: String },
    Path { path: String, commit: String },
    Quote { text: String, commit: String },
    Test { name: String },
    Digest { sha256: String },
}

impl EvidenceRef {
    pub fn parse(value: &str) -> Option<Self> {
        if let Some(id) = value.strip_prefix("commit:") {
            return Some(Self::Commit { id: id.to_string() });
        }
        if let Some(rest) = value.strip_prefix("path:") {
            let (path, commit) = rest.rsplit_once('@')?;
            return Some(Self::Path {
                path: path.to_string(),
                commit: commit.to_string(),
            });
        }
        if let Some(rest) = value.strip_prefix("quote:") {
            let (text, commit) = rest.rsplit_once('@')?;
            return Some(Self::Quote {
                text: text.to_string(),
                commit: commit.to_string(),
            });
        }
        if let Some(name) = value.strip_prefix("test:") {
            return Some(Self::Test {
                name: name.to_string(),
            });
        }
        if let Some(sha256) = value.strip_prefix("digest:") {
            return Some(Self::Digest {
                sha256: sha256.to_string(),
            });
        }
        None
    }

    /// The registered verifier that applies to this evidence shape.
    pub fn verifier(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "commit_reachable",
            Self::Path { .. } => "path_exists",
            Self::Quote { .. } => "quote_at_commit",
            Self::Test { .. } => "test_passed",
            Self::Digest { .. } => "digest_matches",
        }
    }
}

/// The pinned facts a verifier runs against. The workflow engine builds this
/// from the wave's target head, Goal evidence, and staged candidate bytes;
/// tests construct it directly. Verification is a pure function of the claim
/// set and this context.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct VerificationContext {
    #[serde(default)]
    pub target_head: Option<String>,
    /// Commits reachable from the pinned target head.
    #[serde(default)]
    pub reachable_commits: BTreeSet<String>,
    /// Paths that exist at one commit.
    #[serde(default)]
    pub existing_paths: BTreeSet<(String, String)>,
    /// Quoted text present at one commit.
    #[serde(default)]
    pub quotes_at_commit: BTreeMap<String, BTreeSet<String>>,
    /// Recorded test outcomes.
    #[serde(default)]
    pub passed_tests: BTreeSet<String>,
    /// Digests whose staged bytes match their recorded digest.
    #[serde(default)]
    pub matched_digests: BTreeSet<String>,
}

/// One deterministic tier-2 verifier from the registry. Every verifier is a
/// pure function of the evidence reference and the pinned context.
pub fn run_verifier(evidence: &EvidenceRef, context: &VerificationContext) -> VerifierOutcome {
    match evidence {
        EvidenceRef::Commit { id } => {
            if context.reachable_commits.contains(id) {
                VerifierOutcome::Passed
            } else {
                VerifierOutcome::Failed
            }
        }
        EvidenceRef::Path { path, commit } => {
            if context
                .existing_paths
                .contains(&(commit.clone(), path.clone()))
            {
                VerifierOutcome::Passed
            } else {
                VerifierOutcome::Failed
            }
        }
        EvidenceRef::Quote { text, commit } => match context.quotes_at_commit.get(commit) {
            Some(quotes) if quotes.contains(text) => VerifierOutcome::Passed,
            _ => VerifierOutcome::Failed,
        },
        EvidenceRef::Test { name } => {
            if context.passed_tests.contains(name) {
                VerifierOutcome::Passed
            } else {
                VerifierOutcome::Failed
            }
        }
        EvidenceRef::Digest { sha256 } => {
            if context.matched_digests.contains(sha256) {
                VerifierOutcome::Passed
            } else {
                VerifierOutcome::Failed
            }
        }
    }
}

/// What tier-2 verification concluded about one finding.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingVerification {
    /// Every applicable verifier passed.
    Verified,
    /// At least one applicable verifier failed.
    Unverified,
    /// No evidence reference had a registered verifier; tier 3 only.
    NotVerifiable,
}

/// The stable identity of one claimed finding or artifact candidate inside a
/// reconciliation attempt. Deterministic for the same claim set.
pub fn finding_ref(goal_id: &str, goal_round: usize, index: usize) -> String {
    format!("contribution:{goal_id}/{goal_round}/finding/{index}")
}

/// The stable identity of one claimed artifact candidate.
pub fn candidate_ref(goal_id: &str, goal_round: usize, obligation_key: &str) -> String {
    format!("contribution:{goal_id}/{goal_round}/candidate/{obligation_key}")
}

/// Verify one finding's evidence references at tier 2, recording one
/// `VerifierResult` per applicable reference in deterministic order.
pub fn verify_finding(
    finding_ref: &str,
    evidence: &[String],
    context: &VerificationContext,
) -> (FindingVerification, Vec<VerifierResult>) {
    let mut results = Vec::new();
    let mut saw_verifiable = false;
    let mut failed = false;
    for raw in evidence {
        let Some(parsed) = EvidenceRef::parse(raw) else {
            continue;
        };
        saw_verifiable = true;
        let outcome = run_verifier(&parsed, context);
        if outcome == VerifierOutcome::Failed {
            failed = true;
        }
        results.push(VerifierResult {
            finding_ref: finding_ref.to_string(),
            verifier: parsed.verifier().to_string(),
            outcome,
            detail: Some(raw.clone()),
        });
    }
    let verification = if !saw_verifiable {
        FindingVerification::NotVerifiable
    } else if failed {
        FindingVerification::Unverified
    } else {
        FindingVerification::Verified
    };
    (verification, results)
}

/// Verify every finding of one contribution at tier 2.
pub fn verify_contribution_findings(
    goal_id: &str,
    goal_round: usize,
    contribution: &GoalContribution,
    context: &VerificationContext,
) -> Vec<(String, FindingVerification, Vec<VerifierResult>)> {
    contribution
        .findings
        .iter()
        .enumerate()
        .map(|(index, finding)| {
            let reference = finding_ref(goal_id, goal_round, index);
            let (verification, results) = verify_finding(&reference, &finding.evidence, context);
            (reference, verification, results)
        })
        .collect()
}

/// A tier-1 envelope failure with a durable diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvelopeRejection {
    pub finding_ref: String,
    pub reason: String,
}

/// Validate the envelope of one claimed contribution: finding claims are
/// non-empty, artifact candidates bind approved obligations with well-formed
/// digests and bounded sizes. Envelope failures are durable diagnostics, not
/// discarded evidence.
pub fn validate_contribution_envelope(
    goal_id: &str,
    goal_round: usize,
    contribution: &GoalContribution,
    approved_obligation_keys: &BTreeSet<String>,
) -> Vec<EnvelopeRejection> {
    let mut rejections = Vec::new();
    for (index, finding) in contribution.findings.iter().enumerate() {
        if finding.claim.trim().is_empty() {
            rejections.push(EnvelopeRejection {
                finding_ref: finding_ref(goal_id, goal_round, index),
                reason: "finding claim is empty".to_string(),
            });
        }
    }
    for candidate in &contribution.artifact_candidates {
        let reference = candidate_ref(goal_id, goal_round, &candidate.obligation_key);
        if !approved_obligation_keys.contains(&candidate.obligation_key) {
            rejections.push(EnvelopeRejection {
                finding_ref: reference.clone(),
                reason: format!(
                    "artifact candidate {} does not bind an approved obligation",
                    candidate.obligation_key
                ),
            });
            continue;
        }
        if !is_sha256(candidate.digest.as_deref().unwrap_or("")) {
            rejections.push(EnvelopeRejection {
                finding_ref: reference.clone(),
                reason: "artifact candidate digest must be a sha256 hex string".to_string(),
            });
        }
        if candidate.size > MAX_CANDIDATE_BYTES {
            rejections.push(EnvelopeRejection {
                finding_ref: reference,
                reason: format!(
                    "artifact candidate size {} exceeds the {} byte envelope limit",
                    candidate.size, MAX_CANDIDATE_BYTES
                ),
            });
        }
    }
    rejections
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mission::{ArtifactCandidate, Finding};

    fn context() -> VerificationContext {
        let mut context = VerificationContext {
            target_head: Some("head".to_string()),
            ..Default::default()
        };
        context.reachable_commits.insert("c1".to_string());
        context
            .existing_paths
            .insert(("c1".to_string(), "src/main.rs".to_string()));
        context
            .quotes_at_commit
            .entry("c1".to_string())
            .or_default()
            .insert("fn main() {}".to_string());
        context.passed_tests.insert("test_auth".to_string());
        context.matched_digests.insert("a".repeat(64));
        context
    }

    #[test]
    fn evidence_ref_parses_each_shape() {
        assert!(matches!(
            EvidenceRef::parse("commit:c1"),
            Some(EvidenceRef::Commit { .. })
        ));
        assert!(matches!(
            EvidenceRef::parse("path:src/main.rs@c1"),
            Some(EvidenceRef::Path { .. })
        ));
        assert!(matches!(
            EvidenceRef::parse("quote:fn main() {}@c1"),
            Some(EvidenceRef::Quote { .. })
        ));
        assert!(matches!(
            EvidenceRef::parse("test:test_auth"),
            Some(EvidenceRef::Test { .. })
        ));
        assert!(matches!(
            EvidenceRef::parse(&format!("digest:{}", "a".repeat(64))),
            Some(EvidenceRef::Digest { .. })
        ));
        assert!(EvidenceRef::parse("see the docs").is_none());
    }

    #[test]
    fn quote_with_at_sign_parses_from_the_last_separator() {
        let parsed = EvidenceRef::parse("quote:a@b@c1").unwrap();
        match parsed {
            EvidenceRef::Quote { text, commit } => {
                assert_eq!(text, "a@b");
                assert_eq!(commit, "c1");
            }
            _ => panic!("expected quote"),
        }
    }

    #[test]
    fn registry_verifiers_pass_on_pinned_facts() {
        let context = context();
        let checks = [
            (
                EvidenceRef::parse("commit:c1").unwrap(),
                VerifierOutcome::Passed,
            ),
            (
                EvidenceRef::parse("commit:zz").unwrap(),
                VerifierOutcome::Failed,
            ),
            (
                EvidenceRef::parse("path:src/main.rs@c1").unwrap(),
                VerifierOutcome::Passed,
            ),
            (
                EvidenceRef::parse("path:src/other.rs@c1").unwrap(),
                VerifierOutcome::Failed,
            ),
            (
                EvidenceRef::parse("quote:fn main() {}@c1").unwrap(),
                VerifierOutcome::Passed,
            ),
            (
                EvidenceRef::parse("test:test_auth").unwrap(),
                VerifierOutcome::Passed,
            ),
            (
                EvidenceRef::parse(&format!("digest:{}", "a".repeat(64))).unwrap(),
                VerifierOutcome::Passed,
            ),
        ];
        for (evidence, expected) in checks {
            assert_eq!(run_verifier(&evidence, &context), expected);
        }
    }

    #[test]
    fn finding_verification_routes_by_evidence_shape() {
        let context = context();
        let (verified, results) = verify_finding(
            "contribution:g1/1/finding/0",
            &["commit:c1".to_string()],
            &context,
        );
        assert_eq!(verified, FindingVerification::Verified);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].verifier, "commit_reachable");

        let (failed, _) = verify_finding(
            "contribution:g1/1/finding/1",
            &["commit:zz".to_string()],
            &context,
        );
        assert_eq!(failed, FindingVerification::Unverified);

        let (tier3, results) = verify_finding(
            "contribution:g1/1/finding/2",
            &["the code looks fine".to_string()],
            &context,
        );
        assert_eq!(tier3, FindingVerification::NotVerifiable);
        assert!(results.is_empty());
    }

    #[test]
    fn mixed_evidence_fails_when_any_verifier_fails() {
        let context = context();
        let (verification, results) = verify_finding(
            "contribution:g1/1/finding/0",
            &[
                "commit:c1".to_string(),
                "commit:gone".to_string(),
                "prose".to_string(),
            ],
            &context,
        );
        assert_eq!(verification, FindingVerification::Unverified);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn envelope_rejects_unbound_and_malformed_candidates() {
        let contribution = GoalContribution {
            bound_context_digest: None,
            criteria_evidence: Vec::new(),
            findings: vec![Finding {
                claim: "  ".to_string(),
                evidence: Vec::new(),
            }],
            challenged_assumptions: Vec::new(),
            artifact_candidates: vec![
                ArtifactCandidate {
                    obligation_key: "interface-contract".to_string(),
                    kind: "contract".to_string(),
                    media_type: None,
                    size: 10,
                    digest: Some("not-a-digest".to_string()),
                    handoff_ref: None,
                    evidence: Vec::new(),
                    provenance: None,
                    proposed_authority: None,
                },
                ArtifactCandidate {
                    obligation_key: "unapproved".to_string(),
                    kind: "contract".to_string(),
                    media_type: None,
                    size: 10,
                    digest: Some("a".repeat(64)),
                    handoff_ref: None,
                    evidence: Vec::new(),
                    provenance: None,
                    proposed_authority: None,
                },
            ],
            suggested_followups: Vec::new(),
            downstream_invalidations: Vec::new(),
            digest: None,
        };
        let approved = BTreeSet::from(["interface-contract".to_string()]);
        let rejections = validate_contribution_envelope("g1", 1, &contribution, &approved);
        assert_eq!(rejections.len(), 3);
        assert!(rejections[0].reason.contains("empty"));
        assert!(rejections[1].reason.contains("sha256"));
        assert!(rejections[2].reason.contains("approved obligation"));
    }

    #[test]
    fn oversized_candidate_is_rejected() {
        let contribution = GoalContribution {
            bound_context_digest: None,
            criteria_evidence: Vec::new(),
            findings: Vec::new(),
            challenged_assumptions: Vec::new(),
            artifact_candidates: vec![ArtifactCandidate {
                obligation_key: "interface-contract".to_string(),
                kind: "contract".to_string(),
                media_type: None,
                size: MAX_CANDIDATE_BYTES + 1,
                digest: Some("a".repeat(64)),
                handoff_ref: None,
                evidence: Vec::new(),
                provenance: None,
                proposed_authority: None,
            }],
            suggested_followups: Vec::new(),
            downstream_invalidations: Vec::new(),
            digest: None,
        };
        let approved = BTreeSet::from(["interface-contract".to_string()]);
        let rejections = validate_contribution_envelope("g1", 1, &contribution, &approved);
        assert_eq!(rejections.len(), 1);
        assert!(rejections[0].reason.contains("exceeds"));
    }
}
