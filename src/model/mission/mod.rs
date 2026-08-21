//! Mission domain types, status policy, and pure derivations.
//!
//! A Mission is a governed system-level outcome that owns a composite
//! lifecycle. It is not a larger `Goal` record and does not replace Goal
//! workflow. See `docs/mission-spec.md`.

use serde::{Deserialize, Serialize};

use crate::model::{JsonObject, Timestamp};

/// Explicit Mission workflow state. Never mechanically derived from Goal
/// counts or Feature rollups.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissionStatus {
    Draft,
    Investigate,
    Plan,
    Execute,
    Synthesize,
    Quality,
    Governance,
    Review,
    Consolidate,
    Done,
    Failed,
    Cancelled,
}

impl MissionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Investigate => "investigate",
            Self::Plan => "plan",
            Self::Execute => "execute",
            Self::Synthesize => "synthesize",
            Self::Quality => "quality",
            Self::Governance => "governance",
            Self::Review => "review",
            Self::Consolidate => "consolidate",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse_wire(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "investigate" => Some(Self::Investigate),
            "plan" => Some(Self::Plan),
            "execute" => Some(Self::Execute),
            "synthesize" => Some(Self::Synthesize),
            "quality" => Some(Self::Quality),
            "governance" => Some(Self::Governance),
            "review" => Some(Self::Review),
            "consolidate" => Some(Self::Consolidate),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// A stable success criterion carried by a Mission charter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MissionCriterion {
    pub id: String,
    pub description: String,
}

/// A stable artifact-contract obligation carried by a Mission charter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactObligation {
    pub key: String,
    pub kind: String,
    pub purpose: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub validation_policy: Option<String>,
    #[serde(default)]
    pub consumers: Vec<String>,
}

/// The editable current frame used to author the next Round. Active workflow
/// never relies on these mutable projections directly.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionFrame {
    pub name: String,
    pub intent: String,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub success_criteria: Vec<MissionCriterion>,
    #[serde(default)]
    pub artifact_contract: Vec<ArtifactObligation>,
}

/// The top-level mutable Mission record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Mission {
    pub id: String,
    pub name: String,
    pub intent: String,
    pub status: MissionStatus,
    #[serde(default)]
    pub reporter: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub coordinator_node_id: Option<String>,
    #[serde(default)]
    pub success_criteria: Vec<MissionCriterion>,
    #[serde(default)]
    pub artifact_contract: Vec<ArtifactObligation>,
    #[serde(default)]
    pub current_round: Option<usize>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub rounds: Vec<MissionRound>,
    pub created: Timestamp,
    pub updated: Timestamp,
}

/// An append-only history record within a Mission, analogous to `GoalRound`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionRound {
    pub number: usize,
    pub request: MissionRoundRequest,
    #[serde(default)]
    pub input_bindings: Vec<OutcomeBinding>,
    #[serde(default)]
    pub plan: Option<MissionPlan>,
    #[serde(default)]
    pub plan_amendments: Vec<MissionPlan>,
    #[serde(default)]
    pub snapshots: Vec<MissionSnapshot>,
    #[serde(default)]
    pub reconciliation_receipts: Vec<ReconciliationReceipt>,
    #[serde(default)]
    pub phase_evidence: JsonObject,
    #[serde(default)]
    pub review: Option<MissionReview>,
    #[serde(default)]
    pub outcome: Option<OutcomeManifest>,
    #[serde(default)]
    pub outcome_publication: Option<OutcomePublication>,
    #[serde(default)]
    pub failure: Option<MissionFailure>,
    pub created: Timestamp,
    pub updated: Timestamp,
}

/// The frozen charter for one attempt.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionRoundRequest {
    pub intent: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub criteria: Vec<MissionCriterion>,
    #[serde(default)]
    pub artifact_obligations: Vec<ArtifactObligation>,
    pub authorizing_request: String,
    #[serde(default)]
    pub charter_digest: Option<String>,
}

