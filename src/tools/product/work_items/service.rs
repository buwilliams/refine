use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::{Map, Value};

use crate::model::feature::{
    Feature, FeatureDetail, compare_feature_goal_order, failed_goal_feature_blocking_notice,
    is_ordered_feature_goal,
};
use crate::model::goal::{Goal, GoalPriority};
use crate::model::workflow::{
    FeatureOperation, GoalOperation, GoalStatus, feature_operation_allowed, goal_operation_allowed,
    is_automated_status, is_bulk_target_allowed, is_feature_cancel_status,
    is_feature_protected_status, is_terminal_status, user_status_transition,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::observability::logs::{FileLogService, LogService};
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{
    FeatureSummaryProjection, FileProjectStateStore, GoalSummaryProjection, ProjectStateStore,
};

use super::types::*;

pub trait WorkItemService {
    fn create_goal(&self, goal: Goal) -> RefineResult<Goal>;
    fn list_goals(&self) -> RefineResult<Vec<Goal>>;
    fn update_goal(&self, goal: Goal) -> RefineResult<Goal>;
    fn transition_goal(&self, goal_id: &str, target: GoalStatus) -> RefineResult<Goal>;
    fn cancel_goal(&self, goal_id: &str) -> RefineResult<Goal>;
    fn delete_goal(&self, goal_id: &str) -> RefineResult<()>;
    fn create_feature(&self, feature: Feature) -> RefineResult<Feature>;
    fn feature_detail(&self, feature_id: &str) -> RefineResult<FeatureDetail>;
    fn assign_goal(&self, goal_id: &str, feature_id: &str, order: i64) -> RefineResult<Goal>;
    fn reorder_goal(&self, goal_id: &str, order: i64) -> RefineResult<Goal>;
}

pub fn validate_manual_goal_transition(from: &GoalStatus, to: &GoalStatus) -> RefineResult<()> {
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

fn validate_automated_goal_transition(from: &GoalStatus, to: &GoalStatus) -> RefineResult<()> {
    let allowed = matches!(
        (from, to),
        (GoalStatus::Todo, GoalStatus::InProgress)
            | (GoalStatus::InProgress, GoalStatus::ReadyMerge)
            | (GoalStatus::ReadyMerge, GoalStatus::Build)
            | (GoalStatus::Build, GoalStatus::Qa)
            | (GoalStatus::Qa, GoalStatus::Review)
            | (GoalStatus::InProgress, GoalStatus::Failed)
            | (GoalStatus::Qa, GoalStatus::Failed)
            | (GoalStatus::ReadyMerge, GoalStatus::Failed)
            | (GoalStatus::Build, GoalStatus::Failed)
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
    pub active_node_root: Option<PathBuf>,
}

impl FileWorkItemService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            projection_cache_dir: None,
            active_node_root: None,
        }
    }

    pub fn with_projection_cache(
        refine_dir: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        let cache_dir = cache_dir.into();
        let active_node_root = cache_dir.parent().map(PathBuf::from);
        Self {
            refine_dir: refine_dir.into(),
            projection_cache_dir: Some(cache_dir),
            active_node_root,
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
        self.node_registry_service().active_node_id()
    }

    fn node_registry_service(&self) -> FileNodeRegistryService {
        match &self.active_node_root {
            Some(active_root) => {
                FileNodeRegistryService::with_active_root(&self.refine_dir, active_root)
            }
            None => FileNodeRegistryService::new(&self.refine_dir),
        }
    }

    fn ensure_goal_owned(&self, goal: &GoalSummaryProjection) -> RefineResult<()> {
        let owner = goal
            .goal
            .node_id
            .as_deref()
            .filter(|node_id| !node_id.is_empty())
            .unwrap_or("default");
        let active = self.active_node_id()?;
        if owner == active {
            Ok(())
        } else {
            Err(RefineError::Conflict(format!(
                "Goal {} is owned by node {owner}, not active node {active}",
                goal.goal.id
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

    pub fn create_goal_summary(
        &self,
        name: &str,
        id: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RefineError::InvalidInput(
                "Goal name is required".to_string(),
            ));
        }
        let goal_id = id
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_ulid_like);
        if goal_id.len() < 3 {
            return Err(RefineError::InvalidInput(
                "Goal id must be at least three characters".to_string(),
            ));
        }

        let goal_path = goal_json_path(&self.refine_dir, &goal_id);
        if goal_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} already exists"
            )));
        }
        let node_id = self.active_node_id()?;
        let now = now_timestamp();
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(goal_id.clone()));
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
        write_json_atomically(&goal_path, &Value::Object(object))?;
        self.show_goal_summary(&goal_id)
    }

    pub fn show_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let snapshot = self.projection_snapshot()?;
        snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
        })
    }

    pub fn show_goal_detail(&self, goal_id: &str) -> RefineResult<Value> {
        let snapshot = self.projection_snapshot()?;
        let current = snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
        })?;
        let (_, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} is not a JSON object"))
        })?;
        object.insert(
            "reporter".to_string(),
            current
                .goal
                .reporter
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "round_count".to_string(),
            Value::from(current.goal.round_count),
        );
        object.insert(
            "assignee".to_string(),
            current
                .goal
                .assignee
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if let Some(display_name) = current
            .node_display_name
            .or_else(|| self.node_display_name(current.goal.node_id.as_deref()))
        {
            object.insert("node_display_name".to_string(), Value::String(display_name));
        }
        if let Some(feature_id) = current.goal.feature_id.as_deref() {
            let mut feature_goals = snapshot
                .goals
                .values()
                .filter(|projection| projection.goal.feature_id.as_deref() == Some(feature_id))
                .map(|projection| projection.goal.clone())
                .collect::<Vec<_>>();
            feature_goals.sort_by(|a, b| {
                compare_feature_goal_order(a.feature_order, b.feature_order)
                    .then_with(|| a.id.cmp(&b.id))
            });
            if let Some(notice) = failed_goal_feature_blocking_notice(&current.goal, &feature_goals)
            {
                let notice = serde_json::to_value(notice).map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to encode Feature blocking notice: {error}"
                    ))
                })?;
                object.insert("feature_blocking_notice".to_string(), notice);
            }
        }
        self.attach_round_logs(goal_id, object)?;
        Ok(value)
    }

    pub fn list_goal_summaries(&self) -> RefineResult<Vec<GoalSummaryProjection>> {
        let snapshot = self.projection_snapshot()?;
        Ok(snapshot.goals.into_values().collect())
    }

    fn node_display_name(&self, node_id: Option<&str>) -> Option<String> {
        let node_id = node_id.unwrap_or("default");
        self.node_registry_service()
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

    fn attach_round_logs(
        &self,
        goal_id: &str,
        object: &mut Map<String, Value>,
    ) -> RefineResult<()> {
        let Some(rounds) = object.get_mut("rounds").and_then(Value::as_array_mut) else {
            return Ok(());
        };
        let log_service = FileLogService::new(&self.refine_dir);
        let round_count = rounds.len();
        for (idx, round) in rounds.iter_mut().enumerate() {
            let logs = log_service.round_logs(goal_id, idx)?;
            let Some(round_object) = round.as_object_mut() else {
                continue;
            };
            if !logs.is_empty() {
                let value = serde_json::to_value(&logs).map_err(|error| {
                    RefineError::Serialization(format!("failed to encode Goal logs: {error}"))
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

    pub fn assign_goal_to_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::AssignToFeature)?;
        let old_feature_id = current_goal.goal.feature_id.clone();
        self.set_goal_feature_membership(goal_id, Some(feature_id), None)?;
        if let Some(old_feature_id) = old_feature_id
            && old_feature_id != feature_id
        {
            let _ = self.compact_feature_orders(&old_feature_id);
        }
        self.show_feature_summary(feature_id)
    }

    pub fn remove_goal_from_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::RemoveFromFeature)?;
        self.set_goal_feature_membership(goal_id, None, None)?;
        self.compact_feature_orders(feature_id)?;
        self.show_feature_summary(feature_id)
    }

    pub fn order_goal_in_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::ReorderInFeature)?;
        if is_ordered_feature_goal(current_goal.goal.feature_order) {
            return self.show_feature_summary(feature_id);
        }
        let next_order = self.next_feature_order(feature_id)?;
        self.set_goal_feature_membership(goal_id, Some(feature_id), Some(next_order))?;
        self.show_feature_summary(feature_id)
    }

    pub fn unorder_goal_in_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::ReorderInFeature)?;
        if !is_ordered_feature_goal(current_goal.goal.feature_order) {
            return self.show_feature_summary(feature_id);
        }
        self.set_goal_feature_membership(goal_id, Some(feature_id), None)?;
        self.compact_feature_orders(feature_id)?;
        self.show_feature_summary(feature_id)
    }

    pub fn reorder_goal_in_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
        order: i64,
    ) -> RefineResult<FeatureSummaryProjection> {
        if order < 1 {
            return Err(RefineError::InvalidInput(
                "feature order must be at least 1".to_string(),
            ));
        }
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::ReorderInFeature)?;
        let mut goals: Vec<_> = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .filter(|goal| is_ordered_feature_goal(goal.goal.feature_order))
            .collect();
        goals.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        let Some(current_index) = goals.iter().position(|goal| goal.goal.id == goal_id) else {
            return Err(RefineError::NotFound(format!(
                "Goal {goal_id} was not found in Feature {feature_id}"
            )));
        };
        let goal = goals.remove(current_index);
        let insert_index = usize::min(order as usize - 1, goals.len());
        goals.insert(insert_index, goal);
        for (idx, goal) in goals.iter().enumerate() {
            self.set_goal_feature_membership(
                &goal.goal.id,
                Some(feature_id),
                Some(idx as i64 + 1),
            )?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn order_goals_in_feature(
        &self,
        feature_id: &str,
        goal_ids: &[String],
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        for goal_id in goal_ids {
            self.order_goal_in_feature(feature_id, goal_id)?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn move_feature_workflow(
        &self,
        feature_id: &str,
        target: GoalStatus,
    ) -> RefineResult<FeatureSummaryProjection> {
        if !matches!(target, GoalStatus::Backlog | GoalStatus::Todo) {
            return Err(RefineError::InvalidInput(
                "Feature workflow target must be backlog or todo".to_string(),
            ));
        }
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let mut goals: Vec<_> = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .collect();
        goals.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        for goal in goals {
            if is_feature_protected_status(&goal.goal.status) {
                continue;
            }
            self.set_goal_status_unchecked(&goal.goal.id, &target)?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn cancel_feature_summary(
        &self,
        feature_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let goals = self.feature_goal_summaries(feature_id)?;
        validate_feature_operation(
            &goals
                .iter()
                .map(|goal| goal.goal.status.clone())
                .collect::<Vec<_>>(),
            &FeatureOperation::CancelFeature,
        )?;
        for goal in goals {
            if is_feature_cancel_status(&goal.goal.status) {
                self.cancel_goal_summary(&goal.goal.id)?;
            }
        }
        self.show_feature_summary(feature_id)
    }

    pub fn delete_feature_record(&self, feature_id: &str) -> RefineResult<()> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let goals = self.feature_goal_summaries(feature_id)?;
        validate_feature_operation(
            &goals
                .iter()
                .map(|goal| goal.goal.status.clone())
                .collect::<Vec<_>>(),
            &FeatureOperation::DeleteFeature,
        )?;
        for goal in goals {
            self.delete_goal_record(&goal.goal.id)?;
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

    pub fn bulk_update_goals(
        &self,
        selection: BulkGoalSelection,
        update: BulkGoalUpdate,
    ) -> RefineResult<BulkUpdateResult> {
        let (field, raw_value) = match update {
            BulkGoalUpdate::Priority(value) => ("priority".to_string(), value.trim().to_string()),
            BulkGoalUpdate::Status(value) => ("status".to_string(), value.trim().to_lowercase()),
            BulkGoalUpdate::Reporter(value) => ("reporter".to_string(), value.trim().to_string()),
            BulkGoalUpdate::Assignee(value) => ("assignee".to_string(), value.trim().to_string()),
        };
        if field == "priority" && GoalPriority::parse_wire(&raw_value).is_none() {
            return Err(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }
        if field == "status" && raw_value != "__last_workflow_state" {
            let Some(status) = GoalStatus::parse_wire(&raw_value) else {
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
        let (goals, skipped_details) =
            self.select_bulk_goal_summaries(&selection, skip_automated)?;
        let mut ids = Vec::new();
        for goal in goals {
            self.ensure_goal_owned(&goal)?;
            match field.as_str() {
                "priority" => self.set_goal_priority_unchecked(&goal.goal.id, &raw_value)?,
                "status" if raw_value == "__last_workflow_state" => {
                    let restored = restore_last_workflow_status(&goal.goal.status);
                    if restored != goal.goal.status {
                        self.set_goal_status_unchecked(&goal.goal.id, &restored)?;
                    }
                }
                "status" => {
                    let status = GoalStatus::parse_wire(&raw_value)
                        .ok_or_else(|| RefineError::InvalidInput("invalid status".to_string()))?;
                    self.set_goal_status_unchecked(&goal.goal.id, &status)?;
                }
                "reporter" => self.set_goal_reporter_unchecked(&goal.goal.id, &raw_value)?,
                "assignee" => self.set_latest_round_assignee(&goal.goal.id, &raw_value)?,
                _ => unreachable!(),
            }
            ids.push(goal.goal.id);
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

    pub fn bulk_delete_goals(
        &self,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkDeleteResult> {
        let (goals, _) = self.select_bulk_goal_summaries(&selection, false)?;
        let mut ids = Vec::new();
        let mut feature_ids = BTreeSet::new();
        for goal in goals {
            self.ensure_goal_owned(&goal)?;
            if let Some(feature_id) = &goal.goal.feature_id {
                feature_ids.insert(feature_id.clone());
            }
            self.delete_goal_record(&goal.goal.id)?;
            ids.push(goal.goal.id);
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
        update: BulkFeatureUpdate,
    ) -> RefineResult<BulkUpdateResult> {
        let (field, raw_value) = match update {
            BulkFeatureUpdate::Reporter(value) => {
                ("reporter".to_string(), value.trim().to_string())
            }
            BulkFeatureUpdate::Assignee(value) => {
                ("assignee".to_string(), value.trim().to_string())
            }
        };
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
        let features = self.select_bulk_feature_summaries(&selection)?;
        let mut ids = Vec::new();
        for feature in features {
            self.ensure_feature_owned(&feature)?;
            match field.as_str() {
                "reporter" => {
                    self.update_feature_metadata_summary(
                        &feature.feature.id,
                        None,
                        None,
                        Some(&raw_value),
                        None,
                    )?;
                }
                "assignee" => {
                    self.update_feature_metadata_summary(
                        &feature.feature.id,
                        None,
                        None,
                        None,
                        Some(&raw_value),
                    )?;
                }
                _ => unreachable!(),
            }
            ids.push(feature.feature.id);
        }
        Ok(BulkUpdateResult {
            updated: ids.len(),
            ids,
            field,
            value: raw_value,
            skipped: 0,
            skipped_details: Vec::new(),
            failed: 0,
            failures: Vec::new(),
        })
    }

    pub fn bulk_delete_features(
        &self,
        selection: BulkFeatureSelection,
    ) -> RefineResult<BulkDeleteResult> {
        let features = self.select_bulk_feature_summaries(&selection)?;
        let mut ids = Vec::new();
        for feature in features {
            self.delete_feature_record(&feature.feature.id)?;
            ids.push(feature.feature.id);
        }
        Ok(BulkDeleteResult {
            deleted: ids.len(),
            ids,
            failures: Vec::new(),
            failed: 0,
        })
    }

    pub fn bulk_assign_goals_to_feature(
        &self,
        feature_id: &str,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkAssignFeatureResult> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let (goals, mut skipped_details) = self.select_bulk_goal_summaries(&selection, false)?;
        let mut old_feature_ids = BTreeSet::new();
        let mut ids = Vec::new();
        for goal in goals {
            self.ensure_goal_owned(&goal)?;
            if goal.goal.feature_id.as_deref() == Some(feature_id) {
                skipped_details.push(BulkSkippedDetail {
                    id: goal.goal.id,
                    reason: "already-assigned".to_string(),
                });
                continue;
            }
            validate_goal_operation(&goal.goal.status, &GoalOperation::AssignToFeature)?;
            if let Some(old_feature_id) = &goal.goal.feature_id {
                old_feature_ids.insert(old_feature_id.clone());
            }
            self.set_goal_feature_membership(&goal.goal.id, Some(feature_id), None)?;
            ids.push(goal.goal.id);
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

    pub fn bulk_transfer_goals_to_node(
        &self,
        target_node_id: &str,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let (goals, mut skipped_details) = self.select_bulk_goal_summaries(&selection, false)?;
        let mut ids = Vec::new();
        for goal in goals {
            if let Some(reason) = goal_transfer_skip_reason(&goal) {
                skipped_details.push(BulkSkippedDetail {
                    id: goal.goal.id,
                    reason,
                });
                continue;
            }
            self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
            ids.push(goal.goal.id);
        }
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: ids.len(),
            ids,
            skipped: skipped_details.len(),
            skipped_details,
        })
    }

    pub fn transfer_goal_to_node(
        &self,
        target_node_id: &str,
        goal_id: &str,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let goal = self.show_goal_summary(goal_id)?;
        validate_goal_transfer_to_node(&goal)?;
        self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: 1,
            ids: vec![goal.goal.id],
            skipped: 0,
            skipped_details: Vec::new(),
        })
    }

    pub fn transfer_feature_to_node(
        &self,
        target_node_id: &str,
        feature_id: &str,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let feature = self.show_feature_summary(feature_id)?;
        let mut goals = Vec::new();
        for goal_id in &feature.goal_ids {
            let goal = self.show_goal_summary(goal_id)?;
            if let Some(reason) = goal_status_transfer_skip_reason(&goal) {
                return Err(RefineError::Conflict(format!(
                    "Feature {} cannot transfer while Goal {} is not transferable ({reason})",
                    feature.feature.id, goal.goal.id
                )));
            }
            goals.push(goal);
        }
        self.set_feature_node_unchecked(&feature.feature.id, &target_node_id)?;
        let mut ids = vec![feature.feature.id];
        for goal in goals {
            self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
            ids.push(goal.goal.id);
        }
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: ids.len(),
            ids,
            skipped: 0,
            skipped_details: Vec::new(),
        })
    }

    pub fn bulk_transfer_features_to_node(
        &self,
        target_node_id: &str,
        selection: BulkFeatureSelection,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let features = self.select_bulk_feature_summaries(&selection)?;
        let mut ids = Vec::new();
        let mut skipped_details = Vec::new();
        for feature in features {
            self.ensure_feature_owned(&feature)?;
            let mut goals = Vec::new();
            let mut skip_reason = None;
            for goal_id in &feature.goal_ids {
                let goal = self.show_goal_summary(goal_id)?;
                if let Some(reason) = goal_status_transfer_skip_reason(&goal) {
                    skip_reason = Some(format!("goal:{}:{reason}", goal.goal.id));
                    break;
                }
                goals.push(goal);
            }
            if let Some(reason) = skip_reason {
                skipped_details.push(BulkSkippedDetail {
                    id: feature.feature.id,
                    reason,
                });
                continue;
            }
            self.set_feature_node_unchecked(&feature.feature.id, &target_node_id)?;
            ids.push(feature.feature.id);
            for goal in goals {
                self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
                ids.push(goal.goal.id);
            }
        }
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: ids.len(),
            ids,
            skipped: skipped_details.len(),
            skipped_details,
        })
    }

    /// Reassigns node ownership of eligible Goals across the given nodes.
    /// Distribute is the one sanctioned exception to node ownership
    /// enforcement: unclaimed work may move regardless of which node owns it,
    /// because reassignment is the transfer. Eligible means captured or
    /// actionable (backlog/todo) with no active claim; converge instead moves
    /// reviewable Goals home to a single review node. Feature-bound goals are
    /// skipped so Feature ordering stays intact — transfer the Feature to move
    /// them as a unit.
    pub fn distribute_goals_across_nodes(
        &self,
        target_node_ids: &[String],
        converge: bool,
        claimed_goal_ids: &BTreeSet<String>,
        dry_run: bool,
    ) -> RefineResult<DistributeResult> {
        if target_node_ids.is_empty() {
            return Err(RefineError::InvalidInput(
                "distribute requires at least one enabled, healthy node".to_string(),
            ));
        }
        let mut node_ids = Vec::new();
        for node_id in target_node_ids {
            let node_id = self.validate_transfer_target_node(node_id)?;
            if !node_ids.contains(&node_id) {
                node_ids.push(node_id);
            }
        }
        if converge && node_ids.len() != 1 {
            return Err(RefineError::InvalidInput(
                "converge moves reviewable goals to exactly one review node".to_string(),
            ));
        }
        let mut summaries = self.list_goal_summaries()?;
        summaries.sort_by(|a, b| {
            a.goal
                .created
                .cmp(&b.goal.created)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        let mut eligible = Vec::new();
        let mut skipped_details = Vec::new();
        let mut load: Vec<usize> = vec![0; node_ids.len()];
        for goal in &summaries {
            let owner = goal
                .goal
                .node_id
                .clone()
                .unwrap_or_else(|| "default".to_string());
            let matches = if converge {
                goal.goal.status == GoalStatus::Review
            } else {
                matches!(goal.goal.status, GoalStatus::Backlog | GoalStatus::Todo)
            };
            if !matches {
                if !is_terminal_status(&goal.goal.status) {
                    if let Some(index) = node_ids.iter().position(|id| *id == owner) {
                        load[index] += 1;
                    }
                }
                continue;
            }
            if let Some(feature_id) = goal.goal.feature_id.as_deref() {
                skipped_details.push(BulkSkippedDetail {
                    id: goal.goal.id.clone(),
                    reason: format!("feature:{feature_id}"),
                });
                if let Some(index) = node_ids.iter().position(|id| *id == owner) {
                    load[index] += 1;
                }
                continue;
            }
            if claimed_goal_ids.contains(&goal.goal.id) {
                skipped_details.push(BulkSkippedDetail {
                    id: goal.goal.id.clone(),
                    reason: "claimed".to_string(),
                });
                if let Some(index) = node_ids.iter().position(|id| *id == owner) {
                    load[index] += 1;
                }
                continue;
            }
            eligible.push((goal.goal.id.clone(), owner));
        }
        let mut moves = Vec::new();
        for (goal_id, owner) in &eligible {
            let target_index = load
                .iter()
                .enumerate()
                .min_by_key(|(index, count)| (**count, *index))
                .map(|(index, _)| index)
                .unwrap_or(0);
            let to_node_id = node_ids[target_index].clone();
            load[target_index] += 1;
            if to_node_id != *owner {
                moves.push(DistributeMove {
                    goal_id: goal_id.clone(),
                    from_node_id: owner.clone(),
                    to_node_id,
                });
            }
        }
        if !dry_run {
            for entry in &moves {
                self.set_goal_node_unchecked(&entry.goal_id, &entry.to_node_id)?;
            }
        }
        Ok(DistributeResult {
            strategy: if converge {
                "converge".to_string()
            } else if node_ids.len() == 1 {
                "fill".to_string()
            } else {
                "spread".to_string()
            },
            node_ids,
            eligible: eligible.len(),
            moved: moves.len(),
            moves,
            skipped: skipped_details.len(),
            skipped_details,
            dry_run,
        })
    }

    pub fn transfer_item_to_node(
        &self,
        target_node_id: &str,
        item_id: &str,
    ) -> RefineResult<BulkTransferNodeResult> {
        let item_id = item_id.trim();
        if item_id.is_empty() {
            return Err(RefineError::InvalidInput("item_id is required".to_string()));
        }
        match self.show_feature_summary(item_id) {
            Ok(_) => self.transfer_feature_to_node(target_node_id, item_id),
            Err(feature_error) => match self.transfer_goal_to_node(target_node_id, item_id) {
                Ok(result) => Ok(result),
                Err(goal_error)
                    if matches!(
                        feature_error.category(),
                        crate::process::supervisor::errors::ErrorCategory::NotFound
                    ) && matches!(
                        goal_error.category(),
                        crate::process::supervisor::errors::ErrorCategory::NotFound
                    ) =>
                {
                    Err(RefineError::NotFound(format!(
                        "Goal or Feature {item_id} was not found in refine state"
                    )))
                }
                Err(goal_error) => Err(goal_error),
            },
        }
    }

    pub fn verify_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::VerifyReview)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Done)?;
        self.show_goal_summary(goal_id)
    }

    pub fn retry_goal_quality_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::RetryQa)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Qa)?;
        self.show_goal_summary(goal_id)
    }

    pub fn retry_goal_merge_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::RetryMerge)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::ReadyMerge)?;
        self.show_goal_summary(goal_id)
    }

    pub fn submit_goal_for_merge_summary(
        &self,
        goal_id: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        if current.goal.status == GoalStatus::ReadyMerge {
            return Ok(current);
        }
        validate_goal_operation(&current.goal.status, &GoalOperation::SubmitMerge)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::ReadyMerge)?;
        self.show_goal_summary(goal_id)
    }

    pub fn undo_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::Undo)?;
        let target = match current.goal.status {
            GoalStatus::Done => GoalStatus::Review,
            GoalStatus::Review | GoalStatus::Cancelled => GoalStatus::Todo,
            _ => {
                return Err(RefineError::InvalidInput(
                    "Goal undo is only available from done, review, or cancelled".to_string(),
                ));
            }
        };
        self.set_goal_status_unchecked(goal_id, &target)?;
        self.show_goal_summary(goal_id)
    }

    pub fn start_goal_workflow(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        match current.goal.status {
            GoalStatus::Backlog => self.transition_goal_status(goal_id, GoalStatus::Todo),
            GoalStatus::Todo => Ok(current),
            _ => Err(RefineError::InvalidInput(format!(
                "Goal {goal_id} can only be queued from backlog or todo"
            ))),
        }
    }

    pub fn advance_automated_goal_status(
        &self,
        goal_id: &str,
        target: GoalStatus,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_automated_goal_transition(&current.goal.status, &target)?;
        self.set_goal_status_unchecked(goal_id, &target)?;
        self.show_goal_summary(goal_id)
    }

    pub fn rollback_in_progress_goal_to_todo(
        &self,
        goal_id: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        if current.goal.status != GoalStatus::InProgress {
            return Err(RefineError::InvalidInput(format!(
                "Goal {goal_id} is not in-progress"
            )));
        }
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Todo)?;
        self.show_goal_summary(goal_id)
    }

    pub fn set_goal_branch_name(
        &self,
        goal_id: &str,
        branch_name: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let branch_name = branch_name.trim();
        if branch_name.is_empty() {
            return Err(RefineError::InvalidInput(
                "branch name is required".to_string(),
            ));
        }
        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "branch_name".to_string(),
            Value::String(branch_name.to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn workflow_enforcement_summary(&self) -> RefineResult<WorkflowEnforcementSummary> {
        let snapshot = self.projection_snapshot()?;
        let automated = snapshot
            .goals
            .values()
            .filter(|goal| is_automated_status(&goal.goal.status))
            .map(|goal| goal.goal.id.clone())
            .collect();
        Ok(WorkflowEnforcementSummary {
            ok: true,
            checked: snapshot.goals.len(),
            automated,
        })
    }

    pub fn transition_goal_status(
        &self,
        goal_id: &str,
        target: GoalStatus,
    ) -> RefineResult<GoalSummaryProjection> {
        let snapshot = self.projection_snapshot()?;
        let current = snapshot.goals.get(goal_id).ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
        })?;
        self.ensure_goal_owned(current)?;
        validate_manual_goal_transition(&current.goal.status, &target)?;

        let goal_path = self.refine_dir.join(&current.goal.json_path);
        let bytes = fs::read(&goal_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "status".to_string(),
            Value::String(target.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));

        write_json_atomically(&goal_path, &value)?;

        let refreshed = self.projection_snapshot()?;
        refreshed.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} disappeared after transition"))
        })
    }

    pub fn cancel_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        if current.goal.status == GoalStatus::Cancelled {
            return Ok(current);
        }
        if current.goal.status == GoalStatus::Done {
            return Err(RefineError::InvalidInput(
                "done Goals cannot be cancelled".to_string(),
            ));
        }
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Cancelled)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_goal_metadata_summary(
        &self,
        goal_id: &str,
        name: Option<&str>,
        priority: Option<&str>,
        reporter: Option<&str>,
        assignee: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditMetadata)?;

        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                return Err(RefineError::InvalidInput(
                    "Goal name cannot be empty".to_string(),
                ));
            }
            object.insert("name".to_string(), Value::String(name.to_string()));
        }
        if let Some(priority) = priority {
            let Some(priority) = GoalPriority::parse_wire(priority) else {
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
        write_json_atomically(&goal_path, &value)?;
        if let Some(assignee) = assignee {
            self.set_latest_round_assignee(goal_id, assignee)?;
        }
        self.show_goal_summary(goal_id)
    }

    fn validate_goal_assignee(assignee: &str) -> RefineResult<&str> {
        let assignee = assignee.trim();
        if !assignee.is_empty() && !valid_reporter_name(assignee) {
            return Err(RefineError::InvalidInput(
                "invalid assignee name".to_string(),
            ));
        }
        Ok(assignee)
    }

    fn validate_goal_reporter(reporter: &str) -> RefineResult<&str> {
        let reporter = reporter.trim();
        if !reporter.is_empty() && !valid_reporter_name(reporter) {
            return Err(RefineError::InvalidInput(
                "invalid reporter name".to_string(),
            ));
        }
        Ok(reporter)
    }

    pub fn update_goal_assignee_summary(
        &self,
        goal_id: &str,
        assignee: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditMetadata)?;
        self.set_latest_round_assignee(goal_id, assignee)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_goal_reporter_summary(
        &self,
        goal_id: &str,
        reporter: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditMetadata)?;
        self.set_goal_reporter_unchecked(goal_id, reporter)?;
        self.show_goal_summary(goal_id)
    }

    pub fn add_goal_note_summary(
        &self,
        goal_id: &str,
        author: &str,
        body: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditNotes)?;
        let body = body.trim();
        if body.is_empty() {
            return Err(RefineError::InvalidInput(
                "note body cannot be empty".to_string(),
            ));
        }

        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
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
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn replace_goal_notes_summary(
        &self,
        goal_id: &str,
        notes: &[Value],
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditNotes)?;

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

        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert("notes".to_string(), Value::Array(next_notes));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn append_goal_round_summary(
        &self,
        goal_id: &str,
        reporter: &str,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        self.append_goal_round_summary_with_assignee(goal_id, reporter, None, prompt)
    }

    pub fn append_goal_round_summary_with_assignee(
        &self,
        goal_id: &str,
        reporter: &str,
        assignee: Option<&str>,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::SubmitNewRound)?;
        let reporter = Self::validate_goal_reporter(reporter)?;
        let assignee = assignee
            .map(Self::validate_goal_assignee)
            .transpose()?
            .filter(|value| !value.is_empty())
            .unwrap_or(reporter);
        let prompt = prompt.trim();
        if reporter.is_empty() || prompt.is_empty() {
            return Err(RefineError::InvalidInput(
                "round reporter and prompt are required".to_string(),
            ));
        }

        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let round = new_round_value(reporter, assignee, prompt);
        match object.get_mut("rounds") {
            Some(Value::Array(rounds)) => rounds.push(round),
            _ => {
                object.insert("rounds".to_string(), Value::Array(vec![round]));
            }
        }
        if current.goal.status == GoalStatus::Review {
            object.insert(
                "status".to_string(),
                Value::String(GoalStatus::Todo.as_str().to_string()),
            );
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn edit_latest_goal_round_summary(
        &self,
        goal_id: &str,
        reporter: Option<&str>,
        assignee: Option<&str>,
        prompt: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditLatestRound)?;

        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = rounds
            .iter_mut()
            .rev()
            .find(|round| round.is_object())
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = latest.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "latest round for Goal {goal_id} is not a JSON object"
            ))
        })?;
        if let Some(reporter) = reporter {
            latest.insert(
                "reporter".to_string(),
                Value::String(Self::validate_goal_reporter(reporter)?.to_string()),
            );
        }
        if let Some(assignee) = assignee {
            let assignee = Self::validate_goal_assignee(assignee)?;
            latest.insert(
                "assignee".to_string(),
                if assignee.is_empty() {
                    Value::Null
                } else {
                    Value::String(assignee.to_string())
                },
            );
        }
        if let Some(prompt) = prompt {
            latest.insert(
                "prompt".to_string(),
                Value::String(prompt.trim().to_string()),
            );
        }
        let now = now_timestamp();
        latest.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_goal_branch_name(
        &self,
        goal_id: &str,
        branch_name: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
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
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_latest_goal_round_evaluation_summary(
        &self,
        goal_id: &str,
        evaluation: &Value,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let fields = evaluation.as_object().ok_or_else(|| {
            RefineError::InvalidInput("round evaluation body must be a JSON object".to_string())
        })?;

        let (goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = rounds
            .iter_mut()
            .rev()
            .find(|round| round.is_object())
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = latest.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "latest round for Goal {goal_id} is not a JSON object"
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
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn delete_goal_record(&self, goal_id: &str) -> RefineResult<()> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::Delete)?;
        let goal_path = self.refine_dir.join(&current.goal.json_path);
        fs::remove_file(&goal_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to delete Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        if let Some(parent) = goal_path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }

    fn read_goal_value(&self, goal_id: &str) -> RefineResult<(PathBuf, Value)> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        self.read_goal_value_unchecked(&current)
    }

    fn read_goal_value_unchecked(
        &self,
        current: &GoalSummaryProjection,
    ) -> RefineResult<(PathBuf, Value)> {
        let goal_path = self.refine_dir.join(&current.goal.json_path);
        let bytes = fs::read(&goal_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        Ok((goal_path, value))
    }

    fn set_goal_feature_membership(
        &self,
        goal_id: &str,
        feature_id: Option<&str>,
        feature_order: Option<i64>,
    ) -> RefineResult<()> {
        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
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
        write_json_atomically(&goal_path, &value)
    }

    pub(super) fn set_goal_status_unchecked(
        &self,
        goal_id: &str,
        status: &GoalStatus,
    ) -> RefineResult<()> {
        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "status".to_string(),
            Value::String(status.as_str().to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)
    }

    fn set_goal_priority_unchecked(&self, goal_id: &str, priority: &str) -> RefineResult<()> {
        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert("priority".to_string(), Value::String(priority.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)
    }

    fn set_goal_reporter_unchecked(&self, goal_id: &str, reporter: &str) -> RefineResult<()> {
        let reporter = Self::validate_goal_reporter(reporter)?;
        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
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
        write_json_atomically(&goal_path, &value)
    }

    fn set_goal_node_unchecked(&self, goal_id: &str, node_id: &str) -> RefineResult<()> {
        let current = self.show_goal_summary(goal_id)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert("node_id".to_string(), Value::String(node_id.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)
    }

    fn set_feature_node_unchecked(&self, feature_id: &str, node_id: &str) -> RefineResult<()> {
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
        object.insert("node_id".to_string(), Value::String(node_id.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&feature_path, &value)
    }

    fn validate_transfer_target_node(&self, target_node_id: &str) -> RefineResult<String> {
        let target_node_id = target_node_id.trim();
        if target_node_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "target_node_id is required".to_string(),
            ));
        }
        self.node_registry_service()
            .ensure_transfer_target(target_node_id)?;
        Ok(target_node_id.to_string())
    }

    fn set_latest_round_assignee(&self, goal_id: &str, assignee: &str) -> RefineResult<()> {
        let assignee = Self::validate_goal_assignee(assignee)?;
        let (goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = rounds
            .iter_mut()
            .rev()
            .find(|round| round.is_object())
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = latest.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "latest round for Goal {goal_id} is not a JSON object"
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
        write_json_atomically(&goal_path, &value)
    }

    fn next_feature_order(&self, feature_id: &str) -> RefineResult<i64> {
        let max_order = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .filter_map(|goal| goal.goal.feature_order)
            .max()
            .unwrap_or(0);
        Ok(max_order + 1)
    }

    fn compact_feature_orders(&self, feature_id: &str) -> RefineResult<()> {
        let mut goals: Vec<_> = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .filter(|goal| is_ordered_feature_goal(goal.goal.feature_order))
            .collect();
        goals.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        for (idx, goal) in goals.iter().enumerate() {
            self.set_goal_feature_membership(
                &goal.goal.id,
                Some(feature_id),
                Some(idx as i64 + 1),
            )?;
        }
        Ok(())
    }

    fn feature_goal_summaries(&self, feature_id: &str) -> RefineResult<Vec<GoalSummaryProjection>> {
        let mut goals: Vec<_> = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .collect();
        goals.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        Ok(goals)
    }

    fn select_bulk_goal_summaries(
        &self,
        selection: &BulkGoalSelection,
        skip_automated: bool,
    ) -> RefineResult<(Vec<GoalSummaryProjection>, Vec<BulkSkippedDetail>)> {
        let excluded: BTreeSet<_> = selection
            .exclude_ids
            .iter()
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .collect();
        let mut goals = if let Some(selected_ids) = &selection.selected_ids {
            let mut selected = Vec::new();
            for id in selected_ids {
                let id = id.trim().to_uppercase();
                if id.is_empty() || excluded.contains(&id) {
                    continue;
                }
                selected.push(self.show_goal_summary(&id)?);
            }
            selected
        } else {
            self.list_goal_summaries()?
                .into_iter()
                .filter(|goal| !excluded.contains(&goal.goal.id))
                .filter(|goal| bulk_goal_matches_filter(goal, &selection.filter))
                .collect()
        };
        goals.sort_by(|a, b| a.goal.id.cmp(&b.goal.id));
        let mut skipped_details = Vec::new();
        if skip_automated {
            let mut retained = Vec::new();
            for goal in goals {
                if matches!(
                    goal.goal.status,
                    GoalStatus::InProgress | GoalStatus::Qa | GoalStatus::ReadyMerge
                ) {
                    skipped_details.push(BulkSkippedDetail {
                        id: goal.goal.id,
                        reason: format!("status:{}", goal.goal.status.as_str()),
                    });
                } else {
                    retained.push(goal);
                }
            }
            goals = retained;
        }
        Ok((goals, skipped_details))
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

fn bulk_goal_matches_filter(goal: &GoalSummaryProjection, filter: &BulkGoalFilter) -> bool {
    if let Some(status) = filter
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if goal.goal.status.as_str() != status {
            return false;
        }
    }
    if let Some(reporter) = filter
        .reporter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if goal.goal.reporter.as_deref() != Some(reporter) {
            return false;
        }
    }
    if let Some(assignee) = filter
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if goal.goal.assignee.as_deref() != Some(assignee) {
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
            if goal.goal.feature_id.is_some() {
                return false;
            }
        } else if feature != "all" && goal.goal.feature_id.as_deref() != Some(feature) {
            return false;
        }
    }
    if let Some(min_rounds) = filter.rounds_gte {
        if goal.goal.round_count < min_rounds {
            return false;
        }
    }
    if let Some(max_rounds) = filter.rounds_lte {
        if goal.goal.round_count > max_rounds {
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
            && goal.goal.node_id.as_deref().unwrap_or("default") != node
        {
            return false;
        }
    }
    if let Some(query) = filter.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let query = query.to_lowercase();
        let haystack = goal.searchable_text.to_lowercase();
        let reporter = goal.goal.reporter.as_deref().unwrap_or("").to_lowercase();
        let assignee = goal.goal.assignee.as_deref().unwrap_or("").to_lowercase();
        if !haystack.contains(&query)
            && !goal.goal.id.to_lowercase().contains(&query)
            && !reporter.contains(&query)
            && !assignee.contains(&query)
        {
            return false;
        }
    }
    true
}

fn goal_transfer_skip_reason(goal: &GoalSummaryProjection) -> Option<String> {
    if let Some(reason) = goal_status_transfer_skip_reason(goal) {
        return Some(reason);
    }
    goal.goal
        .feature_id
        .as_ref()
        .map(|feature_id| format!("feature:{feature_id}"))
}

fn goal_status_transfer_skip_reason(goal: &GoalSummaryProjection) -> Option<String> {
    if matches!(
        goal.goal.status,
        GoalStatus::InProgress | GoalStatus::Qa | GoalStatus::ReadyMerge | GoalStatus::Build
    ) {
        Some(format!("status:{}", goal.goal.status.as_str()))
    } else {
        None
    }
}

fn validate_goal_transfer_to_node(goal: &GoalSummaryProjection) -> RefineResult<()> {
    if let Some(feature_id) = goal.goal.feature_id.as_deref() {
        return Err(RefineError::Conflict(format!(
            "Goal {} is assigned to Feature {feature_id}; transfer the Feature instead",
            goal.goal.id
        )));
    }
    if let Some(reason) = goal_transfer_skip_reason(goal) {
        return Err(RefineError::Conflict(format!(
            "Goal {} is not transferable ({reason})",
            goal.goal.id
        )));
    }
    Ok(())
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

fn restore_last_workflow_status(status: &GoalStatus) -> GoalStatus {
    match status {
        GoalStatus::Failed | GoalStatus::Review | GoalStatus::Cancelled => GoalStatus::Todo,
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
                RefineError::Serialization(format!("failed to encode latest Goal log: {error}"))
            })?;
            round.insert(key.to_string(), value);
        }
    }
    Ok(())
}

fn new_round_value(reporter: &str, assignee: &str, prompt: &str) -> Value {
    let now = now_timestamp();
    let mut round = Map::new();
    round.insert("reporter".to_string(), Value::String(reporter.to_string()));
    round.insert("assignee".to_string(), Value::String(assignee.to_string()));
    round.insert("prompt".to_string(), Value::String(prompt.to_string()));
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

fn validate_goal_operation(status: &GoalStatus, operation: &GoalOperation) -> RefineResult<()> {
    let decision = goal_operation_allowed(status, operation);
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
    statuses: &[GoalStatus],
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

fn goal_json_path(refine_dir: &std::path::Path, goal_id: &str) -> PathBuf {
    let goal_id = goal_id.to_uppercase();
    refine_dir
        .join("goals")
        .join(&goal_id[..2])
        .join(&goal_id[2..])
        .join("goal.json")
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
                "failed to create Goal directory {}: {error}",
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
