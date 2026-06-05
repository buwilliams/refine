use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::core::product::project_state::{
    FeatureSummaryProjection, FileProjectStateStore, GapSummaryProjection, ProjectStateStore,
};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::feature::{Feature, FeatureDetail};
use crate::model::gap::{Gap, GapPriority};
use crate::model::workflow::{
    FeatureOperation, GapOperation, GapStatus, feature_operation_allowed, gap_operation_allowed,
    is_bulk_target_allowed, is_feature_cancel_status, is_feature_protected_status,
    user_status_transition,
};

pub trait WorkItemService {
    fn create_gap(&self, gap: Gap) -> RefineResult<Gap>;
    fn list_gaps(&self) -> RefineResult<Vec<Gap>>;
    fn update_gap(&self, gap: Gap) -> RefineResult<Gap>;
    fn transition_gap(&self, gap_id: &str, target: GapStatus) -> RefineResult<Gap>;
    fn cancel_gap(&self, gap_id: &str) -> RefineResult<Gap>;
    fn delete_gap(&self, gap_id: &str) -> RefineResult<()>;
    fn create_feature(&self, feature: Feature) -> RefineResult<Feature>;
    fn feature_detail(&self, feature_id: &str) -> RefineResult<FeatureDetail>;
    fn assign_gap(&self, gap_id: &str, feature_id: &str, order: i64) -> RefineResult<Gap>;
    fn reorder_gap(&self, gap_id: &str, order: i64) -> RefineResult<Gap>;
}

pub fn validate_manual_gap_transition(from: &GapStatus, to: &GapStatus) -> RefineResult<()> {
    let decision = user_status_transition(from, to);
    if decision.allowed {
        Ok(())
    } else {
        Err(crate::core::supervisor::errors::RefineError::InvalidInput(
            decision
                .reason
                .unwrap_or_else(|| "transition is not allowed".to_string()),
        ))
    }
}

#[derive(Clone, Debug)]
pub struct FileWorkItemService {
    pub durable_root: PathBuf,
}

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