/// An immutable, digest-addressed manifest of what the Mission accepts at one
/// wave boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionSnapshot {
    pub version: usize,
    #[serde(default)]
    pub parent_version: Option<usize>,
    #[serde(default)]
    pub target_head: Option<String>,
    #[serde(default)]
    pub plan_digest: Option<String>,
    #[serde(default)]
    pub artifact_refs: Vec<ArtifactRef>,
    #[serde(default)]
    pub input_refs: Vec<String>,
    #[serde(default)]
    pub consumed_contribution_refs: Vec<String>,
    #[serde(default)]
    pub knowledge_index: Vec<KnowledgeAssertion>,
    #[serde(default)]
    pub corrects_snapshot: Option<usize>,
    #[serde(default)]
    pub digest: Option<String>,
    pub created: Timestamp,
}

/// The kind of one accepted knowledge assertion.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionKind {
    Fact,
    Model,
    Risk,
    Assumption,
    Contradiction,
    Question,
}

impl AssertionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Model => "model",
            Self::Risk => "risk",
            Self::Assumption => "assumption",
            Self::Contradiction => "contradiction",
            Self::Question => "question",
        }
    }

    pub fn parse_wire(value: &str) -> Option<Self> {
        match value {
            "fact" => Some(Self::Fact),
            "model" => Some(Self::Model),
            "risk" => Some(Self::Risk),
            "assumption" => Some(Self::Assumption),
            "contradiction" => Some(Self::Contradiction),
            "question" => Some(Self::Question),
            _ => None,
        }
    }
}

/// One accepted claim in a snapshot's knowledge ledger. Assertion state is
/// never stored; `active`, `superseded`, `contested`, and `invalidated` are
/// derived by walking the snapshot chain.
///
/// `scope_refs` carries the structural applicability links (criterion ids,
/// artifact keys, Mission Goal keys) so invalidation and capsule compilation
/// stay exact rather than matching free text.
///
/// When `kind` is `Contradiction`, `members` names the conflicting assertion
/// ids and `resolution`/`resolved_by` record how (or whether) the
/// contradiction was resolved. A contradiction is a first-class assertion
/// kind, not an error condition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KnowledgeAssertion {
    pub assertion_id: String,
    pub kind: AssertionKind,
    pub authority: ArtifactAuthority,
    #[serde(default)]
    pub provenance: Option<String>,
    #[serde(default)]
    pub qualified: Option<String>,
    #[serde(default)]
    pub supersedes: Vec<String>,
    #[serde(default)]
    pub corrects: Vec<String>,
    #[serde(default)]
    pub derived_from: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub scope_refs: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub supersedable: bool,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub resolution: Option<ContradictionResolution>,
    #[serde(default)]
    pub resolved_by: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionResolution {
    Evidence,
    ScopeSplit,
    Superseded,
    Open,
}

impl ContradictionResolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Evidence => "evidence",
            Self::ScopeSplit => "scope_split",
            Self::Superseded => "superseded",
            Self::Open => "open",
        }
    }

    pub fn parse_wire(value: &str) -> Option<Self> {
        match value {
            "evidence" => Some(Self::Evidence),
            "scope_split" => Some(Self::ScopeSplit),
            "superseded" => Some(Self::Superseded),
            "open" => Some(Self::Open),
            _ => None,
        }
    }
}

/// Why one derived tier-2 verifier outcome was recorded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierOutcome {
    Passed,
    Failed,
    NotApplicable,
}

impl VerifierOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// One deterministic tier-2 verification result, part of the durable receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifierResult {
    pub finding_ref: String,
    pub verifier: String,
    pub outcome: VerifierOutcome,
    #[serde(default)]
    pub detail: Option<String>,
}

/// A ranked, batched decision request raised by one reconciliation attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecisionRequest {
    pub id: String,
    #[serde(default)]
    pub group: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub load_bearing: bool,
    #[serde(default)]
    pub rank: usize,
    #[serde(default)]
    pub deferred: bool,
}

