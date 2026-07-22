use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::goal::GoalIndexProjection;
use crate::model::workflow::GoalStatus;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BulkGoalFilter {
    pub status: Option<String>,
    pub q: Option<String>,
    pub reporter: Option<String>,
    pub assignee: Option<String>,
    pub feature: Option<String>,
    pub rounds_gte: Option<usize>,
    pub rounds_lte: Option<usize>,
    pub node: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BulkGoalSelection {
    #[serde(default)]
    pub filter: BulkGoalFilter,
    pub selected_ids: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkGoalUpdate {
    Priority(String),
    Status(String),
    Reporter(String),
    Assignee(String),
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BulkFeatureFilter {
    pub status: Option<String>,
    pub q: Option<String>,
    pub reporter: Option<String>,
    pub assignee: Option<String>,
    pub node: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BulkFeatureSelection {
    #[serde(default)]
    pub filter: BulkFeatureFilter,
    pub selected_ids: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkFeatureUpdate {
    Reporter(String),
    Assignee(String),
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DistributeMove {
    pub goal_id: String,
    pub from_node_id: String,
    pub to_node_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DistributeResult {
    pub strategy: String,
    pub node_ids: Vec<String>,
    pub eligible: usize,
    pub moved: usize,
    pub moves: Vec<DistributeMove>,
    pub skipped: usize,
    pub skipped_details: Vec<BulkSkippedDetail>,
    pub dry_run: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkflowEnforcementSummary {
    pub ok: bool,
    pub checked: usize,
    pub automated: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureGoalPlacement {
    #[default]
    Unordered,
    First,
    After(String),
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FeatureGoalAuthoringRequest {
    pub goal_id: Option<String>,
    pub name: Option<String>,
    pub prompt: String,
    pub reporter: String,
    pub assignee: Option<String>,
    pub priority: String,
    #[serde(default)]
    pub placement: FeatureGoalPlacement,
    #[serde(default)]
    pub duplicate_decision: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FeatureGoalAuthoringCapability {
    pub editable: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FeatureGoalDuplicate {
    pub id: String,
    pub name: String,
    pub status: GoalStatus,
    pub node_id: Option<String>,
    pub node_display_name: Option<String>,
    pub prompt: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FeatureGoalDuplicateMove {
    pub moved: bool,
    pub from: GoalStatus,
    pub to: GoalStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FeatureGoalAuthoringResult {
    pub created: bool,
    pub goal: Option<GoalIndexProjection>,
    pub duplicate_action: Option<String>,
    pub duplicate: Option<FeatureGoalDuplicate>,
    pub move_result: Option<FeatureGoalDuplicateMove>,
    pub requires_duplicate_decision: bool,
}
