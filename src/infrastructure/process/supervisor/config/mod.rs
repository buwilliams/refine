use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::application::projects::projection::ActiveGoalIndex;
use crate::application::todos::FileTodoService;
use crate::error::RefineError;
use crate::error::RefineResult;
use crate::infrastructure::process::subprocess::write_json_atomically;
use crate::infrastructure::process::supervisor::coordination::{
    record_lock_key, replace_file_durably, with_record_lock,
};
use crate::model::JsonObject;
use crate::model::node::{Node, NodeDisplayNameAuthority};

pub const SETTINGS_FILE: &str = "settings.json";
pub const GOVERNANCE_FILE: &str = "governance.json";
pub const GUIDANCE_FILE: &str = "guidance.json";
pub const REPORTERS_FILE: &str = "reporters.json";
const REPORTER_CASCADE_FILE: &str = "reporter-cascade.json";
const RETIRED_SUPERVISOR_STALL_KEY: &str = "supervisor_agent_stall_seconds";
// Hibernation now discards all ignored content with an inactive checkout, so a
// per-app allowlist of removable generated paths no longer means anything.
const RETIRED_WORKTREE_GENERATED_PATHS_KEY: &str = "worktree_cleanup_generated_paths";

mod governance;
mod governance_codec;
mod guidance;
mod guidance_codec;
mod persistence;
mod reporter_codec;
mod reporters;
mod settings;
mod settings_codec;
mod settings_sections;

pub use governance::FileGovernanceService;
pub use guidance::FileGuidanceService;
pub use reporters::FileReporterService;
pub use settings::FileSettingsService;
pub use settings_codec::AUTOMATIC_AGENT_RESOURCE_BUDGET_PERCENT_DEFAULT;

use governance_codec::*;
use guidance_codec::*;
use persistence::*;
use reporter_codec::*;
use settings_codec::*;
use settings_sections::*;

pub trait ConfigService {
    fn load(&self) -> RefineResult<JsonObject>;
    fn validate(&self, config: &JsonObject) -> RefineResult<()>;
    fn merge(&self, base: JsonObject, overlay: JsonObject) -> RefineResult<JsonObject>;
}

fn settings_node(id: &str, now: &str) -> Node {
    Node {
        id: id.to_string(),
        display_name: if id == "default" {
            "Default".to_string()
        } else {
            id.to_string()
        },
        display_name_authority: Some(NodeDisplayNameAuthority::System),
        created_at: now.to_string(),
        updated_at: now.to_string(),
        settings: JsonObject::new(),
        enabled: true,
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_identity_path: String::new(),
        ssh_port: 22,
        refine_checkout: "~/refine".to_string(),
        target_app_path: String::new(),
        refine_port: 8082,
        health: None,
        archived: false,
    }
}

fn normalize_reporter_name(name: &str) -> RefineResult<String> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err(RefineError::InvalidInput("name is required".to_string()));
    }
    if clean.chars().any(|ch| ch.is_control()) || clean.len() > 120 {
        return Err(RefineError::InvalidInput(
            "invalid reporter name".to_string(),
        ));
    }
    Ok(clean.to_string())
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn new_config_item_id(prefix: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}",
        Utc::now().timestamp_micros(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod reporter_tests;
#[cfg(test)]
mod tests;