/// One bounded budget and how much of it one attempt consumed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BudgetReport {
    #[serde(default)]
    pub limit: usize,
    #[serde(default)]
    pub used: usize,
}

impl BudgetReport {
    pub fn exhausted(&self) -> bool {
        self.limit != 0 && self.used >= self.limit
    }
}

/// Independent bounded budgets for one reconciliation attempt.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReconciliationBudgets {
    #[serde(default)]
    pub repair: BudgetReport,
    #[serde(default)]
    pub agent: BudgetReport,
    #[serde(default)]
    pub decision: BudgetReport,
    #[serde(default)]
    pub publication: BudgetReport,
}

/// A reference to an immutable artifact file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ArtifactRef {
    pub key: String,
    pub title: String,
    pub kind: String,
    pub authority: ArtifactAuthority,
    pub path: String,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub provenance: Option<String>,
    #[serde(default)]
    pub applicability: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactAuthority {
    Evidence,
    Model,
    Decision,
    Directive,
}

impl ArtifactAuthority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Evidence => "evidence",
            Self::Model => "model",
            Self::Decision => "decision",
            Self::Directive => "directive",
        }
    }

    pub fn parse_wire(value: &str) -> Option<Self> {
        match value {
            "evidence" => Some(Self::Evidence),
            "model" => Some(Self::Model),
            "decision" => Some(Self::Decision),
            "directive" => Some(Self::Directive),
            _ => None,
        }
    }
}

/// An immutable base plan plus append-only amendments.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionPlan {
    #[serde(default)]
    pub charter_digest: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub criteria_coverage: Vec<String>,
    #[serde(default)]
    pub waves: Vec<MissionWave>,
    #[serde(default)]
    pub artifact_obligations: Vec<ArtifactObligation>,
    #[serde(default)]
    pub criticism: Option<String>,
    #[serde(default)]
    pub resolutions: Vec<String>,
    #[serde(default)]
    pub effective_digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionWave {
    pub number: usize,
    pub purpose: String,
    #[serde(default)]
    pub goal_specs: Vec<MissionGoalSpec>,
    #[serde(default)]
    pub required_snapshot: Option<usize>,
    #[serde(default)]
    pub completion_condition: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionGoalSpec {
    pub mission_goal_key: String,
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub criterion_ids: Vec<String>,
    #[serde(default)]
    pub input_artifact_keys: Vec<String>,
    #[serde(default)]
    pub output_artifact_keys: Vec<String>,
    #[serde(default)]
    pub expected_findings: Vec<String>,
    #[serde(default)]
    pub feature_id: Option<String>,
    #[serde(default)]
    pub feature_order: Option<i64>,
    #[serde(default)]
    pub preferred_node: Option<String>,
}

/// A fenced reconciliation attempt receipt. A closed receipt and its claim
/// set are immutable; evidence that settles later is `late` and waits for the
/// next boundary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReconciliationReceipt {
    pub attempt: String,
    pub parent_snapshot: usize,
    pub next_snapshot: usize,
    #[serde(default)]
    pub wave: Option<usize>,
    #[serde(default)]
    pub claim_set: Vec<String>,
    #[serde(default)]
    pub verifier_results: Vec<VerifierResult>,
    #[serde(default)]
    pub accepted: Vec<String>,
    #[serde(default)]
    pub rejected: Vec<String>,
    #[serde(default)]
    pub deferred: Vec<String>,
    #[serde(default)]
    pub contested: Vec<String>,
    #[serde(default)]
    pub dissent: Vec<String>,
    #[serde(default)]
    pub criticism_ref: Option<String>,
    #[serde(default)]
    pub decision_requests: Vec<DecisionRequest>,
    #[serde(default)]
    pub budgets: ReconciliationBudgets,
    #[serde(default)]
    pub plan_quality: Option<String>,
    #[serde(default)]
    pub correction: Option<String>,
    pub created: Timestamp,
}

