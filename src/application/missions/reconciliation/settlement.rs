//! Wave-boundary settlement, stragglers, and late contributions.
//!
//! A wave is settled when every required Goal is terminal or in Review with
//! valid integration, Quality, and Governance evidence, and every optional
//! Goal is terminal, in Review with valid evidence, or has exceeded the
//! optional-wait budget. A closed reconciliation is closed: contributions
//! that settle afterwards are `late` and enter the next boundary's claim set
//! unconditionally, and a mandatory pre-Synthesis sweep claims everything
//! remaining before Synthesis begins, so no evidence is lost to ordering.
//!
//! See `docs/mission-reconciliation.md` ("Wave-boundary semantics").

use serde::{Deserialize, Serialize};

use crate::model::mission::ReconciliationReceipt;

/// The state of one wave Goal relevant to settlement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalWaveState {
    /// Done, Failed, or Cancelled.
    Terminal,
    /// In Review with valid integration, Quality, and Governance evidence.
    ReviewReady,
    /// Still executing or waiting for evidence.
    Pending,
    /// Optional Goal whose configured optional-wait budget expired.
    WaitExceeded,
}

/// One wave Goal's settlement-relevant facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WaveGoalStatus {
    pub mission_goal_key: String,
    pub required: bool,
    pub state: GoalWaveState,
}

/// The settlement evaluation of one wave.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WaveSettlement {
    pub settled: bool,
    /// Required Goals still blocking settlement, with reasons.
    pub blocking: Vec<String>,
    /// Optional Goals whose wait budget expired; settlement proceeds without
    /// them and their later contributions are late.
    pub wait_exceeded: Vec<String>,
}

/// Evaluate whether one wave is settled.
///
/// Expected capacity waits elsewhere in the fleet are neutral and do not
/// delay settlement; they surface as `Pending` here only for Goals that are
/// themselves not yet evidence-ready.
pub fn evaluate_wave_settlement(goals: &[WaveGoalStatus]) -> WaveSettlement {
    let mut settlement = WaveSettlement::default();
    for goal in goals {
        match (goal.required, goal.state) {
            (_, GoalWaveState::Terminal) | (_, GoalWaveState::ReviewReady) => {}
            (false, GoalWaveState::WaitExceeded) => {
                settlement.wait_exceeded.push(goal.mission_goal_key.clone());
            }
            (false, GoalWaveState::Pending) => {
                settlement.blocking.push(format!(
                    "{}: optional Goal still pending",
                    goal.mission_goal_key
                ));
            }
            (true, GoalWaveState::WaitExceeded) | (true, GoalWaveState::Pending) => {
                settlement.blocking.push(format!(
                    "{}: required Goal not terminal or Review-ready",
                    goal.mission_goal_key
                ));
            }
        }
    }
    settlement.settled = settlement.blocking.is_empty();
    settlement
}

/// How one contribution relates to a closed reconciliation attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimClass {
    /// The closed attempt already claimed this contribution.
    Claimed,
    /// The contribution settled after the claim window closed. It is durable,
    /// inspectable evidence owned by its GoalRound, never consumed by the
    /// closed attempt, and enters the next claim set unconditionally.
    Late,
}

/// Classify one contribution digest against a closed receipt's claim set.
pub fn classify_against_receipt(receipt: &ReconciliationReceipt, digest: &str) -> ClaimClass {
    if receipt.claim_set.iter().any(|claimed| claimed == digest) {
        ClaimClass::Claimed
    } else {
        ClaimClass::Late
    }
}

/// The next boundary's claim set: every eligible contribution that a closed
/// attempt did not consume. Deferred findings and late contributions both
/// carry, so a straggler never mints a snapshot by itself and no evidence is
/// lost to ordering.
pub fn next_claim_set(
    receipts: &[ReconciliationReceipt],
    eligible_digests: &[String],
) -> Vec<String> {
    let mut claimed = std::collections::BTreeSet::new();
    for receipt in receipts {
        claimed.extend(receipt.claim_set.iter().cloned());
    }
    let mut next: Vec<String> = eligible_digests
        .iter()
        .filter(|digest| !claimed.contains(*digest))
        .cloned()
        .collect();
    next.sort();
    next.dedup();
    next
}

