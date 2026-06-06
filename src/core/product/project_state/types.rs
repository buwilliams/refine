use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
use crate::model::gap::GapIndexProjection;
use crate::model::log::ActivityEntry;
use crate::model::workflow::GapStatus;
use crate::model::{JsonObject, Timestamp};

pub const PROJECTION_SNAPSHOT_VERSION: u64 = 1;
pub const PROJECTION_SNAPSHOT_FILE: &str = "projection-snapshot.json";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectionSnapshot {
    pub version: u64,
    pub generated_at: Timestamp,
    pub source_fingerprints: BTreeMap<String, SourceFingerprint>,
    pub gaps: BTreeMap<String, GapSummaryProjection>,
    pub features: BTreeMap<String, FeatureSummaryProjection>,
    pub activity: BTreeMap<String, ActivitySummaryProjection>,
    pub changes: BTreeMap<String, ChangeSummaryProjection>,
    pub dashboard: DashboardProjection,
    pub runtime: RuntimeProjection,
}

impl Default for ProjectionSnapshot {
    fn default() -> Self {
        Self {
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "detached".to_string(),
            source_fingerprints: BTreeMap::new(),
            gaps: BTreeMap::new(),
            features: BTreeMap::new(),
            activity: BTreeMap::new(),
            changes: BTreeMap::new(),
            dashboard: DashboardProjection::default(),
            runtime: RuntimeProjection::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceFingerprint {
    pub path: String,
    pub size: u64,
    pub modified_unix_ms: Option<i64>,
    pub content_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GapSummaryProjection {
    #[serde(flatten)]
    pub gap: GapIndexProjection,
    pub node_display_name: Option<String>,
    pub searchable_text: String,
    pub activity_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureSummaryProjection {
    #[serde(flatten)]
    pub feature: FeatureIndexProjection,
    pub status: GapStatus,
    pub gap_ids: Vec<String>,
    pub rollup: FeatureRollup,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActivitySummaryProjection {
    #[serde(flatten)]
    pub entry: ActivityEntry,
    pub searchable_text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChangeSummaryProjection {
    pub commit: String,
    pub committed_time: Timestamp,
    pub subject: String,
    pub gap_id: Option<String>,
    pub branch: Option<String>,
    pub gap_name: Option<String>,
    pub gap_status: Option<GapStatus>,
    pub gap_priority: Option<String>,
    pub searchable_text: String,
    #[serde(default)]
    pub order: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DashboardProjection {
    pub all_node_status_counts: BTreeMap<GapStatus, usize>,
    pub current_node_status_counts: BTreeMap<GapStatus, usize>,
    pub reporter_stats: BTreeMap<String, BTreeMap<GapStatus, usize>>,
    pub attention_indicators: Vec<String>,
    pub recent_activity_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RuntimeProjection {
    pub supervisor: Option<JsonObject>,
    pub processes: Vec<JsonObject>,
    pub background_jobs: Vec<JsonObject>,
    pub target_app: Option<JsonObject>,
    pub performance: Option<JsonObject>,
    pub preflight: Option<JsonObject>,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectionIndex {
    pub gaps_by_status: BTreeMap<GapStatus, BTreeSet<String>>,
    pub gaps_by_node: BTreeMap<String, BTreeSet<String>>,
    pub gaps_by_feature: BTreeMap<String, BTreeSet<String>>,
    pub standalone_gap_ids: BTreeSet<String>,
    pub features_by_status: BTreeMap<GapStatus, BTreeSet<String>>,
    pub activity_by_gap: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageRequest {
    pub limit: usize,
    pub offset: usize,
    pub sort: String,
    pub dir: String,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
            sort: "updated".to_string(),
            dir: "desc".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GapProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub status: Option<GapStatus>,
    pub reporter: Option<String>,
    pub node: Option<String>,
    pub current_node_id: Option<String>,
    pub feature: Option<String>,
    pub rounds_gte: Option<usize>,
    pub rounds_lte: Option<usize>,
    pub severity: Option<String>,
    pub category: Option<String>,
    pub actor: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FeatureProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub status: Option<GapStatus>,
    pub reporter: Option<String>,
    pub node: Option<String>,
    pub current_node_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActivityProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub gap_id: Option<String>,
    pub severity: Option<String>,
    pub category: Option<String>,
    pub actor: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangeProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub gap_id: Option<String>,
    pub status: Option<GapStatus>,
    pub priority: Option<String>,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GapProjectionList {
    pub gaps: Vec<GapIndexProjection>,
    pub total: usize,
    pub filtered_status_counts: BTreeMap<GapStatus, usize>,
    pub matching_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureProjectionList {
    pub features: Vec<FeatureSummaryProjection>,
    pub total: usize,
    pub matching_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ActivityProjectionFacets {
    pub categories: Vec<String>,
    pub severities: Vec<String>,
    pub actors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityProjectionList {
    pub activity: Vec<ActivityEntry>,
    pub total: usize,
    pub matching_ids: Vec<String>,
    pub facets: ActivityProjectionFacets,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChangeProjectionList {
    pub changes: Vec<ChangeSummaryProjection>,
    pub total: usize,
    pub matching_ids: Vec<String>,
}

impl ProjectionIndex {
    pub fn build(snapshot: &ProjectionSnapshot) -> Self {
        let mut index = Self::default();

        for (gap_id, projection) in &snapshot.gaps {
            index
                .gaps_by_status
                .entry(projection.gap.status.clone())
                .or_default()
                .insert(gap_id.clone());

            if let Some(node_id) = &projection.gap.node_id {
                index
                    .gaps_by_node
                    .entry(node_id.clone())
                    .or_default()
                    .insert(gap_id.clone());
            }

            if let Some(feature_id) = &projection.gap.feature_id {
                index
                    .gaps_by_feature
                    .entry(feature_id.clone())
                    .or_default()
                    .insert(gap_id.clone());
            } else {
                index.standalone_gap_ids.insert(gap_id.clone());
            }

            for activity_id in &projection.activity_ids {
                index
                    .activity_by_gap
                    .entry(gap_id.clone())
                    .or_default()
                    .insert(activity_id.clone());
            }
        }

        for (feature_id, projection) in &snapshot.features {
            index
                .features_by_status
                .entry(projection.status.clone())
                .or_default()
                .insert(feature_id.clone());
        }

        index
    }
}
