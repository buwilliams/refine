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
