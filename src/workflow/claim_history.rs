use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Duration, Utc};

use super::{WorkflowAutomationState, WorkflowClaim, WorkflowClaimState, WorkflowGoalClaimSummary};

pub(super) const MAX_TERMINAL_CLAIM_HISTORY: usize = 256;
pub(super) const EXECUTION_FAILURE_QUARANTINE_THRESHOLD: u32 = 5;
pub(super) const CLAIM_HISTORY_VERSION: u32 = 1;
const EXECUTION_RETRY_BASE_SECONDS: i64 = 5;
const EXECUTION_RETRY_MAX_SECONDS: i64 = 300;
const MAX_FAILURE_MESSAGE_CHARS: usize = 1_024;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct EquivalentTerminalClaim {
    goal_id: String,
    node_id: String,
    provider: String,
    target_app_id: String,
    round_idx: Option<usize>,
    goal_revision: Option<u64>,
    failure_stage: Option<String>,
    failure_message: Option<String>,
    state: WorkflowClaimState,
}

impl EquivalentTerminalClaim {
    fn from_claim(claim: &WorkflowClaim) -> Self {
        Self {
            goal_id: claim.goal_id.clone(),
            node_id: claim.node_id.clone(),
            provider: claim.provider.clone(),
            target_app_id: claim.target_app_id.clone(),
            round_idx: claim.round_idx,
            goal_revision: claim.goal_revision,
            failure_stage: claim.failure_stage.clone(),
            failure_message: claim.failure_message.clone(),
            state: claim.state.clone(),
        }
    }
}

impl WorkflowAutomationState {
    pub(crate) fn normalize_claim_history(&mut self) {
        self.rebuild_claim_summaries();

        let mut retained = BTreeMap::<usize, WorkflowClaim>::new();
        let mut equivalent = HashMap::<EquivalentTerminalClaim, usize>::new();
        let mut terminal_count = 0usize;
        let mut terminal = Vec::new();
        for (index, claim) in self.claims.drain(..).enumerate() {
            if claim.is_active() {
                retained.insert(index, claim);
            } else {
                terminal.push((index, claim));
            }
        }
        // Updated time, then insertion order, defines recency. A long-lived
        // active claim that just settled must not be discarded merely because
        // its original vector position predates newer attempts.
        terminal.sort_by(|(left_index, left), (right_index, right)| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right_index.cmp(left_index))
        });
        for (index, claim) in terminal {
            let key = EquivalentTerminalClaim::from_claim(&claim);
            if let Some(retained_index) = equivalent.get(&key).copied() {
                let equivalent_claim = retained
                    .get_mut(&retained_index)
                    .expect("equivalent claim index must remain retained");
                equivalent_claim.occurrences = equivalent_claim
                    .occurrences
                    .saturating_add(claim.occurrences.max(1));
                continue;
            }
            if terminal_count == MAX_TERMINAL_CLAIM_HISTORY {
                continue;
            }
            terminal_count += 1;
            equivalent.insert(key, index);
            retained.insert(index, claim);
        }
        self.claims = retained.into_values().collect();
    }

    fn rebuild_claim_summaries(&mut self) {
        let previous = std::mem::take(&mut self.claim_summaries);
        let mut summaries = if previous.is_empty() {
            BTreeMap::new()
        } else {
            previous
        };

        for claim in &self.claims {
            let summary = summaries.entry(claim.goal_id.clone()).or_default();
            if claim.failure_stage.as_deref() == Some("preparation")
                && summary
                    .latest_preparation_failure
                    .as_ref()
                    .is_none_or(|existing| claim_is_newer_or_equal(claim, existing))
            {
                summary.latest_preparation_failure = Some(claim.clone());
            }

            let transition_is_new = summary
                .latest_claim
                .as_ref()
                .is_none_or(|latest| claim_transition_differs(claim, latest));
            let should_be_latest = summary
                .latest_claim
                .as_ref()
                .is_none_or(|latest| claim_is_newer_or_equal(claim, latest));
            if !should_be_latest {
                continue;
            }
            if transition_is_new {
                apply_latest_transition(summary, claim);
            } else {
                summary.latest_claim = Some(claim.clone());
            }
        }
        self.claim_summaries = summaries;
    }

    pub(crate) fn claim_retry_allowed(&self, goal_id: &str, now: DateTime<Utc>) -> bool {
        let Some(summary) = self.claim_summaries.get(goal_id) else {
            return true;
        };
        if summary.execution_quarantined {
            return false;
        }
        let Some(not_before) = summary.retry_not_before.as_deref() else {
            return true;
        };
        DateTime::parse_from_rfc3339(not_before)
            .map(|not_before| now >= not_before.with_timezone(&Utc))
            .unwrap_or(false)
    }

    pub(crate) fn claim_history_needs_persistence(&self) -> bool {
        self.claim_history_version < CLAIM_HISTORY_VERSION
            && (!self.claims.is_empty() || !self.claim_summaries.is_empty())
    }

    pub(crate) fn active_claim_count(&self) -> usize {
        self.active_claims().count()
    }

    pub(crate) fn active_claim_goal_ids(&self) -> impl Iterator<Item = &str> {
        self.active_claims().map(|claim| claim.goal_id.as_str())
    }

    pub(crate) fn active_claims(&self) -> impl Iterator<Item = &WorkflowClaim> {
        self.claims.iter().filter(|claim| claim.is_active())
    }

    pub(crate) fn active_claims_for_goal(
        &self,
        goal_id: &str,
    ) -> impl Iterator<Item = &WorkflowClaim> {
        self.active_claims()
            .filter(move |claim| claim.goal_id == goal_id)
    }

    pub(crate) fn claim_by_id(&self, claim_id: &str) -> Option<&WorkflowClaim> {
        self.claim_summaries
            .values()
            .filter_map(|summary| summary.latest_claim.as_ref())
            .find(|claim| claim.claim_id == claim_id)
            .or_else(|| self.claims.iter().find(|claim| claim.claim_id == claim_id))
    }

    pub(crate) fn claim_by_execution(&self, execution_id: &str) -> Option<&WorkflowClaim> {
        self.claim_summaries
            .values()
            .filter_map(|summary| summary.latest_claim.as_ref())
            .find(|claim| claim.execution_id.as_deref() == Some(execution_id))
            .or_else(|| {
                self.claims
                    .iter()
                    .rev()
                    .find(|claim| claim.execution_id.as_deref() == Some(execution_id))
            })
    }
}

