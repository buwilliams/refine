use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
use crate::model::goal::GoalIndexProjection;
use crate::model::log::ActivityEntry;
use crate::model::workflow::GoalStatus;
use crate::model::{JsonObject, Timestamp};

pub const PROJECTION_SNAPSHOT_VERSION: u64 = 2;
pub const PROJECTION_SNAPSHOT_FILE: &str = "projection-snapshot.json";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectionSnapshot {
    pub version: u64,
    pub generated_at: Timestamp,
    pub source_fingerprints: BTreeMap<String, SourceFingerprint>,
    pub goals: BTreeMap<String, GoalSummaryProjection>,
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
            goals: BTreeMap::new(),
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
pub struct GoalSummaryProjection {
    #[serde(flatten)]
    pub goal: GoalIndexProjection,
    pub node_display_name: Option<String>,
    /// Trimmed prompt from the latest durable Round. Keeping this in the summary
    /// projection lets exact duplicate detection use one coherent snapshot
    /// without reopening every Goal and attaching its logs.
    #[serde(default)]
    pub latest_round_prompt: Option<String>,
    pub searchable_text: String,
    pub activity_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureSummaryProjection {
    #[serde(flatten)]
    pub feature: FeatureIndexProjection,
    pub status: GoalStatus,
    pub goal_ids: Vec<String>,
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
    pub goal_id: Option<String>,
    pub branch: Option<String>,
    pub goal_name: Option<String>,
    pub goal_status: Option<GoalStatus>,
    pub goal_priority: Option<String>,
    #[serde(default)]
    pub goal_assignee: Option<String>,
    pub searchable_text: String,
    #[serde(default)]
    pub order: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DashboardProjection {
    pub all_node_status_counts: BTreeMap<GoalStatus, usize>,
    pub current_node_status_counts: BTreeMap<GoalStatus, usize>,
    pub reporter_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>>,
    #[serde(default)]
    pub assignee_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>>,
    pub attention_indicators: Vec<String>,
    pub recent_activity_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DashboardProjectionQuery {
    pub node: Option<String>,
    pub current_node_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DashboardProjectionSummary {
    pub node_filter: String,
    pub current_node_id: String,
    pub counts: BTreeMap<GoalStatus, usize>,
    pub all_node_counts: BTreeMap<GoalStatus, usize>,
    pub reporter_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>>,
    pub assignee_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>>,
    pub attention_indicators: Vec<String>,
    pub recent_activity_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RuntimeProjection {
    pub supervisor: Option<JsonObject>,
    pub processes: Vec<JsonObject>,
    pub background_operations: Vec<JsonObject>,
    pub target_app: Option<JsonObject>,
    pub performance: Option<JsonObject>,
    pub preflight: Option<JsonObject>,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectionIndex {
    pub goals_by_status: BTreeMap<GoalStatus, BTreeSet<String>>,
    pub goals_by_node: BTreeMap<String, BTreeSet<String>>,
    pub goals_by_feature: BTreeMap<String, BTreeSet<String>>,
    pub standalone_goal_ids: BTreeSet<String>,
    pub features_by_status: BTreeMap<GoalStatus, BTreeSet<String>>,
    pub activity_by_goal: BTreeMap<String, BTreeSet<String>>,
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
pub struct GoalProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub status: Option<GoalStatus>,
    pub reporter: Option<String>,
    pub assignee: Option<String>,
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
    pub status: Option<GoalStatus>,
    pub reporter: Option<String>,
    pub assignee: Option<String>,
    pub node: Option<String>,
    pub current_node_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActivityProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub goal_id: Option<String>,
    pub severity: Option<String>,
    pub category: Option<String>,
    pub actor: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangeProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub goal_id: Option<String>,
    pub status: Option<GoalStatus>,
    pub priority: Option<String>,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GoalProjectionList {
    pub goals: Vec<GoalIndexProjection>,
    pub total: usize,
    pub filtered_status_counts: BTreeMap<GoalStatus, usize>,
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

        for (goal_id, projection) in &snapshot.goals {
            index
                .goals_by_status
                .entry(projection.goal.status.clone())
                .or_default()
                .insert(goal_id.clone());

            if let Some(node_id) = &projection.goal.node_id {
                index
                    .goals_by_node
                    .entry(node_id.clone())
                    .or_default()
                    .insert(goal_id.clone());
            }

            if let Some(feature_id) = &projection.goal.feature_id {
                index
                    .goals_by_feature
                    .entry(feature_id.clone())
                    .or_default()
                    .insert(goal_id.clone());
            } else {
                index.standalone_goal_ids.insert(goal_id.clone());
            }

            for activity_id in &projection.activity_ids {
                index
                    .activity_by_goal
                    .entry(goal_id.clone())
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