impl FileWorkItemService {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
        }
    }

    pub fn create_gap_summary(
        &self,
        name: &str,
        id: Option<&str>,
    ) -> RefineResult<GapSummaryProjection> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RefineError::InvalidInput(
                "Gap name is required".to_string(),
            ));
        }
        let gap_id = id
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_ulid_like);
        if gap_id.len() < 3 {
            return Err(RefineError::InvalidInput(
                "Gap id must be at least three characters".to_string(),
            ));
        }

        let gap_path = gap_json_path(&self.durable_root, &gap_id);
        if gap_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Gap {gap_id} already exists"
            )));
        }
        let now = now_timestamp();
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(gap_id.clone()));
        object.insert("name".to_string(), Value::String(name.to_string()));
        object.insert("status".to_string(), Value::String("backlog".to_string()));
        object.insert("priority".to_string(), Value::String("low".to_string()));
        object.insert("branch_name".to_string(), Value::Null);
        object.insert("feature_id".to_string(), Value::Null);
        object.insert("feature_order".to_string(), Value::Null);
        object.insert("node_id".to_string(), Value::String("default".to_string()));
        object.insert("created".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        object.insert("notes".to_string(), Value::Array(Vec::new()));
        object.insert("rounds".to_string(), Value::Array(Vec::new()));
        write_json_atomically(&gap_path, &Value::Object(object))?;
        self.show_gap_summary(&gap_id)
    }

    pub fn show_gap_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let store = FileProjectStateStore::new(&self.durable_root);
        let snapshot = store.rebuild_projection()?;
        snapshot.gaps.get(gap_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Gap {gap_id} was not found in durable state"))
        })
    }

    pub fn list_gap_summaries(&self) -> RefineResult<Vec<GapSummaryProjection>> {
        let store = FileProjectStateStore::new(&self.durable_root);
        let snapshot = store.rebuild_projection()?;
        Ok(snapshot.gaps.into_values().collect())
    }

    pub fn create_feature_summary(
        &self,
        name: &str,
        id: Option<&str>,
        description: Option<&str>,
        reporter: Option<&str>,
    ) -> RefineResult<FeatureSummaryProjection> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RefineError::InvalidInput(
                "Feature name is required".to_string(),
            ));
        }
        let feature_id = id
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_ulid_like);
        if feature_id.len() < 3 {
            return Err(RefineError::InvalidInput(
                "Feature id must be at least three characters".to_string(),
            ));
        }

        let feature_path = feature_json_path(&self.durable_root, &feature_id);
        if feature_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Feature {feature_id} already exists"
            )));
        }
        let now = now_timestamp();
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(feature_id.clone()));
        object.insert("name".to_string(), Value::String(name.to_string()));
        object.insert(
            "description".to_string(),
            Value::String(description.unwrap_or("").trim().to_string()),
        );
        object.insert(
            "reporter".to_string(),
            Value::String(reporter.unwrap_or("").trim().to_string()),
        );
        object.insert("node_id".to_string(), Value::String("default".to_string()));
        object.insert("created".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&feature_path, &Value::Object(object))?;
        self.show_feature_summary(&feature_id)
    }

    pub fn show_feature_summary(&self, feature_id: &str) -> RefineResult<FeatureSummaryProjection> {
        let store = FileProjectStateStore::new(&self.durable_root);
        let snapshot = store.rebuild_projection()?;
        snapshot.features.get(feature_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!(
                "Feature {feature_id} was not found in durable state"
            ))
        })
    }

    pub fn update_feature_metadata_summary(
        &self,
        feature_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        reporter: Option<&str>,
    ) -> RefineResult<FeatureSummaryProjection> {
        self.show_feature_summary(feature_id)?;
        let feature_path = feature_json_path(&self.durable_root, feature_id);
        let bytes = fs::read(&feature_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Feature {}: {error}",
                feature_path.display()
            ))
        })?;
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Feature {}: {error}",
                feature_path.display()
            ))
        })?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "Feature {} is not a JSON object",
                feature_path.display()
            ))
        })?;
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                return Err(RefineError::InvalidInput(
                    "Feature name cannot be empty".to_string(),
                ));
            }
            object.insert("name".to_string(), Value::String(name.to_string()));
        }
        if let Some(description) = description {
            object.insert(
                "description".to_string(),
                Value::String(description.trim().to_string()),
            );
        }
        if let Some(reporter) = reporter {
            object.insert(
                "reporter".to_string(),
                Value::String(reporter.trim().to_string()),
            );
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&feature_path, &value)?;
        self.show_feature_summary(feature_id)
    }

    pub fn list_feature_summaries(&self) -> RefineResult<Vec<FeatureSummaryProjection>> {
        let store = FileProjectStateStore::new(&self.durable_root);
        let snapshot = store.rebuild_projection()?;
        Ok(snapshot.features.into_values().collect())
    }

    pub fn assign_gap_to_feature(
        &self,
        feature_id: &str,
        gap_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        self.show_feature_summary(feature_id)?;
        let current_gap = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current_gap.gap.status, &GapOperation::AssignToFeature)?;
        let next_order = self.next_feature_order(feature_id)?;
        self.set_gap_feature_membership(gap_id, Some(feature_id), Some(next_order))?;
        self.show_feature_summary(feature_id)
    }

    pub fn remove_gap_from_feature(
        &self,
        feature_id: &str,
        gap_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        self.show_feature_summary(feature_id)?;
        let current_gap = self.show_gap_summary(gap_id)?;
        if current_gap.gap.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Gap {gap_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_gap_operation(&current_gap.gap.status, &GapOperation::RemoveFromFeature)?;
        self.set_gap_feature_membership(gap_id, None, None)?;
        self.compact_feature_orders(feature_id)?;
        self.show_feature_summary(feature_id)
    }

    pub fn reorder_gap_in_feature(
        &self,
        feature_id: &str,
        gap_id: &str,
        order: i64,
    ) -> RefineResult<FeatureSummaryProjection> {
        if order < 1 {
            return Err(RefineError::InvalidInput(
                "feature order must be at least 1".to_string(),
            ));
        }
        self.show_feature_summary(feature_id)?;
        let current_gap = self.show_gap_summary(gap_id)?;
        if current_gap.gap.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Gap {gap_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_gap_operation(&current_gap.gap.status, &GapOperation::ReorderInFeature)?;
        let mut gaps: Vec<_> = self
            .list_gap_summaries()?
            .into_iter()
            .filter(|gap| gap.gap.feature_id.as_deref() == Some(feature_id))
            .collect();
        gaps.sort_by(|a, b| {
            a.gap
                .feature_order
                .unwrap_or(i64::MAX)
                .cmp(&b.gap.feature_order.unwrap_or(i64::MAX))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });
        let Some(current_index) = gaps.iter().position(|gap| gap.gap.id == gap_id) else {
            return Err(RefineError::NotFound(format!(
                "Gap {gap_id} was not found in Feature {feature_id}"
            )));
        };
        let gap = gaps.remove(current_index);
        let insert_index = usize::min(order as usize - 1, gaps.len());
        gaps.insert(insert_index, gap);
        for (idx, gap) in gaps.iter().enumerate() {
            self.set_gap_feature_membership(&gap.gap.id, Some(feature_id), Some(idx as i64 + 1))?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn move_feature_workflow(
        &self,
        feature_id: &str,
        target: GapStatus,
    ) -> RefineResult<FeatureSummaryProjection> {
        if !matches!(target, GapStatus::Backlog | GapStatus::Todo) {
            return Err(RefineError::InvalidInput(
                "Feature workflow target must be backlog or todo".to_string(),
            ));
        }
        self.show_feature_summary(feature_id)?;
        let mut gaps: Vec<_> = self
            .list_gap_summaries()?
            .into_iter()
            .filter(|gap| gap.gap.feature_id.as_deref() == Some(feature_id))
            .collect();
        gaps.sort_by(|a, b| {
            a.gap
                .feature_order
                .unwrap_or(i64::MAX)
                .cmp(&b.gap.feature_order.unwrap_or(i64::MAX))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });
        for gap in gaps {
            if is_feature_protected_status(&gap.gap.status) {
                continue;
            }
            self.set_gap_status_unchecked(&gap.gap.id, &target)?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn cancel_feature_summary(
        &self,
        feature_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        self.show_feature_summary(feature_id)?;
        let gaps = self.feature_gap_summaries(feature_id)?;
        validate_feature_operation(
            &gaps
                .iter()
                .map(|gap| gap.gap.status.clone())
                .collect::<Vec<_>>(),
            &FeatureOperation::CancelFeature,
        )?;
        for gap in gaps {
            if is_feature_cancel_status(&gap.gap.status) {
                self.cancel_gap_summary(&gap.gap.id)?;
            }
        }
        self.show_feature_summary(feature_id)
    }

    pub fn delete_feature_record(&self, feature_id: &str) -> RefineResult<()> {
        self.show_feature_summary(feature_id)?;
        let gaps = self.feature_gap_summaries(feature_id)?;
        validate_feature_operation(
            &gaps
                .iter()
                .map(|gap| gap.gap.status.clone())
                .collect::<Vec<_>>(),
            &FeatureOperation::DeleteFeature,
        )?;
        for gap in gaps {
            self.delete_gap_record(&gap.gap.id)?;
        }
        let feature_path = feature_json_path(&self.durable_root, feature_id);
        fs::remove_file(&feature_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to delete Feature {}: {error}",
                feature_path.display()
            ))
        })?;
        if let Some(parent) = feature_path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }

    pub fn bulk_update_gaps(
        &self,
        selection: BulkGapSelection,
        update: BulkGapUpdate,
    ) -> RefineResult<BulkUpdateResult> {
        let (field, raw_value) = match update {
            BulkGapUpdate::Priority(value) => ("priority".to_string(), value.trim().to_string()),
            BulkGapUpdate::Status(value) => ("status".to_string(), value.trim().to_lowercase()),
            BulkGapUpdate::Reporter(value) => ("reporter".to_string(), value.trim().to_string()),
        };
        if field == "priority" && GapPriority::parse_wire(&raw_value).is_none() {
            return Err(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }
        if field == "status" && raw_value != "__last_workflow_state" {
            let Some(status) = GapStatus::parse_wire(&raw_value) else {
                return Err(RefineError::InvalidInput("invalid status".to_string()));
            };
            if !is_bulk_target_allowed(&status) {
                return Err(RefineError::Conflict(
                    "Bulk status updates cannot set in-progress, qa, or ready-merge".to_string(),
                ));
            }
        }
        if field == "reporter" && !valid_reporter_name(&raw_value) {
            return Err(RefineError::InvalidInput(
                "invalid reporter name".to_string(),
            ));
        }

        let skip_automated = field == "status" && raw_value != "__last_workflow_state";
        let (gaps, skipped_details) = self.select_bulk_gap_summaries(&selection, skip_automated)?;
        let mut ids = Vec::new();
        for gap in gaps {
            match field.as_str() {
                "priority" => self.set_gap_priority_unchecked(&gap.gap.id, &raw_value)?,
                "status" if raw_value == "__last_workflow_state" => {
                    let restored = restore_last_workflow_status(&gap.gap.status);
                    if restored != gap.gap.status {
                        self.set_gap_status_unchecked(&gap.gap.id, &restored)?;
                    }
                }
                "status" => {
                    let status = GapStatus::parse_wire(&raw_value)
                        .ok_or_else(|| RefineError::InvalidInput("invalid status".to_string()))?;
                    self.set_gap_status_unchecked(&gap.gap.id, &status)?;
                }
                "reporter" => self.set_latest_round_reporter(&gap.gap.id, &raw_value)?,
                _ => unreachable!(),
            }
            ids.push(gap.gap.id);
        }
        Ok(BulkUpdateResult {
            updated: ids.len(),
            ids,
            field,
            value: raw_value,
            skipped: skipped_details.len(),
            skipped_details,
            failed: 0,
            failures: Vec::new(),
        })
    }

    pub fn bulk_delete_gaps(&self, selection: BulkGapSelection) -> RefineResult<BulkDeleteResult> {
        let (gaps, _) = self.select_bulk_gap_summaries(&selection, false)?;
        let mut ids = Vec::new();
        let mut feature_ids = BTreeSet::new();
        for gap in gaps {
            if let Some(feature_id) = &gap.gap.feature_id {
                feature_ids.insert(feature_id.clone());
            }
            self.delete_gap_record(&gap.gap.id)?;
            ids.push(gap.gap.id);
        }
        for feature_id in feature_ids {
            let _ = self.compact_feature_orders(&feature_id);
        }
        Ok(BulkDeleteResult {
            deleted: ids.len(),
            ids,
            failures: Vec::new(),
            failed: 0,
        })
    }

    pub fn bulk_assign_gaps_to_feature(
        &self,
        feature_id: &str,
        selection: BulkGapSelection,
    ) -> RefineResult<BulkAssignFeatureResult> {
        self.show_feature_summary(feature_id)?;
        let (gaps, mut skipped_details) = self.select_bulk_gap_summaries(&selection, false)?;
        let mut next_order = self.next_feature_order(feature_id)?;
        let mut old_feature_ids = BTreeSet::new();
        let mut ids = Vec::new();
        for gap in gaps {
            if gap.gap.feature_id.as_deref() == Some(feature_id) {
                skipped_details.push(BulkSkippedDetail {
                    id: gap.gap.id,
                    reason: "already-assigned".to_string(),
                });
                continue;
            }
            validate_gap_operation(&gap.gap.status, &GapOperation::AssignToFeature)?;
            if let Some(old_feature_id) = &gap.gap.feature_id {
                old_feature_ids.insert(old_feature_id.clone());
            }
            self.set_gap_feature_membership(&gap.gap.id, Some(feature_id), Some(next_order))?;
            next_order += 1;
            ids.push(gap.gap.id);
        }
        for old_feature_id in old_feature_ids {
            let _ = self.compact_feature_orders(&old_feature_id);
        }
        Ok(BulkAssignFeatureResult {
            feature_id: feature_id.to_string(),
            updated: ids.len(),
            ids,
            skipped: skipped_details.len(),
            skipped_details,
        })
    }

    pub fn bulk_transfer_gaps_to_node(
        &self,
        target_node_id: &str,
        selection: BulkGapSelection,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = target_node_id.trim();
        if target_node_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "target_node_id is required".to_string(),
            ));
        }
        let (gaps, mut skipped_details) = self.select_bulk_gap_summaries(&selection, false)?;
        let mut ids = Vec::new();
        for gap in gaps {
            if matches!(
                gap.gap.status,
                GapStatus::InProgress
                    | GapStatus::Qa
                    | GapStatus::ReadyMerge
                    | GapStatus::AwaitingRebuild
            ) {
                skipped_details.push(BulkSkippedDetail {
                    id: gap.gap.id,
                    reason: format!("status:{}", gap.gap.status.as_str()),
                });
                continue;
            }
            self.set_gap_node_unchecked(&gap.gap.id, target_node_id)?;
            ids.push(gap.gap.id);
        }
        Ok(BulkTransferNodeResult {
            target_node_id: target_node_id.to_string(),
            updated: ids.len(),
            ids,
            skipped: skipped_details.len(),
            skipped_details,
        })
    }

    pub fn verify_gap_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::VerifyReview)?;
        self.set_gap_status_unchecked(gap_id, &GapStatus::Done)?;
        self.show_gap_summary(gap_id)
    }

    pub fn retry_gap_quality_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::RetryQa)?;
        self.set_gap_status_unchecked(gap_id, &GapStatus::Qa)?;
        self.show_gap_summary(gap_id)
    }

    pub fn retry_gap_merge_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::RetryMerge)?;
        self.set_gap_status_unchecked(gap_id, &GapStatus::ReadyMerge)?;
        self.show_gap_summary(gap_id)
    }

    pub fn merge_gap_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::Merge)?;
        self.set_gap_status_unchecked(gap_id, &GapStatus::Done)?;
        self.show_gap_summary(gap_id)
    }

    pub fn undo_gap_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::Undo)?;
        let target = match current.gap.status {
            GapStatus::Done => GapStatus::Review,
            GapStatus::Review | GapStatus::Cancelled => GapStatus::Todo,
            _ => {
                return Err(RefineError::InvalidInput(
                    "Gap undo is only available from done, review, or cancelled".to_string(),
                ));
            }
        };
        self.set_gap_status_unchecked(gap_id, &target)?;
        self.show_gap_summary(gap_id)
    }

    pub fn start_gap_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::StartImplementation)?;
        self.set_gap_status_unchecked(gap_id, &GapStatus::InProgress)?;
        self.show_gap_summary(gap_id)
    }

    pub fn transition_gap_status(
        &self,
        gap_id: &str,
        target: GapStatus,
    ) -> RefineResult<GapSummaryProjection> {
        let store = FileProjectStateStore::new(&self.durable_root);
        let snapshot = store.rebuild_projection()?;
        let current = snapshot.gaps.get(gap_id).ok_or_else(|| {
            RefineError::NotFound(format!("Gap {gap_id} was not found in durable state"))
        })?;
        validate_manual_gap_transition(&current.gap.status, &target)?;

        let gap_path = self.durable_root.join(&current.gap.json_path);
        let bytes = fs::read(&gap_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Gap {}: {error}",
                gap_path.display()
            ))
        })?;
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Gap {}: {error}",
                gap_path.display()
            ))
        })?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert(
            "status".to_string(),
            Value::String(target.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));

        write_json_atomically(&gap_path, &value)?;

        let refreshed = store.rebuild_projection()?;
        refreshed.gaps.get(gap_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Gap {gap_id} disappeared after transition"))
        })
    }

    pub fn cancel_gap_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        if current.gap.status == GapStatus::Cancelled {
            return Ok(current);
        }
        if current.gap.status == GapStatus::Done {
            return Err(RefineError::InvalidInput(
                "done Gaps cannot be cancelled".to_string(),
            ));
        }
        self.set_gap_status_unchecked(gap_id, &GapStatus::Cancelled)?;
        self.show_gap_summary(gap_id)
    }

    pub fn update_gap_metadata_summary(
        &self,
        gap_id: &str,
        name: Option<&str>,
        priority: Option<&str>,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::EditMetadata)?;

        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                return Err(RefineError::InvalidInput(
                    "Gap name cannot be empty".to_string(),
                ));
            }
            object.insert("name".to_string(), Value::String(name.to_string()));
        }
        if let Some(priority) = priority {
            let Some(priority) = GapPriority::parse_wire(priority) else {
                return Err(RefineError::InvalidInput(
                    "priority must be one of low, medium, or high".to_string(),
                ));
            };
            object.insert(
                "priority".to_string(),
                Value::String(priority.as_str().to_string()),
            );
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn add_gap_note_summary(
        &self,
        gap_id: &str,
        author: &str,
        body: &str,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::EditNotes)?;
        let body = body.trim();
        if body.is_empty() {
            return Err(RefineError::InvalidInput(
                "note body cannot be empty".to_string(),
            ));
        }

        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        let now = now_timestamp();
        let mut note = Map::new();
        note.insert("id".to_string(), Value::String(new_ulid_like()));
        note.insert(
            "author".to_string(),
            Value::String(author.trim().to_string()),
        );
        note.insert("body".to_string(), Value::String(body.to_string()));
        note.insert("created".to_string(), Value::String(now.clone()));
        note.insert("updated".to_string(), Value::String(now.clone()));
        match object.get_mut("notes") {
            Some(Value::Array(notes)) => notes.push(Value::Object(note)),
            _ => {
                object.insert("notes".to_string(), Value::Array(vec![Value::Object(note)]));
            }
        }
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn append_gap_round_summary(
        &self,
        gap_id: &str,
        reporter: &str,
        actual: &str,
        target: &str,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::SubmitNewRound)?;
        let reporter = reporter.trim();
        let actual = actual.trim();
        let target = target.trim();
        if reporter.is_empty() || actual.is_empty() || target.is_empty() {
            return Err(RefineError::InvalidInput(
                "round reporter, actual, and target are required".to_string(),
            ));
        }

        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        let round = new_round_value(reporter, actual, target);
        match object.get_mut("rounds") {
            Some(Value::Array(rounds)) => rounds.push(round),
            _ => {
                object.insert("rounds".to_string(), Value::Array(vec![round]));
            }
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn edit_latest_gap_round_summary(
        &self,
        gap_id: &str,
        reporter: Option<&str>,
        actual: Option<&str>,
        target: Option<&str>,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::EditLatestRound)?;

        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Gap {gap_id} has no rounds")))?;
        let latest = rounds
            .iter_mut()
            .rev()
            .find(|round| round.is_object())
            .ok_or_else(|| RefineError::NotFound(format!("Gap {gap_id} has no rounds")))?;
        let latest = latest.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "latest round for Gap {gap_id} is not a JSON object"
            ))
        })?;
        if let Some(reporter) = reporter {
            latest.insert(
                "reporter".to_string(),
                Value::String(reporter.trim().to_string()),
            );
        }
        if let Some(actual) = actual {
            latest.insert(
                "actual".to_string(),
                Value::String(actual.trim().to_string()),
            );
        }
        if let Some(target) = target {
            latest.insert(
                "target".to_string(),
                Value::String(target.trim().to_string()),
            );
        }
        let now = now_timestamp();
        latest.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn delete_gap_record(&self, gap_id: &str) -> RefineResult<()> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::Delete)?;
        let gap_path = self.durable_root.join(&current.gap.json_path);
        fs::remove_file(&gap_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to delete Gap {}: {error}",
                gap_path.display()
            ))
        })?;
        if let Some(parent) = gap_path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }

    fn read_gap_value(&self, gap_id: &str) -> RefineResult<(PathBuf, Value)> {
        let current = self.show_gap_summary(gap_id)?;
        let gap_path = self.durable_root.join(&current.gap.json_path);
        let bytes = fs::read(&gap_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Gap {}: {error}",
                gap_path.display()
            ))
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Gap {}: {error}",
                gap_path.display()
            ))
        })?;
        Ok((gap_path, value))
    }

    fn set_gap_feature_membership(
        &self,
        gap_id: &str,
        feature_id: Option<&str>,
        feature_order: Option<i64>,
    ) -> RefineResult<()> {
        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert(
            "feature_id".to_string(),
            feature_id
                .map(|id| Value::String(id.to_string()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "feature_order".to_string(),
            feature_order
                .map(|order| Value::Number(order.into()))
                .unwrap_or(Value::Null),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)
    }

    fn set_gap_status_unchecked(&self, gap_id: &str, status: &GapStatus) -> RefineResult<()> {
        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert(
            "status".to_string(),
            Value::String(status.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)
    }

    fn set_gap_priority_unchecked(&self, gap_id: &str, priority: &str) -> RefineResult<()> {
        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert("priority".to_string(), Value::String(priority.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)
    }

    fn set_gap_node_unchecked(&self, gap_id: &str, node_id: &str) -> RefineResult<()> {
        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert("node_id".to_string(), Value::String(node_id.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)
    }

    fn set_latest_round_reporter(&self, gap_id: &str, reporter: &str) -> RefineResult<()> {
        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Gap {gap_id} has no rounds")))?;
        let latest = rounds
            .iter_mut()
            .rev()
            .find(|round| round.is_object())
            .ok_or_else(|| RefineError::NotFound(format!("Gap {gap_id} has no rounds")))?;
        let latest = latest.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "latest round for Gap {gap_id} is not a JSON object"
            ))
        })?;
        let now = now_timestamp();
        latest.insert("reporter".to_string(), Value::String(reporter.to_string()));
        latest.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&gap_path, &value)
    }

    fn next_feature_order(&self, feature_id: &str) -> RefineResult<i64> {
        let max_order = self
            .list_gap_summaries()?
            .into_iter()
            .filter(|gap| gap.gap.feature_id.as_deref() == Some(feature_id))
            .filter_map(|gap| gap.gap.feature_order)
            .max()
            .unwrap_or(0);
        Ok(max_order + 1)
    }

    fn compact_feature_orders(&self, feature_id: &str) -> RefineResult<()> {
        let mut gaps: Vec<_> = self
            .list_gap_summaries()?
            .into_iter()
            .filter(|gap| gap.gap.feature_id.as_deref() == Some(feature_id))
            .collect();
        gaps.sort_by(|a, b| {
            a.gap
                .feature_order
                .unwrap_or(i64::MAX)
                .cmp(&b.gap.feature_order.unwrap_or(i64::MAX))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });
        for (idx, gap) in gaps.iter().enumerate() {
            self.set_gap_feature_membership(&gap.gap.id, Some(feature_id), Some(idx as i64 + 1))?;
        }
        Ok(())
    }

    fn feature_gap_summaries(&self, feature_id: &str) -> RefineResult<Vec<GapSummaryProjection>> {
        let mut gaps: Vec<_> = self
            .list_gap_summaries()?
            .into_iter()
            .filter(|gap| gap.gap.feature_id.as_deref() == Some(feature_id))
            .collect();
        gaps.sort_by(|a, b| {
            a.gap
                .feature_order
                .unwrap_or(i64::MAX)
                .cmp(&b.gap.feature_order.unwrap_or(i64::MAX))
                .then_with(|| a.gap.id.cmp(&b.gap.id))
        });
        Ok(gaps)
    }

    fn select_bulk_gap_summaries(
        &self,
        selection: &BulkGapSelection,
        skip_automated: bool,
    ) -> RefineResult<(Vec<GapSummaryProjection>, Vec<BulkSkippedDetail>)> {
        let excluded: BTreeSet<_> = selection
            .exclude_ids
            .iter()
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .collect();
        let mut gaps = if let Some(selected_ids) = &selection.selected_ids {
            let mut selected = Vec::new();
            for id in selected_ids {
                let id = id.trim().to_uppercase();
                if id.is_empty() || excluded.contains(&id) {
                    continue;
                }
                selected.push(self.show_gap_summary(&id)?);
            }
            selected
        } else {
            self.list_gap_summaries()?
                .into_iter()
                .filter(|gap| !excluded.contains(&gap.gap.id))
                .filter(|gap| bulk_gap_matches_filter(gap, &selection.filter))
                .collect()
        };
        gaps.sort_by(|a, b| a.gap.id.cmp(&b.gap.id));
        let mut skipped_details = Vec::new();
        if skip_automated {
            let mut retained = Vec::new();
            for gap in gaps {
                if matches!(
                    gap.gap.status,
                    GapStatus::InProgress | GapStatus::Qa | GapStatus::ReadyMerge
                ) {
                    skipped_details.push(BulkSkippedDetail {
                        id: gap.gap.id,
                        reason: format!("status:{}", gap.gap.status.as_str()),
                    });
                } else {
                    retained.push(gap);
                }
            }
            gaps = retained;
        }
        Ok((gaps, skipped_details))
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn bulk_gap_matches_filter(gap: &GapSummaryProjection, filter: &BulkGapFilter) -> bool {
    if let Some(status) = filter
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if gap.gap.status.as_str() != status {
            return false;
        }
    }
    if let Some(reporter) = filter
        .reporter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if gap.gap.reporter.as_deref() != Some(reporter) {
            return false;
        }
    }
    if let Some(feature) = filter
        .feature
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if feature == "standalone" {
            if gap.gap.feature_id.is_some() {
                return false;
            }
        } else if feature != "all" && gap.gap.feature_id.as_deref() != Some(feature) {
            return false;
        }
    }
    if let Some(min_rounds) = filter.rounds_gte {
        if gap.gap.round_count < min_rounds {
            return false;
        }
    }
    if let Some(max_rounds) = filter.rounds_lte {
        if gap.gap.round_count > max_rounds {
            return false;
        }
    }
    if let Some(node) = filter
        .node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if node != "all"
            && node != "current"
            && gap.gap.node_id.as_deref().unwrap_or("default") != node
        {
            return false;
        }
    }
    if let Some(query) = filter.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let query = query.to_lowercase();
        let haystack = gap.searchable_text.to_lowercase();
        let reporter = gap.gap.reporter.as_deref().unwrap_or("").to_lowercase();
        if !haystack.contains(&query)
            && !gap.gap.id.to_lowercase().contains(&query)
            && !reporter.contains(&query)
        {
            return false;
        }
    }
    true
}

