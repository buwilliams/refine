use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::model::JsonObject;
use crate::model::feature::compare_feature_goal_order;
use crate::model::goal::GoalIndexProjection;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::RefineResult;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{ActiveGoalIndex, ProjectionSnapshot};
use crate::tools::product::work_items::FileWorkItemService;

#[derive(Clone, Debug)]
pub struct BacklogPromotionService {
    refine_dir: PathBuf,
    runtime_root: PathBuf,
}

impl BacklogPromotionService {
    pub fn new(refine_dir: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn promote_backlog_to_todo(&self) -> RefineResult<usize> {
        let settings =
            FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root).load()?;
        let threshold = setting_i64(&settings, "backlog_promote_after_seconds", 3600);
        if threshold < 0 {
            return Ok(0);
        }
        // Backlog Goals still hold their Feature's ordering queue, so they are
        // in the scheduler index. Reading it here keeps the promotion pass from
        // loading a projection of the whole project and undoing the bound the
        // index exists to provide.
        let active = ActiveGoalIndex::load_or_rebuild(&self.refine_dir)?;
        let service = FileWorkItemService::new(&self.refine_dir);
        let active_node_id =
            FileNodeRegistryService::with_active_root(&self.refine_dir, &self.runtime_root)
                .active_node_id()?;
        let now = Utc::now();
        let mut candidates = active
            .goals()
            .filter(|goal| goal.status == GoalStatus::Backlog)
            .filter(|goal| goal.node_id.as_deref().unwrap_or("default") == active_node_id)
            .filter(|goal| backlog_goal_age_seconds(goal, now) >= Some(threshold))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| {
            compare_feature_goal_order(a.feature_order, b.feature_order)
                .then_with(|| a.updated.cmp(&b.updated))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut promoted = 0;
        for goal in candidates {
            service.transition_goal_status(&goal.id, GoalStatus::Todo)?;
            promoted += 1;
        }
        Ok(promoted)
    }

    /// Promotes from a caller-owned coherent project snapshot and an optional
    /// newly persisted Goal. Status writes validate the projected revision and
    /// deliberately do not refresh the projection; the caller performs one
    /// rebuild after every mutation in the request is complete.
    pub fn promote_backlog_to_todo_from_projection(
        &self,
        snapshot: &ProjectionSnapshot,
        created_goal: Option<&GoalIndexProjection>,
    ) -> RefineResult<usize> {
        let settings =
            FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root).load()?;
        let threshold = setting_i64(&settings, "backlog_promote_after_seconds", 3600);
        if threshold < 0 {
            return Ok(0);
        }
        let active_node_id =
            FileNodeRegistryService::with_active_root(&self.refine_dir, &self.runtime_root)
                .active_node_id()?;
        let now = Utc::now();
        let mut candidates = snapshot
            .goals
            .values()
            .map(|projection| &projection.goal)
            .filter(|goal| goal.status == GoalStatus::Backlog)
            .filter(|goal| goal.node_id.as_deref().unwrap_or("default") == active_node_id)
            .filter(|goal| backlog_goal_age_seconds(goal, now) >= Some(threshold))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(created_goal) = created_goal
            && created_goal.status == GoalStatus::Backlog
            && created_goal.node_id.as_deref().unwrap_or("default") == active_node_id
            && backlog_goal_age_seconds(created_goal, now) >= Some(threshold)
        {
            candidates.push(created_goal.clone());
        }
        candidates.sort_by(|a, b| {
            compare_feature_goal_order(a.feature_order, b.feature_order)
                .then_with(|| a.updated.cmp(&b.updated))
                .then_with(|| a.id.cmp(&b.id))
        });
        let service = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        for goal in &candidates {
            service.transition_goal_status_from_projection(goal, GoalStatus::Todo)?;
        }
        Ok(candidates.len())
    }
}

fn setting_i64(settings: &JsonObject, key: &str, fallback: i64) -> i64 {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback)
}

fn backlog_goal_age_seconds(goal: &GoalIndexProjection, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(&goal.updated)
        .ok()
        .map(|timestamp| {
            now.signed_duration_since(timestamp.with_timezone(&Utc))
                .num_seconds()
        })
}