/// A Mission Review settlement.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionReview {
    pub reviewer: String,
    pub rationale: String,
    pub criteria_results: Vec<CriterionResult>,
    pub approved: bool,
    pub created: Timestamp,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CriterionResult {
    pub criterion_id: String,
    pub result: CriterionOutcome,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionOutcome {
    Met,
    Partial,
    Unmet,
    Contradicted,
    Waived,
}

impl CriterionOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Met => "met",
            Self::Partial => "partial",
            Self::Unmet => "unmet",
            Self::Contradicted => "contradicted",
            Self::Waived => "waived",
        }
    }
}

/// The immutable accepted result of one Mission Round.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OutcomeManifest {
    pub mission_id: String,
    pub mission_round: usize,
    #[serde(default)]
    pub charter_digest: Option<String>,
    #[serde(default)]
    pub final_snapshot: Option<usize>,
    #[serde(default)]
    pub criteria_results: Vec<CriterionResult>,
    #[serde(default)]
    pub artifact_refs: Vec<ArtifactRef>,
    #[serde(default)]
    pub goal_evidence_refs: Vec<String>,
    #[serde(default)]
    pub target_commit_refs: Vec<String>,
    #[serde(default)]
    pub input_bindings: Vec<OutcomeBinding>,
    #[serde(default)]
    pub manifest_digest: Option<String>,
    #[serde(default)]
    pub approved_at: Option<Timestamp>,
    #[serde(default)]
    pub approved_by: Option<String>,
}

/// Publication evidence recorded after Git assigns the manifest commit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OutcomePublication {
    #[serde(default)]
    pub manifest_digest: Option<String>,
    #[serde(default)]
    pub outcome_state_commit: Option<String>,
    #[serde(default)]
    pub verified_path_digests: Vec<String>,
    #[serde(default)]
    pub published_by: Option<String>,
    #[serde(default)]
    pub verified_at: Option<Timestamp>,
}

/// An exact published Outcome consumed by a Mission Round.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OutcomeBinding {
    pub source_mission_id: String,
    pub source_mission_round: usize,
    #[serde(default)]
    pub source_manifest_digest: Option<String>,
    #[serde(default)]
    pub source_state_commit: Option<String>,
    #[serde(default)]
    pub selected_artifact_refs: Vec<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionFailure {
    pub reason: String,
    pub created: Timestamp,
}

/// The optional Mission binding stored on a Goal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MissionGoalBinding {
    pub mission_id: String,
    pub mission_goal_key: String,
}

/// The typed Mission context binding stored on a Mission-bound GoalRound.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoalRoundMissionContext {
    pub mission_id: String,
    pub mission_round: usize,
    pub snapshot_version: usize,
    #[serde(default)]
    pub snapshot_digest: Option<String>,
    #[serde(default)]
    pub capsule_digest: Option<String>,
    #[serde(default)]
    pub capsule_manifest_digest: Option<String>,
}

/// A Mission-bound GoalRound's settled contribution.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GoalContribution {
    #[serde(default)]
    pub bound_context_digest: Option<String>,
    #[serde(default)]
    pub criteria_evidence: Vec<String>,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub challenged_assumptions: Vec<String>,
    #[serde(default)]
    pub artifact_candidates: Vec<ArtifactCandidate>,
    #[serde(default)]
    pub suggested_followups: Vec<String>,
    #[serde(default)]
    pub downstream_invalidations: Vec<String>,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Finding {
    pub claim: String,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ArtifactCandidate {
    pub obligation_key: String,
    pub kind: String,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub handoff_ref: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub provenance: Option<String>,
    #[serde(default)]
    pub proposed_authority: Option<ArtifactAuthority>,
}

/// Derived index projection for the Missions list.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MissionIndexProjection {
    pub id: String,
    pub name: String,
    pub status: MissionStatus,
    pub reporter: Option<String>,
    pub assignee: Option<String>,
    pub coordinator_node_id: Option<String>,
    pub current_round: Option<usize>,
    pub current_wave: Option<usize>,
    pub criteria_summary: MissionCriteriaSummary,
    pub outcome_available: bool,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub json_path: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MissionCriteriaSummary {
    pub total: usize,
    pub met: usize,
    pub partial: usize,
    pub unmet: usize,
    pub contradicted: usize,
    pub waived: usize,
}

/// Derived rollup of contained Goals.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MissionRollup {
    pub goal_count: usize,
    pub done_count: usize,
    pub active_count: usize,
    pub failed_count: usize,
    pub cancelled_count: usize,
    pub required_failures: usize,
}