fn valid_reporter_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 80 && !value.chars().any(|ch| ch.is_control())
}

fn restore_last_workflow_status(status: &GapStatus) -> GapStatus {
    match status {
        GapStatus::Failed | GapStatus::Review | GapStatus::Cancelled => GapStatus::Todo,
        other => other.clone(),
    }
}

fn new_round_value(reporter: &str, actual: &str, target: &str) -> Value {
    let now = now_timestamp();
    let mut round = Map::new();
    round.insert("reporter".to_string(), Value::String(reporter.to_string()));
    round.insert("actual".to_string(), Value::String(actual.to_string()));
    round.insert("target".to_string(), Value::String(target.to_string()));
    round.insert("created".to_string(), Value::String(now.clone()));
    round.insert("updated".to_string(), Value::String(now));
    round.insert("logs".to_string(), Value::Array(Vec::new()));
    round.insert(
        "rule_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "meta_rule_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "product_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "constitution_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "governance_message".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_details".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_checked_at".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_rule_actions".to_string(),
        Value::Array(Vec::new()),
    );
    round.insert(
        "quality_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert("quality_message".to_string(), Value::String(String::new()));
    round.insert("quality_details".to_string(), Value::String(String::new()));
    round.insert(
        "quality_checked_at".to_string(),
        Value::String(String::new()),
    );
    Value::Object(round)
}

fn validate_gap_operation(status: &GapStatus, operation: &GapOperation) -> RefineResult<()> {
    let decision = gap_operation_allowed(status, operation);
    if decision.allowed {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(
            decision
                .reason
                .unwrap_or_else(|| "operation is not allowed".to_string()),
        ))
    }
}

