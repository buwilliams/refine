use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::{Map, Value};

use crate::model::feature::{Feature, FeatureDetail};
use crate::model::gap::{Gap, GapPriority};
use crate::model::workflow::{
    FeatureOperation, GapOperation, GapStatus, feature_operation_allowed, gap_operation_allowed,
    is_automated_status, is_bulk_target_allowed, is_feature_cancel_status,
    is_feature_protected_status, user_status_transition,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::observability::logs::{FileLogService, LogService};
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{
    FeatureSummaryProjection, FileProjectStateStore, GapSummaryProjection, ProjectStateStore,
};

use super::types::*;

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
        Err(
            crate::process::supervisor::errors::RefineError::InvalidInput(
                decision
                    .reason
                    .unwrap_or_else(|| "transition is not allowed".to_string()),
            ),
        )
    }
}

fn validate_automated_gap_transition(from: &GapStatus, to: &GapStatus) -> RefineResult<()> {
    let allowed = matches!(
        (from, to),
        (GapStatus::Todo, GapStatus::InProgress)
            | (GapStatus::InProgress, GapStatus::ReadyMerge)
            | (GapStatus::ReadyMerge, GapStatus::Build)
            | (GapStatus::Build, GapStatus::Qa)
            | (GapStatus::Qa, GapStatus::Review)
            | (GapStatus::InProgress, GapStatus::Failed)
            | (GapStatus::Qa, GapStatus::Failed)
            | (GapStatus::ReadyMerge, GapStatus::Failed)
            | (GapStatus::Build, GapStatus::Failed)
    );
    if allowed {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(format!(
            "automated transition {} -> {} is not allowed",
            from.as_str(),
            to.as_str()
        )))
    }
}

#[derive(Clone, Debug)]
pub struct FileWorkItemService {
    pub refine_dir: PathBuf,
    pub projection_cache_dir: Option<PathBuf>,
}

