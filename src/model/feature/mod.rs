use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::model::Timestamp;
use crate::model::gap::GapIndexProjection;
use crate::model::workflow::GapStatus;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Feature {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub reporter: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub node_id: Option<String>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub json_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureIndexProjection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub reporter: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub node_id: Option<String>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub json_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureDetail {
    pub feature: Feature,
    pub gaps: Vec<GapIndexProjection>,
    pub node_display_name: Option<String>,
    pub rollup: FeatureRollup,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureRollup {
    pub status: GapStatus,
    pub gap_count: usize,
    pub done_count: usize,
    pub active_count: usize,
    pub failed_count: usize,
    pub cancelled_count: usize,
    pub blocked_count: usize,
    pub next_gap: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureGapBlockingNotice {
    pub feature_id: String,
    pub blocking_gap_id: String,
    pub blocked_gap_ids: Vec<String>,
    pub blocked_count: usize,
    pub next_blocked_gap_id: Option<String>,
    pub message: String,
}

pub fn is_ordered_feature_gap(feature_order: Option<i64>) -> bool {
    feature_order.is_some()
}

pub fn compare_feature_gap_order(a_order: Option<i64>, b_order: Option<i64>) -> Ordering {
    match (a_order, b_order) {
        (Some(a_order), Some(b_order)) => a_order.cmp(&b_order),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

impl FeatureRollup {
    pub fn derive(gaps: &[GapIndexProjection]) -> Self {
        let gap_count = gaps.len();
        let done_count = gaps
            .iter()
            .filter(|gap| gap.status == GapStatus::Done)
            .count();
        let active_count = gaps
            .iter()
            .filter(|gap| {
                matches!(
                    gap.status,
                    GapStatus::InProgress
                        | GapStatus::Qa
                        | GapStatus::ReadyMerge
                        | GapStatus::Build
                        | GapStatus::Review
                )
            })
            .count();
        let failed_count = gaps
            .iter()
            .filter(|gap| gap.status == GapStatus::Failed)
            .count();
        let cancelled_count = gaps
            .iter()
            .filter(|gap| gap.status == GapStatus::Cancelled)
            .count();
        let blocked_count = gaps
            .iter()
            .filter(|gap| matches!(gap.status, GapStatus::Failed | GapStatus::Cancelled))
            .count();
        let next_gap = gaps
            .iter()
            .find(|gap| matches!(gap.status, GapStatus::Todo | GapStatus::Backlog))
            .map(|gap| gap.id.clone());

        let status = if gap_count > 0 && done_count == gap_count {
            GapStatus::Done
        } else if active_count > 0 {
            GapStatus::InProgress
        } else if failed_count > 0 {
            GapStatus::Failed
        } else if cancelled_count == gap_count && gap_count > 0 {
            GapStatus::Cancelled
        } else if gaps.iter().any(|gap| gap.status == GapStatus::Todo) {
            GapStatus::Todo
        } else {
            GapStatus::Backlog
        };

        Self {
            status,
            gap_count,
            done_count,
            active_count,
            failed_count,
            cancelled_count,
            blocked_count,
            next_gap,
        }
    }
}

pub fn failed_gap_feature_blocking_notice(
    gap: &GapIndexProjection,
    feature_gaps: &[GapIndexProjection],
) -> Option<FeatureGapBlockingNotice> {
    if gap.status != GapStatus::Failed {
        return None;
    }
    let feature_id = gap.feature_id.as_deref()?;
    let feature_order = gap.feature_order?;
    let node_id = gap.node_id.as_deref().unwrap_or("default");
    let mut blocked_gaps = feature_gaps
        .iter()
        .filter(|other| other.id != gap.id)
        .filter(|other| other.feature_id.as_deref() == Some(feature_id))
        .filter(|other| other.node_id.as_deref().unwrap_or("default") == node_id)
        .filter(|other| {
            other
                .feature_order
                .is_some_and(|order| order > feature_order)
        })
        .filter(|other| !matches!(other.status, GapStatus::Done | GapStatus::Cancelled))
        .cloned()
        .collect::<Vec<_>>();
    blocked_gaps.sort_by(|a, b| {
        compare_feature_gap_order(a.feature_order, b.feature_order).then_with(|| a.id.cmp(&b.id))
    });
    if blocked_gaps.is_empty() {
        return None;
    }
    let blocked_gap_ids = blocked_gaps
        .iter()
        .map(|blocked_gap| blocked_gap.id.clone())
        .collect::<Vec<_>>();
    let blocked_count = blocked_gap_ids.len();
    let next_blocked_gap_id = blocked_gap_ids.first().cloned();
    let plural = if blocked_count == 1 { "Gap" } else { "Gaps" };
    let message = match next_blocked_gap_id.as_deref() {
        Some(next) if blocked_count == 1 => format!(
            "This failed Gap is blocking the next Gap in Feature {feature_id} ({next}). Submit a recovery round so this Gap can finish, or cancel it to let later Feature work continue."
        ),
        Some(next) => format!(
            "This failed Gap is blocking {blocked_count} later {plural} in Feature {feature_id}; next blocked Gap: {next}. Submit a recovery round so this Gap can finish, or cancel it to let later Feature work continue."
        ),
        None => format!(
            "This failed Gap is blocking later Feature work in Feature {feature_id}. Submit a recovery round so this Gap can finish, or cancel it to let later Feature work continue."
        ),
    };
    Some(FeatureGapBlockingNotice {
        feature_id: feature_id.to_string(),
        blocking_gap_id: gap.id.clone(),
        blocked_gap_ids,
        blocked_count,
        next_blocked_gap_id,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::gap::GapPriority;

    #[test]
    fn failed_gap_feature_blocking_notice_names_later_feature_work() {
        let failed = gap("GAP1", GapStatus::Failed, Some(1));
        let blocked = gap("GAP2", GapStatus::Todo, Some(2));
        let done = gap("GAP3", GapStatus::Done, Some(3));

        let notice =
            failed_gap_feature_blocking_notice(&failed, &[blocked.clone(), done, failed.clone()])
                .expect("failed first Gap should block later unfinished Feature work");

        assert_eq!(notice.feature_id, "FEA1");
        assert_eq!(notice.blocking_gap_id, "GAP1");
        assert_eq!(notice.blocked_gap_ids, vec!["GAP2"]);
        assert_eq!(notice.next_blocked_gap_id.as_deref(), Some("GAP2"));
        assert!(
            notice
                .message
                .contains("This failed Gap is blocking the next Gap")
        );
    }

    #[test]
    fn failed_gap_feature_blocking_notice_ignores_unordered_or_terminal_work() {
        let failed = gap("GAP1", GapStatus::Failed, Some(1));
        let done = gap("GAP2", GapStatus::Done, Some(2));
        let unordered = gap("GAP3", GapStatus::Todo, None);
        let active = gap("GAP4", GapStatus::Todo, Some(0));

        assert!(failed_gap_feature_blocking_notice(&failed, &[done, unordered, active],).is_none());
        assert!(
            failed_gap_feature_blocking_notice(
                &gap("GAP1", GapStatus::Todo, Some(1)),
                &[gap("GAP2", GapStatus::Todo, Some(2))],
            )
            .is_none()
        );
    }

    fn gap(id: &str, status: GapStatus, feature_order: Option<i64>) -> GapIndexProjection {
        GapIndexProjection {
            id: id.to_string(),
            name: id.to_string(),
            status,
            priority: GapPriority::Medium,
            reporter: None,
            assignee: None,
            round_count: 1,
            created: "created".to_string(),
            updated: "updated".to_string(),
            branch_name: None,
            node_id: Some("default".to_string()),
            feature_id: Some("FEA1".to_string()),
            feature_order,
            json_path: format!("gaps/{id}/gap.json"),
        }
    }
}