fn apply_latest_transition(summary: &mut WorkflowGoalClaimSummary, claim: &WorkflowClaim) {
    match claim.state {
        WorkflowClaimState::Failed if claim.failure_stage.as_deref() == Some("execution") => {
            summary.consecutive_execution_failures = summary
                .consecutive_execution_failures
                .saturating_add(claim.occurrences.max(1))
                .min(EXECUTION_FAILURE_QUARANTINE_THRESHOLD);
            summary.execution_quarantined =
                summary.consecutive_execution_failures >= EXECUTION_FAILURE_QUARANTINE_THRESHOLD;
            summary.retry_not_before = Some(execution_retry_not_before(
                &claim.updated_at,
                summary.consecutive_execution_failures,
            ));
        }
        WorkflowClaimState::Completed => {
            summary.consecutive_execution_failures = 0;
            summary.execution_quarantined = false;
            summary.retry_not_before = None;
        }
        WorkflowClaimState::Cancelled | WorkflowClaimState::Interrupted => {
            summary.consecutive_execution_failures = 0;
            summary.execution_quarantined = false;
            summary.retry_not_before = None;
        }
        _ => {}
    }
    summary.latest_claim = Some(claim.clone());
}

fn execution_retry_not_before(updated_at: &str, failure_count: u32) -> String {
    let shift = failure_count.saturating_sub(1).min(31);
    let multiplier = 1_i64.checked_shl(shift).unwrap_or(i64::MAX);
    let delay = EXECUTION_RETRY_BASE_SECONDS
        .saturating_mul(multiplier)
        .min(EXECUTION_RETRY_MAX_SECONDS);
    let failed_at = DateTime::parse_from_rfc3339(updated_at)
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    (failed_at + Duration::seconds(delay)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn claim_is_newer_or_equal(candidate: &WorkflowClaim, current: &WorkflowClaim) -> bool {
    candidate.updated_at >= current.updated_at
        || (candidate.claim_id == current.claim_id
            && candidate.decision_version >= current.decision_version)
}

fn claim_transition_differs(candidate: &WorkflowClaim, current: &WorkflowClaim) -> bool {
    candidate.claim_id != current.claim_id
        || candidate.decision_version != current.decision_version
        || candidate.state != current.state
        || candidate.failure_stage != current.failure_stage
}

pub(super) fn bounded_failure_message(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let bounded = chars
        .by_ref()
        .take(MAX_FAILURE_MESSAGE_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{bounded}...")
    } else {
        bounded
    }
}
