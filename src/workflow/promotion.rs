use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::model::JsonObject;
use crate::model::feature::compare_feature_goal_order;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::RefineResult;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{FileProjectStateStore, GoalSummaryProjection};
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
        let snapshot =
            FileProjectStateStore::with_runtime_root(&self.refine_dir, &self.runtime_root)
                .load_or_refresh_projection(&self.runtime_root.join("cache"))?;
        let service = FileWorkItemService::new(&self.refine_dir);
        let active_node_id =
            FileNodeRegistryService::with_active_root(&self.refine_dir, &self.runtime_root)
                .active_node_id()?;
        let now = Utc::now();
        let mut candidates = snapshot
            .goals
            .values()
            .filter(|projection| projection.goal.status == GoalStatus::Backlog)
            .filter(|projection| {
                projection.goal.node_id.as_deref().unwrap_or("default") == active_node_id
            })
            .filter(|projection| backlog_goal_age_seconds(projection, now) >= Some(threshold))
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.updated.cmp(&b.goal.updated))
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        let mut promoted = 0;
        for goal in candidates {
            service.transition_goal_status(&goal.goal.id, GoalStatus::Todo)?;
            promoted += 1;
        }
        Ok(promoted)
    }
}

fn setting_i64(settings: &JsonObject, key: &str, fallback: i64) -> i64 {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(fallback)
}

fn backlog_goal_age_seconds(goal: &GoalSummaryProjection, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(&goal.goal.updated)
        .ok()
        .map(|timestamp| {
            now.signed_duration_since(timestamp.with_timezone(&Utc))
                .num_seconds()
        })
}
