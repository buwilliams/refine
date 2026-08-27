mod activity;
mod cache;
mod features;
mod goals;

use crate::infrastructure::observability::activity::ACTIVITY_LOG_FILE;
use crate::infrastructure::observability::logs::FileLogService;
use crate::model::feature::FeatureRollup;
use crate::model::workflow::GoalStatus;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::error::{RefineError, RefineResult};
use crate::model::feature::FeatureIndexProjection;
use crate::model::goal::{GoalIndexProjection, GoalPriority};
use crate::model::log::{ActivityEntry, LogEntry};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn goal_projection(id: &str, status: GoalStatus, node_id: Option<&str>) -> GoalSummaryProjection {
    GoalSummaryProjection {
        goal: GoalIndexProjection {
            id: id.to_string(),
            name: id.to_string(),
            status,
            priority: GoalPriority::Medium,
            reporter: None,
            assignee: None,
            round_count: 0,
            created: "created".to_string(),
            updated: "updated".to_string(),
            branch_name: None,
            node_id: node_id.map(str::to_string),
            feature_id: None,
            feature_order: None,
            json_path: format!("{id}/goal.json"),
        },
        node_display_name: None,
        latest_round_prompt: None,
        searchable_text: id.to_string(),
        activity_ids: Vec::new(),
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn git(root: &Path, args: &[&str]) -> RefineResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RefineError::Conflict(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }
}
