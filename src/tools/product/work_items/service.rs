mod bulk_operations;
mod features;
mod goal_authoring;
mod goal_filters;
mod persistence;
mod record_persistence;
mod round_helpers;
mod rounds_and_metadata;
mod validation;
mod workflow;
use std::collections::BTreeSet;
use std::fs::{self};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::{Map, Value};

use crate::model::feature::{
    Feature, FeatureDetail, compare_feature_goal_order, failed_goal_feature_blocking_notice,
    is_ordered_feature_goal,
};
use crate::model::goal::{Goal, GoalIndexProjection, GoalPriority};
use crate::model::workflow::{
    FeatureOperation, GoalOperation, GoalStatus, feature_operation_allowed, goal_operation_allowed,
    is_automated_status, is_bulk_target_allowed, is_feature_cancel_status,
    is_feature_protected_status, is_terminal_status, user_status_transition,
};
use crate::process::supervisor::config::FileReporterService;
use crate::process::supervisor::coordination::{
    RecordLease, acquire_record_lock, record_lock_key, replace_file_durably, with_record_lock,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::observability::logs::{FileLogService, LogService};
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{
    ActiveGoalIndex, FeatureSummaryProjection, FileProjectStateStore, GoalSummaryProjection,
    ProjectStateStore, ProjectionSnapshot, goal_text_matches,
};
use crate::workflow::WorkflowEngine;

use super::types::*;

use goal_filters::*;
use record_persistence::*;
use round_helpers::*;
use validation::*;

pub(crate) use record_persistence::workflow_revision;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GoalCancellationExpectation {
    pub status: GoalStatus,
    pub round_count: usize,
    pub updated: String,
    pub node_id: String,
}

/// Serializes mutations of one Goal against other writers of that Goal.
struct GoalMutationLock {
    _record: RecordLease,
}

#[derive(Clone, Copy)]
enum BulkGoalStatusProtection {
    None,
    Automated,
    WorkflowOwned,
}

enum BulkGoalStatusMutation {
    Updated,
    Skipped(String),
}

pub(crate) struct GoalCancellationTransaction {
    service: FileWorkItemService,
    _lock: GoalMutationLock,
    goal_id: String,
    goal_path: PathBuf,
    original: Value,
    settled: Value,
    committed: bool,
}

impl GoalCancellationTransaction {
    pub(crate) fn commit(&mut self) -> RefineResult<()> {
        if self.committed {
            return Ok(());
        }
        let mut expected = self.settled.clone();
        set_workflow_revision(&mut expected, workflow_revision(&self.original))?;
        write_json_atomically(&self.goal_path, &expected)?;
        self.committed = true;
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> RefineResult<Value> {
        if self.committed {
            let mut expected = self.original.clone();
            set_workflow_revision(&mut expected, workflow_revision(&self.settled))?;
            write_json_atomically(&self.goal_path, &expected)?;
            self.committed = false;
        }
        let bytes = fs::read(&self.goal_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read restored Goal {}: {error}",
                self.goal_path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse restored Goal {}: {error}",
                self.goal_path.display()
            ))
        })
    }

    pub(crate) fn projection(&self) -> RefineResult<GoalSummaryProjection> {
        self.service.show_goal_summary(&self.goal_id)
    }

    pub(crate) fn original_value(&self) -> Value {
        self.original.clone()
    }

    pub(crate) fn settled_value(&self) -> Value {
        let mut settled = self.settled.clone();
        if let Some(object) = settled.as_object_mut() {
            object.insert(
                "workflow_revision".to_string(),
                Value::from(workflow_revision(&self.original).saturating_add(1)),
            );
        }
        settled
    }
}

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
            | (GoalStatus::Todo, GoalStatus::Qa)
            | (GoalStatus::InProgress, GoalStatus::Qa)
            | (GoalStatus::InProgress, GoalStatus::ReadyMerge)
            | (GoalStatus::Qa, GoalStatus::ReadyMerge)
            | (GoalStatus::ReadyMerge, GoalStatus::Build)
            | (GoalStatus::ReadyMerge, GoalStatus::Qa)
            | (GoalStatus::Build, GoalStatus::Qa)
            | (GoalStatus::Build, GoalStatus::Review)
            | (GoalStatus::Qa, GoalStatus::Review)
            | (GoalStatus::Qa, GoalStatus::Build)
            | (GoalStatus::Qa, GoalStatus::Done)
            | (GoalStatus::ReadyMerge, GoalStatus::Todo)
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

