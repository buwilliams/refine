use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;

use serde_json::{Value, json};

use crate::application::agents::sessions::{
    ToolbarGoalAgentAttachmentStatus, find_goal_agent_session, queue_toolbar_goal_agent_attachment,
    toolbar_goal_agent_attachment_status,
};
use crate::application::exports::jira::FileGoalExportService;
use crate::application::imports::{
    FileImportService, ImportPersistFailureKind, import_drafts_from_value,
    import_extraction_prompt, parse_provider_import_result, parse_structured_import_result,
    validate_import_extraction_result,
};
use crate::application::operations::process_control::FileProcessControlService;
use crate::application::projects::projection::{
    ActivityProjectionQuery, ChangeProjectionQuery, FeatureProjectionQuery, GoalProjectionQuery,
    PROJECTION_SNAPSHOT_FILE, PageRequest, ProjectionQuery,
};
use crate::application::work_items::{
    BulkFeatureSelection, BulkFeatureUpdate, BulkGoalSelection, BulkGoalUpdate,
    FeatureGoalAuthoringRequest, FileWorkItemService, GoalAuthoringRequest,
};
use crate::application::workers::FileRunnerWorkerService;
use crate::application::workflow::WorkflowEngine;
use crate::application::workflow::engine::scheduling::BacklogPromotionService;
use crate::application::workflow::governance::integration::FileGovernanceIntegrationService;
use crate::error::RefineError;
use crate::infrastructure::agents::invocation::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::infrastructure::git::with_repository_git_lock;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::infrastructure::observability::activity::{ActivityService, FileActivityService};
use crate::infrastructure::observability::logs::FileLogService;
use crate::infrastructure::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::infrastructure::process::supervisor::config::ConfigService;
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;

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
    ) -> BTreeMap<String, crate::application::fleet::nodes::NodeIdentity> {
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
mod mission_routes;
mod terminal_profiles;
#[cfg(test)]
mod tests;
mod toolbar_attachment;

use feature_contract::{feature_detail_response_from_goals, feature_reorder_order_from_body};
use import_contract::{
    WebImportPersistObserver, import_extraction_response, import_extraction_text,
    import_provider_from_settings,
};
pub(super) use terminal_profiles::{TerminalSessionLaunchSurface, terminal_profile_prompt};
use terminal_profiles::{
    cleanup_failed_terminal_worktree, create_terminal_standalone_worktree,
    resume_terminal_standalone_worktree,
};
