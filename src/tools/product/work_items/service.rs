mod bulk_operations;
mod features;
mod goal_authoring;
mod persistence;
mod rounds_and_metadata;
mod workflow;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use fs2::FileExt;
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
use crate::process::supervisor::coordination::{
    WorkflowCoordinationLease, acquire_workflow_coordination, replace_file_durably,
    with_workflow_coordination,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::observability::logs::{FileLogService, LogService};
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{
    FeatureSummaryProjection, FileProjectStateStore, GoalSummaryProjection, ProjectStateStore,
    ProjectionSnapshot,
};
use crate::workflow::WorkflowEngine;

use super::types::*;

const GOAL_MUTATION_LOCK_FILE: &str = ".goal-mutations.lock";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GoalCancellationExpectation {
    pub status: GoalStatus,
    pub round_count: usize,
    pub updated: String,
}

struct GoalMutationLock {
    _coordination: WorkflowCoordinationLease,
    file: File,
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

impl Drop for GoalMutationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) struct GoalCancellationTransaction {
    service: FileWorkItemService,
    _lock: GoalMutationLock,
    goal_id: String,
    goal_path: PathBuf,
    original: Value,
    cancelled: Value,
    committed: bool,
}

impl GoalCancellationTransaction {
    pub(crate) fn commit(&mut self) -> RefineResult<()> {
        if self.committed {
            return Ok(());
        }
        let mut expected = self.cancelled.clone();
        set_workflow_revision(&mut expected, workflow_revision(&self.original))?;
        write_json_atomically(&self.goal_path, &expected)?;
        self.committed = true;
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> RefineResult<Value> {
        if self.committed {
            let mut expected = self.original.clone();
            set_workflow_revision(&mut expected, workflow_revision(&self.cancelled))?;
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

    pub(crate) fn cancelled_value(&self) -> Value {
        let mut cancelled = self.cancelled.clone();
        if let Some(object) = cancelled.as_object_mut() {
            object.insert(
                "workflow_revision".to_string(),
                Value::from(workflow_revision(&self.original).saturating_add(1)),
            );
        }
        cancelled
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
            | (GoalStatus::InProgress, GoalStatus::Qa)
            | (GoalStatus::InProgress, GoalStatus::ReadyMerge)
            | (GoalStatus::Qa, GoalStatus::ReadyMerge)
            | (GoalStatus::ReadyMerge, GoalStatus::Build)
            | (GoalStatus::ReadyMerge, GoalStatus::Qa)
            | (GoalStatus::Build, GoalStatus::Qa)
            | (GoalStatus::Build, GoalStatus::Review)
            | (GoalStatus::Qa, GoalStatus::Review)
            | (GoalStatus::Qa, GoalStatus::Build)
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

    fn mutate_bulk_goal_status_if_eligible(
        &self,
        goal_id: &str,
        raw_target: &str,
        status_protection: BulkGoalStatusProtection,
    ) -> RefineResult<BulkGoalStatusMutation> {
        let _goal_lock = self.acquire_goal_mutation_lock()?;
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

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn derive_goal_name(prompt: &str) -> Option<String> {
    let collapsed = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name = collapsed.chars().take(80).collect::<String>();
    if collapsed.chars().count() > 80 {
        name = name
            .trim_end_matches(|ch: char| !ch.is_alphanumeric())
            .to_string();
    }
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

fn bulk_goal_matches_filter(goal: &GoalSummaryProjection, filter: &BulkGoalFilter) -> bool {
    if let Some(status) = filter
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && goal.goal.status.as_str() != status
    {
        return false;
    }
    if let Some(reporter) = filter
        .reporter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && goal.goal.reporter.as_deref() != Some(reporter)
    {
        return false;
    }
    if let Some(assignee) = filter
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && goal.goal.assignee.as_deref() != Some(assignee)
    {
        return false;
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
    if let Some(min_rounds) = filter.rounds_gte
        && goal.goal.round_count < min_rounds
    {
        return false;
    }
    if let Some(max_rounds) = filter.rounds_lte
        && goal.goal.round_count > max_rounds
    {
        return false;
    }
    if let Some(node) = filter
        .node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && node != "all"
        && node != "current"
        && goal.goal.node_id.as_deref().unwrap_or("default") != node
    {
        return false;
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
        && feature.status.as_str() != status
    {
        return false;
    }
    if let Some(reporter) = filter
        .reporter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && feature.feature.reporter.as_deref() != Some(reporter)
    {
        return false;
    }
    if let Some(assignee) = filter
        .assignee
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && feature.feature.assignee.as_deref() != Some(assignee)
    {
        return false;
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

/// A recorded failure describes the round that failed. Retrying a Goal reuses
/// that same round, so leaving the reason behind would show a live failure on
/// work that has since moved on.
fn clear_latest_round_failure(object: &mut Map<String, Value>) {
    let Some(round) = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .and_then(|rounds| rounds.last_mut())
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for key in ["failure_category", "failure_message", "failure_at"] {
        if round.contains_key(key) {
            round.insert(key.to_string(), Value::String(String::new()));
        }
    }
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
    round.insert("implementation_report".to_string(), Value::Null);
    round.insert("implementation_reported_at".to_string(), Value::Null);
    round.insert("agent_context".to_string(), Value::Null);
    round.insert("guidance_decision".to_string(), Value::Null);
    round.insert("workflow_quality_timing".to_string(), Value::Null);
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
    round.insert("failure_category".to_string(), Value::String(String::new()));
    round.insert("failure_message".to_string(), Value::String(String::new()));
    round.insert("failure_at".to_string(), Value::String(String::new()));
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
    let coordination_root = workflow_record_root(path);
    with_workflow_coordination(&coordination_root, || {
        let expected_revision = workflow_revision(value);
        let current = match fs::read(path) {
            Ok(bytes) => Some(serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse current workflow record {}: {error}",
                    path.display()
                ))
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read current workflow record {}: {error}",
                    path.display()
                )));
            }
        };
        match current.as_ref() {
            Some(current) if workflow_revision(current) != expected_revision => {
                return Err(RefineError::Conflict(format!(
                    "workflow record {} changed after it was read (expected revision {}, current revision {})",
                    path.display(),
                    expected_revision,
                    workflow_revision(current)
                )));
            }
            Some(_) => {}
            None if expected_revision != 0 => {
                return Err(RefineError::Conflict(format!(
                    "workflow record {} was removed after it was read",
                    path.display()
                )));
            }
            None => {}
        }

        let mut next = value.clone();
        let object = next.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "workflow record {} is not a JSON object",
                path.display()
            ))
        })?;
        object.insert(
            "workflow_revision".to_string(),
            Value::from(expected_revision.saturating_add(1)),
        );
        let encoded = serde_json::to_vec_pretty(&next).map_err(|error| {
            RefineError::Serialization(format!("failed to encode workflow JSON: {error}"))
        })?;
        replace_file_durably(path, &encoded)
    })
}

pub(crate) fn workflow_revision(value: &Value) -> u64 {
    value
        .get("workflow_revision")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn set_workflow_revision(value: &mut Value, revision: u64) -> RefineResult<()> {
    let object = value.as_object_mut().ok_or_else(|| {
        RefineError::Serialization("workflow record is not a JSON object".to_string())
    })?;
    object.insert("workflow_revision".to_string(), Value::from(revision));
    Ok(())
}

fn workflow_record_root(path: &std::path::Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if matches!(
            ancestor.file_name().and_then(|name| name.to_str()),
            Some("goals" | "features")
        ) {
            return ancestor
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| path.parent().unwrap_or(path).to_path_buf());
        }
    }
    path.parent().unwrap_or(path).to_path_buf()
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