/// The Mission status transition policy.
///
/// The forward lifecycle is `Draft -> Investigate -> Plan -> Execute ->
/// Synthesize -> Quality -> Governance -> Review -> Consolidate -> Done`.
/// `Failed` and `Cancelled` are terminal for the current Round; a new Round may
/// resume the Mission when explicitly authorized.
pub fn mission_status_transition(from: &MissionStatus, to: &MissionStatus) -> bool {
    use MissionStatus::*;
    if from == to {
        return true;
    }
    matches!(
        (from, to),
        (Draft, Investigate)
            | (Investigate, Plan)
            | (Plan, Execute)
            | (Execute, Synthesize)
            | (Synthesize, Quality)
            | (Quality, Governance)
            | (Governance, Review)
            | (Review, Consolidate)
            | (Consolidate, Done)
            // Recovery and revision paths.
            | (Synthesize, Execute)
            | (Quality, Execute)
            | (Governance, Execute)
            | (Review, Execute)
            | (Consolidate, Review)
            // Terminal transitions.
            | (Draft, Cancelled)
            | (Investigate, Cancelled)
            | (Plan, Cancelled)
            | (Execute, Cancelled)
            | (Synthesize, Cancelled)
            | (Quality, Cancelled)
            | (Governance, Cancelled)
            | (Review, Cancelled)
            | (Consolidate, Cancelled)
            | (Draft, Failed)
            | (Investigate, Failed)
            | (Plan, Failed)
            | (Execute, Failed)
            | (Synthesize, Failed)
            | (Quality, Failed)
            | (Governance, Failed)
            | (Review, Failed)
            | (Consolidate, Failed)
            // A new Round may resume a terminal Mission.
            | (Failed, Draft)
            | (Cancelled, Draft)
            | (Done, Draft)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_lifecycle_is_allowed() {
        let order = [
            MissionStatus::Draft,
            MissionStatus::Investigate,
            MissionStatus::Plan,
            MissionStatus::Execute,
            MissionStatus::Synthesize,
            MissionStatus::Quality,
            MissionStatus::Governance,
            MissionStatus::Review,
            MissionStatus::Consolidate,
            MissionStatus::Done,
        ];
        for pair in order.windows(2) {
            assert!(
                mission_status_transition(&pair[0], &pair[1]),
                "{:?} -> {:?} should be allowed",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn terminal_statuses_are_terminal() {
        assert!(MissionStatus::Done.is_terminal());
        assert!(MissionStatus::Failed.is_terminal());
        assert!(MissionStatus::Cancelled.is_terminal());
        assert!(!MissionStatus::Review.is_terminal());
    }

    #[test]
    fn status_round_trips_through_wire() {
        for status in [
            MissionStatus::Draft,
            MissionStatus::Execute,
            MissionStatus::Governance,
            MissionStatus::Done,
        ] {
            assert_eq!(
                MissionStatus::parse_wire(status.as_str()),
                Some(status.clone())
            );
            assert_eq!(
                serde_json::from_str::<MissionStatus>(&format!("\"{}\"", status.as_str())).unwrap(),
                status
            );
        }
    }

    #[test]
    fn arbitrary_forward_jumps_are_denied() {
        assert!(!mission_status_transition(
            &MissionStatus::Draft,
            &MissionStatus::Execute
        ));
        assert!(!mission_status_transition(
            &MissionStatus::Plan,
            &MissionStatus::Review
        ));
    }
}
