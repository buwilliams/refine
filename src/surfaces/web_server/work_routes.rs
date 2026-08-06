use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;

use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::agent_sessions::find_goal_agent_session;
use crate::process::runner::FileRunnerWorkerService;
use crate::process::supervisor::config::ConfigService;
use crate::process::supervisor::errors::RefineError;
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::logs::FileLogService;
use crate::tools::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::tools::product::goal_exports::FileGoalExportService;
use crate::tools::product::imports::{
    FileImportService, ImportPersistFailureKind, import_drafts_from_value,
    import_extraction_prompt, parse_provider_import_result, parse_structured_import_result,
    validate_import_extraction_result,
};
use crate::tools::product::merging::FileMergerService;
use crate::tools::product::process_control::FileProcessControlService;
use crate::tools::product::project_state::{
    ActivityProjectionQuery, ChangeProjectionQuery, FeatureProjectionQuery, GoalProjectionQuery,
    PROJECTION_SNAPSHOT_FILE, PageRequest, ProjectionQuery,
};
use crate::tools::product::work_items::{
    BulkFeatureSelection, BulkFeatureUpdate, BulkGoalSelection, BulkGoalUpdate,
    FeatureGoalAuthoringRequest, FileWorkItemService, GoalAuthoringRequest,
};
use crate::workflow::WorkflowEngine;
use crate::workflow::promotion::BacklogPromotionService;

use super::support::*;
use super::*;

impl InProcessWebServer {
    fn active_node_id_for_routes(&self) -> String {
        self.current_refine_dir()
            .ok()
            .flatten()
            .and_then(|refine_dir| self.node_registry_service(refine_dir).active_node_id().ok())
            .filter(|node_id| !node_id.trim().is_empty())
            .unwrap_or_else(|| "default".to_string())
    }

    fn node_identities_for_routes(
        &self,
    ) -> BTreeMap<String, crate::tools::product::nodes::NodeIdentity> {
        self.current_refine_dir()
            .ok()
            .flatten()
            .and_then(|refine_dir| {
                self.node_registry_service(refine_dir)
                    .node_identities()
                    .ok()
            })
            .unwrap_or_default()
    }
}

mod activity_routes;
mod feature_contract;
mod feature_routes;
mod file_terminal_routes;
mod goal_routes;
mod import_contract;
mod import_routes;
mod terminal_profiles;
#[cfg(test)]
mod tests;

use feature_contract::{feature_detail_response_from_goals, feature_reorder_order_from_body};
use import_contract::{
    WebImportPersistObserver, import_extraction_response, import_extraction_text,
    import_provider_from_settings,
};
pub(super) use terminal_profiles::terminal_profile_prompt;
use terminal_profiles::{
    cleanup_failed_terminal_worktree, create_terminal_standalone_worktree,
    resume_terminal_standalone_worktree,
};