fn validate_feature_operation(
    statuses: &[GapStatus],
    operation: &FeatureOperation,
) -> RefineResult<()> {
    let decision = feature_operation_allowed(statuses, operation);
    if decision.allowed {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(decision.reason.unwrap_or_else(
            || "feature operation is not allowed".to_string(),
        )))
    }
}

fn gap_json_path(durable_root: &std::path::Path, gap_id: &str) -> PathBuf {
    let gap_id = gap_id.to_uppercase();
    durable_root
        .join("gaps")
        .join(&gap_id[..2])
        .join(&gap_id[2..])
        .join("gap.json")
}

fn feature_json_path(durable_root: &std::path::Path, feature_id: &str) -> PathBuf {
    let feature_id = feature_id.to_uppercase();
    durable_root
        .join("features")
        .join(&feature_id[..2])
        .join(&feature_id[2..])
        .join("feature.json")
}

fn write_json_atomically(path: &std::path::Path, value: &Value) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create Gap directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let temp_path = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    fs::write(&temp_path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write temp file {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        RefineError::Io(format!(
            "failed to commit JSON file {}: {error}",
            path.display()
        ))
    })
}

fn new_ulid_like() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut value = (now.as_millis() << 64)
        ^ ((now.subsec_nanos() as u128) << 32)
        ^ ((std::process::id() as u128) << 16)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let mut chars = [b'0'; 26];
    for idx in (0..26).rev() {
        chars[idx] = ALPHABET[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_work_item_service_transitions_gap_via_durable_json() {
        let temp_root = unique_temp_dir("work-item-transition");
        let durable_root = temp_root.join(".refine");
        let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
        fs::create_dir_all(&gap_dir).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "Transition me",
              "status": "backlog",
              "priority": "low",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
        )
        .unwrap();

        let updated =
            FileWorkItemService::new(&durable_root).transition_gap_status("GAP1", GapStatus::Todo);
        assert_eq!(updated.unwrap().gap.status, GapStatus::Todo);
        let written = fs::read_to_string(gap_dir.join("gap.json")).unwrap();
        assert!(written.contains("\"status\": \"todo\""));
        assert!(written.contains("\"updated\": \"20"));
        assert!(written.contains("Z\""));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_creates_and_lists_gap_json() {
        let temp_root = unique_temp_dir("work-item-create");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);

        let gap = service
            .create_gap_summary("Created from Rust", Some("GAP1"))
            .unwrap();
        assert_eq!(gap.gap.id, "GAP1");
        assert_eq!(gap.gap.status, GapStatus::Backlog);
        assert!(durable_root.join("gaps/GA/P1/gap.json").exists());
        assert_eq!(service.list_gap_summaries().unwrap().len(), 1);
        assert_eq!(
            service.show_gap_summary("GAP1").unwrap().gap.name,
            "Created from Rust"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_edits_notes_and_deletes_gap_json() {
        let temp_root = unique_temp_dir("work-item-edit-note-delete");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);
        service
            .create_gap_summary("Original", Some("GAP1"))
            .unwrap();

        let edited = service
            .update_gap_metadata_summary("GAP1", Some("Renamed"), Some("high"))
            .unwrap();
        assert_eq!(edited.gap.name, "Renamed");
        assert_eq!(edited.gap.priority, GapPriority::High);

        service
            .add_gap_note_summary("GAP1", "Reviewer", "Needs a note")
            .unwrap();
        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"author\": \"Reviewer\""));
        assert!(written.contains("\"body\": \"Needs a note\""));

        service.delete_gap_record("GAP1").unwrap();
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_appends_and_edits_latest_round() {
        let temp_root = unique_temp_dir("work-item-rounds");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);
        service
            .create_gap_summary("Round Gap", Some("GAP1"))
            .unwrap();

        let gap = service
            .append_gap_round_summary("GAP1", "Reporter", "Actual", "Target")
            .unwrap();
        assert_eq!(gap.gap.round_count, 1);
        let gap = service
            .edit_latest_gap_round_summary("GAP1", Some("Reviewer"), Some("New actual"), None)
            .unwrap();
        assert_eq!(gap.gap.reporter.as_deref(), Some("Reviewer"));
        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"reporter\": \"Reviewer\""));
        assert!(written.contains("\"actual\": \"New actual\""));
        assert!(written.contains("\"target\": \"Target\""));
        assert!(written.contains("\"rule_state\": \"unclassified\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_creates_features_and_updates_gap_membership() {
        let temp_root = unique_temp_dir("work-item-feature");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);
        service.create_gap_summary("Gap A", Some("GAP1")).unwrap();
        service.create_gap_summary("Gap B", Some("GAP2")).unwrap();

        let feature = service
            .create_feature_summary("Feature A", Some("FEA1"), Some("desc"), Some("Reporter"))
            .unwrap();
        assert_eq!(feature.feature.id, "FEA1");
        assert!(durable_root.join("features/FE/A1/feature.json").exists());

        let feature = service.assign_gap_to_feature("FEA1", "GAP1").unwrap();
        assert_eq!(feature.gap_ids, vec!["GAP1"]);
        let feature = service.assign_gap_to_feature("FEA1", "GAP2").unwrap();
        assert_eq!(feature.gap_ids, vec!["GAP1", "GAP2"]);
        assert_eq!(
            service.show_gap_summary("GAP2").unwrap().gap.feature_order,
            Some(2)
        );

        let feature = service.remove_gap_from_feature("FEA1", "GAP1").unwrap();
        assert_eq!(feature.gap_ids, vec!["GAP2"]);
        assert_eq!(
            service.show_gap_summary("GAP2").unwrap().gap.feature_order,
            Some(1)
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_reorders_and_moves_feature_workflow() {
        let temp_root = unique_temp_dir("work-item-feature-workflow");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);
        service.create_gap_summary("Gap A", Some("GAP1")).unwrap();
        service.create_gap_summary("Gap B", Some("GAP2")).unwrap();
        service.create_gap_summary("Gap C", Some("GAP3")).unwrap();
        service
            .create_feature_summary("Feature A", Some("FEA1"), None, None)
            .unwrap();
        service.assign_gap_to_feature("FEA1", "GAP1").unwrap();
        service.assign_gap_to_feature("FEA1", "GAP2").unwrap();
        service.assign_gap_to_feature("FEA1", "GAP3").unwrap();

        let reordered = service.reorder_gap_in_feature("FEA1", "GAP3", 1).unwrap();
        assert_eq!(reordered.gap_ids, vec!["GAP3", "GAP1", "GAP2"]);
        service
            .transition_gap_status("GAP2", GapStatus::Todo)
            .unwrap();
        let moved = service
            .move_feature_workflow("FEA1", GapStatus::Backlog)
            .unwrap();
        assert_eq!(moved.rollup.status, GapStatus::Backlog);
        assert_eq!(
            service.show_gap_summary("GAP2").unwrap().gap.status,
            GapStatus::Backlog
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_cancels_and_deletes_features_through_gap_paths() {
        let temp_root = unique_temp_dir("work-item-feature-cancel-delete");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);
        for (id, name) in [
            ("GAP1", "Backlog Gap"),
            ("GAP2", "Todo Gap"),
            ("GAP3", "Done Gap"),
        ] {
            service.create_gap_summary(name, Some(id)).unwrap();
        }
        service
            .create_feature_summary("Feature A", Some("FEA1"), None, None)
            .unwrap();
        for gap_id in ["GAP1", "GAP2", "GAP3"] {
            service.assign_gap_to_feature("FEA1", gap_id).unwrap();
        }
        service
            .transition_gap_status("GAP2", GapStatus::Todo)
            .unwrap();
        service
            .set_gap_status_unchecked("GAP3", &GapStatus::Done)
            .unwrap();

        let cancelled = service.cancel_feature_summary("FEA1").unwrap();
        assert_eq!(cancelled.rollup.cancelled_count, 2);
        assert_eq!(
            service.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Cancelled
        );
        assert_eq!(
            service.show_gap_summary("GAP2").unwrap().gap.status,
            GapStatus::Cancelled
        );
        assert_eq!(
            service.show_gap_summary("GAP3").unwrap().gap.status,
            GapStatus::Done
        );

        service.delete_feature_record("FEA1").unwrap();
        assert!(!durable_root.join("features/FE/A1/feature.json").exists());
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
        assert!(!durable_root.join("gaps/GA/P2/gap.json").exists());
        assert!(!durable_root.join("gaps/GA/P3/gap.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_merges_and_undoes_gap_workflow() {
        let temp_root = unique_temp_dir("work-item-merge-undo");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);
        service
            .create_gap_summary("Merge Gap", Some("GAP1"))
            .unwrap();
        service
            .set_gap_status_unchecked("GAP1", &GapStatus::ReadyMerge)
            .unwrap();

        let merged = service.merge_gap_summary("GAP1").unwrap();
        assert_eq!(merged.gap.status, GapStatus::Done);

        let undone = service.undo_gap_summary("GAP1").unwrap();
        assert_eq!(undone.gap.status, GapStatus::Review);
        let undone = service.undo_gap_summary("GAP1").unwrap();
        assert_eq!(undone.gap.status, GapStatus::Todo);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_bulk_updates_deletes_and_assigns_gaps() {
        let temp_root = unique_temp_dir("work-item-bulk");
        let durable_root = temp_root.join(".refine");
        let service = FileWorkItemService::new(&durable_root);
        for (id, name) in [
            ("GAP1", "Bulk one"),
            ("GAP2", "Bulk two"),
            ("GAP3", "Skip me"),
        ] {
            service.create_gap_summary(name, Some(id)).unwrap();
            service
                .append_gap_round_summary(id, "Original", "Actual", "Target")
                .unwrap();
        }
        service
            .set_gap_status_unchecked("GAP3", &GapStatus::Qa)
            .unwrap();

        let status_result = service
            .bulk_update_gaps(
                BulkGapSelection {
                    selected_ids: Some(vec![
                        "GAP1".to_string(),
                        "GAP2".to_string(),
                        "GAP3".to_string(),
                    ]),
                    ..Default::default()
                },
                BulkGapUpdate::Status("todo".to_string()),
            )
            .unwrap();
        assert_eq!(status_result.updated, 2);
        assert_eq!(status_result.skipped, 1);
        assert_eq!(
            service.show_gap_summary("GAP1").unwrap().gap.status,
            GapStatus::Todo
        );
        assert_eq!(
            service.show_gap_summary("GAP3").unwrap().gap.status,
            GapStatus::Qa
        );

        let reporter_result = service
            .bulk_update_gaps(
                BulkGapSelection {
                    selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
                    ..Default::default()
                },
                BulkGapUpdate::Reporter("Reviewer".to_string()),
            )
            .unwrap();
        assert_eq!(reporter_result.updated, 2);
        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"reporter\": \"Reviewer\""));

        service
            .create_feature_summary("Bulk Feature", Some("FEA1"), None, None)
            .unwrap();
        let assign_result = service
            .bulk_assign_gaps_to_feature(
                "FEA1",
                BulkGapSelection {
                    selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(assign_result.updated, 2);
        assert_eq!(
            service.show_feature_summary("FEA1").unwrap().gap_ids,
            vec!["GAP1", "GAP2"]
        );

        let delete_result = service
            .bulk_delete_gaps(BulkGapSelection {
                selected_ids: Some(vec!["GAP1".to_string(), "GAP2".to_string()]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(delete_result.deleted, 2);
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
        assert!(!durable_root.join("gaps/GA/P2/gap.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_work_item_service_rejects_invalid_manual_transition() {
        let temp_root = unique_temp_dir("work-item-invalid-transition");
        let durable_root = temp_root.join(".refine");
        let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
        fs::create_dir_all(&gap_dir).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "Transition me",
              "status": "backlog",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
        )
        .unwrap();

        let err = FileWorkItemService::new(&durable_root)
            .transition_gap_status("GAP1", GapStatus::InProgress)
            .unwrap_err();
        assert_eq!(
            err.category(),
            crate::core::supervisor::errors::ErrorCategory::InvalidInput
        );
        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}
