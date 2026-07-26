use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::project_layout::{prepare_refine_dir, refine_dir_for_target_root};
use crate::tools::product::work_items::FileWorkItemService;

const RELEASE_REQUESTS_DIR: &str = "releases/requests";

mod planning;
mod publication;
mod service;
mod shell_host;

pub use planning::bump_version;
pub use service::FileReleaseService;
pub use shell_host::ShellReleaseHost;

use planning::*;
use publication::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseBump {
    Major,
    Minor,
    Patch,
}

impl ReleaseBump {
    pub fn parse(value: &str) -> RefineResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "major" => Ok(Self::Major),
            "minor" => Ok(Self::Minor),
            "patch" => Ok(Self::Patch),
            _ => Err(RefineError::InvalidInput(
                "release bump must be major, minor, or patch".to_string(),
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseChange {
    pub commit: String,
    pub summary: String,
    pub breaking: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleasePlan {
    pub current_version: String,
    pub proposed_version: String,
    pub proposed_tag: String,
    pub previous_tag: Option<String>,
    pub bump: ReleaseBump,
    pub changes: Vec<ReleaseChange>,
    pub completed_goals: Vec<String>,
    pub breaking_changes: Vec<String>,
    pub version_files: Vec<String>,
    pub documentation_files: Vec<String>,
    pub gates: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TrustedPreparation {
    pub preparation_id: String,
    pub goal_id: String,
    pub version: String,
    pub tag: String,
    pub branch: String,
    pub target_branch: String,
    pub candidate_commit: String,
    pub release_notes: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicationPreflight {
    pub main_commit: String,
    pub remote: String,
    pub branch: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublishedRelease {
    pub version: String,
    pub tag: String,
    pub commit: String,
    pub remote: String,
    pub deployment: String,
    pub release_url: String,
    pub verified: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ReleaseRequest {
    Prepare {
        plan: Box<ReleasePlan>,
        goal_id: Option<String>,
    },
    Publish {
        preparation_id: String,
    },
}

/// External publication is split into idempotent stages so a retry can inspect
/// already-created state, reject conflicts, and continue at the first missing stage.
pub trait ReleaseHost {
    fn plan(&mut self, bump: ReleaseBump) -> RefineResult<ReleasePlan>;
    fn preflight(&mut self, preparation: &TrustedPreparation)
    -> RefineResult<PublicationPreflight>;
    fn ensure_local_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()>;
    fn ensure_remote_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()>;
    fn ensure_github_release(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String>;
    fn observe_delivery(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String>;
    fn verify(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String>;
}

#[cfg(test)]
mod tests;