#[derive(Clone)]
pub struct FileWorkItemService {
    pub refine_dir: PathBuf,
    pub projection_cache_dir: Option<PathBuf>,
    pub active_node_root: Option<PathBuf>,
    pub active_node_id_override: Option<String>,
    #[cfg(test)]
    after_bulk_goal_selection_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    after_reporter_registration_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for FileWorkItemService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileWorkItemService")
            .field("refine_dir", &self.refine_dir)
            .field("projection_cache_dir", &self.projection_cache_dir)
            .field("active_node_root", &self.active_node_root)
            .field("active_node_id_override", &self.active_node_id_override)
            .finish_non_exhaustive()
    }
}

impl FileWorkItemService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            projection_cache_dir: None,
            active_node_root: None,
            active_node_id_override: None,
            #[cfg(test)]
            after_bulk_goal_selection_hook: None,
            #[cfg(test)]
            after_reporter_registration_hook: None,
        }
    }

    /// Uses one already-resolved Node identity for all ownership checks performed by this
    /// service. Long-running capability work uses this instead of re-reading mutable runtime
    /// selection state between validation and durable persistence.
    pub fn for_node(refine_dir: impl Into<PathBuf>, node_id: impl Into<String>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            projection_cache_dir: None,
            active_node_root: None,
            active_node_id_override: Some(node_id.into()),
            #[cfg(test)]
            after_bulk_goal_selection_hook: None,
            #[cfg(test)]
            after_reporter_registration_hook: None,
        }
    }

    /// Reads projections through `cache_dir` while resolving Node ownership and
    /// project state against `runtime_root`.
    ///
    /// The runtime root is passed explicitly rather than inferred from the cache
    /// directory. It used to be taken as `cache_dir.parent()`, which held only for
    /// callers whose cache directory sat immediately under the runtime root: a
    /// caller scoping its cache per unit of work (`cache/workflow/<claim id>`)
    /// resolved the runtime root to `cache/workflow`, where no `active-node.json`
    /// exists, so every ownership check compared against the `default` fallback
    /// and rejected goals owned by the real active Node.
    pub fn with_projection_cache(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            projection_cache_dir: Some(cache_dir.into()),
            active_node_root: Some(runtime_root.into()),
            active_node_id_override: None,
            #[cfg(test)]
            after_bulk_goal_selection_hook: None,
            #[cfg(test)]
            after_reporter_registration_hook: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_after_bulk_goal_selection_hook(
        mut self,
        hook: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.after_bulk_goal_selection_hook = Some(std::sync::Arc::new(hook));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_after_reporter_registration_hook(
        mut self,
        hook: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.after_reporter_registration_hook = Some(std::sync::Arc::new(hook));
        self
    }

    fn mutate_bulk_goal_status_if_eligible(
        &self,
        goal_id: &str,
        raw_target: &str,
        status_protection: BulkGoalStatusProtection,
    ) -> RefineResult<BulkGoalStatusMutation> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let goal_path = goal_json_path(&self.refine_dir, goal_id);
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
        let object = value.as_object().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let current_status = object
            .get("status")
            .and_then(Value::as_str)
            .and_then(GoalStatus::parse_wire)
            .unwrap_or(GoalStatus::Backlog);
        let owner = object
            .get("node_id")
            .and_then(Value::as_str)
            .filter(|node_id| !node_id.is_empty())
            .or_else(|| {
                object
                    .get("instance_id")
                    .and_then(Value::as_str)
                    .filter(|node_id| !node_id.is_empty())
            })
            .unwrap_or("default");
        let active_node = self.active_node_id()?;
        if owner != active_node {
            return Ok(BulkGoalStatusMutation::Skipped(format!("node:{owner}")));
        }
        let protected = is_automated_status(&current_status)
            || matches!(status_protection, BulkGoalStatusProtection::WorkflowOwned)
                && matches!(current_status, GoalStatus::Review | GoalStatus::Done);
        if protected {
            return Ok(BulkGoalStatusMutation::Skipped(format!(
                "status:{}",
                current_status.as_str()
            )));
        }
        if let Some(runtime_root) = &self.active_node_root {
            let state = WorkflowEngine::new(runtime_root).load_state()?;
            if let Some(claim) = state.active_claim(goal_id) {
                return Ok(BulkGoalStatusMutation::Skipped(format!(
                    "claim:{}",
                    claim.claim_id
                )));
            }
        }

        let target = if raw_target == "__last_workflow_state" {
            restore_last_workflow_status(&current_status)
        } else {
            GoalStatus::parse_wire(raw_target)
                .ok_or_else(|| RefineError::InvalidInput("invalid status".to_string()))?
        };
        if target != current_status || raw_target != "__last_workflow_state" {
            self.write_goal_status_value(&goal_path, &mut value, &target)?;
        }
        Ok(BulkGoalStatusMutation::Updated)
    }
}
