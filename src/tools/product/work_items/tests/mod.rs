mod bulk;
mod features;
mod goals;
mod rounds_metadata;
mod workflow;

use crate::model::goal::GoalPriority;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

use super::*;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::RefineError;
use crate::workflow::{WorkflowAutomation, WorkflowEngine};
use std::time::{SystemTime, UNIX_EPOCH};

// Claim execution scopes its projection cache per claim
// (`cache/workflow/<claim id>`), so inferring the runtime root from the cache
// directory landed on `cache/workflow`. No `active-node.json` exists there, the
// active Node fell back to `default`, and every goal owned by the real active
// Node failed its ownership check — automation claimed goals, failed instantly,
// and left them in `todo` forever.

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
