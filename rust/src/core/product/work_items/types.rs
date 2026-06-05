use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BulkGapFilter {
    pub status: Option<String>,
    pub q: Option<String>,
    pub reporter: Option<String>,
    pub feature: Option<String>,
    pub rounds_gte: Option<usize>,
    pub rounds_lte: Option<usize>,
    pub node: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BulkGapSelection {
    #[serde(default)]
    pub filter: BulkGapFilter,
    pub selected_ids: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkGapUpdate {
    Priority(String),
    Status(String),
    Reporter(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BulkSkippedDetail {
    pub id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BulkUpdateResult {
    pub updated: usize,
    pub ids: Vec<String>,
    pub field: String,
    pub value: String,
    pub skipped: usize,
    pub skipped_details: Vec<BulkSkippedDetail>,
    pub failed: usize,
    pub failures: Vec<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BulkDeleteResult {
    pub deleted: usize,
    pub ids: Vec<String>,
    pub failures: Vec<Value>,
    pub failed: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BulkAssignFeatureResult {
    pub feature_id: String,
    pub updated: usize,
    pub ids: Vec<String>,
    pub skipped: usize,
    pub skipped_details: Vec<BulkSkippedDetail>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BulkTransferNodeResult {
    pub target_node_id: String,
    pub updated: usize,
    pub ids: Vec<String>,
    pub skipped: usize,
    pub skipped_details: Vec<BulkSkippedDetail>,
}