impl FileWorkItemService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            projection_cache_dir: None,
        }
    }

    pub fn with_projection_cache(
        refine_dir: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            projection_cache_dir: Some(cache_dir.into()),
        }
    }

    fn projection_snapshot(
        &self,
    ) -> RefineResult<crate::tools::product::project_state::ProjectionSnapshot> {
        if let Some(cache_dir) = &self.projection_cache_dir {
            let store = cache_dir
                .parent()
                .map(|runtime_root| {
                    FileProjectStateStore::with_runtime_root(&self.refine_dir, runtime_root)
                })
                .unwrap_or_else(|| FileProjectStateStore::new(&self.refine_dir));
            store.load_or_refresh_projection(cache_dir)
        } else {
            let store = FileProjectStateStore::new(&self.refine_dir);
            store.rebuild_projection()
        }
    }

    fn active_node_id(&self) -> RefineResult<String> {
        FileNodeRegistryService::new(&self.refine_dir).active_node_id()
    }

    fn ensure_gap_owned(&self, gap: &GapSummaryProjection) -> RefineResult<()> {
        let owner = gap
            .gap
            .node_id
            .as_deref()
            .filter(|node_id| !node_id.is_empty())
            .unwrap_or("default");
        let active = self.active_node_id()?;
        if owner == active {
            Ok(())
        } else {
            Err(RefineError::Conflict(format!(
                "Gap {} is owned by node {owner}, not active node {active}",
                gap.gap.id
            )))
        }
    }

    fn ensure_feature_owned(&self, feature: &FeatureSummaryProjection) -> RefineResult<()> {
        let owner = feature
            .feature
            .node_id
            .as_deref()
            .filter(|node_id| !node_id.is_empty())
            .unwrap_or("default");
        let active = self.active_node_id()?;
        if owner == active {
            Ok(())
        } else {
            Err(RefineError::Conflict(format!(
                "Feature {} is owned by node {owner}, not active node {active}",
                feature.feature.id
            )))
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

        let gap_path = gap_json_path(&self.refine_dir, &gap_id);
        if gap_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Gap {gap_id} already exists"
            )));
        }
        let node_id = self.active_node_id()?;
        let now = now_timestamp();
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(gap_id.clone()));
        object.insert("name".to_string(), Value::String(name.to_string()));
        object.insert("status".to_string(), Value::String("backlog".to_string()));
        object.insert("priority".to_string(), Value::String("low".to_string()));
        object.insert("reporter".to_string(), Value::Null);
        object.insert("branch_name".to_string(), Value::Null);
        object.insert("feature_id".to_string(), Value::Null);
        object.insert("feature_order".to_string(), Value::Null);
        object.insert("node_id".to_string(), Value::String(node_id));
        object.insert("created".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        object.insert("notes".to_string(), Value::Array(Vec::new()));
        object.insert("rounds".to_string(), Value::Array(Vec::new()));
        write_json_atomically(&gap_path, &Value::Object(object))?;
        self.show_gap_summary(&gap_id)
    }

    pub fn show_gap_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let snapshot = self.projection_snapshot()?;
        snapshot.gaps.get(gap_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Gap {gap_id} was not found in refine state"))
        })
    }

    pub fn show_gap_detail(&self, gap_id: &str) -> RefineResult<Value> {
        let current = self.show_gap_summary(gap_id)?;
        let (_, mut value) = self.read_gap_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {gap_id} is not a JSON object"))
        })?;
        object.insert(
            "reporter".to_string(),
            current
                .gap
                .reporter
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "round_count".to_string(),
            Value::from(current.gap.round_count),
        );
        object.insert(
            "assignee".to_string(),
            current
                .gap
                .assignee
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if let Some(display_name) = current
            .node_display_name
            .or_else(|| self.node_display_name(current.gap.node_id.as_deref()))
        {
            object.insert("node_display_name".to_string(), Value::String(display_name));
        }
        self.attach_round_logs(gap_id, object)?;
        Ok(value)
    }

    pub fn list_gap_summaries(&self) -> RefineResult<Vec<GapSummaryProjection>> {
        let snapshot = self.projection_snapshot()?;
        Ok(snapshot.gaps.into_values().collect())
    }

    fn node_display_name(&self, node_id: Option<&str>) -> Option<String> {
        let node_id = node_id.unwrap_or("default");
        FileNodeRegistryService::new(&self.refine_dir)
            .show(node_id)
            .ok()
            .and_then(|value| {
                value
                    .get("node")
                    .and_then(|node| node.get("display_name"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    fn attach_round_logs(&self, gap_id: &str, object: &mut Map<String, Value>) -> RefineResult<()> {
        let Some(rounds) = object.get_mut("rounds").and_then(Value::as_array_mut) else {
            return Ok(());
        };
        let log_service = FileLogService::new(&self.refine_dir);
        let round_count = rounds.len();
        for (idx, round) in rounds.iter_mut().enumerate() {
            let logs = log_service.round_logs(gap_id, idx)?;
            let Some(round_object) = round.as_object_mut() else {
                continue;
            };
            if !logs.is_empty() {
                let value = serde_json::to_value(&logs).map_err(|error| {
                    RefineError::Serialization(format!("failed to encode Gap logs: {error}"))
                })?;
                round_object.insert("logs".to_string(), value);
            }
            if idx + 1 == round_count {
                attach_latest_log_fields(round_object, &logs)?;
            }
        }
        Ok(())
    }

    pub fn create_feature_summary(
        &self,
        name: &str,
        id: Option<&str>,
        description: Option<&str>,
        reporter: Option<&str>,
        assignee: Option<&str>,
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

        let feature_path = feature_json_path(&self.refine_dir, &feature_id);
        if feature_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Feature {feature_id} already exists"
            )));
        }
        let node_id = self.active_node_id()?;
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
        object.insert(
            "assignee".to_string(),
            Value::String(assignee.or(reporter).unwrap_or("").trim().to_string()),
        );
        object.insert("node_id".to_string(), Value::String(node_id));
        object.insert("created".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&feature_path, &Value::Object(object))?;
        self.show_feature_summary(&feature_id)
    }

    pub fn show_feature_summary(&self, feature_id: &str) -> RefineResult<FeatureSummaryProjection> {
        let snapshot = self.projection_snapshot()?;
        snapshot.features.get(feature_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!(
                "Feature {feature_id} was not found in refine state"
            ))
        })
    }

    pub fn update_feature_metadata_summary(
        &self,
        feature_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        reporter: Option<&str>,
        assignee: Option<&str>,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let feature_path = feature_json_path(&self.refine_dir, feature_id);
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
        if let Some(assignee) = assignee {
            let assignee = assignee.trim();
            if !assignee.is_empty() && !valid_reporter_name(assignee) {
                return Err(RefineError::InvalidInput(
                    "invalid assignee name".to_string(),
                ));
            }
            object.insert(
                "assignee".to_string(),
                if assignee.is_empty() {
                    Value::Null
                } else {
                    Value::String(assignee.to_string())
                },
            );
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&feature_path, &value)?;
        self.show_feature_summary(feature_id)
    }

    pub fn list_feature_summaries(&self) -> RefineResult<Vec<FeatureSummaryProjection>> {
        let snapshot = self.projection_snapshot()?;
        Ok(snapshot.features.into_values().collect())
    }

    pub fn assign_gap_to_feature(
        &self,
        feature_id: &str,
        gap_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_gap = self.show_gap_summary(gap_id)?;
        self.ensure_gap_owned(&current_gap)?;
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
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_gap = self.show_gap_summary(gap_id)?;
        self.ensure_gap_owned(&current_gap)?;
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
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_gap = self.show_gap_summary(gap_id)?;
        self.ensure_gap_owned(&current_gap)?;
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
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
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
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
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
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
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
        let feature_path = feature_json_path(&self.refine_dir, feature_id);
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
            BulkGapUpdate::Assignee(value) => ("assignee".to_string(), value.trim().to_string()),
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
        if field == "assignee" && !valid_reporter_name(&raw_value) {
            return Err(RefineError::InvalidInput(
                "invalid assignee name".to_string(),
            ));
        }

        let skip_automated = field == "status" && raw_value != "__last_workflow_state";
        let (gaps, skipped_details) = self.select_bulk_gap_summaries(&selection, skip_automated)?;
        let mut ids = Vec::new();
        for gap in gaps {
            self.ensure_gap_owned(&gap)?;
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
                "reporter" => self.set_gap_reporter_unchecked(&gap.gap.id, &raw_value)?,
                "assignee" => self.set_latest_round_assignee(&gap.gap.id, &raw_value)?,
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
            self.ensure_gap_owned(&gap)?;
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

    pub fn bulk_update_features(
        &self,
        selection: BulkFeatureSelection,
        assignee: &str,
    ) -> RefineResult<BulkUpdateResult> {
        let assignee = assignee.trim();
        if !valid_reporter_name(assignee) {
            return Err(RefineError::InvalidInput(
                "invalid assignee name".to_string(),
            ));
        }
        let features = self.select_bulk_feature_summaries(&selection)?;
        let mut ids = Vec::new();
        for feature in features {
            self.ensure_feature_owned(&feature)?;
            self.set_feature_assignee_unchecked(&feature.feature.id, assignee)?;
            ids.push(feature.feature.id);
        }
        Ok(BulkUpdateResult {
            updated: ids.len(),
            ids,
            field: "assignee".to_string(),
            value: assignee.to_string(),
            skipped: 0,
            skipped_details: Vec::new(),
            failed: 0,
            failures: Vec::new(),
        })
    }

    pub fn bulk_assign_gaps_to_feature(
        &self,
        feature_id: &str,
        selection: BulkGapSelection,
    ) -> RefineResult<BulkAssignFeatureResult> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let (gaps, mut skipped_details) = self.select_bulk_gap_summaries(&selection, false)?;
        let mut next_order = self.next_feature_order(feature_id)?;
        let mut old_feature_ids = BTreeSet::new();
        let mut ids = Vec::new();
        for gap in gaps {
            self.ensure_gap_owned(&gap)?;
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
                GapStatus::InProgress | GapStatus::Qa | GapStatus::ReadyMerge | GapStatus::Build
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

    pub fn submit_gap_for_merge_summary(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        if current.gap.status == GapStatus::ReadyMerge {
            return Ok(current);
        }
        validate_gap_operation(&current.gap.status, &GapOperation::SubmitMerge)?;
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

    pub fn start_gap_workflow(&self, gap_id: &str) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        if current.gap.status == GapStatus::Backlog {
            self.transition_gap_status(gap_id, GapStatus::Todo)?;
        }
        self.start_gap_summary(gap_id)
    }

    pub fn advance_automated_gap_status(
        &self,
        gap_id: &str,
        target: GapStatus,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_automated_gap_transition(&current.gap.status, &target)?;
        self.set_gap_status_unchecked(gap_id, &target)?;
        self.show_gap_summary(gap_id)
    }

    pub fn rollback_in_progress_gap_to_todo(
        &self,
        gap_id: &str,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        self.ensure_gap_owned(&current)?;
        if current.gap.status != GapStatus::InProgress {
            return Err(RefineError::InvalidInput(format!(
                "Gap {gap_id} is not in-progress"
            )));
        }
        self.set_gap_status_unchecked(gap_id, &GapStatus::Todo)?;
        self.show_gap_summary(gap_id)
    }

    pub fn set_gap_branch_name(
        &self,
        gap_id: &str,
        branch_name: &str,
    ) -> RefineResult<GapSummaryProjection> {
        let branch_name = branch_name.trim();
        if branch_name.is_empty() {
            return Err(RefineError::InvalidInput(
                "branch name is required".to_string(),
            ));
        }
        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert(
            "branch_name".to_string(),
            Value::String(branch_name.to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn workflow_enforcement_summary(&self) -> RefineResult<WorkflowEnforcementSummary> {
        let snapshot = self.projection_snapshot()?;
        let automated = snapshot
            .gaps
            .values()
            .filter(|gap| is_automated_status(&gap.gap.status))
            .map(|gap| gap.gap.id.clone())
            .collect();
        Ok(WorkflowEnforcementSummary {
            ok: true,
            checked: snapshot.gaps.len(),
            automated,
        })
    }

    pub fn transition_gap_status(
        &self,
        gap_id: &str,
        target: GapStatus,
    ) -> RefineResult<GapSummaryProjection> {
        let snapshot = self.projection_snapshot()?;
        let current = snapshot.gaps.get(gap_id).ok_or_else(|| {
            RefineError::NotFound(format!("Gap {gap_id} was not found in refine state"))
        })?;
        self.ensure_gap_owned(current)?;
        validate_manual_gap_transition(&current.gap.status, &target)?;

        let gap_path = self.refine_dir.join(&current.gap.json_path);
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

        let refreshed = self.projection_snapshot()?;
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
        reporter: Option<&str>,
        assignee: Option<&str>,
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
        if let Some(reporter) = reporter {
            let reporter = reporter.trim();
            if !reporter.is_empty() && !valid_reporter_name(reporter) {
                return Err(RefineError::InvalidInput(
                    "invalid reporter name".to_string(),
                ));
            }
            object.insert(
                "reporter".to_string(),
                if reporter.is_empty() {
                    Value::Null
                } else {
                    Value::String(reporter.to_string())
                },
            );
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)?;
        if let Some(assignee) = assignee {
            self.set_latest_round_assignee(gap_id, assignee)?;
        }
        self.show_gap_summary(gap_id)
    }

    fn validate_gap_assignee(assignee: &str) -> RefineResult<&str> {
        let assignee = assignee.trim();
        if !assignee.is_empty() && !valid_reporter_name(assignee) {
            return Err(RefineError::InvalidInput(
                "invalid assignee name".to_string(),
            ));
        }
        Ok(assignee)
    }

    fn validate_gap_reporter(reporter: &str) -> RefineResult<&str> {
        let reporter = reporter.trim();
        if !reporter.is_empty() && !valid_reporter_name(reporter) {
            return Err(RefineError::InvalidInput(
                "invalid reporter name".to_string(),
            ));
        }
        Ok(reporter)
    }

    pub fn update_gap_assignee_summary(
        &self,
        gap_id: &str,
        assignee: &str,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::EditMetadata)?;
        self.set_latest_round_assignee(gap_id, assignee)?;
        self.show_gap_summary(gap_id)
    }

    pub fn update_gap_reporter_summary(
        &self,
        gap_id: &str,
        reporter: &str,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::EditMetadata)?;
        self.set_gap_reporter_unchecked(gap_id, reporter)?;
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

    pub fn replace_gap_notes_summary(
        &self,
        gap_id: &str,
        notes: &[Value],
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::EditNotes)?;

        let now = now_timestamp();
        let mut next_notes = Vec::new();
        for note in notes {
            let object = note.as_object().ok_or_else(|| {
                RefineError::InvalidInput("notes must be an array of objects".to_string())
            })?;
            let body = object
                .get("body")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            if body.is_empty() {
                return Err(RefineError::InvalidInput(
                    "note body cannot be empty".to_string(),
                ));
            }
            let mut cleaned = Map::new();
            cleaned.insert(
                "id".to_string(),
                Value::String(
                    object
                        .get("id")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(new_ulid_like),
                ),
            );
            cleaned.insert(
                "author".to_string(),
                Value::String(
                    object
                        .get("author")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                ),
            );
            cleaned.insert("body".to_string(), Value::String(body.to_string()));
            cleaned.insert(
                "created".to_string(),
                Value::String(
                    object
                        .get("created")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| now.clone()),
                ),
            );
            cleaned.insert("updated".to_string(), Value::String(now.clone()));
            next_notes.push(Value::Object(cleaned));
        }

        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert("notes".to_string(), Value::Array(next_notes));
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
        self.append_gap_round_summary_with_assignee(gap_id, reporter, None, actual, target)
    }

    pub fn append_gap_round_summary_with_assignee(
        &self,
        gap_id: &str,
        reporter: &str,
        assignee: Option<&str>,
        actual: &str,
        target: &str,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        validate_gap_operation(&current.gap.status, &GapOperation::SubmitNewRound)?;
        let reporter = Self::validate_gap_reporter(reporter)?;
        let assignee = assignee
            .map(Self::validate_gap_assignee)
            .transpose()?
            .filter(|value| !value.is_empty())
            .unwrap_or(reporter);
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
        let round = new_round_value(reporter, assignee, actual, target);
        match object.get_mut("rounds") {
            Some(Value::Array(rounds)) => rounds.push(round),
            _ => {
                object.insert("rounds".to_string(), Value::Array(vec![round]));
            }
        }
        if current.gap.status == GapStatus::Review {
            object.insert(
                "status".to_string(),
                Value::String(GapStatus::Todo.as_str().to_string()),
            );
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn edit_latest_gap_round_summary(
        &self,
        gap_id: &str,
        reporter: Option<&str>,
        assignee: Option<&str>,
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
                Value::String(Self::validate_gap_reporter(reporter)?.to_string()),
            );
        }
        if let Some(assignee) = assignee {
            let assignee = Self::validate_gap_assignee(assignee)?;
            latest.insert(
                "assignee".to_string(),
                if assignee.is_empty() {
                    Value::Null
                } else {
                    Value::String(assignee.to_string())
                },
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

    pub fn update_gap_branch_name(
        &self,
        gap_id: &str,
        branch_name: Option<&str>,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        self.ensure_gap_owned(&current)?;
        let (gap_path, mut value) = self.read_gap_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        match branch_name.map(str::trim).filter(|value| !value.is_empty()) {
            Some(branch) => {
                object.insert("branch_name".to_string(), Value::String(branch.to_string()));
            }
            None => {
                object.insert("branch_name".to_string(), Value::Null);
            }
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn update_latest_gap_round_evaluation_summary(
        &self,
        gap_id: &str,
        evaluation: &Value,
    ) -> RefineResult<GapSummaryProjection> {
        let current = self.show_gap_summary(gap_id)?;
        self.ensure_gap_owned(&current)?;
        let fields = evaluation.as_object().ok_or_else(|| {
            RefineError::InvalidInput("round evaluation body must be a JSON object".to_string())
        })?;

        let (gap_path, mut value) = self.read_gap_value_unchecked(&current)?;
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
        for key in [
            "rule_state",
            "meta_rule_state",
            "product_state",
            "constitution_state",
            "governance_message",
            "governance_details",
            "governance_checked_at",
            "governance_rule_actions",
            "quality_state",
            "quality_message",
            "quality_details",
            "quality_checked_at",
        ] {
            if let Some(value) = fields.get(key) {
                latest.insert(key.to_string(), value.clone());
            }
        }
        let now = now_timestamp();
        latest.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&gap_path, &value)?;
        self.show_gap_summary(gap_id)
    }

    pub fn delete_gap_record(&self, gap_id: &str) -> RefineResult<()> {
        let current = self.show_gap_summary(gap_id)?;
        self.ensure_gap_owned(&current)?;
        validate_gap_operation(&current.gap.status, &GapOperation::Delete)?;
        let gap_path = self.refine_dir.join(&current.gap.json_path);
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
        self.ensure_gap_owned(&current)?;
        self.read_gap_value_unchecked(&current)
    }

    fn read_gap_value_unchecked(
        &self,
        current: &GapSummaryProjection,
    ) -> RefineResult<(PathBuf, Value)> {
        let gap_path = self.refine_dir.join(&current.gap.json_path);
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

    pub(super) fn set_gap_status_unchecked(
        &self,
        gap_id: &str,
        status: &GapStatus,
    ) -> RefineResult<()> {
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

    fn set_gap_reporter_unchecked(&self, gap_id: &str, reporter: &str) -> RefineResult<()> {
        let reporter = Self::validate_gap_reporter(reporter)?;
        let (gap_path, mut value) = self.read_gap_value(gap_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert(
            "reporter".to_string(),
            if reporter.is_empty() {
                Value::Null
            } else {
                Value::String(reporter.to_string())
            },
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)
    }

    fn set_feature_assignee_unchecked(&self, feature_id: &str, assignee: &str) -> RefineResult<()> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let feature_path = feature_json_path(&self.refine_dir, feature_id);
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
        object.insert("assignee".to_string(), Value::String(assignee.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&feature_path, &value)
    }

    fn set_gap_node_unchecked(&self, gap_id: &str, node_id: &str) -> RefineResult<()> {
        let current = self.show_gap_summary(gap_id)?;
        let (gap_path, mut value) = self.read_gap_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Gap {} is not a JSON object", gap_path.display()))
        })?;
        object.insert("node_id".to_string(), Value::String(node_id.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&gap_path, &value)
    }

    fn set_latest_round_assignee(&self, gap_id: &str, assignee: &str) -> RefineResult<()> {
        let assignee = Self::validate_gap_assignee(assignee)?;
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
        latest.insert(
            "assignee".to_string(),
            if assignee.is_empty() {
                Value::Null
            } else {
                Value::String(assignee.to_string())
            },
        );
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

    fn select_bulk_feature_summaries(
        &self,
        selection: &BulkFeatureSelection,
    ) -> RefineResult<Vec<FeatureSummaryProjection>> {
        let excluded: BTreeSet<_> = selection
            .exclude_ids
            .iter()
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .collect();
        let mut features = if let Some(selected_ids) = &selection.selected_ids {
            let mut selected = Vec::new();
            for id in selected_ids {
                let id = id.trim().to_uppercase();
                if id.is_empty() || excluded.contains(&id) {
                    continue;
                }
                selected.push(self.show_feature_summary(&id)?);
            }
            selected
        } else {
            let active_node_id = self.active_node_id()?;
            self.list_feature_summaries()?
                .into_iter()
                .filter(|feature| !excluded.contains(&feature.feature.id))
                .filter(|feature| {
                    bulk_feature_matches_filter(feature, &selection.filter, &active_node_id)
                })
                .collect()
        };
        features.sort_by(|a, b| a.feature.id.cmp(&b.feature.id));
        Ok(features)
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
    if let Some(assignee) = filter
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if gap.gap.assignee.as_deref() != Some(assignee) {
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
        let assignee = gap.gap.assignee.as_deref().unwrap_or("").to_lowercase();
        if !haystack.contains(&query)
            && !gap.gap.id.to_lowercase().contains(&query)
            && !reporter.contains(&query)
            && !assignee.contains(&query)
        {
            return false;
        }
    }
    true
}

fn bulk_feature_matches_filter(
    feature: &FeatureSummaryProjection,
    filter: &BulkFeatureFilter,
    active_node_id: &str,
) -> bool {
    if let Some(status) = filter
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if feature.status.as_str() != status {
            return false;
        }
    }
    if let Some(reporter) = filter
        .reporter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if feature.feature.reporter.as_deref() != Some(reporter) {
            return false;
        }
    }
    if let Some(assignee) = filter
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if feature.feature.assignee.as_deref() != Some(assignee) {
            return false;
        }
    }
    if let Some(node) = filter
        .node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match node {
            "all" => {}
            "current" => {
                if feature.feature.node_id.as_deref().unwrap_or("default") != active_node_id {
                    return false;
                }
            }
            node => {
                if feature.feature.node_id.as_deref().unwrap_or("default") != node {
                    return false;
                }
            }
        }
    }
    if let Some(query) = filter.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let query = query.to_lowercase();
        let reporter = feature
            .feature
            .reporter
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        let assignee = feature
            .feature
            .assignee
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        let description = feature
            .feature
            .description
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        if !feature.feature.id.to_lowercase().contains(&query)
            && !feature.feature.name.to_lowercase().contains(&query)
            && !description.contains(&query)
            && !reporter.contains(&query)
            && !assignee.contains(&query)
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

fn attach_latest_log_fields(
    round: &mut Map<String, Value>,
    logs: &[crate::model::log::RoundLogEntry],
) -> RefineResult<()> {
    let latest_log = logs.last();
    let latest_error_log = logs
        .iter()
        .rev()
        .find(|log| log.entry.severity == "error" || log.entry.severity == "warn");
    let latest_state_log = logs.iter().rev().find(|log| log.entry.category == "state");
    let latest_workflow_log = logs
        .iter()
        .rev()
        .find(|log| log.entry.message.contains("Workflow status changed:"));
    for (key, value) in [
        ("latest_log", latest_log),
        ("latest_error_log", latest_error_log),
        ("latest_state_log", latest_state_log),
        ("latest_workflow_log", latest_workflow_log),
    ] {
        if let Some(log) = value {
            let value = serde_json::to_value(log).map_err(|error| {
                RefineError::Serialization(format!("failed to encode latest Gap log: {error}"))
            })?;
            round.insert(key.to_string(), value);
        }
    }
    Ok(())
}

fn new_round_value(reporter: &str, assignee: &str, actual: &str, target: &str) -> Value {
    let now = now_timestamp();
    let mut round = Map::new();
    round.insert("reporter".to_string(), Value::String(reporter.to_string()));
    round.insert("assignee".to_string(), Value::String(assignee.to_string()));
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

fn gap_json_path(refine_dir: &std::path::Path, gap_id: &str) -> PathBuf {
    let gap_id = gap_id.to_uppercase();
    refine_dir
        .join("gaps")
        .join(&gap_id[..2])
        .join(&gap_id[2..])
        .join("gap.json")
}

fn feature_json_path(refine_dir: &std::path::Path, feature_id: &str) -> PathBuf {
    let feature_id = feature_id.to_uppercase();
    refine_dir
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