/// Whether a mandatory pre-Synthesis sweep reconciliation is required: all
/// execution waves are settled and eligible contributions remain unclaimed.
pub fn pre_synthesis_sweep_needed(
    receipts: &[ReconciliationReceipt],
    eligible_digests: &[String],
    execution_waves_settled: bool,
) -> bool {
    execution_waves_settled && !next_claim_set(receipts, eligible_digests).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(key: &str, required: bool, state: GoalWaveState) -> WaveGoalStatus {
        WaveGoalStatus {
            mission_goal_key: key.to_string(),
            required,
            state,
        }
    }

    #[test]
    fn required_goals_must_be_terminal_or_review_ready() {
        let settled = evaluate_wave_settlement(&[
            goal("k1", true, GoalWaveState::Terminal),
            goal("k2", true, GoalWaveState::ReviewReady),
            goal("k3", false, GoalWaveState::Pending),
        ]);
        assert!(!settled.settled);
        assert!(settled.blocking[0].contains("k3"));

        let open = evaluate_wave_settlement(&[goal("k1", true, GoalWaveState::Pending)]);
        assert!(!open.settled);
        assert!(open.blocking[0].contains("required"));
    }

    #[test]
    fn optional_wait_expiry_settles_the_wave() {
        let settled = evaluate_wave_settlement(&[
            goal("k1", true, GoalWaveState::Terminal),
            goal("k2", false, GoalWaveState::WaitExceeded),
        ]);
        assert!(settled.settled);
        assert_eq!(settled.wait_exceeded, vec!["k2".to_string()]);
    }

    #[test]
    fn late_contribution_classification() {
        let receipt = ReconciliationReceipt {
            attempt: "mission:M:round:1:reconcile:1:1".to_string(),
            parent_snapshot: 1,
            next_snapshot: 2,
            wave: Some(1),
            claim_set: vec!["digest-a".to_string()],
            verifier_results: Vec::new(),
            accepted: Vec::new(),
            rejected: Vec::new(),
            deferred: Vec::new(),
            contested: Vec::new(),
            dissent: Vec::new(),
            criticism_ref: None,
            decision_requests: Vec::new(),
            budgets: Default::default(),
            plan_quality: None,
            correction: None,
            created: "2026-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(
            classify_against_receipt(&receipt, "digest-a"),
            ClaimClass::Claimed
        );
        assert_eq!(
            classify_against_receipt(&receipt, "digest-b"),
            ClaimClass::Late
        );
    }

    #[test]
    fn next_claim_set_carries_unclaimed_and_is_deduplicated() {
        let receipt = ReconciliationReceipt {
            attempt: "a".to_string(),
            parent_snapshot: 1,
            next_snapshot: 2,
            wave: Some(1),
            claim_set: vec!["digest-a".to_string(), "digest-b".to_string()],
            verifier_results: Vec::new(),
            accepted: Vec::new(),
            rejected: Vec::new(),
            deferred: Vec::new(),
            contested: Vec::new(),
            dissent: Vec::new(),
            criticism_ref: None,
            decision_requests: Vec::new(),
            budgets: Default::default(),
            plan_quality: None,
            correction: None,
            created: "2026-01-01T00:00:00Z".to_string(),
        };
        let next = next_claim_set(
            &[receipt],
            &[
                "digest-b".to_string(),
                "digest-c".to_string(),
                "digest-c".to_string(),
                "digest-d".to_string(),
            ],
        );
        assert_eq!(next, vec!["digest-c".to_string(), "digest-d".to_string()]);
    }

    #[test]
    fn sweep_runs_only_after_all_waves_settled_with_remaining_evidence() {
        let empty: Vec<ReconciliationReceipt> = Vec::new();
        assert!(pre_synthesis_sweep_needed(
            &empty,
            &["digest-a".to_string()],
            true
        ));
        assert!(!pre_synthesis_sweep_needed(&empty, &[], true));
        assert!(!pre_synthesis_sweep_needed(
            &empty,
            &["digest-a".to_string()],
            false
        ));
    }
}
