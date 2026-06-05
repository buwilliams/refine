use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::host::agent_providers::{AgentProviderService, HostAgentProviderService};
use crate::core::host::cluster::{ClusterBootstrapRequest, bootstrap_remote_node};
use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::core::host::installation::{
    FileInstallationService, InstallTarget, InstallationService,
};
use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcess, ProcessSupervisor,
};
use crate::core::host::quality::{
    FileQualityService, QualityCheckRequest, QualityJobRunner, QualityService, QualitySettingsPatch,
};
use crate::core::host::target_apps::{FileTargetAppService, TargetAppSnapshot};
use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::core::observability::logs::FileLogService;
use crate::core::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::core::product::chat::{ChatAttachment, ChatService, ChatSessionRecord, FileChatService};
use crate::core::product::imports::{FileImportService, import_drafts_from_value};
use crate::core::product::project_registry::{
    FileProjectRegistryService, ProjectRegistryService, registry_apps_array,
};
use crate::core::product::project_state::{
    ActivityProjectionQuery, ChangeProjectionQuery, FeatureProjectionQuery, FileProjectStateStore,
    GapProjectionQuery, PROJECTION_SNAPSHOT_FILE, PageRequest, ProjectStateStore, ProjectionQuery,
    ProjectionSnapshot, RuntimeProjection,
};
use crate::core::product::scheduling::{FileSchedulingService, SchedulingService};
use crate::core::product::work_items::{BulkGapSelection, BulkGapUpdate, FileWorkItemService};
use crate::core::supervisor::config::{
    ConfigService, FileGovernanceService, FileGuidanceService, FileReporterService,
    FileSettingsService,
};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::jobs::{FileJobRegistry, JobHandle, JobRegistry, JobState};
use crate::core::supervisor::lifecycle::DaemonStatus;
use crate::core::supervisor::security::{AuthToken, FileSecurityService, SecurityService};
use crate::core::supervisor::sessions::{FileSessionService, SessionService, SurfaceKind};
use crate::model::JsonObject;
use crate::model::cluster::{valid_cluster_node_id, valid_ssh_host};
use crate::model::log::LogEntry;
use crate::model::project::RegisteredApp;
use crate::model::workflow::GapStatus;

pub const API_CONTRACT_VERSION: &str = "1";
pub const IDEMPOTENCY_DIR: &str = "idempotency";
pub const API_EVENTS_FILE: &str = "api-events.jsonl";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiRouteGroup {
    pub prefix: &'static str,
    pub capability: &'static str,
}

pub const API_GROUPS: &[ApiRouteGroup] = &[
    ApiRouteGroup {
        prefix: "/system",
        capability: "install state, daemon status, update, doctor",
    },
    ApiRouteGroup {
        prefix: "/apps",
        capability: "target-app registry, attach, switch, detach, commands",
    },
    ApiRouteGroup {
        prefix: "/work",
        capability: "Gaps, Features, imports, state transitions",
    },
    ApiRouteGroup {
        prefix: "/agents",
        capability: "provider configuration, auth, diagnostics",
    },
    ApiRouteGroup {
        prefix: "/jobs",
        capability: "operation status, logs, cancel, retry",
    },
    ApiRouteGroup {
        prefix: "/processes",
        capability: "managed process list and controls",
    },
    ApiRouteGroup {
        prefix: "/quality",
        capability: "checks, regressions, screenshots",
    },
    ApiRouteGroup {
        prefix: "/chat",
        capability: "sessions, messages, streaming events",
    },
    ApiRouteGroup {
        prefix: "/settings",
        capability: "project and runtime settings",
    },
];

pub trait LocalDaemonWebServer {
    fn serve(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn route_groups(&self) -> &'static [ApiRouteGroup] {
        API_GROUPS
    }
    fn server_sent_events(&self, stream: &str) -> RefineResult<String>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiRequest {
    pub method: String,
    pub path: String,
    pub auth_token: Option<String>,
    pub body: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiResponse {
    pub status: u16,
    pub content_type: String,
    pub body: serde_json::Value,
}

impl ApiResponse {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json".to_string(),
            body,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct IdempotencyRecord {
    key: String,
    fingerprint: String,
    response: ApiResponse,
    created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ApiMutationEvent {
    method: String,
    path: String,
    status: u16,
    created_at: String,
}

#[derive(Clone, Debug)]
pub struct InProcessWebServer {
    pub status: DaemonStatus,
    pub projection: ProjectionSnapshot,
    pub auth_token: Option<String>,
    pub durable_root: Option<PathBuf>,
    pub runtime_root: Option<PathBuf>,
}

impl InProcessWebServer {
    pub fn handle(&self, mut request: ApiRequest) -> ApiResponse {
        let raw_path = request.path.clone();
        request.path = normalize_api_path(&request.path);
        if request.method != "GET"
            && !is_unauthenticated_mutation(&request.path)
            && !self.authorized(&request)
        {
            return ApiResponse::json(
                401,
                json!({
                    "error": {
                        "code": "unauthorized",
                        "message": "mutation request requires local authorization"
                    }
                }),
            );
        }

        if request.method == "GET" && request.path == "/system/version" {
            return ApiResponse::json(
                200,
                json!({
                    "product": "refine",
                    "version": env!("CARGO_PKG_VERSION"),
                    "api_contract_version": API_CONTRACT_VERSION,
                    "supported_api_contract_versions": [API_CONTRACT_VERSION]
                }),
            );
        }

        if request.method == "POST" && request.path == "/sessions" {
            return self.handle_session_create(request);
        }

        if request.method == "POST" && request.path == "/workflow/schedule" {
            return self.handle_workflow_schedule();
        }

        if request.method == "GET" && request.path == "/activity" {
            return self.handle_activity_list(&raw_path);
        }

        if request.method == "POST" && request.path == "/activity/cleanup" {
            return self.handle_activity_cleanup(request);
        }

        if request.method == "POST" && request.path == "/activity/ui-error" {
            return self.handle_activity_ui_error(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/jobs/")
            && request.path.ends_with("/logs")
        {
            return self.handle_job_logs(request, &raw_path);
        }

        if request.method == "GET" && request.path.starts_with("/jobs/") {
            return self.handle_job_status(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/jobs/")
            && request.path.ends_with("/retry")
        {
            return self.handle_job_retry(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/jobs/")
            && request.path.ends_with("/cancel")
        {
            return self.handle_job_cancel(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/processes/")
            && request.path.ends_with("/stream")
        {
            return self.handle_process_stream(request);
        }

        if request.method == "GET" && request.path == "/processes" {
            return self.handle_processes();
        }

        if request.method == "GET" && request.path == "/system/install" {
            return self.handle_install_status();
        }

        if request.method == "POST" && request.path == "/system/install" {
            return self.handle_install(request);
        }

        if request.method == "POST" && request.path == "/system/repair" {
            return self.handle_install_repair();
        }

        if request.method == "POST" && request.path == "/system/update" {
            return self.handle_install_update(request);
        }

        if request.method == "POST" && request.path == "/system/rollback" {
            return self.handle_install_rollback();
        }

        if request.method == "POST" && request.path == "/system/uninstall" {
            return self.handle_install_uninstall();
        }

        if request.method == "POST" && request.path == "/processes/background" {
            return self.handle_processes_background(request);
        }

        if request.method == "POST" && request.path == "/processes/agents" {
            return self.handle_processes_agents(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/agents/")
            && request.path.ends_with("/diagnostics")
        {
            return self.handle_agent_diagnostics(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/agents/")
            && request.path.ends_with("/configure")
        {
            return self.handle_agent_configure(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/agents/")
            && (request.path.ends_with("/auth") || request.path.ends_with("/authenticate"))
        {
            return self.handle_agent_authenticate(request);
        }

        if request.method == "GET" && request.path == "/agents" {
            return self.handle_agents();
        }

        if request.method == "GET" && request.path == "/diagnostics" {
            return self.handle_diagnostics();
        }

        if request.method == "GET" && request.path == "/dashboard" {
            return self.handle_dashboard();
        }

        if request.method == "GET" && request.path == "/nodes" {
            return self.handle_nodes();
        }

        if request.method == "POST" && request.path == "/nodes" {
            return self.handle_node_create(request);
        }

        if request.method == "POST" && request.path == "/nodes/activate" {
            return self.handle_node_activate(request);
        }

        if request.method == "POST" && request.path == "/nodes/transfer-gaps" {
            return self.handle_node_transfer_gaps(request);
        }

        if request.method == "POST" && request.path == "/nodes/copy-settings" {
            return self.handle_node_copy_settings(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/nodes/") {
            return self.handle_node_update(request);
        }

        if request.method == "GET" && request.path == "/cluster" {
            return self.handle_cluster();
        }

        if request.method == "POST" && request.path == "/cluster/nodes" {
            return self.handle_cluster_node_upsert(request, None);
        }

        if request.method == "PATCH" && request.path.starts_with("/cluster/nodes/") {
            let node_id = cluster_node_id_from_path(&request.path);
            return self.handle_cluster_node_upsert(request, node_id);
        }

        if request.method == "POST"
            && request.path.starts_with("/cluster/nodes/")
            && request.path.ends_with("/bootstrap")
        {
            return self.handle_cluster_node_bootstrap(request);
        }

        if request.method == "GET" && request.path == "/target-app/status" {
            return self.handle_target_app_status();
        }

        if (request.method == "GET" || request.method == "POST")
            && request.path == "/target-app/health"
        {
            return self.handle_target_app_health();
        }

        if request.method == "POST"
            && matches!(
                request.path.as_str(),
                "/target-app/start" | "/target-app/stop" | "/target-app/rebuild"
            )
        {
            return self.handle_target_app_action(request);
        }

        if request.method == "POST" && request.path == "/target-app/generate-instructions" {
            return self.handle_target_app_generate_instructions();
        }

        if request.method == "POST"
            && request.path == "/runner-workers/target-app-rebuilder/rebuild"
        {
            return self.handle_target_app_rebuild_queue();
        }

        if request.method == "POST" && request.path == "/runner-workers/merger/hard-reset-worktree"
        {
            return self.handle_merger_hard_reset_worktree();
        }

        if request.method == "GET" && request.path == "/changes" {
            return self.handle_changes_list(&raw_path);
        }

        if request.method == "POST" && request.path == "/changes/undo" {
            return self.handle_changes_undo(request);
        }

        if request.method == "POST" && request.path == "/cache/rebuild" {
            return self.handle_cache_rebuild();
        }

        if request.method == "GET" && request.path == "/performance" {
            return self.handle_performance_list(&raw_path);
        }

        if request.method == "POST" && request.path == "/performance/cleanup" {
            return self.handle_performance_cleanup(request);
        }

        if request.method == "GET" && request.path == "/files/tree" {
            return self.handle_files_tree(&raw_path);
        }

        if request.method == "GET" && request.path == "/files/read" {
            return self.handle_files_read(&raw_path);
        }

        if request.method == "GET" && request.path == "/files/search" {
            return self.handle_files_search(&raw_path);
        }

        if request.method == "POST" && request.path == "/import/extract" {
            return self.handle_import_extract(request);
        }

        if request.method == "POST" && request.path == "/import/csv/parse" {
            return self.handle_import_csv_parse(request);
        }

        if request.method == "POST" && request.path == "/import/dedup" {
            return self.handle_import_dedup(request);
        }

        if request.method == "POST" && request.path == "/import/persist" {
            return self.handle_import_persist(request);
        }

        if request.method == "GET"
            && (request.path == "/project/status" || request.path == "/apps/status")
        {
            return self.handle_project_status();
        }

        if request.method == "GET" && request.path == "/project/path" {
            return self.handle_project_path(&raw_path);
        }

        if request.method == "GET" && request.path == "/project/directories" {
            return self.handle_project_directories(&raw_path);
        }

        if request.method == "GET" && (request.path == "/projects" || request.path == "/apps") {
            return self.handle_project_list();
        }

        if request.method == "DELETE" && (request.path == "/projects" || request.path == "/apps") {
            return self.handle_project_remove(request);
        }

        if request.method == "POST"
            && (request.path == "/project/attach" || request.path == "/apps/attach")
        {
            return self.handle_project_attach(request);
        }

        if request.method == "POST" && request.path == "/apps/register" {
            return self.handle_project_register(request);
        }

        if request.method == "POST" && request.path == "/apps/switch" {
            return self.handle_project_switch(request);
        }

        if request.method == "POST"
            && (request.path == "/project/detach" || request.path == "/apps/detach")
        {
            return self.handle_project_detach();
        }

        if request.method == "POST" && request.path == "/project/sync" {
            return self.handle_project_sync();
        }

        if request.method == "GET" && request.path == "/project/templates" {
            return self.handle_project_templates();
        }

        if request.method == "POST" && request.path == "/project/scaffold" {
            return self.handle_project_scaffold(request);
        }

        if request.method == "GET" && request.path == "/settings" {
            return self.handle_settings_get();
        }

        if request.method == "PATCH" && request.path == "/settings" {
            return self.handle_settings_update(request);
        }

        if request.method == "POST" && request.path == "/settings/recheck-auth" {
            return self.handle_recheck_auth();
        }

        if request.method == "GET" && request.path == "/upgrade" {
            return self.handle_upgrade_status();
        }

        if request.method == "GET" && request.path == "/governance" {
            return self.handle_governance_get();
        }

        if request.method == "PATCH" && request.path == "/governance" {
            return self.handle_governance_save(request);
        }

        if request.method == "POST" && request.path == "/governance/generate-rules" {
            return self.handle_governance_generate_rules(request);
        }

        if request.method == "GET" && request.path == "/guidance" {
            return self.handle_guidance_list();
        }

        if request.method == "PUT" && request.path == "/guidance" {
            return self.handle_guidance_update(request);
        }

        if request.method == "GET" && request.path == "/reporters" {
            return self.handle_reporters_list();
        }

        if request.method == "POST" && request.path == "/reporters" {
            return self.handle_reporter_create(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/reporters/") {
            return self.handle_reporter_rename(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/reporters/")
            && request.path.ends_with("/merge")
        {
            return self.handle_reporter_merge(request);
        }

        if request.method == "DELETE" && request.path.starts_with("/reporters/") {
            return self.handle_reporter_delete(request);
        }

        if request.method == "GET" && request.path == "/quality" {
            return self.handle_quality_get();
        }

        if request.method == "PATCH" && request.path == "/quality" {
            return self.handle_quality_save(request);
        }

        if request.method == "POST" && request.path == "/quality/checks" {
            return self.handle_quality_checks(request);
        }

        if request.method == "GET" && request.path == "/quality/screenshots" {
            return self.handle_quality_screenshots(&raw_path);
        }

        if request.method == "POST" && request.path == "/quality/regressions" {
            return self.handle_quality_regression_create(request);
        }

        if request.method == "POST" && request.path == "/quality/regressions/run" {
            return self.handle_quality_regression_run();
        }

        if request.method == "PATCH" && request.path.starts_with("/quality/regressions/") {
            return self.handle_quality_regression_update(request);
        }

        if request.method == "DELETE" && request.path.starts_with("/quality/regressions/") {
            return self.handle_quality_regression_delete(request);
        }

        if request.method == "POST" && request.path == "/chat/start" {
            return self.handle_chat_start(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/chat/")
            && request.path.ends_with("/input")
        {
            return self.handle_chat_input(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/chat/")
            && request.path.ends_with("/read")
        {
            return self.handle_chat_read(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/chat/")
            && request.path.ends_with("/stop")
        {
            return self.handle_chat_stop(request);
        }

        if request.method == "POST" && request.path == "/work/gaps" {
            return self.handle_gap_create(request);
        }

        if request.method == "POST" && request.path == "/work/gaps/bulk" {
            return self.handle_gap_bulk_update(request);
        }

        if request.method == "POST" && request.path == "/work/gaps/bulk/delete" {
            return self.handle_gap_bulk_delete(request);
        }

        if request.method == "POST" && request.path == "/work/features" {
            return self.handle_feature_create(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/gaps/bulk")
        {
            return self.handle_feature_bulk_assign_gaps(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/gaps")
        {
            return self.handle_feature_add_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.contains("/gaps/")
            && !request.path.ends_with("/reorder")
        {
            return self.handle_feature_add_gap_path(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/reorder")
        {
            return self.handle_feature_reorder_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/move")
        {
            return self.handle_feature_move(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/features/")
            && request.path.ends_with("/cancel")
        {
            return self.handle_feature_cancel(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/work/features/") {
            return self.handle_feature_update(request);
        }

        if request.method == "DELETE"
            && request.path.starts_with("/work/features/")
            && request.path.contains("/gaps/")
        {
            return self.handle_feature_remove_gap(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/notes")
        {
            return self.handle_gap_note(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/rounds")
        {
            return self.handle_gap_round_append(request);
        }

        if request.method == "PATCH"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/rounds/latest")
        {
            return self.handle_gap_round_edit_latest(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.contains("/rounds/")
            && request.path.ends_with("/logs")
        {
            return self.handle_gap_round_log_append(request);
        }

        if request.method == "GET"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/logs")
        {
            return self.handle_gap_logs(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/transition")
        {
            return self.handle_gap_transition(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && (request.path.ends_with("/verify")
                || request.path.ends_with("/retry-quality")
                || request.path.ends_with("/retry-merge")
                || request.path.ends_with("/merge")
                || request.path.ends_with("/undo"))
        {
            return self.handle_gap_action(request);
        }

        if request.method == "POST"
            && request.path.starts_with("/work/gaps/")
            && request.path.ends_with("/cancel")
        {
            return self.handle_gap_cancel(request);
        }

        if request.method == "PATCH" && request.path.starts_with("/work/gaps/") {
            return self.handle_gap_update(request);
        }

        if request.method == "DELETE" && request.path.starts_with("/work/gaps/") {
            return self.handle_gap_delete(request);
        }

        if request.method == "GET" && request.path.starts_with("/work/gaps/") {
            return self.handle_gap_show(request);
        }

        if request.method == "GET" && request.path.starts_with("/work/features/") {
            return self.handle_feature_show(request);
        }

        if request.method == "DELETE" && request.path.starts_with("/work/features/") {
            return self.handle_feature_delete(request);
        }

        if request.method == "GET" && request.path == "/work/gaps" {
            return self.handle_gaps_list(&raw_path);
        }

        if request.method == "GET" && request.path == "/work/features" {
            return self.handle_features_list(&raw_path);
        }

        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/system/status") => ApiResponse::json(200, json!(self.status)),
            ("GET", "/system/api-groups") => {
                let groups: Vec<_> = API_GROUPS
                    .iter()
                    .map(|group| json!({"prefix": group.prefix, "capability": group.capability}))
                    .collect();
                ApiResponse::json(200, json!({"groups": groups}))
            }
            ("GET", "/events") => ApiResponse::json(
                200,
                json!({
                    "stream": "sse",
                    "events": ["activity", "process", "job", "chat"]
                }),
            ),
            _ => ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": format!("no route for {} {}", request.method, request.path)
                    }
                }),
            ),
        }
    }

    fn authorized(&self, request: &ApiRequest) -> bool {
        if let (Some(runtime_root), Some(token)) = (&self.runtime_root, &request.auth_token) {
            let command = format!("{} {}", request.method, request.path);
            if FileSecurityService::new(runtime_root)
                .authorize_mutation(
                    &AuthToken {
                        token: token.clone(),
                    },
                    &command,
                )
                .is_ok()
            {
                return true;
            }
        }
        match &self.auth_token {
            Some(expected) => request.auth_token.as_deref() == Some(expected.as_str()),
            None => self.runtime_root.is_none(),
        }
    }

    fn handle_session_create(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("create surface sessions");
        };
        let surface = request
            .body
            .as_ref()
            .and_then(|body| body.get("surface"))
            .and_then(|surface| surface.as_str())
            .and_then(parse_surface_kind)
            .unwrap_or(SurfaceKind::Browser);
        match FileSessionService::new(
            runtime_root,
            format!("http://127.0.0.1:{}", self.status.port),
        )
        .authenticate_local_surface(surface)
        {
            Ok(session) => ApiResponse::json(
                201,
                json!({
                    "session": session,
                    "local_url": format!("http://127.0.0.1:{}", self.status.port),
                    "api_contract_version": API_CONTRACT_VERSION
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_workflow_schedule(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("schedule work items");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("persist scheduler state");
        };
        let scheduler = FileSchedulingService::with_durable_root(runtime_root, durable_root);
        match scheduler.promote().and_then(|promoted| {
            scheduler
                .load_state()
                .map(|state| json!({"promoted": promoted, "reservations": state.reservations}))
        }) {
            Ok(body) => ApiResponse::json(200, body),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_transition(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("mutate work items");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/transition"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Gap transition route requires a Gap id"
                    }
                }),
            );
        };
        let Some(status) = request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
            .and_then(GapStatus::parse_wire)
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_status",
                        "message": "body.status must be a valid Gap status"
                    }
                }),
            );
        };

        match FileWorkItemService::new(durable_root).transition_gap_status(gap_id, status) {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_action(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("mutate work items");
        };
        let Some((gap_id, action)) = gap_id_and_action(&request.path) else {
            return gap_id_required();
        };
        let service = FileWorkItemService::new(durable_root);
        let result = match action {
            "verify" => service.verify_gap_summary(gap_id),
            "retry-quality" => service.retry_gap_quality_summary(gap_id),
            "retry-merge" => service.retry_gap_merge_summary(gap_id),
            "merge" => service.merge_gap_summary(gap_id),
            "undo" => service.undo_gap_summary(gap_id),
            _ => {
                return ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": "unknown Gap action"
                        }
                    }),
                );
            }
        };
        match result {
            Ok(gap) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "message": gap_action_message(action),
                    "gap": gap.gap
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_create(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("create work items");
        };
        let Some(name) = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name is required"
                    }
                }),
            );
        };
        let id = request
            .body
            .as_ref()
            .and_then(|body| body.get("id"))
            .and_then(|id| id.as_str());

        match FileWorkItemService::new(durable_root).create_gap_summary(name, id) {
            Ok(gap) => ApiResponse::json(201, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_bulk_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("bulk update work items");
        };
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        let Some(update) = parse_bulk_gap_update(body) else {
            return invalid_bulk_body();
        };
        match FileWorkItemService::new(durable_root).bulk_update_gaps(selection, update) {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_bulk_delete(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("bulk delete work items");
        };
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match FileWorkItemService::new(durable_root).bulk_delete_gaps(selection) {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_create(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("create features");
        };
        let Some(name) = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name is required"
                    }
                }),
            );
        };
        let id = request
            .body
            .as_ref()
            .and_then(|body| body.get("id"))
            .and_then(|id| id.as_str());
        let description = request
            .body
            .as_ref()
            .and_then(|body| body.get("description"))
            .and_then(|description| description.as_str());
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|reporter| reporter.as_str());
        match FileWorkItemService::new(durable_root).create_feature_summary(
            name,
            id,
            description,
            reporter,
        ) {
            Ok(feature) => ApiResponse::json(
                201,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("update features");
        };
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileWorkItemService::new(durable_root).update_feature_metadata_summary(
            feature_id,
            body.get("name").and_then(|value| value.as_str()),
            body.get("description").and_then(|value| value.as_str()),
            body.get("reporter").and_then(|value| value.as_str()),
        ) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_bulk_assign_gaps(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("bulk assign Gaps to Features");
        };
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/gaps/bulk"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let Some(body) = request.body.as_ref() else {
            return invalid_bulk_body();
        };
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match FileWorkItemService::new(durable_root)
            .bulk_assign_gaps_to_feature(feature_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("update work items");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|name| name.as_str());
        let priority = request
            .body
            .as_ref()
            .and_then(|body| body.get("priority"))
            .and_then(|priority| priority.as_str());
        match FileWorkItemService::new(durable_root)
            .update_gap_metadata_summary(gap_id, name, priority)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_note(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("edit work items");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/notes"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return gap_id_required();
        };
        let Some(body) = request
            .body
            .as_ref()
            .and_then(|body| body.get("body"))
            .and_then(|body| body.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_note",
                        "message": "body.body is required"
                    }
                }),
            );
        };
        let author = request
            .body
            .as_ref()
            .and_then(|body| body.get("author"))
            .and_then(|author| author.as_str())
            .unwrap_or("");
        match FileWorkItemService::new(durable_root).add_gap_note_summary(gap_id, author, body) {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_round_append(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("append Gap rounds");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/rounds"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return gap_id_required();
        };
        let Some(reporter) = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let Some(actual) = request
            .body
            .as_ref()
            .and_then(|body| body.get("actual"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        let Some(target) = request
            .body
            .as_ref()
            .and_then(|body| body.get("target"))
            .and_then(|value| value.as_str())
        else {
            return invalid_round_body();
        };
        match FileWorkItemService::new(durable_root)
            .append_gap_round_summary(gap_id, reporter, actual, target)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_round_edit_latest(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("edit latest Gap round");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/rounds/latest"))
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return gap_id_required();
        };
        let reporter = request
            .body
            .as_ref()
            .and_then(|body| body.get("reporter"))
            .and_then(|value| value.as_str());
        let actual = request
            .body
            .as_ref()
            .and_then(|body| body.get("actual"))
            .and_then(|value| value.as_str());
        let target = request
            .body
            .as_ref()
            .and_then(|body| body.get("target"))
            .and_then(|value| value.as_str());
        match FileWorkItemService::new(durable_root)
            .edit_latest_gap_round_summary(gap_id, reporter, actual, target)
        {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_round_log_append(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("append Gap round logs");
        };
        let Some(rest) = request.path.strip_prefix("/work/gaps/") else {
            return gap_id_required();
        };
        let Some((gap_id, round_part)) = rest.split_once("/rounds/") else {
            return gap_id_required();
        };
        let Some(round_idx) = round_part
            .strip_suffix("/logs")
            .and_then(|value| value.parse::<usize>().ok())
        else {
            return ApiResponse::json(
                400,
                json!({"error": {"code": "invalid_round", "message": "round index is required"}}),
            );
        };
        let gap = match FileWorkItemService::new(durable_root).show_gap_summary(gap_id) {
            Ok(gap) => gap,
            Err(error) => return error_response(error),
        };
        if round_idx >= gap.gap.round_count {
            return ApiResponse::json(
                404,
                json!({"error": {"code": "not_found", "message": "Round not found"}}),
            );
        }
        let body = request.body.unwrap_or_else(|| json!({}));
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return ApiResponse::json(
                400,
                json!({"error": {"code": "invalid_log", "message": "log message is required"}}),
            );
        }
        let entry = LogEntry {
            datetime: body
                .get("datetime")
                .and_then(|datetime| datetime.as_str())
                .unwrap_or("")
                .to_string(),
            severity: body
                .get("severity")
                .and_then(|severity| severity.as_str())
                .unwrap_or("info")
                .to_string(),
            category: body
                .get("category")
                .and_then(|category| category.as_str())
                .unwrap_or("state")
                .to_string(),
            message: message.to_string(),
            details: body
                .get("details")
                .and_then(|details| details.as_object())
                .cloned(),
            actions: Vec::new(),
            actor: body
                .get("actor")
                .and_then(|actor| actor.as_str())
                .map(str::to_string),
            gap_id: Some(gap_id.to_string()),
        };
        match FileLogService::new(durable_root).append_round_log(gap_id, round_idx, entry) {
            Ok(log) => ApiResponse::json(
                200,
                json!({"log": log, "gap_id": gap_id, "round_idx": round_idx}),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_logs(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("read Gap round logs");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        let gap = match FileWorkItemService::new(durable_root).show_gap_summary(gap_id) {
            Ok(gap) => gap,
            Err(error) => return error_response(error),
        };
        if gap.gap.round_count == 0 {
            return ApiResponse::json(
                404,
                json!({"error": {"code": "not_found", "message": "Round not found"}}),
            );
        }
        let round_idx = 0;
        match FileLogService::new(durable_root).page_round_logs(gap_id, round_idx, 50, 0) {
            Ok((logs, has_more, total)) => ApiResponse::json(
                200,
                json!({
                    "gap_id": gap_id,
                    "round_idx": round_idx,
                    "logs": logs,
                    "pagination": {
                        "limit": 50,
                        "offset": 0,
                        "total": total,
                        "has_more": has_more
                    },
                    "round_log_count": total,
                    "activity_count": 0
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_delete(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("delete work items");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        match FileWorkItemService::new(durable_root).delete_gap_record(gap_id) {
            Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": gap_id})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_cancel(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("cancel work items");
        };
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|gap_id| !gap_id.is_empty() && !gap_id.contains('/'))
        else {
            return gap_id_required();
        };
        match FileWorkItemService::new(durable_root).cancel_gap_summary(gap_id) {
            Ok(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_show(&self, request: ApiRequest) -> ApiResponse {
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        match self.current_projection() {
            Ok(projection) => match projection.features.get(feature_id) {
                Some(feature) => ApiResponse::json(
                    200,
                    json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
                ),
                None => ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": format!("Feature {feature_id} was not found")
                        }
                    }),
                ),
            },
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_add_gap(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("assign Gaps to Features");
        };
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/gaps"))
            .filter(|feature_id| !feature_id.is_empty())
        else {
            return feature_id_required();
        };
        let Some(gap_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("gap_id"))
            .and_then(|gap_id| gap_id.as_str())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_gap_id",
                        "message": "body.gap_id is required"
                    }
                }),
            );
        };
        match FileWorkItemService::new(durable_root).assign_gap_to_feature(feature_id, gap_id) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_add_gap_path(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("assign Gaps to Features");
        };
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, gap_part)) = rest.split_once("/gaps/") else {
            return feature_id_required();
        };
        let gap_id = gap_part;
        if feature_id.is_empty() || gap_id.is_empty() || gap_id.contains('/') {
            return feature_id_required();
        }
        match FileWorkItemService::new(durable_root).assign_gap_to_feature(feature_id, gap_id) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_remove_gap(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("remove Gaps from Features");
        };
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, gap_part)) = rest.split_once("/gaps/") else {
            return feature_id_required();
        };
        let gap_id = gap_part;
        if feature_id.is_empty() || gap_id.is_empty() || gap_id.contains('/') {
            return feature_id_required();
        }
        match FileWorkItemService::new(durable_root).remove_gap_from_feature(feature_id, gap_id) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_reorder_gap(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("reorder Feature Gaps");
        };
        let Some(rest) = request.path.strip_prefix("/work/features/") else {
            return feature_id_required();
        };
        let Some((feature_id, gap_part)) = rest.split_once("/gaps/") else {
            return feature_id_required();
        };
        let Some(gap_id) = gap_part.strip_suffix("/reorder") else {
            return gap_id_required();
        };
        let Some(order) = request
            .body
            .as_ref()
            .and_then(|body| body.get("order"))
            .and_then(|order| order.as_i64())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_order",
                        "message": "body.order is required"
                    }
                }),
            );
        };
        match FileWorkItemService::new(durable_root)
            .reorder_gap_in_feature(feature_id, gap_id, order)
        {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_move(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("move Feature workflow");
        };
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/move"))
            .filter(|feature_id| !feature_id.is_empty())
        else {
            return feature_id_required();
        };
        let Some(target) = request
            .body
            .as_ref()
            .and_then(|body| body.get("status"))
            .and_then(|status| status.as_str())
            .and_then(GapStatus::parse_wire)
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_status",
                        "message": "body.status must be backlog or todo"
                    }
                }),
            );
        };
        match FileWorkItemService::new(durable_root).move_feature_workflow(feature_id, target) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({"feature": feature.feature, "gap_ids": feature.gap_ids, "rollup": feature.rollup}),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_cancel(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("cancel Features");
        };
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        let gap_ids = match self.current_projection() {
            Ok(projection) => projection
                .features
                .get(feature_id)
                .map(|feature| feature.gap_ids.clone())
                .unwrap_or_default(),
            Err(error) => return error_response(error),
        };
        let runtime_reconciled = match self.reconcile_feature_runtime_work(feature_id, &gap_ids) {
            Ok(summary) => summary,
            Err(error) => return error_response(error),
        };
        match FileWorkItemService::new(durable_root).cancel_feature_summary(feature_id) {
            Ok(feature) => ApiResponse::json(
                200,
                json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup,
                    "runtime_reconciled": {
                        "processes": runtime_reconciled.processes,
                        "jobs": runtime_reconciled.jobs
                    }
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_feature_delete(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("delete Features");
        };
        let Some(feature_id) = request
            .path
            .strip_prefix("/work/features/")
            .filter(|feature_id| !feature_id.is_empty() && !feature_id.contains('/'))
        else {
            return feature_id_required();
        };
        match FileWorkItemService::new(durable_root).delete_feature_record(feature_id) {
            Ok(()) => ApiResponse::json(200, json!({"deleted": true, "id": feature_id})),
            Err(error) => error_response(error),
        }
    }

    fn handle_gap_show(&self, request: ApiRequest) -> ApiResponse {
        let Some(gap_id) = request
            .path
            .strip_prefix("/work/gaps/")
            .filter(|gap_id| !gap_id.is_empty())
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Gap route requires a Gap id"
                    }
                }),
            );
        };
        match self.current_projection() {
            Ok(projection) => match projection.gaps.get(gap_id) {
                Some(gap) => ApiResponse::json(200, json!({"gap": gap.gap})),
                None => ApiResponse::json(
                    404,
                    json!({
                        "error": {
                            "code": "not_found",
                            "message": format!("Gap {gap_id} was not found")
                        }
                    }),
                ),
            },
            Err(error) => error_response(error),
        }
    }

    fn handle_gaps_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let page = bounded_query_usize(raw_path, "page", 1, usize::MAX).max(1);
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| (page - 1).saturating_mul(limit));
        let query = GapProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "updated".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            status: query_param(raw_path, "status").and_then(|value| GapStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            node: query_param(raw_path, "node"),
            current_node_id: Some("default".to_string()),
            feature: query_param(raw_path, "feature"),
            rounds_gte: query_param(raw_path, "rounds_gte")
                .and_then(|value| value.parse::<usize>().ok()),
            rounds_lte: query_param(raw_path, "rounds_lte")
                .and_then(|value| value.parse::<usize>().ok()),
            severity: query_param(raw_path, "severity"),
            category: query_param(raw_path, "category"),
            actor: query_param(raw_path, "actor"),
        };
        let result = projection.list_gaps(query);
        ApiResponse::json(
            200,
            json!({
                "gaps": result.gaps,
                "counts": projection.status_counts(),
                "filtered_counts": result.filtered_status_counts,
                "matching_ids": result.matching_ids,
                "projection_version": projection.version,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "page": page,
                    "total": result.total,
                    "has_more": offset + limit < result.total
                }
            }),
        )
    }

    fn handle_features_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let page = bounded_query_usize(raw_path, "page", 1, usize::MAX).max(1);
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| (page - 1).saturating_mul(limit));
        let query = FeatureProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "updated".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            status: query_param(raw_path, "status").and_then(|value| GapStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            node: query_param(raw_path, "node"),
            current_node_id: Some("default".to_string()),
        };
        let result = projection.list_features(query);
        let features: Vec<_> = result
            .features
            .into_iter()
            .map(|feature| {
                json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                })
            })
            .collect();
        ApiResponse::json(
            200,
            json!({
                "features": features,
                "matching_ids": result.matching_ids,
                "projection_version": projection.version,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "page": page,
                    "total": result.total,
                    "has_more": offset + limit < result.total
                }
            }),
        )
    }

    fn handle_activity_list(&self, raw_path: &str) -> ApiResponse {
        if self.durable_root.is_none() {
            return durable_root_unavailable("read activity");
        }
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        let result = projection.list_activity(ActivityProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "datetime".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            gap_id: query_param(raw_path, "gap_id"),
            severity: query_param(raw_path, "severity"),
            category: query_param(raw_path, "category"),
            actor: query_param(raw_path, "actor"),
            q: query_param(raw_path, "q"),
        });
        ApiResponse::json(
            200,
            json!({
                "activity": result.activity,
                "facets": result.facets,
                "matching_ids": result.matching_ids,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "has_more": offset + limit < result.total,
                    "total": result.total
                }
            }),
        )
    }

    fn handle_activity_ui_error(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("record UI activity");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let message = body
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("UI error")
            .trim();
        let service = FileActivityService::new(durable_root);
        let mut entry = service.new_entry(
            if message.is_empty() {
                "UI error"
            } else {
                message
            },
            "error",
            "ui",
            body.get("gap_id")
                .and_then(|gap_id| gap_id.as_str())
                .map(str::to_string),
            Some("browser".to_string()),
        );
        if let Some(details) = body.as_object() {
            entry.details = Some(details.clone());
        }
        match service.append(entry.clone()) {
            Ok(()) => ApiResponse::json(200, json!({"recorded": true, "entry": entry})),
            Err(error) => error_response(error),
        }
    }

    fn handle_activity_cleanup(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("clean up activity");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let days = body
            .get("days")
            .and_then(|value| value.as_i64())
            .unwrap_or(7);
        let clear = body
            .get("clear")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || days == 0;
        let service = FileActivityService::new(durable_root);
        match service.cleanup(days, clear) {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "deleted": result.deleted,
                    "retained": result.retained,
                    "cleared": result.cleared,
                    "retention_days": days
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_changes_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        let result = projection.list_changes(ChangeProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "committed".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            gap_id: query_param(raw_path, "gap_id"),
            status: query_param(raw_path, "status")
                .and_then(|status| GapStatus::parse_wire(&status)),
            priority: query_param(raw_path, "priority"),
            branch: query_param(raw_path, "branch"),
        });
        let branch = result
            .changes
            .iter()
            .find_map(|change| change.branch.clone())
            .unwrap_or_else(|| "main".to_string());
        let changes = result
            .changes
            .iter()
            .map(|change| {
                json!({
                    "commit": change.commit,
                    "gap_id": change.gap_id,
                    "name": change.gap_name,
                    "status": change.gap_status,
                    "priority": change.gap_priority,
                    "committed": change.committed_time,
                    "subject": change.subject,
                    "branch": change.branch
                })
            })
            .collect::<Vec<_>>();
        ApiResponse::json(
            200,
            json!({
                "branch": branch,
                "changes": changes,
                "matching_ids": result.matching_ids,
                "page": {
                    "limit": limit,
                    "offset": offset,
                    "has_more": offset + limit < result.total,
                    "total": result.total
                }
            }),
        )
    }

    fn handle_changes_undo(&self, request: ApiRequest) -> ApiResponse {
        let commit = request
            .body
            .as_ref()
            .and_then(|body| body.get("commit"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if commit.is_empty() {
            return ApiResponse::json(
                400,
                json!({
                    "ok": false,
                    "error": {
                        "code": "invalid_input",
                        "message": "body.commit is required"
                    }
                }),
            );
        }
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("undo Git changes");
        };
        match FileGitWorktreeService::new(source_root).revert_commit(commit) {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "pushed": false,
                    "commit": commit,
                    "conflicts": result.conflicts,
                    "message": result.message.unwrap_or_default()
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_cache_rebuild(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("rebuild projection cache");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("rebuild projection cache");
        };
        let store = FileProjectStateStore::new(durable_root);
        let projection = match store.rebuild_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let cache_dir = runtime_root.join("cache");
        if let Err(error) = store.persist_projection_snapshot(&cache_dir, &projection) {
            return error_response(error);
        }
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "mode": "rebuilt",
                "gaps": projection.gaps.len(),
                "features": projection.features.len(),
                "projection_version": projection.version,
                "cache": cache_dir.join(PROJECTION_SNAPSHOT_FILE).display().to_string()
            }),
        )
    }

    fn handle_performance_list(&self, raw_path: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read performance metrics");
        };
        let query = PerformanceQuery {
            limit: bounded_query_usize(raw_path, "limit", 50, 1000),
            offset: bounded_query_usize(raw_path, "offset", 0, usize::MAX),
            operation: query_param(raw_path, "operation").filter(|value| !value.is_empty()),
            success: query_param(raw_path, "success").and_then(|value| match value.as_str() {
                "1" | "true" | "True" | "TRUE" => Some(true),
                "0" | "false" | "False" | "FALSE" => Some(false),
                _ => None,
            }),
        };
        match performance_report_value(runtime_root, query) {
            Ok(value) => {
                let response = ApiResponse::json(200, value.clone());
                if let Some(performance) = value.as_object().cloned() {
                    let _ = self.persist_runtime_projection_override(|runtime| {
                        runtime.performance = Some(performance);
                    });
                }
                response
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_performance_cleanup(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("clean up performance metrics");
        };
        let clear = request
            .body
            .as_ref()
            .and_then(|body| body.get("clear"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let service = FileMetricsService::new(runtime_root);
        match service.cleanup(clear) {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "deleted": result.deleted,
                    "retained": result.retained,
                    "cleared": result.cleared,
                    "retention_days": service.retention_days
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_files_tree(&self, raw_path: &str) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("read source files");
        };
        let path = query_param(raw_path, "path").unwrap_or_default();
        let recursive = query_param(raw_path, "recursive")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let max_depth = query_param(raw_path, "max_depth")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .min(8);
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(200)
            .clamp(1, 1000);
        match files_tree_response(&source_root, &path, recursive, max_depth, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_files_read(&self, raw_path: &str) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("read source file");
        };
        let path = query_param(raw_path, "path").unwrap_or_default();
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param(raw_path, "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(128_000)
            .clamp(1, 512_000);
        match files_read_response(&source_root, &path, offset, limit) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_files_search(&self, raw_path: &str) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("search source files");
        };
        let query = query_param(raw_path, "q").unwrap_or_default();
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20)
            .clamp(1, 200);
        match files_search_response(&source_root, &query, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_merger_hard_reset_worktree(&self) -> ApiResponse {
        let Some(source_root) = self.source_root() else {
            return durable_root_unavailable("hard-reset Git worktree");
        };
        match FileGitWorktreeService::new(source_root).hard_reset() {
            Ok(result) => ApiResponse::json(
                200,
                json!({
                    "ok": result.ok,
                    "conflicts": result.conflicts,
                    "message": result.message.unwrap_or_default()
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_import_extract(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let text = body_text(&body);
        match FileImportService::new(PathBuf::new())
            .parse_text(text, body.get("reporter").and_then(|value| value.as_str()))
        {
            Ok(drafts) => ApiResponse::json(200, json!({"drafts": drafts})),
            Err(error) => error_response(error),
        }
    }

    fn handle_import_csv_parse(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileImportService::new(PathBuf::new()).parse_csv(
            body_text(&body),
            body.get("reporter").and_then(|value| value.as_str()),
        ) {
            Ok(drafts) => ApiResponse::json(200, json!({"drafts": drafts})),
            Err(error) => error_response(error),
        }
    }

    fn handle_import_dedup(&self, request: ApiRequest) -> ApiResponse {
        let Some(body) = request.body.as_ref() else {
            return error_response(RefineError::InvalidInput(
                "body.drafts must be an array".to_string(),
            ));
        };
        let drafts = match import_drafts_from_value(body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let mut matches = Vec::new();
        for (index, draft) in drafts.iter().enumerate() {
            let needle = normalized_dedup_text(&[
                draft.name.as_str(),
                draft.actual.as_str(),
                draft.target.as_str(),
            ]);
            if needle.is_empty() {
                continue;
            }
            if let Some(existing) = projection.gaps.values().find(|gap| {
                let haystack = normalized_dedup_text(&[
                    gap.gap.name.as_str(),
                    gap.searchable_text.as_str(),
                    gap.gap.id.as_str(),
                ]);
                haystack == needle || (!haystack.is_empty() && haystack.contains(&needle))
            }) {
                matches.push(json!({
                    "index": index + 1,
                    "score": 1.0,
                    "match": {
                        "id": existing.gap.id,
                        "name": existing.gap.name,
                        "status": existing.gap.status,
                        "priority": existing.gap.priority,
                        "reporter": existing.gap.reporter
                    }
                }));
            }
        }
        ApiResponse::json(
            200,
            json!({
                "matches": matches,
                "threshold": 1.0,
                "algorithm": "normalized_exact"
            }),
        )
    }

    fn handle_import_persist(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("persist imported Gaps");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let drafts = match import_drafts_from_value(&body, None) {
            Ok(drafts) => drafts,
            Err(error) => return error_response(error),
        };
        let service = FileWorkItemService::new(durable_root);
        let mut failures = Vec::new();
        let mut feature_response = serde_json::Value::Null;
        let feature_id = match import_destination_feature_id(&service, &body) {
            Ok(feature) => {
                feature_response = feature
                    .as_ref()
                    .map(feature_import_response)
                    .unwrap_or(serde_json::Value::Null);
                feature.map(|feature| feature.feature.id)
            }
            Err(error) => {
                failures.push(json!({
                    "index": 0,
                    "name": "feature",
                    "message": error.to_string()
                }));
                None
            }
        };
        let import_result = if failures.is_empty() {
            match FileImportService::new(durable_root).persist(drafts, feature_id.as_deref()) {
                Ok(result) => Some(result),
                Err(error) => {
                    failures.push(json!({
                        "index": 0,
                        "name": "import",
                        "message": error.to_string()
                    }));
                    None
                }
            }
        } else {
            None
        };
        let created = import_result
            .as_ref()
            .map(|result| {
                result
                    .gap_ids
                    .iter()
                    .filter_map(|gap_id| service.show_gap_summary(gap_id).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(feature_id) = feature_id.as_deref() {
            if let Ok(feature) = service.show_feature_summary(feature_id) {
                feature_response = feature_import_response(&feature);
            }
        }

        ApiResponse::json(
            if failures.is_empty() { 201 } else { 207 },
            json!({
                "ok": failures.is_empty(),
                "count": created.len(),
                "created": created,
                "gaps": created.iter().map(|gap| &gap.gap).collect::<Vec<_>>(),
                "failures": failures,
                "duplicate_actions": {},
                "feature": feature_response
            }),
        )
    }

    fn handle_job_status(&self, request: ApiRequest) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read background jobs");
        }
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        match self.current_projection_with_runtime() {
            Ok(projection) => projection
                .runtime
                .background_jobs
                .into_iter()
                .find(|job| job.get("id").and_then(|value| value.as_str()) == Some(job_id))
                .map(|job| ApiResponse::json(200, json!({"job": job})))
                .unwrap_or_else(|| {
                    error_response(RefineError::NotFound(format!("Job {job_id} was not found")))
                }),
            Err(error) => error_response(error),
        }
    }

    fn handle_job_logs(&self, request: ApiRequest, raw_path: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read background job logs");
        };
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 200);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        match FileJobRegistry::new(runtime_root).page_logs(job_id, limit, offset) {
            Ok((logs, has_more, total)) => {
                let log_count = logs.len();
                ApiResponse::json(
                    200,
                    json!({
                        "job_id": job_id,
                        "logs": logs,
                        "log_count": log_count,
                        "has_more": has_more,
                        "total": total,
                        "page": {
                            "limit": limit,
                            "offset": offset,
                            "has_more": has_more,
                            "total": total
                        }
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_job_cancel(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("cancel background jobs");
        };
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        match FileJobRegistry::new(runtime_root).cancel(job_id) {
            Ok(job) => {
                let job = job_response(job);
                if let Err(error) = self.current_projection_with_runtime() {
                    return error_response(error);
                }
                ApiResponse::json(200, json!({"job": job}))
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_job_retry(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("retry background jobs");
        };
        let Some(job_id) = request
            .path
            .strip_prefix("/jobs/")
            .and_then(|path| path.strip_suffix("/retry"))
            .filter(|job_id| !job_id.is_empty() && !job_id.contains('/'))
        else {
            return job_id_required();
        };
        let scheduler = if let Some(durable_root) = &self.durable_root {
            FileSchedulingService::with_durable_root(runtime_root, durable_root)
        } else {
            FileSchedulingService::new(runtime_root)
        };
        match scheduler.retry(job_id) {
            Ok(retried_job_id) => {
                match FileJobRegistry::new(runtime_root).status(&retried_job_id) {
                    Ok(job) => {
                        let job = job_response(job);
                        if let Err(error) = self.current_projection_with_runtime() {
                            return error_response(error);
                        }
                        ApiResponse::json(
                            200,
                            json!({
                                "retried_from": job_id,
                                "job": job
                            }),
                        )
                    }
                    Err(error) => error_response(error),
                }
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_processes(&self) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read managed processes");
        }
        match self.current_projection_with_runtime() {
            Ok(projection) => {
                ApiResponse::json(200, runtime_process_summary_value(&projection.runtime))
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_process_stream(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("stream managed process output");
        };
        let Some(process_id) = request
            .path
            .strip_prefix("/processes/")
            .and_then(|path| path.strip_suffix("/stream"))
            .filter(|process_id| !process_id.is_empty() && !process_id.contains('/'))
        else {
            return process_id_required();
        };
        match FileProcessSupervisor::new(runtime_root).stream(process_id) {
            Ok(output) => ApiResponse::json(
                200,
                json!({
                    "process_id": process_id,
                    "output": output,
                    "backend": {
                        "process_model": "supervisor"
                    }
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_processes_background(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control background processes");
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        let current = match supervisor.pause_state() {
            Ok(state) => state,
            Err(error) => return error_response(error),
        };
        let stopped = request
            .body
            .as_ref()
            .and_then(|body| body.get("stopped"))
            .and_then(|stopped| stopped.as_bool())
            .unwrap_or(!current.background_processes_stopped);
        match supervisor.set_background_processes_stopped(stopped) {
            Ok(_) => self.handle_processes(),
            Err(error) => error_response(error),
        }
    }

    fn handle_processes_agents(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("control agent processes");
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        let current = match supervisor.pause_state() {
            Ok(state) => state,
            Err(error) => return error_response(error),
        };
        let paused = request
            .body
            .as_ref()
            .and_then(|body| body.get("paused"))
            .and_then(|paused| paused.as_bool())
            .unwrap_or(!current.agents_paused);
        match supervisor.set_agents_paused(paused) {
            Ok(_) => self.handle_processes(),
            Err(error) => error_response(error),
        }
    }

    fn handle_install_status(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).status() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    fn handle_install(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("install Refine");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let target = parse_install_target(body.get("target").and_then(|value| value.as_str()));
        let version = body
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        match FileInstallationService::new(runtime_root, version).install(target) {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    fn handle_install_repair(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("repair install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).repair() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    fn handle_install_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("update install state");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(version) = body.get("version").and_then(|value| value.as_str()) else {
            return error_response(RefineError::InvalidInput("version is required".to_string()));
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).update(version)
        {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    fn handle_install_rollback(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("rollback install state");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).rollback() {
            Ok(status) => ApiResponse::json(200, json!({"install": status})),
            Err(error) => error_response(error),
        }
    }

    fn handle_install_uninstall(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("uninstall Refine");
        };
        match FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION")).uninstall() {
            Ok(()) => ApiResponse::json(200, json!({"uninstalled": true})),
            Err(error) => error_response(error),
        }
    }

    fn handle_agents(&self) -> ApiResponse {
        provider_status_response()
    }

    fn handle_agent_diagnostics(&self, request: ApiRequest) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "diagnostics") else {
            return provider_id_required();
        };
        match HostAgentProviderService::new().diagnose(provider) {
            Ok(diagnostics) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "diagnostics": diagnostics
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_agent_configure(&self, request: ApiRequest) -> ApiResponse {
        let Some(provider) = agent_provider_from_path(&request.path, "configure") else {
            return provider_id_required();
        };
        match HostAgentProviderService::new().configure(provider) {
            Ok(()) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "configured": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_agent_authenticate(&self, request: ApiRequest) -> ApiResponse {
        let suffix = if request.path.ends_with("/authenticate") {
            "authenticate"
        } else {
            "auth"
        };
        let Some(provider) = agent_provider_from_path(&request.path, suffix) else {
            return provider_id_required();
        };
        match HostAgentProviderService::new().authenticate(provider) {
            Ok(()) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "provider": provider,
                    "authenticated": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_recheck_auth(&self) -> ApiResponse {
        provider_status_response()
    }

    fn handle_diagnostics(&self) -> ApiResponse {
        let projection = match self.current_projection_with_runtime() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read diagnostics");
        };
        let repo_root = self
            .source_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let doctor = match FileDiagnosticsService::new(
            self.durable_root.clone(),
            runtime_root.clone(),
            repo_root,
        )
        .doctor()
        {
            Ok(report) => report,
            Err(error) => return error_response(error),
        };
        let provider = projection
            .runtime
            .preflight
            .clone()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"ok": false, "providers": []}));
        let process = runtime_process_summary_value(&projection.runtime);
        ApiResponse::json(
            200,
            json!({
                "reachable": true,
                "backend": {
                    "process_model": "supervisor",
                    "native": true
                },
                "provider": provider,
                "processes": process,
                "doctor": doctor
            }),
        )
    }

    fn handle_dashboard(&self) -> ApiResponse {
        let projection = match self.current_projection_with_runtime() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let activity = projection
            .dashboard
            .recent_activity_ids
            .iter()
            .filter_map(|activity_id| projection.activity.get(activity_id))
            .map(|activity| activity.entry.clone())
            .collect::<Vec<_>>();
        let process = runtime_process_summary_value(&projection.runtime);
        let preflight = projection
            .runtime
            .preflight
            .clone()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"ok": false, "providers": []}));
        ApiResponse::json(
            200,
            json!({
                "counts": projection.dashboard.current_node_status_counts,
                "all_node_counts": projection.dashboard.all_node_status_counts,
                "running": [],
                "merger": null,
                "governance": null,
                "preflight": preflight,
                "activity": activity,
                "runner_reachable": process.get("runner_reachable").and_then(|value| value.as_bool()).unwrap_or(false),
                "reporter_stats": projection.dashboard.reporter_stats,
                "node_scope": "current",
                "node_filter": "current",
                "quality_timing": self.quality_timing_setting(),
                "active_node_id": "default",
                "active_node_display_name": "Default",
                "needs_attention": projection.dashboard.attention_indicators,
                "attached": self.durable_root.is_some()
            }),
        )
    }

    fn handle_nodes(&self) -> ApiResponse {
        match self.nodes_response() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_node_create(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("create node");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let display_name = body
            .get("display_name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if display_name.is_empty() {
            return error_response(RefineError::InvalidInput(
                "display_name is required".to_string(),
            ));
        }
        let mut registry = match load_node_registry(durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let id = unique_node_id(&registry, display_name);
        let now = now_timestamp_web();
        registry.nodes.push(json!({
            "id": id,
            "display_name": display_name,
            "archived": false,
            "active": false,
            "created_at": now,
            "updated_at": now
        }));
        match save_node_registry(durable_root, &registry) {
            Ok(()) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    fn handle_node_activate(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("activate node");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let node_id = body
            .get("node_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        let registry = match load_node_registry(durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        if !registry
            .nodes
            .iter()
            .any(|node| node_id_value(node) == node_id && !node_archived(node))
        {
            return error_response(RefineError::NotFound(format!(
                "node {node_id} was not found or is archived"
            )));
        }
        match save_active_node_id(durable_root, node_id) {
            Ok(()) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    fn handle_node_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("update node");
        };
        let Some(node_id) = request
            .path
            .strip_prefix("/nodes/")
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput("node id is required".to_string()));
        };
        let mut registry = match load_node_registry(durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let active_node_id = match load_active_node_id(durable_root) {
            Ok(active) => active,
            Err(error) => return error_response(error),
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(node) = registry
            .nodes
            .iter_mut()
            .find(|node| node_id_value(node) == node_id)
        else {
            return error_response(RefineError::NotFound(format!(
                "node {node_id} was not found"
            )));
        };
        if let Some(display_name) = body.get("display_name").and_then(|value| value.as_str()) {
            let display_name = display_name.trim();
            if display_name.is_empty() {
                return error_response(RefineError::InvalidInput(
                    "display_name cannot be empty".to_string(),
                ));
            }
            node["display_name"] = json!(display_name);
        }
        if let Some(archived) = body.get("archived").and_then(|value| value.as_bool()) {
            if archived && node_id == active_node_id {
                return error_response(RefineError::Conflict(
                    "active node cannot be archived".to_string(),
                ));
            }
            node["archived"] = json!(archived);
        }
        node["updated_at"] = json!(now_timestamp_web());
        match save_node_registry(durable_root, &registry) {
            Ok(()) => self.handle_nodes(),
            Err(error) => error_response(error),
        }
    }

    fn handle_node_transfer_gaps(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("transfer Gaps to node");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let target_node_id = body
            .get("target_node_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        let registry = match load_node_registry(durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        if !registry
            .nodes
            .iter()
            .any(|node| node_id_value(node) == target_node_id && !node_archived(node))
        {
            return error_response(RefineError::NotFound(format!(
                "node {target_node_id} was not found or is archived"
            )));
        }
        let selection = match serde_json::from_value::<BulkGapSelection>(body.clone()) {
            Ok(selection) => selection,
            Err(_) => return invalid_bulk_body(),
        };
        match FileWorkItemService::new(durable_root)
            .bulk_transfer_gaps_to_node(target_node_id, selection)
        {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    fn handle_node_copy_settings(&self, request: ApiRequest) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "source_node_id": body.get("source_node_id").and_then(|value| value.as_str()).unwrap_or(""),
                "section": body.get("section").and_then(|value| value.as_str()).unwrap_or(""),
                "copied_count": 0,
                "message": "node-scoped settings copy has no native per-node settings to copy yet"
            }),
        )
    }

    fn handle_cluster(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return ApiResponse::json(
                200,
                json!({
                    "nodes": [],
                    "maintenance": null,
                    "enabled": false,
                    "message": "No cluster nodes configured."
                }),
            );
        };
        match load_cluster_registry(durable_root) {
            Ok(registry) => ApiResponse::json(200, cluster_response(registry)),
            Err(error) => error_response(error),
        }
    }

    fn handle_cluster_node_upsert(
        &self,
        request: ApiRequest,
        path_id: Option<String>,
    ) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("configure cluster node");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let id = path_id
            .or_else(|| {
                body.get("id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let id = id.trim();
        if !valid_cluster_node_id(id) {
            return error_response(RefineError::InvalidInput(
                "cluster node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            ));
        }
        let mut registry = match load_cluster_registry(durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let existing_index = registry
            .nodes
            .iter()
            .position(|node| cluster_node_id_value(node) == id);
        let mut node = existing_index
            .and_then(|index| registry.nodes.get(index).cloned())
            .unwrap_or_else(|| default_cluster_node(id));
        if let Some(display_name) = body.get("display_name").and_then(|value| value.as_str()) {
            node["display_name"] = json!(display_name.trim());
        }
        if let Some(ssh_host) = body.get("ssh_host").and_then(|value| value.as_str()) {
            let ssh_host = ssh_host.trim();
            if !valid_ssh_host(ssh_host) {
                return error_response(RefineError::InvalidInput(
                    "ssh_host must be a host without user@ prefix".to_string(),
                ));
            }
            node["ssh_host"] = json!(ssh_host);
        }
        for (field, default_value) in [("ssh_port", 22_u64), ("refine_port", 8080_u64)] {
            if let Some(port) = body.get(field).and_then(|value| value.as_u64()) {
                node[field] = json!(if port == 0 { default_value } else { port });
            }
        }
        for field in ["refine_checkout", "target_app_path"] {
            if let Some(value) = body.get(field).and_then(|value| value.as_str()) {
                node[field] = json!(value.trim());
            }
        }
        if let Some(enabled) = body.get("enabled").and_then(|value| value.as_bool()) {
            node["enabled"] = json!(enabled);
        }
        node["updated_at"] = json!(now_timestamp_web());
        if let Some(index) = existing_index {
            registry.nodes[index] = node;
        } else {
            registry.nodes.push(node);
        }
        registry.updated_at = now_timestamp_web();
        match save_cluster_registry(durable_root, &registry) {
            Ok(()) => ApiResponse::json(200, cluster_response(registry)),
            Err(error) => error_response(error),
        }
    }

    fn handle_cluster_node_bootstrap(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("bootstrap cluster node");
        };
        let Some(node_id) = request
            .path
            .strip_prefix("/cluster/nodes/")
            .and_then(|path| path.strip_suffix("/bootstrap"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "cluster node id is required".to_string(),
            ));
        };
        let mut registry = match load_cluster_registry(durable_root) {
            Ok(registry) => registry,
            Err(error) => return error_response(error),
        };
        let Some(index) = registry
            .nodes
            .iter()
            .position(|node| cluster_node_id_value(node) == node_id)
        else {
            return error_response(RefineError::NotFound(format!(
                "cluster node {node_id} was not found"
            )));
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let node = registry.nodes[index].clone();
        let dry_run = body
            .get("dry_run")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let result = match bootstrap_remote_node(ClusterBootstrapRequest {
            node_id: node_id.to_string(),
            ssh_host: node
                .get("ssh_host")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            ssh_port: node
                .get("ssh_port")
                .and_then(|value| value.as_u64())
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(22),
            refine_checkout: node
                .get("refine_checkout")
                .and_then(|value| value.as_str())
                .unwrap_or("~/refine")
                .to_string(),
            target_app_path: node
                .get("target_app_path")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            refine_port: node
                .get("refine_port")
                .and_then(|value| value.as_u64())
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(8080),
            dry_run,
        }) {
            Ok(result) => result,
            Err(error) => return error_response(error),
        };
        registry.nodes[index]["health"] = json!({
            "status": if result.ok { "ready" } else { "failed" },
            "checked_at": now_timestamp_web(),
            "details": {
                "bootstrap": result
            }
        });
        registry.updated_at = now_timestamp_web();
        if let Err(error) = save_cluster_registry(durable_root, &registry) {
            return error_response(error);
        }
        let result = registry.nodes[index]
            .get("health")
            .and_then(|health| health.get("details"))
            .and_then(|details| details.get("bootstrap"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        ApiResponse::json(
            200,
            json!({
                "ok": result.get("ok").and_then(|value| value.as_bool()).unwrap_or(false),
                "node_id": node_id,
                "dry_run": dry_run,
                "result": result,
                "cluster": cluster_response(registry)
            }),
        )
    }

    fn nodes_response(&self) -> RefineResult<serde_json::Value> {
        let Some(durable_root) = &self.durable_root else {
            return Ok(json!({
                "active_node_id": "default",
                "active_node": "Default",
                "nodes": [default_node("default", "Default", true)],
                "counts": {}
            }));
        };
        let registry = load_node_registry(durable_root)?;
        let active_node_id = load_active_node_id(durable_root)?;
        let projection = self.current_projection()?;
        let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
        for gap in projection.gaps.values() {
            let node_id = gap.gap.node_id.as_deref().unwrap_or("default").to_string();
            *counts
                .entry(node_id)
                .or_default()
                .entry(gap.gap.status.as_str().to_string())
                .or_insert(0) += 1;
        }
        let nodes: Vec<_> = registry
            .nodes
            .iter()
            .map(|node| {
                let mut node = node.clone();
                node["active"] = json!(node_id_value(&node) == active_node_id);
                node
            })
            .collect();
        let active_node = nodes
            .iter()
            .find(|node| node_id_value(node) == active_node_id)
            .and_then(|node| node.get("display_name"))
            .and_then(|value| value.as_str())
            .unwrap_or("Default");
        Ok(json!({
            "active_node_id": active_node_id,
            "active_node": active_node,
            "nodes": nodes,
            "counts": counts
        }))
    }

    fn handle_target_app_status(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.status())
        {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    fn handle_target_app_health(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.health())
        {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    fn handle_target_app_action(&self, request: ApiRequest) -> ApiResponse {
        let kind = request
            .path
            .strip_prefix("/target-app/")
            .unwrap_or("status")
            .to_string();
        let result = match self.target_app_service() {
            Ok(service) => match kind.as_str() {
                "start" => service.start(),
                "stop" => service.stop(),
                "rebuild" => service.rebuild(),
                _ => Err(RefineError::InvalidInput(format!(
                    "unknown target-app action {kind}"
                ))),
            },
            Err(error) => Err(error),
        };
        match result {
            Ok(snapshot) => ApiResponse::json(200, self.target_app_response(snapshot)),
            Err(error) => error_response(error),
        }
    }

    fn handle_target_app_generate_instructions(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.generate_config())
        {
            Ok(config) => {
                let settings = json!({
                    "target_app_start_command": config.start_command.clone(),
                    "target_app_stop_command": config.stop_command.clone(),
                    "target_app_rebuild_command": config.rebuild_command.clone(),
                    "target_app_status_command": config.status_command.clone(),
                    "target_app_cwd": config.cwd.clone(),
                    "target_app_env_json": serde_json::to_string_pretty(&config.env).unwrap_or_else(|_| "{}".to_string()),
                    "target_app_start_timeout_seconds": config.start_timeout_seconds.to_string(),
                    "target_app_stop_timeout_seconds": config.stop_timeout_seconds.to_string(),
                    "target_app_rebuild_timeout_seconds": config.rebuild_timeout_seconds.to_string(),
                    "target_app_status_timeout_seconds": config.status_timeout_seconds.to_string(),
                    "target_app_log_path": config.log_path.clone(),
                    "target_app_http_check_url": config.http_check_url.clone(),
                    "target_app_tcp_check_host": config.tcp_check_host.clone(),
                    "target_app_tcp_check_port": config.tcp_check_port.clone(),
                    "target_app_process_check_command": config.process_check_command.clone()
                });
                ApiResponse::json(
                    200,
                    json!({
                        "ok": true,
                        "config": config,
                        "settings": settings,
                        "message": "Generated target-app configuration from local project files."
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_target_app_rebuild_queue(&self) -> ApiResponse {
        match self
            .target_app_service()
            .and_then(|service| service.rebuild())
        {
            Ok(snapshot) => {
                let mut value = self.target_app_response(snapshot);
                value["queued"] =
                    json!(value.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false));
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_project_status(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read project status");
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).status() {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_path(&self, raw_path: &str) -> ApiResponse {
        let path = query_param(raw_path, "path").unwrap_or_default();
        let resolved = resolve_project_utility_path(&path);
        ApiResponse::json(
            200,
            json!({
                "path": resolved.display().to_string(),
                "input": path,
                "exists": resolved.exists(),
                "is_dir": resolved.is_dir(),
                "parent": resolved.parent().map(|path| path.display().to_string())
            }),
        )
    }

    fn handle_project_directories(&self, raw_path: &str) -> ApiResponse {
        let path = query_param(raw_path, "path").unwrap_or_default();
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(200)
            .clamp(1, 1000);
        match project_directories_response(&path, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_list(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("list projects");
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone())
            .list_response()
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_attach(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("attach projects");
        };
        let Some(path) = request
            .body
            .as_ref()
            .and_then(|body| body.get("path"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "path is required"
                    }
                }),
            );
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).attach(path)
        {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_register(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("register projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(path) = body
            .get("path")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput("path is required".to_string()));
        };
        let name = body
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                Path::new(path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(path)
                    .to_string()
            });
        let app_path = PathBuf::from(path.trim());
        let app_path = if app_path.is_absolute() {
            app_path
        } else {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(app_path),
                Err(error) => {
                    return error_response(RefineError::Io(format!(
                        "failed to inspect cwd: {error}"
                    )));
                }
            }
        };
        let app = RegisteredApp {
            name,
            path: app_path.display().to_string(),
            added_at: now_timestamp_web(),
            last_used_at: None,
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).register(app)
        {
            Ok(registry) => ApiResponse::json(
                201,
                json!({
                    "ok": true,
                    "apps": registry_apps_array(&registry),
                    "current": registry.active_app.unwrap_or_default(),
                    "registry_enabled": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_switch(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("switch projects");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let Some(name) = body
            .get("name")
            .or_else(|| body.get("path"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return error_response(RefineError::InvalidInput(
                "name or path is required".to_string(),
            ));
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).switch(name)
        {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_detach(&self) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("detach project");
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).detach() {
            Ok(status) => ApiResponse::json(200, project_status_value(status)),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_remove(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("remove projects");
        };
        let Some(path) = request
            .body
            .as_ref()
            .and_then(|body| body.get("path").or_else(|| body.get("name")))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "path is required"
                    }
                }),
            );
        };
        match FileProjectRegistryService::new(runtime_root, self.durable_root.clone()).remove(path)
        {
            Ok(registry) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "apps": registry_apps_array(&registry),
                    "current": registry.active_app.unwrap_or_default(),
                    "registry_enabled": true
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_project_sync(&self) -> ApiResponse {
        let projection = match self.current_projection() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        ApiResponse::json(
            200,
            json!({
                "ok": true,
                "message": "Project state projection rebuilt.",
                "projection_version": projection.version,
                "gap_count": projection.gaps.len(),
                "feature_count": projection.features.len()
            }),
        )
    }

    fn handle_project_templates(&self) -> ApiResponse {
        ApiResponse::json(200, json!({"templates": []}))
    }

    fn handle_project_scaffold(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("create scaffold Gaps");
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("Scaffold target application");
        match FileWorkItemService::new(durable_root).create_gap_summary(name, None) {
            Ok(gap) => ApiResponse::json(201, json!({"ok": true, "gap": gap.gap})),
            Err(error) => error_response(error),
        }
    }

    fn handle_settings_get(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("read settings");
        };
        match FileSettingsService::new(durable_root).list_response() {
            Ok(value) => ApiResponse::json(200, self.with_runtime_settings(value)),
            Err(error) => error_response(error),
        }
    }

    fn handle_settings_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("update settings");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        if let Some(paused) = body.get("paused").map(runtime_bool_setting)
            && let Some(runtime_root) = &self.runtime_root
        {
            let supervisor = FileProcessSupervisor::new(runtime_root);
            if let Err(error) = supervisor.set_agents_paused(paused) {
                return error_response(error);
            }
            if let Err(error) = supervisor.set_background_processes_stopped(paused) {
                return error_response(error);
            }
        }
        match FileSettingsService::new(durable_root).update(&body) {
            Ok(value) => {
                let value = self.with_runtime_settings(value);
                if let Err(error) = self.current_projection_with_runtime() {
                    return error_response(error);
                }
                ApiResponse::json(200, value)
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_upgrade_status(&self) -> ApiResponse {
        let current_version = env!("CARGO_PKG_VERSION");
        let command = std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "refine-native".to_string());
        ApiResponse::json(
            200,
            json!({
                "upgrade": {
                    "available": false,
                    "upgrade_available": false,
                    "current_version": current_version,
                    "latest_version": current_version,
                    "local_development": true,
                    "message": format!("Running native Refine {current_version}; remote release discovery is not configured for this build."),
                    "command": command
                }
            }),
        )
    }

    fn handle_governance_get(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("read governance settings");
        };
        match FileGovernanceService::new(durable_root).load() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_governance_save(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("save governance settings");
        };
        match FileGovernanceService::new(durable_root)
            .save(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_governance_generate_rules(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("generate governance rules");
        };
        match FileGovernanceService::new(durable_root)
            .generate_rules(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_guidance_list(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("read guidance");
        };
        match FileGuidanceService::new(durable_root).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_guidance_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("update guidance");
        };
        match FileGuidanceService::new(durable_root)
            .update(&request.body.unwrap_or_else(|| json!({})))
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_reporters_list(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("list reporters");
        };
        match FileReporterService::new(durable_root).list() {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_reporter_create(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("create reporters");
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(durable_root).create(name) {
            Ok(value) => ApiResponse::json(201, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_reporter_rename(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("rename reporters");
        };
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        let name = request
            .body
            .as_ref()
            .and_then(|body| body.get("name"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileReporterService::new(durable_root).rename(id, name) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_reporter_merge(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("merge reporters");
        };
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "/merge") else {
            return reporter_id_required();
        };
        let Some(target_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("target_id"))
            .and_then(|value| value.as_u64())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_input",
                        "message": "target_id is required"
                    }
                }),
            );
        };
        match FileReporterService::new(durable_root).merge(id, target_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn handle_reporter_delete(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("delete reporters");
        };
        let Some(id) = reporter_id_from_path(&request.path, "/reporters/", "") else {
            return reporter_id_required();
        };
        match FileReporterService::new(durable_root).delete(id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    fn quality_timing_setting(&self) -> String {
        self.durable_root
            .as_ref()
            .and_then(|root| FileSettingsService::new(root).load().ok())
            .and_then(|settings| {
                settings
                    .get("quality_timing")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "pre_merge".to_string())
    }

    fn with_runtime_settings(&self, mut value: Value) -> Value {
        if let Some(runtime) = self.runtime_settings_value()
            && let Some(object) = value.as_object_mut()
        {
            object.insert("runtime".to_string(), runtime);
        }
        value
    }

    fn runtime_settings_value(&self) -> Option<Value> {
        let runtime_root = self.runtime_root.as_ref()?;
        let pause_state = FileProcessSupervisor::new(runtime_root)
            .pause_state()
            .ok()?;
        Some(json!({
            "paused": pause_state.agents_paused || pause_state.background_processes_stopped,
            "agents_paused": pause_state.agents_paused,
            "background_processes_stopped": pause_state.background_processes_stopped,
            "runtime_root": runtime_root.display().to_string()
        }))
    }

    fn target_app_service(&self) -> RefineResult<FileTargetAppService> {
        let Some(durable_root) = &self.durable_root else {
            return Err(RefineError::Degraded(
                "durable root is required for target-app operations".to_string(),
            ));
        };
        let Some(runtime_root) = &self.runtime_root else {
            return Err(RefineError::Degraded(
                "runtime root is required for target-app operations".to_string(),
            ));
        };
        let source_root = durable_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(FileTargetAppService::new(
            durable_root,
            runtime_root,
            source_root,
        ))
    }

    fn target_app_response(&self, snapshot: TargetAppSnapshot) -> serde_json::Value {
        let settings = self
            .durable_root
            .as_ref()
            .and_then(|root| FileSettingsService::new(root).load().ok())
            .unwrap_or_default();
        let get = |key: &str| {
            settings
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string()
        };
        json!({
            "ok": snapshot.ok,
            "state": snapshot.state,
            "health_url": first_non_empty(&get("target_app_http_check_url"), &get("target_app_health_url")),
            "app_url": get("target_app_url"),
            "has_start_command": !get("target_app_start_command").trim().is_empty(),
            "has_stop_command": !get("target_app_stop_command").trim().is_empty(),
            "has_rebuild_command": !get("target_app_rebuild_command").trim().is_empty(),
            "has_status_checks": !get("target_app_status_command").trim().is_empty()
                || !get("target_app_http_check_url").trim().is_empty()
                || !get("target_app_tcp_check_host").trim().is_empty()
                || !get("target_app_process_check_command").trim().is_empty(),
            "has_start_instructions": !get("target_app_start_command").trim().is_empty()
                || !get("target_app_start_instructions").trim().is_empty(),
            "has_stop_instructions": !get("target_app_stop_command").trim().is_empty()
                || !get("target_app_stop_instructions").trim().is_empty(),
            "last_check_at": snapshot.last_check_at,
            "last_check_ok": snapshot.last_check_ok,
            "last_check_message": snapshot.last_check_message,
            "last_health_at": snapshot.last_health_at,
            "last_health_ok": snapshot.last_health_ok,
            "last_health_message": snapshot.last_health_message,
            "last_error": snapshot.last_error,
            "last_operation_id": snapshot.last_operation_id,
            "last_operation": snapshot.last_operation,
            "process_id": snapshot.process_id,
            "pid": snapshot.pid,
            "auto_rebuild": get("target_app_auto_rebuild"),
            "auto_rebuild_hour_utc": get("target_app_auto_rebuild_hour_utc"),
            "auto_rebuild_last_started_at": "",
            "auto_rebuild_last_finished_at": "",
            "auto_rebuild_last_ok": false,
            "auto_rebuild_last_message": "",
            "legacy_config_present": !get("target_app_start_instructions").trim().is_empty()
                || !get("target_app_stop_instructions").trim().is_empty()
                || !get("target_app_health_url").trim().is_empty(),
            "message": snapshot.message
        })
    }

    fn handle_quality_get(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("load quality settings");
        };
        match FileQualityService::new(durable_root).load_settings() {
            Ok(settings) => ApiResponse::json(200, json!(settings)),
            Err(error) => error_response(error),
        }
    }

    fn handle_quality_save(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("save quality settings");
        };
        let patch = match serde_json::from_value::<QualitySettingsPatch>(
            request.body.unwrap_or_else(|| json!({})),
        ) {
            Ok(patch) => patch,
            Err(error) => {
                return ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_input",
                            "message": format!("invalid quality settings body: {error}")
                        }
                    }),
                );
            }
        };
        match FileQualityService::new(durable_root).save_settings(patch) {
            Ok(settings) => {
                append_quality_activity(durable_root, "Quality settings updated".to_string());
                ApiResponse::json(200, json!(settings))
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_quality_checks(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("run quality checks");
        };
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("run quality checks");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let owner_id = body
            .get("owner_id")
            .or_else(|| body.get("gap_id"))
            .or_else(|| body.get("feature_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("app")
            .to_string();
        let command = body
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let browser_required = body
            .get("browser_required")
            .or_else(|| body.get("browser"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        match QualityJobRunner::new(durable_root, runtime_root).run_checks(QualityCheckRequest {
            owner_id,
            command,
            browser_required,
        }) {
            Ok(job_result) => {
                append_quality_activity(
                    durable_root,
                    format!(
                        "Quality checks completed for {}",
                        job_result.result.owner_id
                    ),
                );
                ApiResponse::json(
                    200,
                    json!({
                        "ok": job_result.result.ok,
                        "result": job_result.result,
                        "job": job_response(job_result.job)
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_quality_screenshots(&self, raw_path: &str) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("list quality screenshots");
        };
        let owner_id = query_param(raw_path, "owner_id").unwrap_or_else(|| "app".to_string());
        match FileQualityService::new(durable_root).screenshots(&owner_id) {
            Ok(screenshots) => {
                let screenshot_count = screenshots.len();
                ApiResponse::json(
                    200,
                    json!({
                        "ok": true,
                        "owner_id": owner_id,
                        "screenshots": screenshots,
                        "screenshot_count": screenshot_count
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_quality_regression_create(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("create quality regressions");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let title = body
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let prompt = body
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let description = body
            .get("description")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        match FileQualityService::new(durable_root).create_regression(title, description, prompt) {
            Ok(regression) => {
                append_quality_activity(
                    durable_root,
                    format!("Regression created: {}", regression.title),
                );
                ApiResponse::json(201, json!({"ok": true, "regression": regression}))
            }
            Err(error) => error_response(error),
        }
    }

    fn handle_quality_regression_update(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("update quality regressions");
        };
        let Some(regression_id) = request
            .path
            .strip_prefix("/quality/regressions/")
            .filter(|regression_id| !regression_id.is_empty() && !regression_id.contains('/'))
        else {
            return regression_id_required();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileQualityService::new(durable_root).update_regression(regression_id, &body) {
            Ok(regression) => ApiResponse::json(200, json!({"ok": true, "regression": regression})),
            Err(error) => error_response(error),
        }
    }

    fn handle_quality_regression_delete(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("delete quality regressions");
        };
        let Some(regression_id) = request
            .path
            .strip_prefix("/quality/regressions/")
            .filter(|regression_id| !regression_id.is_empty() && !regression_id.contains('/'))
        else {
            return regression_id_required();
        };
        match FileQualityService::new(durable_root).delete_regression(regression_id) {
            Ok(()) => ApiResponse::json(200, json!({"ok": true})),
            Err(error) => error_response(error),
        }
    }

    fn handle_quality_regression_run(&self) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("run quality regressions");
        };
        match FileQualityService::new(durable_root).run_regressions(true) {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    fn handle_chat_start(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("start chat sessions");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let attachment = if let Some(gap_id) = body.get("gap_id").and_then(|value| value.as_str()) {
            ChatAttachment::Gap(gap_id.to_string())
        } else if let Some(feature_id) = body.get("feature_id").and_then(|value| value.as_str()) {
            ChatAttachment::Feature(feature_id.to_string())
        } else {
            ChatAttachment::Standalone
        };
        let mode = body
            .get("purpose")
            .or_else(|| body.get("mode"))
            .and_then(|value| value.as_str());
        let provider = body.get("provider").and_then(|value| value.as_str());
        let service = self.chat_service(durable_root);
        match service.start_with_options(attachment, provider, mode) {
            Ok(session) => ApiResponse::json(
                201,
                json!({
                    "ok": true,
                    "session_id": session.id,
                    "provider": session.provider,
                    "mode": session.mode
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn handle_chat_input(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("send chat input");
        };
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/input"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let Some(text) = request
            .body
            .as_ref()
            .and_then(|body| body.get("text"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "bad_request",
                        "message": "text is required"
                    }
                }),
            );
        };
        let service = self.chat_service(durable_root);
        match service.append_user_message(session_id, text) {
            Ok(()) => ApiResponse::json(200, json!({"ok": true})),
            Err(error) => error_response(error),
        }
    }

    fn handle_chat_read(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("read chat sessions");
        };
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/read"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let service = self.chat_service(durable_root);
        match service.read(session_id) {
            Ok(result) => ApiResponse::json(200, json!(result)),
            Err(error) => error_response(error),
        }
    }

    fn handle_chat_stop(&self, request: ApiRequest) -> ApiResponse {
        let Some(durable_root) = &self.durable_root else {
            return durable_root_unavailable("stop chat sessions");
        };
        let Some(session_id) = request
            .path
            .strip_prefix("/chat/")
            .and_then(|path| path.strip_suffix("/stop"))
            .filter(|session_id| !session_id.is_empty() && !session_id.contains('/'))
        else {
            return chat_session_id_required();
        };
        let service = self.chat_service(durable_root);
        match service.stop(session_id) {
            Ok(session) => ApiResponse::json(
                200,
                json!({
                    "ok": true,
                    "session_id": session.id,
                    "alive": !session.closed,
                    "closed_reason": session.interruption_detail
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    fn current_projection(&self) -> RefineResult<ProjectionSnapshot> {
        if let Some(durable_root) = &self.durable_root {
            let store = FileProjectStateStore::new(durable_root);
            if let Some(runtime_root) = &self.runtime_root {
                store.load_or_refresh_projection(&runtime_root.join("cache"))
            } else {
                store.rebuild_projection()
            }
        } else {
            Ok(self.projection.clone())
        }
    }

    fn chat_service(&self, durable_root: &Path) -> FileChatService {
        if let Some(runtime_root) = &self.runtime_root {
            FileChatService::with_runtime_root(durable_root, runtime_root)
        } else {
            FileChatService::new(durable_root)
        }
    }

    fn current_projection_with_runtime(&self) -> RefineResult<ProjectionSnapshot> {
        let mut projection = self.current_projection()?;
        projection.runtime = self.runtime_projection()?;
        self.persist_runtime_projection_snapshot(&projection)?;
        Ok(projection)
    }

    fn runtime_projection(&self) -> RefineResult<RuntimeProjection> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeProjection::default());
        };
        let process = process_summary_value(runtime_root)?;
        let processes = process
            .get("processes")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(value_object)
            .collect::<Vec<_>>();
        let background_jobs = FileJobRegistry::new(runtime_root)
            .recover()?
            .into_iter()
            .map(job_response)
            .filter_map(value_object)
            .collect::<Vec<_>>();
        let target_app = match self.target_app_service() {
            Ok(service) => {
                let snapshot = service.snapshot()?;
                value_object(self.target_app_response(snapshot))
            }
            Err(_) => None,
        };
        let performance = performance_report_value(runtime_root, PerformanceQuery::default())
            .ok()
            .and_then(value_object);
        let preflight = provider_status_value().ok().and_then(value_object);
        Ok(RuntimeProjection {
            supervisor: value_object(process),
            processes,
            background_jobs,
            target_app,
            performance,
            preflight,
        })
    }

    fn refresh_projection_cache_after_mutation(&self) -> RefineResult<()> {
        let (Some(durable_root), Some(runtime_root)) = (&self.durable_root, &self.runtime_root)
        else {
            return Ok(());
        };
        FileProjectStateStore::new(durable_root)
            .load_or_refresh_projection(&runtime_root.join("cache"))
            .map(|_| ())
    }

    fn persist_runtime_projection_override(
        &self,
        apply: impl FnOnce(&mut RuntimeProjection),
    ) -> RefineResult<()> {
        let mut projection = self.current_projection_with_runtime()?;
        apply(&mut projection.runtime);
        self.persist_runtime_projection_snapshot(&projection)
    }

    fn persist_runtime_projection_snapshot(
        &self,
        projection: &ProjectionSnapshot,
    ) -> RefineResult<()> {
        let (Some(durable_root), Some(runtime_root)) = (&self.durable_root, &self.runtime_root)
        else {
            return Ok(());
        };
        FileProjectStateStore::new(durable_root)
            .persist_projection_snapshot(&runtime_root.join("cache"), projection)
    }

    fn reconcile_feature_runtime_work(
        &self,
        feature_id: &str,
        gap_ids: &[String],
    ) -> RefineResult<RuntimeReconcileSummary> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeReconcileSummary::default());
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        let mut processes = 0;
        for process in supervisor.list()? {
            if process.state == "running" && runtime_record_matches(&process, feature_id, gap_ids) {
                supervisor.signal(&process.id, "terminate")?;
                processes += 1;
            }
        }

        let registry = FileJobRegistry::new(runtime_root);
        let mut jobs = 0;
        for job in registry.recover()? {
            if matches!(
                job.state,
                JobState::Pending | JobState::Running | JobState::Cancelling
            ) && job_owner_matches(&job.owner, feature_id, gap_ids)
            {
                registry.cancel(&job.id)?;
                jobs += 1;
            }
        }
        Ok(RuntimeReconcileSummary { processes, jobs })
    }

    fn source_root(&self) -> Option<PathBuf> {
        self.durable_root
            .as_ref()
            .and_then(|root| root.parent().map(Path::to_path_buf))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RuntimeReconcileSummary {
    processes: usize,
    jobs: usize,
}

fn runtime_record_matches(process: &ManagedProcess, feature_id: &str, gap_ids: &[String]) -> bool {
    process_text_matches(process.label.as_deref(), feature_id, gap_ids)
        || process_text_matches(process.details.as_deref(), feature_id, gap_ids)
}

fn job_owner_matches(owner: &str, feature_id: &str, gap_ids: &[String]) -> bool {
    process_text_matches(Some(owner), feature_id, gap_ids)
}

fn process_text_matches(text: Option<&str>, feature_id: &str, gap_ids: &[String]) -> bool {
    let Some(text) = text else {
        return false;
    };
    text.contains(feature_id) || gap_ids.iter().any(|gap_id| text.contains(gap_id))
}

fn durable_root_unavailable(action: &str) -> ApiResponse {
    ApiResponse::json(
        503,
        json!({
            "error": {
                "code": "durable_root_unavailable",
                "message": format!("daemon cannot {action} without a durable root")
            }
        }),
    )
}

fn runtime_root_unavailable(action: &str) -> ApiResponse {
    ApiResponse::json(
        503,
        json!({
            "error": {
                "code": "runtime_root_unavailable",
                "message": format!("daemon cannot {action} without a runtime root")
            }
        }),
    )
}

fn job_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Job route requires a job id"
            }
        }),
    )
}

fn process_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Process route requires a process id"
            }
        }),
    )
}

fn provider_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Provider route requires a provider id"
            }
        }),
    )
}

fn agent_provider_from_path<'a>(path: &'a str, suffix: &str) -> Option<&'a str> {
    path.strip_prefix("/agents/")
        .and_then(|path| path.strip_suffix(&format!("/{suffix}")))
        .map(str::trim)
        .filter(|provider| !provider.is_empty() && !provider.contains('/'))
}

fn regression_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Regression route requires a regression id"
            }
        }),
    )
}

fn chat_session_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Chat route requires a session id"
            }
        }),
    )
}

fn reporter_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Reporter route requires a reporter id"
            }
        }),
    )
}

fn reporter_id_from_path(path: &str, prefix: &str, suffix: &str) -> Option<u64> {
    path.strip_prefix(prefix)
        .and_then(|path| {
            if suffix.is_empty() {
                Some(path)
            } else {
                path.strip_suffix(suffix)
            }
        })
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .and_then(|id| id.parse::<u64>().ok())
}

fn first_non_empty(first: &str, second: &str) -> String {
    if first.trim().is_empty() {
        second.to_string()
    } else {
        first.to_string()
    }
}

fn append_quality_activity(durable_root: &Path, message: String) {
    let service = FileActivityService::new(durable_root);
    let entry = service.new_entry(message, "info", "quality", None, Some("refine".to_string()));
    let _ = service.append(entry);
}

fn job_response(job: JobHandle) -> serde_json::Value {
    json!({
        "id": job.id,
        "owner": job.owner,
        "status": job.state.as_api_status(),
        "state": job.state,
        "progress": {},
        "result": {},
        "error": null
    })
}

fn process_summary_response(runtime_root: &Path) -> ApiResponse {
    match process_summary_value(runtime_root) {
        Ok(value) => ApiResponse::json(200, value),
        Err(error) => error_response(error),
    }
}

fn process_summary_value(runtime_root: &Path) -> RefineResult<Value> {
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let pause_state = supervisor.pause_state()?;
    let processes = supervisor.list()?;
    let process_values: Vec<_> = processes
        .into_iter()
        .map(|process| process.api_json())
        .collect();
    Ok(json!({
        "runner_reachable": true,
        "paused": pause_state.background_processes_stopped || pause_state.agents_paused,
        "background_processes_stopped": pause_state.background_processes_stopped,
        "agents_paused": pause_state.agents_paused,
        "processes": process_values,
        "runner_work": [],
        "backend": {
            "process_model": "supervisor"
        }
    }))
}

fn provider_status_response() -> ApiResponse {
    match provider_status_value() {
        Ok(value) => ApiResponse::json(200, value),
        Err(error) => error_response(error),
    }
}

fn provider_status_value() -> RefineResult<Value> {
    let service = HostAgentProviderService::new();
    let providers = service.detect()?;
    let selected = providers
        .iter()
        .find(|provider| provider.name == "claude")
        .or_else(|| providers.iter().find(|provider| provider.installed));
    let ok = selected.map(|provider| provider.installed).unwrap_or(false);
    let message = if ok {
        selected
            .map(|provider| format!("{} CLI detected", provider.display_name))
            .unwrap_or_else(|| "provider detected".to_string())
    } else {
        "No supported provider CLI detected on PATH".to_string()
    };
    Ok(json!({
        "ok": ok,
        "stage": "provider_detection",
        "message": message,
        "selected_provider": selected.map(|provider| provider.name.clone()).unwrap_or_else(|| "claude".to_string()),
        "providers": providers
    }))
}

fn performance_report_value(runtime_root: &Path, query: PerformanceQuery) -> RefineResult<Value> {
    let service = FileMetricsService::new(runtime_root);
    let report = service.report(query)?;
    Ok(json!({
        "summary": report.summary,
        "recent": report.recent,
        "events": report.events,
        "operations": report.operations,
        "event_count": report.event_count,
        "filtered_event_count": report.filtered_event_count,
        "total_event_count": report.total_event_count,
        "retention_days": report.retention_days,
        "page": report.page,
        "backend": {
            "process_model": "supervisor",
            "native": true,
            "store": "jsonl"
        }
    }))
}

fn runtime_process_summary_value(runtime: &RuntimeProjection) -> Value {
    let mut summary = runtime
        .supervisor
        .clone()
        .unwrap_or_else(|| serde_json::Map::new());
    summary.insert(
        "processes".to_string(),
        Value::Array(
            runtime
                .processes
                .iter()
                .cloned()
                .map(Value::Object)
                .collect(),
        ),
    );
    summary
        .entry("runner_reachable".to_string())
        .or_insert(Value::Bool(false));
    summary
        .entry("runner_work".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    Value::Object(summary)
}

fn value_object(value: Value) -> Option<JsonObject> {
    match value {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

fn runtime_bool_setting(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64().unwrap_or_default() != 0,
        Value::String(value) => {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }
        _ => false,
    }
}

fn parse_install_target(value: Option<&str>) -> InstallTarget {
    match value.unwrap_or("").trim().to_lowercase().as_str() {
        "macos" | "macos_app_bundle" | "macos-app-bundle" | "app_bundle" => {
            InstallTarget::MacOsAppBundle
        }
        "windows" | "windows_installer" | "windows-installer" | "installer" => {
            InstallTarget::WindowsInstaller
        }
        "linux" | "linux_cli_web" | "linux-cli-web" | "cli_web" => InstallTarget::LinuxCliWeb,
        _ => match std::env::consts::OS {
            "macos" => InstallTarget::MacOsAppBundle,
            "windows" => InstallTarget::WindowsInstaller,
            _ => InstallTarget::LinuxCliWeb,
        },
    }
}

fn project_status_value(status: crate::model::project::ProjectStatus) -> serde_json::Value {
    let apps = registry_apps_array(&status.apps);
    json!({
        "attached": status.attached,
        "registry_enabled": status.registry_enabled,
        "client_repo": status.client_repo,
        "volume_root": status.volume_root,
        "config_path": status.config_path,
        "schema": status.schema,
        "maintenance": status.maintenance,
        "apps": apps,
        "active_node_id": status.active_node_id,
        "active_node": status.active_node,
        "nodes": [{
            "id": status.active_node_id.clone().unwrap_or_else(|| "default".to_string()),
            "display_name": status.active_node.clone().unwrap_or_else(|| "Default".to_string()),
            "active": true
        }],
        "message": status.message
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NodeRegistryDocument {
    nodes: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ClusterRegistryDocument {
    nodes: Vec<Value>,
    updated_at: String,
}

fn load_node_registry(durable_root: &Path) -> RefineResult<NodeRegistryDocument> {
    let path = durable_root.join("nodes.json");
    if !path.exists() {
        return Ok(NodeRegistryDocument {
            nodes: vec![default_node("default", "Default", false)],
        });
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read node registry {}: {error}",
            path.display()
        ))
    })?;
    let mut registry: NodeRegistryDocument = serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse node registry {}: {error}",
            path.display()
        ))
    })?;
    if !registry
        .nodes
        .iter()
        .any(|node| node_id_value(node) == "default")
    {
        registry
            .nodes
            .insert(0, default_node("default", "Default", false));
    }
    Ok(registry)
}

fn save_node_registry(durable_root: &Path, registry: &NodeRegistryDocument) -> RefineResult<()> {
    write_json_atomically_web(&durable_root.join("nodes.json"), &json!(registry))
}

fn load_active_node_id(durable_root: &Path) -> RefineResult<String> {
    let path = durable_root.join("active-node.json");
    if !path.exists() {
        return Ok("default".to_string());
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read active node {}: {error}",
            path.display()
        ))
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse active node {}: {error}",
            path.display()
        ))
    })?;
    Ok(value
        .get("active_node_id")
        .and_then(|value| value.as_str())
        .unwrap_or("default")
        .to_string())
}

fn save_active_node_id(durable_root: &Path, node_id: &str) -> RefineResult<()> {
    write_json_atomically_web(
        &durable_root.join("active-node.json"),
        &json!({
            "active_node_id": node_id,
            "updated_at": now_timestamp_web()
        }),
    )
}

fn default_node(id: &str, display_name: &str, active: bool) -> Value {
    let now = now_timestamp_web();
    json!({
        "id": id,
        "display_name": display_name,
        "archived": false,
        "active": active,
        "created_at": now,
        "updated_at": now
    })
}

fn unique_node_id(registry: &NodeRegistryDocument, display_name: &str) -> String {
    let base = slug_id(display_name, "node");
    if !registry
        .nodes
        .iter()
        .any(|node| node_id_value(node) == base)
    {
        return base;
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}-{suffix}");
        if !registry
            .nodes
            .iter()
            .any(|node| node_id_value(node) == candidate)
        {
            return candidate;
        }
    }
    format!("{base}-{}", Utc::now().timestamp())
}

fn slug_id(value: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

fn node_id_value(node: &Value) -> &str {
    node.get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn node_archived(node: &Value) -> bool {
    node.get("archived")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn load_cluster_registry(durable_root: &Path) -> RefineResult<ClusterRegistryDocument> {
    let path = durable_root.join("cluster.json");
    if !path.exists() {
        return Ok(ClusterRegistryDocument {
            nodes: Vec::new(),
            updated_at: now_timestamp_web(),
        });
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read cluster registry {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse cluster registry {}: {error}",
            path.display()
        ))
    })
}

fn save_cluster_registry(
    durable_root: &Path,
    registry: &ClusterRegistryDocument,
) -> RefineResult<()> {
    write_json_atomically_web(&durable_root.join("cluster.json"), &json!(registry))
}

fn cluster_response(registry: ClusterRegistryDocument) -> Value {
    json!({
        "nodes": registry.nodes,
        "maintenance": null,
        "enabled": !registry.nodes.is_empty(),
        "updated_at": registry.updated_at,
        "message": if registry.nodes.is_empty() {
            "No cluster nodes configured."
        } else {
            "Cluster nodes configured."
        }
    })
}

fn default_cluster_node(id: &str) -> Value {
    let now = now_timestamp_web();
    json!({
        "id": id,
        "display_name": id,
        "ssh_host": "",
        "ssh_port": 22,
        "refine_checkout": "~/refine",
        "target_app_path": "",
        "refine_port": 8080,
        "enabled": true,
        "health": null,
        "created_at": now,
        "updated_at": now
    })
}

fn cluster_node_id_value(node: &Value) -> &str {
    node.get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn cluster_node_id_from_path(path: &str) -> Option<String> {
    path.strip_prefix("/cluster/nodes/")
        .and_then(|rest| rest.split('/').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn now_timestamp_web() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn write_json_atomically_web(path: &Path, value: &Value) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let temp_path = path.with_extension("json.tmp");
    let encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    fs::write(&temp_path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write temp file {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        RefineError::Io(format!(
            "failed to commit JSON file {}: {error}",
            path.display()
        ))
    })
}

fn resolve_project_utility_path(path: &str) -> PathBuf {
    let path = path.trim();
    if path.is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn project_directories_response(path: &str, max_entries: usize) -> RefineResult<Value> {
    let selected_path = resolve_project_utility_path(path);
    let list_path = if selected_path.is_dir() {
        selected_path.clone()
    } else {
        selected_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| selected_path.clone())
    };
    if !list_path.exists() {
        return Err(RefineError::NotFound(format!(
            "directory {} was not found",
            list_path.display()
        )));
    }
    if !list_path.is_dir() {
        return Err(RefineError::InvalidInput(format!(
            "{} is not a directory",
            list_path.display()
        )));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in fs::read_dir(&list_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read directory {}: {error}",
            list_path.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read directory entry {}: {error}",
                list_path.display()
            ))
        })?;
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat directory entry {}: {error}",
                entry.path().display()
            ))
        })?;
        if !metadata.is_dir() {
            continue;
        }
        if entries.len() >= max_entries {
            truncated = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        entries.push(json!({
            "name": name,
            "path": entry.path().display().to_string()
        }));
    }
    entries.sort_by(|a, b| {
        a.get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .cmp(b.get("name").and_then(|value| value.as_str()).unwrap_or(""))
    });
    Ok(json!({
        "path": list_path.display().to_string(),
        "selected_path": selected_path.display().to_string(),
        "parent": list_path.parent().map(|path| path.display().to_string()),
        "entries": entries,
        "truncated": truncated
    }))
}

fn files_tree_response(
    source_root: &Path,
    path: &str,
    recursive: bool,
    max_depth: usize,
    max_entries: usize,
) -> RefineResult<Value> {
    let rel_path = normalize_file_path(path)?;
    let absolute = source_root.join(&rel_path);
    if !absolute.exists() {
        return Err(RefineError::NotFound(format!(
            "source path {} was not found",
            display_rel_path(&rel_path)
        )));
    }
    if !absolute.is_dir() {
        return Err(RefineError::InvalidInput(format!(
            "source path {} is not a directory",
            display_rel_path(&rel_path)
        )));
    }
    let mut entries_by_path = serde_json::Map::new();
    let mut meta_by_path = serde_json::Map::new();
    let mut remaining = max_entries;
    collect_file_tree(
        source_root,
        &rel_path,
        recursive,
        max_depth,
        0,
        &mut remaining,
        &mut entries_by_path,
        &mut meta_by_path,
    )?;
    let path = display_rel_path(&rel_path);
    let entries = entries_by_path
        .get(&path)
        .cloned()
        .unwrap_or_else(|| json!([]));
    let truncated = meta_by_path
        .values()
        .any(|meta| meta.get("truncated").and_then(|value| value.as_bool()) == Some(true));
    Ok(json!({
        "path": path,
        "entries": entries,
        "entries_by_path": entries_by_path,
        "meta_by_path": meta_by_path,
        "truncated": truncated
    }))
}

fn collect_file_tree(
    source_root: &Path,
    rel_path: &Path,
    recursive: bool,
    max_depth: usize,
    depth: usize,
    remaining: &mut usize,
    entries_by_path: &mut serde_json::Map<String, Value>,
    meta_by_path: &mut serde_json::Map<String, Value>,
) -> RefineResult<()> {
    let absolute = source_root.join(rel_path);
    let mut entries = read_file_entries(source_root, &absolute, rel_path)?;
    let mut truncated = false;
    if entries.len() > *remaining {
        entries.truncate(*remaining);
        truncated = true;
        *remaining = 0;
    } else {
        *remaining -= entries.len();
    }
    let rel_key = display_rel_path(rel_path);
    entries_by_path.insert(rel_key.clone(), json!(entries));
    meta_by_path.insert(
        rel_key,
        json!({
            "truncated": truncated,
            "depth": depth
        }),
    );
    if recursive && depth < max_depth && *remaining > 0 {
        let child_dirs: Vec<PathBuf> = entries_by_path
            .get(&display_rel_path(rel_path))
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter(|entry| entry.get("type").and_then(|value| value.as_str()) == Some("directory"))
            .filter_map(|entry| {
                entry
                    .get("path")
                    .and_then(|value| value.as_str())
                    .map(PathBuf::from)
            })
            .collect();
        for child in child_dirs {
            if *remaining == 0 {
                break;
            }
            collect_file_tree(
                source_root,
                &child,
                recursive,
                max_depth,
                depth + 1,
                remaining,
                entries_by_path,
                meta_by_path,
            )?;
        }
    }
    Ok(())
}

fn read_file_entries(
    source_root: &Path,
    absolute: &Path,
    rel_path: &Path,
) -> RefineResult<Vec<Value>> {
    let mut entries = Vec::new();
    let read_dir = fs::read_dir(absolute).map_err(|error| {
        RefineError::Io(format!(
            "failed to read source directory {}: {error}",
            absolute.display()
        ))
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read source directory entry {}: {error}",
                absolute.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_source_entry(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat source path {}: {error}",
                path.display()
            ))
        })?;
        let child_rel = rel_path.join(&name);
        let kind = if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        entries.push(json!({
            "name": name,
            "path": display_rel_path(&child_rel),
            "type": kind,
            "size": metadata.len()
        }));
    }
    entries.sort_by(|a, b| {
        let a_type = a.get("type").and_then(|value| value.as_str()).unwrap_or("");
        let b_type = b.get("type").and_then(|value| value.as_str()).unwrap_or("");
        let a_name = a.get("name").and_then(|value| value.as_str()).unwrap_or("");
        let b_name = b.get("name").and_then(|value| value.as_str()).unwrap_or("");
        b_type.cmp(a_type).then_with(|| a_name.cmp(b_name))
    });
    let _ = source_root;
    Ok(entries)
}

fn files_read_response(
    source_root: &Path,
    path: &str,
    offset: usize,
    limit: usize,
) -> RefineResult<Value> {
    let rel_path = normalize_file_path(path)?;
    if rel_path.as_os_str().is_empty() {
        return Err(RefineError::InvalidInput(
            "file path is required".to_string(),
        ));
    }
    let absolute = source_root.join(&rel_path);
    if !absolute.exists() {
        return Err(RefineError::NotFound(format!(
            "source file {} was not found",
            display_rel_path(&rel_path)
        )));
    }
    if !absolute.is_file() {
        return Err(RefineError::InvalidInput(format!(
            "source path {} is not a file",
            display_rel_path(&rel_path)
        )));
    }
    let bytes = fs::read(&absolute).map_err(|error| {
        RefineError::Io(format!(
            "failed to read source file {}: {error}",
            absolute.display()
        ))
    })?;
    let name = rel_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    if is_binary_bytes(&bytes) {
        return Ok(json!({
            "path": display_rel_path(&rel_path),
            "name": name,
            "kind": "binary",
            "previewable": false,
            "reason": "Binary preview is not available yet.",
            "size": bytes.len(),
            "offset": offset,
            "limit": limit,
            "has_more": false,
            "next_offset": null,
            "large": bytes.len() > limit
        }));
    }
    let offset = offset.min(bytes.len());
    let end = (offset + limit).min(bytes.len());
    let content = String::from_utf8_lossy(&bytes[offset..end]).to_string();
    let start_line = bytes[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1;
    Ok(json!({
        "path": display_rel_path(&rel_path),
        "name": name,
        "kind": "text",
        "previewable": true,
        "content": content,
        "size": bytes.len(),
        "offset": offset,
        "limit": limit,
        "start_line": start_line,
        "has_more": end < bytes.len(),
        "next_offset": if end < bytes.len() { json!(end) } else { Value::Null },
        "large": bytes.len() > limit
    }))
}

fn files_search_response(
    source_root: &Path,
    query: &str,
    max_entries: usize,
) -> RefineResult<Value> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(json!({
            "query": "",
            "entries": [],
            "truncated": false
        }));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    search_source_paths(
        source_root,
        Path::new(""),
        &query,
        max_entries,
        &mut entries,
        &mut truncated,
    )?;
    Ok(json!({
        "query": query,
        "entries": entries,
        "truncated": truncated
    }))
}

fn search_source_paths(
    source_root: &Path,
    rel_path: &Path,
    query: &str,
    max_entries: usize,
    entries: &mut Vec<Value>,
    truncated: &mut bool,
) -> RefineResult<()> {
    if entries.len() >= max_entries {
        *truncated = true;
        return Ok(());
    }
    let absolute = source_root.join(rel_path);
    for entry in fs::read_dir(&absolute).map_err(|error| {
        RefineError::Io(format!(
            "failed to search source directory {}: {error}",
            absolute.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read source directory entry {}: {error}",
                absolute.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_source_entry(&name) {
            continue;
        }
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat source path {}: {error}",
                path.display()
            ))
        })?;
        let child_rel = rel_path.join(&name);
        let rel_display = display_rel_path(&child_rel);
        let kind = if metadata.is_dir() {
            "directory"
        } else {
            "file"
        };
        if name.to_lowercase().contains(query) || rel_display.to_lowercase().contains(query) {
            if entries.len() >= max_entries {
                *truncated = true;
                return Ok(());
            }
            entries.push(json!({
                "name": name,
                "path": rel_display,
                "type": kind,
                "size": metadata.len()
            }));
        }
        if metadata.is_dir() {
            search_source_paths(
                source_root,
                &child_rel,
                query,
                max_entries,
                entries,
                truncated,
            )?;
            if *truncated {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn normalize_file_path(path: &str) -> RefineResult<PathBuf> {
    let path = path.replace('\\', "/");
    let path = path.trim().trim_start_matches('/');
    let mut normalized = PathBuf::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(RefineError::InvalidInput(
                    "source path cannot contain ..".to_string(),
                ));
            }
            value => normalized.push(value),
        }
    }
    Ok(normalized)
}

fn display_rel_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn should_skip_source_entry(name: &str) -> bool {
    matches!(name, ".git" | ".refine" | "node_modules" | "target")
}

fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

fn query_param(raw_path: &str, key: &str) -> Option<String> {
    let query = raw_path.split_once('?')?.1;
    for pair in query.split('&') {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(raw_key) == key {
            return Some(percent_decode(raw_value));
        }
    }
    None
}

fn bounded_query_usize(raw_path: &str, key: &str, default: usize, max: usize) -> usize {
    query_param(raw_path, key)
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.min(max))
        .unwrap_or(default)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    output.push(byte);
                    index += 3;
                    continue;
                }
            }
        }
        output.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&output).to_string()
}

fn normalize_api_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path);
    let mut normalized = if let Some(rest) = path.strip_prefix("/api/gaps") {
        format!("/work/gaps{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/features") {
        format!("/work/features{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/activity") {
        format!("/activity{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/import") {
        format!("/import{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/changes") {
        format!("/changes{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/cache") {
        format!("/cache{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/performance") {
        format!("/performance{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/files") {
        format!("/files{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/jobs") {
        format!("/jobs{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/processes") {
        format!("/processes{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/quality") {
        format!("/quality{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/chat") {
        format!("/chat{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/project") {
        format!("/project{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/projects") {
        format!("/projects{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/apps") {
        format!("/apps{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/governance") {
        format!("/governance{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/guidance") {
        format!("/guidance{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/reporters") {
        format!("/reporters{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/target-app") {
        format!("/target-app{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/runner-workers") {
        format!("/runner-workers{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/dashboard") {
        format!("/dashboard{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/diagnostics") {
        format!("/diagnostics{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/nodes") {
        format!("/nodes{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/cluster") {
        format!("/cluster{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/agents") {
        format!("/agents{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/settings") {
        format!("/settings{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/sessions") {
        format!("/sessions{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/workflow") {
        format!("/workflow{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/system") {
        format!("/system{rest}")
    } else if let Some(rest) = path.strip_prefix("/api/upgrade") {
        format!("/upgrade{rest}")
    } else {
        path.to_string()
    };
    if normalized.starts_with("/work/features/") && normalized.ends_with("/workflow") {
        normalized = normalized
            .strip_suffix("/workflow")
            .map(|prefix| format!("{prefix}/move"))
            .unwrap_or(normalized);
    }
    normalized
}

fn is_unauthenticated_mutation(path: &str) -> bool {
    matches!(path, "/activity/ui-error" | "/sessions")
}

fn parse_surface_kind(value: &str) -> Option<SurfaceKind> {
    match value {
        "desktop" => Some(SurfaceKind::Desktop),
        "browser" => Some(SurfaceKind::Browser),
        "cli" => Some(SurfaceKind::Cli),
        _ => None,
    }
}

fn local_origin_allowed(request: &HttpRequest) -> bool {
    let Some(origin) = request
        .headers
        .get("origin")
        .or_else(|| request.headers.get("referer"))
    else {
        return true;
    };
    origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("http://localhost:")
        || origin.starts_with("tauri://")
        || origin.starts_with("https://tauri.localhost/")
}

fn valid_idempotency_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn idempotency_fingerprint(method: &str, path: &str, body: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in method
        .as_bytes()
        .iter()
        .chain([0].iter())
        .chain(path.as_bytes())
        .chain([0].iter())
        .chain(body)
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn idempotency_path(runtime_root: &Path, key: &str) -> PathBuf {
    runtime_root
        .join(IDEMPOTENCY_DIR)
        .join(format!("{}.json", key.replace(':', "_")))
}

fn load_idempotency_record(
    runtime_root: &Path,
    key: &str,
) -> RefineResult<Option<IdempotencyRecord>> {
    let path = idempotency_path(runtime_root, key);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read idempotency record {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice::<IdempotencyRecord>(&bytes)
        .map(Some)
        .map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse idempotency record {}: {error}",
                path.display()
            ))
        })
}

fn save_idempotency_record(
    runtime_root: &Path,
    key: &str,
    fingerprint: &str,
    response: &ApiResponse,
) -> RefineResult<()> {
    let dir = runtime_root.join(IDEMPOTENCY_DIR);
    fs::create_dir_all(&dir).map_err(|error| {
        RefineError::Io(format!(
            "failed to create idempotency directory {}: {error}",
            dir.display()
        ))
    })?;
    let record = IdempotencyRecord {
        key: key.to_string(),
        fingerprint: fingerprint.to_string(),
        response: response.clone(),
        created_at: now_timestamp_web(),
    };
    let encoded = serde_json::to_vec_pretty(&record).map_err(|error| {
        RefineError::Serialization(format!("failed to encode idempotency record: {error}"))
    })?;
    let path = idempotency_path(runtime_root, key);
    fs::write(&path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write idempotency record {}: {error}",
            path.display()
        ))
    })
}

fn append_api_mutation_event(
    runtime_root: &Path,
    method: &str,
    path: &str,
    status: u16,
) -> RefineResult<()> {
    fs::create_dir_all(runtime_root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create runtime root {}: {error}",
            runtime_root.display()
        ))
    })?;
    let event = ApiMutationEvent {
        method: method.to_string(),
        path: normalize_api_path(path),
        status,
        created_at: now_timestamp_web(),
    };
    let line = serde_json::to_string(&event).map_err(|error| {
        RefineError::Serialization(format!("failed to encode API mutation event: {error}"))
    })?;
    let path = runtime_root.join(API_EVENTS_FILE);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open API event log {}: {error}",
                path.display()
            ))
        })?;
    writeln!(file, "{line}").map_err(|error| {
        RefineError::Io(format!(
            "failed to write API event log {}: {error}",
            path.display()
        ))
    })
}

fn recent_api_mutation_events(
    runtime_root: &Path,
    limit: usize,
) -> RefineResult<Vec<ApiMutationEvent>> {
    let path = runtime_root.join(API_EVENTS_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read API event log {}: {error}",
            path.display()
        ))
    })?;
    let mut events = text
        .lines()
        .rev()
        .take(limit)
        .filter_map(|line| serde_json::from_str::<ApiMutationEvent>(line).ok())
        .collect::<Vec<_>>();
    events.reverse();
    Ok(events)
}

fn recent_job_sse_events(runtime_root: &Path, limit: usize) -> RefineResult<Vec<Value>> {
    let registry = FileJobRegistry::new(runtime_root);
    let jobs = registry.recover()?;
    let mut events = Vec::new();
    for job in jobs.into_iter().rev().take(limit) {
        let (logs, _, _) = registry.page_logs(&job.id, 5, 0)?;
        let latest_log = logs.last().cloned();
        events.push(json!({
            "job": job_response(job),
            "logs": logs,
            "latest_log": latest_log,
            "timestamp": now_timestamp_web()
        }));
    }
    events.reverse();
    Ok(events)
}

fn recent_process_sse_events(runtime_root: &Path, limit: usize) -> RefineResult<Vec<Value>> {
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let mut events = Vec::new();
    for process in supervisor.list()?.into_iter().rev().take(limit) {
        let (output, truncated) = if process.stdout_path.is_some() || process.stderr_path.is_some()
        {
            let full_output = supervisor.stream(&process.id)?;
            let truncated = full_output.chars().count() > 4000;
            (tail_text(full_output, 4000), truncated)
        } else {
            (String::new(), false)
        };
        events.push(json!({
            "process_id": process.id,
            "process": process.api_json(),
            "output": output,
            "truncated": truncated,
            "timestamp": now_timestamp_web()
        }));
    }
    events.reverse();
    Ok(events)
}

fn recent_chat_sse_events(durable_root: &Path, limit: usize) -> RefineResult<Vec<Value>> {
    let sessions_dir = durable_root.join("chat/sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(&sessions_dir).map_err(|error| {
        RefineError::Io(format!(
            "failed to read chat sessions directory {}: {error}",
            sessions_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect chat session entry {}: {error}",
                sessions_dir.display()
            ))
        })?;
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read_to_string(entry.path()).map_err(|error| {
            RefineError::Io(format!(
                "failed to read chat session {}: {error}",
                entry.path().display()
            ))
        })?;
        let session = serde_json::from_str::<ChatSessionRecord>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse chat session {}: {error}",
                entry.path().display()
            ))
        })?;
        sessions.push(session);
    }
    sessions.sort_by(|a, b| {
        a.updated_at
            .cmp(&b.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut events = Vec::new();
    for session in sessions.into_iter().rev() {
        for event in session.transcript_events.iter().rev() {
            events.push(json!({
                "session_id": session.id,
                "mode": session.mode,
                "provider": session.provider,
                "attachment": &session.attachment,
                "in_flight": session.in_flight,
                "closed": session.closed,
                "event": event,
                "timestamp": event.get("created_at").and_then(|value| value.as_str()).unwrap_or(&session.updated_at)
            }));
            if events.len() >= limit {
                events.reverse();
                return Ok(events);
            }
        }
    }
    events.reverse();
    Ok(events)
}

fn tail_text(text: String, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text;
    }
    text.chars().skip(count - max_chars).collect()
}

fn body_text(body: &serde_json::Value) -> &str {
    body.get("text")
        .or_else(|| body.get("csv"))
        .or_else(|| body.get("content"))
        .or_else(|| body.get("input"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn import_destination_feature_id(
    service: &FileWorkItemService,
    body: &serde_json::Value,
) -> RefineResult<Option<crate::core::product::project_state::FeatureSummaryProjection>> {
    if let Some(name) = body
        .get("new_feature_name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return service
            .create_feature_summary(
                name,
                None,
                body.get("new_feature_description")
                    .or_else(|| body.get("feature_description"))
                    .and_then(|value| value.as_str()),
                body.get("feature_reporter")
                    .or_else(|| body.get("reporter"))
                    .and_then(|value| value.as_str()),
            )
            .map(Some);
    }
    if let Some(feature_id) = body
        .get("feature_id")
        .or_else(|| body.get("feature"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|feature_id| !feature_id.is_empty())
    {
        return service.show_feature_summary(feature_id).map(Some);
    }
    Ok(None)
}

fn feature_import_response(
    feature: &crate::core::product::project_state::FeatureSummaryProjection,
) -> serde_json::Value {
    json!({
        "id": feature.feature.id,
        "name": feature.feature.name,
        "gap_ids": feature.gap_ids,
        "rollup": feature.rollup
    })
}

fn normalized_dedup_text(values: &[&str]) -> String {
    values
        .iter()
        .flat_map(|value| value.split_whitespace())
        .map(|part| {
            part.chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn gap_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Gap route requires a Gap id"
            }
        }),
    )
}

fn gap_id_and_action(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/work/gaps/")?;
    let (gap_id, action) = rest.rsplit_once('/')?;
    if gap_id.is_empty() || gap_id.contains('/') || action.is_empty() {
        return None;
    }
    Some((gap_id, action))
}

fn gap_action_message(action: &str) -> &'static str {
    match action {
        "verify" => "Verified",
        "retry-quality" => "Queued for QA",
        "retry-merge" => "Queued for merge",
        "merge" => "Merged",
        "undo" => "Undone",
        _ => "Gap action completed",
    }
}

fn feature_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Feature route requires a Feature id"
            }
        }),
    )
}

fn invalid_round_body() -> ApiResponse {
    ApiResponse::json(
        400,
        json!({
            "error": {
                "code": "invalid_round",
                "message": "round reporter, actual, and target are required"
            }
        }),
    )
}

fn invalid_bulk_body() -> ApiResponse {
    ApiResponse::json(
        400,
        json!({
            "error": {
                "code": "invalid_bulk",
                "message": "bulk request must include selection fields and exactly one update when updating"
            }
        }),
    )
}

fn parse_bulk_gap_update(body: &serde_json::Value) -> Option<BulkGapUpdate> {
    let update = body.get("update")?.as_object()?;
    let mut entries = update
        .iter()
        .filter(|(key, _)| matches!(key.as_str(), "priority" | "status" | "reporter"));
    let (field, value) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    let value = value.as_str()?.to_string();
    match field.as_str() {
        "priority" => Some(BulkGapUpdate::Priority(value)),
        "status" => Some(BulkGapUpdate::Status(value)),
        "reporter" => Some(BulkGapUpdate::Reporter(value)),
        _ => None,
    }
}

fn error_response(error: RefineError) -> ApiResponse {
    let (status, code) = match &error {
        RefineError::InvalidInput(_) => (400, "invalid_input"),
        RefineError::NotFound(_) => (404, "not_found"),
        RefineError::Unauthorized(_) => (401, "unauthorized"),
        RefineError::Conflict(_) => (409, "conflict"),
        RefineError::Degraded(_) => (503, "degraded"),
        RefineError::Io(_) | RefineError::Serialization(_) => (500, "storage_error"),
        RefineError::NotImplemented(_) => (501, "not_implemented"),
    };
    ApiResponse::json(
        status,
        json!({
            "error": {
                "code": code,
                "message": error.to_string()
            }
        }),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireResponse {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: String,
    pub body: Vec<u8>,
    pub extra_headers: Vec<(String, String)>,
}

impl WireResponse {
    pub fn json(response: ApiResponse) -> Self {
        let body = serde_json::to_vec_pretty(&response.body).unwrap_or_else(|_| b"{}".to_vec());
        Self {
            status: response.status,
            reason: reason_phrase(response.status),
            content_type: response.content_type,
            body,
            extra_headers: Vec::new(),
        }
    }

    pub fn bytes(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status,
            reason: reason_phrase(status),
            content_type: content_type.into(),
            body,
            extra_headers: Vec::new(),
        }
    }

    pub fn sse(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            reason: "OK",
            content_type: "text/event-stream".to_string(),
            body: body.into().into_bytes(),
            extra_headers: vec![
                ("Cache-Control".to_string(), "no-cache".to_string()),
                ("Connection".to_string(), "close".to_string()),
            ],
        }
    }
}

fn sse_event(event: &str, data: Value) -> RefineResult<String> {
    let encoded = serde_json::to_string(&data).map_err(|error| {
        RefineError::Serialization(format!("failed to encode SSE event: {error}"))
    })?;
    Ok(format!("event: {event}\ndata: {encoded}\n\n"))
}

#[derive(Clone, Debug)]
pub struct LocalHttpDaemon {
    pub server: InProcessWebServer,
    pub static_root: Option<PathBuf>,
}

impl LocalHttpDaemon {
    pub fn recover_runtime_state(&self) -> RefineResult<()> {
        if let Some(durable_root) = &self.server.durable_root {
            self.server
                .chat_service(durable_root)
                .recover_interrupted_turns(
                    "Daemon restarted before the provider turn completed.",
                )?;
        }
        Ok(())
    }

    pub fn bind_loopback(port: u16) -> RefineResult<TcpListener> {
        TcpListener::bind(("127.0.0.1", port)).map_err(|error| {
            RefineError::Io(format!(
                "failed to bind local daemon web server on 127.0.0.1:{port}: {error}"
            ))
        })
    }

    pub fn local_addr(listener: &TcpListener) -> RefineResult<SocketAddr> {
        listener.local_addr().map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect daemon listener address: {error}"
            ))
        })
    }

    pub fn serve_next(&self, listener: &TcpListener) -> RefineResult<()> {
        let (stream, _) = listener.accept().map_err(|error| {
            RefineError::Io(format!("failed to accept daemon HTTP request: {error}"))
        })?;
        self.handle_stream(stream)
    }

    pub fn handle_stream(&self, mut stream: TcpStream) -> RefineResult<()> {
        let request = read_http_request(&mut stream)?;
        let response = self.handle_wire_request(request);
        write_http_response(&mut stream, response)
    }

    pub fn handle_wire_request(&self, request: HttpRequest) -> WireResponse {
        if request.path == "/events" || request.path == "/api/sse" {
            return match self.server_sent_events("events") {
                Ok(events) => WireResponse::sse(events),
                Err(error) => WireResponse::json(error_response(error)),
            };
        }

        if request.method != "GET" {
            if !local_origin_allowed(&request) {
                return WireResponse::json(ApiResponse::json(
                    403,
                    json!({
                        "error": {
                            "code": "forbidden_origin",
                            "message": "mutation request origin must be local"
                        }
                    }),
                ));
            }
            if let Some(version) = request.headers.get("x-refine-api-version")
                && version != API_CONTRACT_VERSION
            {
                return WireResponse::json(ApiResponse::json(
                    426,
                    json!({
                        "error": {
                            "code": "api_version_mismatch",
                            "message": "unsupported Refine API contract version"
                        },
                        "api_contract_version": API_CONTRACT_VERSION,
                        "supported_api_contract_versions": [API_CONTRACT_VERSION]
                    }),
                ));
            }
            if let Some(key) = request.headers.get("idempotency-key")
                && !valid_idempotency_key(key)
            {
                return WireResponse::json(ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_idempotency_key",
                            "message": "idempotency key must be 1-128 ASCII letters, digits, '.', '_', '-', or ':'"
                        }
                    }),
                ));
            }
        }

        if let Some(response) = self.try_static_response(&request.path) {
            return response;
        }

        let auth_token = request
            .headers
            .get("authorization")
            .and_then(|header| header.strip_prefix("Bearer "))
            .map(str::to_string)
            .or_else(|| request.headers.get("x-refine-token").cloned());
        let idempotency_key = request.headers.get("idempotency-key").cloned();
        let fingerprint = idempotency_key.as_ref().map(|_| {
            idempotency_fingerprint(
                &request.method,
                &normalize_api_path(&request.path),
                request.body.as_deref().unwrap_or(&[]),
            )
        });
        if let (Some(runtime_root), Some(key), Some(fingerprint)) = (
            self.server.runtime_root.as_ref(),
            idempotency_key.as_deref(),
            fingerprint.as_deref(),
        ) {
            match load_idempotency_record(runtime_root, key) {
                Ok(Some(record)) if record.fingerprint == fingerprint => {
                    return WireResponse::json(record.response);
                }
                Ok(Some(_)) => {
                    return WireResponse::json(ApiResponse::json(
                        409,
                        json!({
                            "error": {
                                "code": "idempotency_conflict",
                                "message": "idempotency key was already used for a different request"
                            }
                        }),
                    ));
                }
                Ok(None) => {}
                Err(error) => return WireResponse::json(error_response(error)),
            }
        }

        let method = request.method.clone();
        let path = request.path.clone();
        let response = self.server.handle(ApiRequest {
            method: request.method,
            path: request.path,
            auth_token,
            body: request
                .body
                .and_then(|body| serde_json::from_slice(&body).ok()),
        });
        if method != "GET"
            && response.status < 400
            && let Some(runtime_root) = self.server.runtime_root.as_ref()
            && let Err(error) =
                append_api_mutation_event(runtime_root, &method, &path, response.status)
        {
            return WireResponse::json(error_response(error));
        }
        if method != "GET"
            && response.status < 400
            && let Err(error) = self.server.refresh_projection_cache_after_mutation()
        {
            return WireResponse::json(error_response(error));
        }
        if let (Some(runtime_root), Some(key), Some(fingerprint)) = (
            self.server.runtime_root.as_ref(),
            idempotency_key.as_deref(),
            fingerprint.as_deref(),
        ) {
            if let Err(error) = save_idempotency_record(runtime_root, key, fingerprint, &response) {
                return WireResponse::json(error_response(error));
            }
        }
        WireResponse::json(response)
    }

    fn try_static_response(&self, path: &str) -> Option<WireResponse> {
        let static_root = self.static_root.as_ref()?;
        let relative = match path {
            "/" => PathBuf::from("index.html"),
            path => {
                let trimmed = path.trim_start_matches('/');
                if trimmed.contains("..") || trimmed.starts_with('/') {
                    return Some(WireResponse::json(ApiResponse::json(
                        400,
                        json!({
                            "error": {
                                "code": "invalid_path",
                                "message": "static asset path is invalid"
                            }
                        }),
                    )));
                }
                PathBuf::from(trimmed)
            }
        };
        let full_path = static_root.join(relative);
        if !is_within(static_root, &full_path) || !full_path.is_file() {
            return None;
        }
        let bytes = match fs::read(&full_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Some(WireResponse::json(ApiResponse::json(
                    500,
                    json!({
                        "error": {
                            "code": "static_read_failed",
                            "message": format!("failed to read static asset: {error}")
                        }
                    }),
                )));
            }
        };
        Some(WireResponse::bytes(
            200,
            content_type_for_path(&full_path),
            bytes,
        ))
    }
}

impl LocalDaemonWebServer for LocalHttpDaemon {
    fn serve(&self, port: u16) -> RefineResult<DaemonStatus> {
        self.recover_runtime_state()?;
        let listener = Self::bind_loopback(port)?;
        self.serve_next(&listener)?;
        Ok(self.server.status.clone())
    }

    fn server_sent_events(&self, stream: &str) -> RefineResult<String> {
        let mut events = String::new();
        events.push_str("retry: 3000\n");
        events.push_str(&sse_event(
            "ready",
            json!({
                "stream": stream,
                "backend": "native",
                "timestamp": now_timestamp_web()
            }),
        )?);
        events.push_str(&sse_event(
            "project_updated",
            json!({
                "gap_count": self.server.projection.gaps.len(),
                "feature_count": self.server.projection.features.len(),
                "status_counts": self.server.projection.status_counts(),
                "dashboard": self.server.projection.dashboard
            }),
        )?);
        events.push_str(&sse_event(
            "status_change",
            json!({
                "status_counts": self.server.projection.status_counts(),
                "attention": self.server.projection.dashboard.attention_indicators
            }),
        )?);
        if let Some(durable_root) = &self.server.durable_root {
            if let Some(entry) = FileActivityService::new(durable_root)
                .recent(1)?
                .into_iter()
                .next()
            {
                events.push_str(&sse_event("activity_added", json!(entry))?);
            }
            for payload in recent_chat_sse_events(durable_root, 10)? {
                events.push_str(&sse_event("chat_event", payload)?);
            }
        }
        if let Some(runtime_root) = &self.server.runtime_root {
            let process = process_summary_response(runtime_root).body;
            events.push_str(&sse_event(
                "system_operation",
                json!({
                    "message": "Process snapshot refreshed",
                    "status": "info",
                    "category": "process",
                    "timestamp": now_timestamp_web(),
                    "details": process
                }),
            )?);
            for payload in recent_process_sse_events(runtime_root, 10)? {
                events.push_str(&sse_event("process_output", payload)?);
            }
            for payload in recent_job_sse_events(runtime_root, 10)? {
                events.push_str(&sse_event("job_progress", payload)?);
            }
            for event in recent_api_mutation_events(runtime_root, 25)? {
                events.push_str(&sse_event(
                    "api_mutation",
                    json!({
                        "method": event.method,
                        "path": event.path,
                        "status": event.status,
                        "timestamp": event.created_at
                    }),
                )?);
            }
        }
        Ok(events)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
}

fn read_http_request(stream: &mut TcpStream) -> RefineResult<HttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| RefineError::Io(format!("failed to read HTTP request: {error}")))?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 1024 * 1024 {
            return Err(RefineError::InvalidInput(
                "HTTP request headers exceed 1 MiB".to_string(),
            ));
        }
    }

    let header_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| RefineError::InvalidInput("malformed HTTP request".to_string()))?
        + 4;
    let header_text = std::str::from_utf8(&buffer[..header_end]).map_err(|error| {
        RefineError::InvalidInput(format!("HTTP headers are not valid UTF-8: {error}"))
    })?;
    let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());
    let request_line = lines
        .next()
        .ok_or_else(|| RefineError::InvalidInput("missing HTTP request line".to_string()))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| RefineError::InvalidInput("missing HTTP method".to_string()))?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| RefineError::InvalidInput("missing HTTP path".to_string()))?
        .to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = stream
            .read(&mut chunk)
            .map_err(|error| RefineError::Io(format!("failed to read HTTP body: {error}")))?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = if content_length > 0 && buffer.len() >= header_end + content_length {
        Some(buffer[header_end..header_end + content_length].to_vec())
    } else {
        None
    };

    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_http_response(stream: &mut TcpStream, response: WireResponse) -> RefineResult<()> {
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: http://127.0.0.1\r\n",
        response.status,
        response.reason,
        response.content_type,
        response.body.len()
    );
    for (name, value) in response.extra_headers {
        headers.push_str(&format!("{name}: {value}\r\n"));
    }
    headers.push_str("\r\n");
    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(&response.body))
        .and_then(|_| stream.flush())
        .map_err(|error| RefineError::Io(format!("failed to write HTTP response: {error}")))
}

fn is_within(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root)
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        426 => "Upgrade Required",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::process::Command;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::core::host::process_supervision::ProcessSupervisor;
    use crate::core::product::project_state::{
        DashboardProjection, FeatureSummaryProjection, FileProjectStateStore, GapSummaryProjection,
        PROJECTION_SNAPSHOT_VERSION, ProjectStateStore, ProjectionSnapshot, RuntimeProjection,
    };
    use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
    use crate::model::gap::{GapIndexProjection, GapPriority};
    use crate::model::log::ActivityEntry;
    use crate::model::workflow::GapStatus;

    #[test]
    fn web_server_routes_work_gap_queries_through_projection() {
        let mut server = server_with_projection();
        server.projection.gaps.insert(
            "GAP2".to_string(),
            GapSummaryProjection {
                gap: GapIndexProjection {
                    id: "GAP2".to_string(),
                    name: "Settings route".to_string(),
                    status: GapStatus::Done,
                    priority: GapPriority::High,
                    reporter: Some("Alice".to_string()),
                    round_count: 3,
                    created: "created2".to_string(),
                    updated: "updated2".to_string(),
                    branch_name: None,
                    node_id: Some("node-b".to_string()),
                    feature_id: Some("FEA1".to_string()),
                    feature_order: Some(1),
                    json_path: "gaps/02/GAP2/gap.json".to_string(),
                },
                node_display_name: Some("Node B".to_string()),
                searchable_text: "Settings route Alice".to_string(),
                activity_ids: Vec::new(),
            },
        );
        server.projection.features.insert(
            "FEA1".to_string(),
            FeatureSummaryProjection {
                feature: FeatureIndexProjection {
                    id: "FEA1".to_string(),
                    name: "Settings Feature".to_string(),
                    description: Some("Settings work".to_string()),
                    reporter: Some("Alice".to_string()),
                    node_id: Some("node-b".to_string()),
                    created: "created".to_string(),
                    updated: "updated".to_string(),
                    json_path: "features/FE/A1/feature.json".to_string(),
                },
                status: GapStatus::Done,
                gap_ids: vec!["GAP2".to_string()],
                rollup: FeatureRollup {
                    status: GapStatus::Done,
                    gap_count: 1,
                    done_count: 1,
                    active_count: 0,
                    failed_count: 0,
                    cancelled_count: 0,
                    blocked_count: 0,
                    next_gap: None,
                },
            },
        );
        let response = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: None,
            body: None,
        });

        assert_eq!(response.status, 200);
        assert_eq!(response.body["gaps"].as_array().unwrap().len(), 2);
        assert_eq!(response.body["counts"]["todo"], 1);
        assert_eq!(response.body["counts"]["done"], 1);

        let filtered = server.handle(ApiRequest {
            method: "GET".to_string(),
            path:
                "/api/gaps?reporter=Alice&feature=FEA1&rounds_gte=2&sort=priority&dir=desc&limit=1"
                    .to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(filtered.status, 200);
        assert_eq!(filtered.body["gaps"][0]["id"], "GAP2");
        assert_eq!(filtered.body["filtered_counts"]["done"], 1);
        assert_eq!(filtered.body["matching_ids"], json!(["GAP2"]));
        assert_eq!(filtered.body["page"]["total"], 1);

        let features = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/features?q=settings&status=done&reporter=Alice&node=node-b".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(features.status, 200);
        assert_eq!(features.body["features"][0]["feature"]["id"], "FEA1");
        assert_eq!(features.body["matching_ids"], json!(["FEA1"]));
    }

    #[test]
    fn web_server_rejects_unauthorized_mutations() {
        let response = server_with_projection().handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: None,
            body: None,
        });

        assert_eq!(response.status, 401);
        assert_eq!(response.body["error"]["code"], "unauthorized");
    }

    #[test]
    fn web_server_issues_session_tokens_for_local_surface_mutations() {
        let temp_root = unique_temp_dir("http-session-auth");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let mut server = server_with_projection();
        server.auth_token = None;
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let session = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/sessions".to_string(),
            auth_token: None,
            body: Some(json!({"surface": "desktop"})),
        });
        assert_eq!(session.status, 201);
        let token = session.body["session"]["token"]
            .as_str()
            .unwrap()
            .to_string();

        let create = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some(token),
            body: Some(json!({"id": "GAP1", "name": "Session API Gap"})),
        });
        assert_eq!(create.status, 201);
        assert!(durable_root.join("gaps/GA/P1/gap.json").exists());
        assert!(runtime_root.join("surface-sessions.json").exists());
        assert!(runtime_root.join("security-audit.jsonl").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn local_http_daemon_validates_origin_version_and_idempotency_headers() {
        let daemon = LocalHttpDaemon {
            server: server_with_projection(),
            static_root: None,
        };

        let forbidden = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            headers: BTreeMap::from([("origin".to_string(), "https://example.com".to_string())]),
            body: Some(br#"{"name":"Bad"}"#.to_vec()),
        });
        assert_eq!(forbidden.status, 403);

        let version = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            headers: BTreeMap::from([("x-refine-api-version".to_string(), "999".to_string())]),
            body: Some(br#"{"name":"Bad"}"#.to_vec()),
        });
        assert_eq!(version.status, 426);

        let idempotency = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            headers: BTreeMap::from([("idempotency-key".to_string(), "bad key".to_string())]),
            body: Some(br#"{"name":"Bad"}"#.to_vec()),
        });
        assert_eq!(idempotency.status, 400);
    }

    #[test]
    fn local_http_daemon_replays_idempotent_mutation_responses() {
        let temp_root = unique_temp_dir("http-idempotency");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());
        let daemon = LocalHttpDaemon {
            server,
            static_root: None,
        };
        let body = br#"{"id":"GAP1","name":"Idempotent Gap"}"#.to_vec();
        let headers = BTreeMap::from([
            ("authorization".to_string(), "Bearer secret".to_string()),
            ("idempotency-key".to_string(), "create-gap-1".to_string()),
        ]);

        let first = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            headers: headers.clone(),
            body: Some(body.clone()),
        });
        assert_eq!(first.status, 201);
        let second = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            headers: headers.clone(),
            body: Some(body),
        });
        assert_eq!(second.status, 201);
        assert_eq!(first.body, second.body);
        assert_eq!(
            fs::read_dir(durable_root.join("gaps/GA/P1"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name() == "gap.json")
                .count(),
            1
        );
        assert!(
            runtime_root
                .join(IDEMPOTENCY_DIR)
                .join("create-gap-1.json")
                .exists()
        );
        let cached_projection: ProjectionSnapshot = serde_json::from_str(
            &fs::read_to_string(runtime_root.join("cache").join(PROJECTION_SNAPSHOT_FILE)).unwrap(),
        )
        .unwrap();
        assert!(cached_projection.gaps.contains_key("GAP1"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn local_http_daemon_rejects_idempotency_key_reuse_for_different_requests() {
        let temp_root = unique_temp_dir("http-idempotency-conflict");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root);
        server.runtime_root = Some(runtime_root);
        let daemon = LocalHttpDaemon {
            server,
            static_root: None,
        };
        let headers = BTreeMap::from([
            ("authorization".to_string(), "Bearer secret".to_string()),
            (
                "idempotency-key".to_string(),
                "create-gap-conflict".to_string(),
            ),
        ]);

        let first = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            headers: headers.clone(),
            body: Some(br#"{"id":"GAP1","name":"First"}"#.to_vec()),
        });
        assert_eq!(first.status, 201);
        let conflict = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            headers,
            body: Some(br#"{"id":"GAP2","name":"Second"}"#.to_vec()),
        });
        assert_eq!(conflict.status, 409);
        let body: serde_json::Value = serde_json::from_slice(&conflict.body).unwrap();
        assert_eq!(body["error"]["code"], "idempotency_conflict");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn local_http_daemon_persists_successful_mutations_for_sse() {
        let temp_root = unique_temp_dir("http-mutation-sse");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root);
        server.runtime_root = Some(runtime_root.clone());
        let daemon = LocalHttpDaemon {
            server,
            static_root: None,
        };

        let create = daemon.handle_wire_request(HttpRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            headers: BTreeMap::from([("authorization".to_string(), "Bearer secret".to_string())]),
            body: Some(br#"{"id":"GAP1","name":"SSE Gap"}"#.to_vec()),
        });
        assert_eq!(create.status, 201);
        assert!(runtime_root.join(API_EVENTS_FILE).exists());

        let sse = daemon.handle_wire_request(HttpRequest {
            method: "GET".to_string(),
            path: "/api/sse".to_string(),
            headers: BTreeMap::new(),
            body: None,
        });
        assert_eq!(sse.status, 200);
        let body = String::from_utf8(sse.body).unwrap();
        assert!(body.contains("event: api_mutation"));
        assert!(body.contains("\"path\":\"/work/gaps\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn local_http_daemon_serves_projection_routes_over_tcp() {
        let daemon = LocalHttpDaemon {
            server: server_with_projection(),
            static_root: None,
        };
        let listener = LocalHttpDaemon::bind_loopback(0).unwrap();
        let addr = LocalHttpDaemon::local_addr(&listener).unwrap();
        let handle = thread::spawn(move || daemon.serve_next(&listener).unwrap());

        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /work/gaps HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        handle.join().unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"id\": \"GAP1\""));
        assert!(response.contains("\"counts\""));
    }

    #[test]
    fn local_http_daemon_serves_static_assets() {
        let temp_root = unique_temp_dir("static-assets");
        fs::create_dir_all(&temp_root).unwrap();
        fs::write(
            temp_root.join("index.html"),
            "<!doctype html><title>Refine</title>",
        )
        .unwrap();
        let daemon = LocalHttpDaemon {
            server: server_with_projection(),
            static_root: Some(temp_root.clone()),
        };

        let response = daemon.handle_wire_request(HttpRequest {
            method: "GET".to_string(),
            path: "/".to_string(),
            headers: BTreeMap::new(),
            body: None,
        });

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert!(String::from_utf8(response.body).unwrap().contains("Refine"));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_transitions_gap_with_local_auth_and_durable_root() {
        let temp_root = unique_temp_dir("http-transition");
        let durable_root = temp_root.join(".refine");
        let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
        fs::create_dir_all(&gap_dir).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "HTTP transition",
              "status": "backlog",
              "priority": "low",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-01T00:00:00Z",
              "rounds": []
            }"#,
        )
        .unwrap();
        let projection = FileProjectStateStore::new(&durable_root)
            .rebuild_projection()
            .unwrap();
        let mut server = server_with_projection();
        server.projection = projection;
        server.durable_root = Some(durable_root.clone());

        let response = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps/GAP1/transition".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"status": "todo"})),
        });

        assert_eq!(response.status, 200);
        assert_eq!(response.body["gap"]["status"], "todo");
        assert!(
            fs::read_to_string(gap_dir.join("gap.json"))
                .unwrap()
                .contains("\"status\": \"todo\"")
        );
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_creates_and_shows_gap_with_local_auth() {
        let temp_root = unique_temp_dir("http-create-show");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        let create = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Created by API"})),
        });
        assert_eq!(create.status, 201);
        assert_eq!(create.body["gap"]["id"], "GAP1");
        assert!(durable_root.join("gaps/GA/P1/gap.json").exists());

        let show = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/work/gaps/GAP1".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(show.status, 200);
        assert_eq!(show.body["gap"]["name"], "Created by API");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_edits_notes_and_deletes_gap_with_local_auth() {
        let temp_root = unique_temp_dir("http-edit-note-delete");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Original"})),
        });

        let edit = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/work/gaps/GAP1".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"name": "Renamed", "priority": "high"})),
        });
        assert_eq!(edit.status, 200);
        assert_eq!(edit.body["gap"]["name"], "Renamed");
        assert_eq!(edit.body["gap"]["priority"], "high");

        let note = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps/GAP1/notes".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"author": "Reviewer", "body": "Needs context"})),
        });
        assert_eq!(note.status, 200);
        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"body\": \"Needs context\""));

        let delete = server.handle(ApiRequest {
            method: "DELETE".to_string(),
            path: "/work/gaps/GAP1".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(delete.status, 200);
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_appends_and_edits_latest_round_with_local_auth() {
        let temp_root = unique_temp_dir("http-rounds");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Round Gap"})),
        });

        let append = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps/GAP1/rounds".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"reporter": "Reporter", "actual": "Actual", "target": "Target"})),
        });
        assert_eq!(append.status, 200);
        assert_eq!(append.body["gap"]["round_count"], 1);

        let edit = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/work/gaps/GAP1/rounds/latest".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"reporter": "Reviewer", "actual": "Revised"})),
        });
        assert_eq!(edit.status, 200);
        assert_eq!(edit.body["gap"]["reporter"], "Reviewer");
        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"reporter\": \"Reviewer\""));
        assert!(written.contains("\"actual\": \"Revised\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_appends_and_reads_gap_round_logs_with_local_auth() {
        let temp_root = unique_temp_dir("http-gap-round-logs");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Logged Gap"})),
        });
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP1/rounds".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"reporter": "Reporter", "actual": "Actual", "target": "Target"})),
        });

        let append = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP1/rounds/0/logs".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "severity": "info",
                "category": "state",
                "actor": "refine",
                "message": "Workflow status changed: backlog -> todo"
            })),
        });
        assert_eq!(append.status, 200);
        assert!(durable_root.join("gaps/GA/P1/logs.jsonl").exists());

        let logs = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/gaps/GAP1/logs".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(logs.status, 200);
        assert_eq!(logs.body["round_log_count"], 1);
        assert_eq!(
            logs.body["logs"][0]["message"],
            "Workflow status changed: backlog -> todo"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_creates_features_and_updates_membership_with_local_auth() {
        let temp_root = unique_temp_dir("http-feature-membership");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Gap One"})),
        });

        let create_feature = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "FEA1", "name": "Feature One"})),
        });
        assert_eq!(create_feature.status, 201);
        assert_eq!(create_feature.body["feature"]["id"], "FEA1");

        let add_gap = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"gap_id": "GAP1"})),
        });
        assert_eq!(add_gap.status, 200);
        assert_eq!(add_gap.body["gap_ids"], json!(["GAP1"]));

        let show = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/work/features/FEA1".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(show.status, 200);
        assert_eq!(show.body["gap_ids"], json!(["GAP1"]));

        let remove_gap = server.handle(ApiRequest {
            method: "DELETE".to_string(),
            path: "/work/features/FEA1/gaps/GAP1".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(remove_gap.status, 200);
        assert_eq!(remove_gap.body["gap_ids"], json!([]));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_reorders_and_moves_feature_workflow_with_local_auth() {
        let temp_root = unique_temp_dir("http-feature-reorder-move");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
            server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/work/gaps".to_string(),
                auth_token: Some("secret".to_string()),
                body: Some(json!({"id": id, "name": name})),
            });
        }
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "FEA1", "name": "Feature One"})),
        });
        for gap_id in ["GAP1", "GAP2"] {
            server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/work/features/FEA1/gaps".to_string(),
                auth_token: Some("secret".to_string()),
                body: Some(json!({"gap_id": gap_id})),
            });
        }

        let reorder = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/gaps/GAP2/reorder".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"order": 1})),
        });
        assert_eq!(reorder.status, 200);
        assert_eq!(reorder.body["gap_ids"], json!(["GAP2", "GAP1"]));

        let move_feature = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/move".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"status": "todo"})),
        });
        assert_eq!(move_feature.status, 200);
        assert_eq!(move_feature.body["rollup"]["status"], "todo");
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"status\": \"todo\"")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_updates_feature_metadata_and_runs_gap_actions() {
        let temp_root = unique_temp_dir("http-feature-gap-actions");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        for (id, name) in [
            ("GAP1", "Verify Gap"),
            ("GAP2", "Retry Quality"),
            ("GAP3", "Retry Merge"),
        ] {
            server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/api/gaps".to_string(),
                auth_token: Some("secret".to_string()),
                body: Some(json!({"id": id, "name": name})),
            });
        }
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/features".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "FEA1", "name": "Original Feature"})),
        });

        let feature = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/features/FEA1".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "name": "Renamed Feature",
                "description": "Updated description",
                "reporter": "QA"
            })),
        });
        assert_eq!(feature.status, 200);
        assert_eq!(feature.body["feature"]["name"], "Renamed Feature");
        assert_eq!(
            feature.body["feature"]["description"],
            "Updated description"
        );

        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/bulk".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "selected_ids": ["GAP1"],
                "update": {"status": "review"}
            })),
        });
        let verified = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP1/verify".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(verified.status, 200);
        assert_eq!(verified.body["ok"], true);
        assert_eq!(verified.body["gap"]["status"], "done");

        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/bulk".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "selected_ids": ["GAP2", "GAP3"],
                "update": {"status": "failed"}
            })),
        });
        let retry_quality = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP2/retry-quality".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(retry_quality.status, 200);
        assert_eq!(retry_quality.body["gap"]["status"], "qa");

        let retry_merge = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP3/retry-merge".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(retry_merge.status, 200);
        assert_eq!(retry_merge.body["gap"]["status"], "ready-merge");

        let merge = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP3/merge".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(merge.status, 200);
        assert_eq!(merge.body["gap"]["status"], "done");

        let undo = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP3/undo".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(undo.status, 200);
        assert_eq!(undo.body["gap"]["status"], "review");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_schedules_workflow_through_file_scheduler_service() {
        let temp_root = unique_temp_dir("http-workflow-schedule");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Schedulable"})),
        });
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP1/transition".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"status": "todo"})),
        });

        let schedule = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/workflow/schedule".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(schedule.status, 200);
        assert_eq!(schedule.body["promoted"], 1);
        assert_eq!(schedule.body["reservations"][0]["gap_id"], "GAP1");
        assert!(runtime_root.join("scheduler-state.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_cancels_and_deletes_features_with_local_auth() {
        let temp_root = unique_temp_dir("http-feature-cancel-delete");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());
        for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
            server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/work/gaps".to_string(),
                auth_token: Some("secret".to_string()),
                body: Some(json!({"id": id, "name": name})),
            });
        }
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "FEA1", "name": "Feature One"})),
        });
        for gap_id in ["GAP1", "GAP2"] {
            server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/work/features/FEA1/gaps".to_string(),
                auth_token: Some("secret".to_string()),
                body: Some(json!({"gap_id": gap_id})),
            });
        }

        let gap_cancel = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/gaps/GAP1/cancel".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(gap_cancel.status, 200);
        assert_eq!(gap_cancel.body["gap"]["status"], "cancelled");

        let supervisor = FileProcessSupervisor::new(&runtime_root);
        let process = supervisor
            .register(ManagedProcess {
                id: "agent-gap2".to_string(),
                owner: crate::core::host::process_supervision::ProcessOwner::Agent,
                pid: None,
                state: "running".to_string(),
                label: Some("agent".to_string()),
                details: Some("working on GAP2".to_string()),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: "now".to_string(),
                exit_code: None,
            })
            .unwrap();
        let job = FileJobRegistry::new(&runtime_root)
            .register("feature FEA1 gap GAP2")
            .unwrap();

        let feature_cancel = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/work/features/FEA1/cancel".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(feature_cancel.status, 200);
        assert_eq!(feature_cancel.body["rollup"]["cancelled_count"], 2);
        assert_eq!(feature_cancel.body["runtime_reconciled"]["processes"], 1);
        assert_eq!(feature_cancel.body["runtime_reconciled"]["jobs"], 1);
        assert_eq!(supervisor.inspect(&process.id).unwrap().state, "stopped");
        assert_eq!(
            FileJobRegistry::new(&runtime_root)
                .status(&job.id)
                .unwrap()
                .state,
            JobState::Cancelled
        );

        let feature_delete = server.handle(ApiRequest {
            method: "DELETE".to_string(),
            path: "/work/features/FEA1".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(feature_delete.status, 200);
        assert!(!durable_root.join("features/FE/A1/feature.json").exists());
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
        assert!(!durable_root.join("gaps/GA/P2/gap.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_accepts_static_ui_api_aliases_for_work_routes() {
        let temp_root = unique_temp_dir("http-api-aliases");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        let create_gap = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Gap One"})),
        });
        assert_eq!(create_gap.status, 201);
        let create_feature = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/features".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "FEA1", "name": "Feature One"})),
        });
        assert_eq!(create_feature.status, 201);

        let add_gap = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/features/FEA1/gaps/GAP1".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(add_gap.status, 200);
        assert_eq!(add_gap.body["gap_ids"], json!(["GAP1"]));

        let workflow = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/features/FEA1/workflow".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"status": "todo"})),
        });
        assert_eq!(workflow.status, 200);
        assert_eq!(workflow.body["rollup"]["status"], "todo");

        let cancel = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/GAP1/cancel".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(cancel.status, 200);
        assert_eq!(cancel.body["gap"]["status"], "cancelled");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_accepts_static_ui_bulk_api_aliases() {
        let temp_root = unique_temp_dir("http-bulk-api-aliases");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        for (id, name) in [("GAP1", "Bulk One"), ("GAP2", "Bulk Two")] {
            let create = server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/api/gaps".to_string(),
                auth_token: Some("secret".to_string()),
                body: Some(json!({"id": id, "name": name})),
            });
            assert_eq!(create.status, 201);
        }
        let create_feature = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/features".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "FEA1", "name": "Bulk Feature"})),
        });
        assert_eq!(create_feature.status, 201);

        let bulk_status = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/bulk".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "selected_ids": ["GAP1", "GAP2"],
                "update": {"status": "todo"}
            })),
        });
        assert_eq!(bulk_status.status, 200);
        assert_eq!(bulk_status.body["updated"], 2);

        let bulk_assign = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/features/FEA1/gaps/bulk".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"selected_ids": ["GAP1", "GAP2"]})),
        });
        assert_eq!(bulk_assign.status, 200);
        assert_eq!(bulk_assign.body["updated"], 2);
        assert_eq!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"feature_id\": \"FEA1\""),
            true
        );

        let bulk_delete = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps/bulk/delete".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"selected_ids": ["GAP1"]})),
        });
        assert_eq!(bulk_delete.status, 200);
        assert_eq!(bulk_delete.body["deleted"], 1);
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
        assert!(durable_root.join("gaps/GA/P2/gap.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_records_and_lists_activity_for_static_ui() {
        let temp_root = unique_temp_dir("http-activity");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        let recorded = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/activity/ui-error".to_string(),
            auth_token: None,
            body: Some(json!({"message": "Boom", "source": "test"})),
        });
        assert_eq!(recorded.status, 200);
        assert_eq!(recorded.body["recorded"], true);
        assert!(durable_root.join("logs/activity.jsonl").exists());

        let listed = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/activity".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(listed.status, 200);
        assert_eq!(listed.body["activity"][0]["message"], "Boom");
        assert_eq!(listed.body["facets"]["categories"], json!(["ui"]));

        let filtered = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/activity?q=source&limit=1".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(filtered.status, 200);
        assert_eq!(filtered.body["page"]["limit"], 1);
        assert_eq!(filtered.body["activity"][0]["message"], "Boom");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_parses_and_persists_imported_gaps_with_feature_destination() {
        let temp_root = unique_temp_dir("http-import-persist");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        let parsed = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/import/csv/parse".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "text": "name,actual,target,reporter,priority\nCSV Gap,Actual state,Target state,QA,high\n"
            })),
        });
        assert_eq!(parsed.status, 200);
        assert_eq!(parsed.body["drafts"][0]["name"], "CSV Gap");
        assert_eq!(parsed.body["drafts"][0]["priority"], "high");

        let persisted = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/import/persist".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "new_feature_name": "Imported Feature",
                "drafts": [{
                    "name": "Imported Gap",
                    "actual": "Actual state",
                    "target": "Target state",
                    "reporter": "QA",
                    "priority": "high"
                }]
            })),
        });
        assert_eq!(persisted.status, 201);
        assert_eq!(persisted.body["count"], 1);
        assert_eq!(persisted.body["feature"]["name"], "Imported Feature");
        let gap_id = persisted.body["gaps"][0]["id"].as_str().unwrap();
        let feature_id = persisted.body["feature"]["id"].as_str().unwrap();

        let gap = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/gaps/{gap_id}"),
            auth_token: None,
            body: None,
        });
        assert_eq!(gap.status, 200);
        assert_eq!(gap.body["gap"]["priority"], "high");
        assert_eq!(gap.body["gap"]["reporter"], "QA");
        assert_eq!(gap.body["gap"]["round_count"], 1);
        assert_eq!(gap.body["gap"]["feature_id"], feature_id);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_rebuilds_projection_cache_and_serves_changes_performance_routes() {
        let temp_root = unique_temp_dir("http-cache-changes-performance");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Cached Gap"})),
        });
        let rebuilt = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/cache/rebuild".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"background": true})),
        });
        assert_eq!(rebuilt.status, 200);
        assert_eq!(rebuilt.body["gaps"], 1);
        assert!(
            runtime_root
                .join("cache")
                .join(PROJECTION_SNAPSHOT_FILE)
                .exists()
        );

        let changes = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/changes?limit=10".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(changes.status, 200);
        assert_eq!(changes.body["branch"], "main");
        assert_eq!(changes.body["changes"], json!([]));

        let metrics = FileMetricsService::new(&runtime_root);
        metrics
            .record_operation(
                "cache.rebuild",
                25.0,
                true,
                json!({"resource_backend": "jsonl"}),
            )
            .unwrap();
        metrics
            .record_operation("provider.turn", 50.0, false, json!({}))
            .unwrap();
        let performance = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/performance?operation=cache.rebuild&limit=10".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(performance.status, 200);
        assert_eq!(performance.body["events"][0]["operation"], "cache.rebuild");
        assert_eq!(performance.body["filtered_event_count"], 1);
        assert_eq!(performance.body["total_event_count"], 2);
        assert_eq!(
            performance.body["operations"],
            json!(["cache.rebuild", "provider.turn"])
        );
        let cached = FileProjectStateStore::new(&durable_root)
            .load_projection_snapshot(&runtime_root.join("cache"))
            .unwrap()
            .unwrap();
        assert_eq!(
            cached
                .runtime
                .performance
                .unwrap()
                .get("filtered_event_count"),
            Some(&json!(1))
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_lists_git_changes_and_reverts_commits() {
        let temp_root = unique_temp_dir("http-git-changes");
        let durable_root = temp_root.join(".refine");
        fs::create_dir_all(&durable_root).unwrap();
        git(&temp_root, &["init"]).unwrap();
        git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
        git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
        fs::write(temp_root.join("app.txt"), "one\n").unwrap();
        git(&temp_root, &["add", "app.txt"]).unwrap();
        git(&temp_root, &["commit", "-m", "initial"]).unwrap();
        fs::write(temp_root.join("app.txt"), "two\n").unwrap();
        git(&temp_root, &["commit", "-am", "update app"]).unwrap();

        let mut server = server_with_projection();
        server.durable_root = Some(durable_root);

        let changes = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/changes?limit=5".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(changes.status, 200);
        assert_eq!(changes.body["changes"][0]["subject"], "update app");
        let commit = changes.body["changes"][0]["commit"].as_str().unwrap();

        let undo = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/changes/undo".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"commit": commit})),
        });
        assert_eq!(undo.status, 200);
        assert_eq!(undo.body["ok"], true);
        assert_eq!(
            fs::read_to_string(temp_root.join("app.txt")).unwrap(),
            "one\n"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_hard_resets_git_worktree() {
        let temp_root = unique_temp_dir("http-git-reset");
        let durable_root = temp_root.join(".refine");
        fs::create_dir_all(&durable_root).unwrap();
        git(&temp_root, &["init"]).unwrap();
        git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
        git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
        fs::write(temp_root.join("app.txt"), "committed\n").unwrap();
        git(&temp_root, &["add", "app.txt"]).unwrap();
        git(&temp_root, &["commit", "-m", "initial"]).unwrap();
        fs::write(temp_root.join("app.txt"), "dirty\n").unwrap();

        let mut server = server_with_projection();
        server.durable_root = Some(durable_root);
        let reset = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/runner-workers/merger/hard-reset-worktree".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(reset.status, 200);
        assert_eq!(reset.body["ok"], true);
        assert_eq!(
            fs::read_to_string(temp_root.join("app.txt")).unwrap(),
            "committed\n"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_cleans_activity_and_reports_unconnected_native_actions() {
        let temp_root = unique_temp_dir("http-cleanups");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/activity/ui-error".to_string(),
            auth_token: None,
            body: Some(json!({"message": "Boom"})),
        });
        assert!(durable_root.join("logs/activity.jsonl").exists());
        let cleanup = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/activity/cleanup".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"days": 0})),
        });
        assert_eq!(cleanup.status, 200);
        assert_eq!(cleanup.body["deleted"], 1);
        assert!(!durable_root.join("logs/activity.jsonl").exists());

        let runtime_root = temp_root.join("run/8080");
        server.runtime_root = Some(runtime_root.clone());
        let metrics = FileMetricsService::new(&runtime_root);
        metrics
            .record_operation("old", 10.0, true, json!({}))
            .unwrap();
        let performance_cleanup = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/performance/cleanup".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"clear": true})),
        });
        assert_eq!(performance_cleanup.status, 200);
        assert_eq!(performance_cleanup.body["deleted"], 1);
        assert!(!metrics.path().exists());

        let undo = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/changes/undo".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"commit": "abc123"})),
        });
        assert_eq!(undo.status, 200);
        assert_eq!(undo.body["ok"], false);

        let reset = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/runner-workers/merger/hard-reset-worktree".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(reset.status, 200);
        assert_eq!(reset.body["ok"], false);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_manages_nodes_and_transfers_gap_ownership() {
        let temp_root = unique_temp_dir("http-node-transfer");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        for (id, name) in [("GAP1", "Transfer One"), ("GAP2", "Transfer Two")] {
            server.handle(ApiRequest {
                method: "POST".to_string(),
                path: "/api/gaps".to_string(),
                auth_token: Some("secret".to_string()),
                body: Some(json!({"id": id, "name": name})),
            });
        }

        let created = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/nodes".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"display_name": "Remote QA"})),
        });
        assert_eq!(created.status, 200);
        assert!(
            created.body["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["id"] == "remote-qa")
        );

        let activated = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/nodes/activate".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"node_id": "remote-qa"})),
        });
        assert_eq!(activated.status, 200);
        assert_eq!(activated.body["active_node_id"], "remote-qa");
        assert!(durable_root.join("active-node.json").exists());

        let transfer = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/nodes/transfer-gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "selected_ids": ["GAP1", "GAP2"],
                "target_node_id": "remote-qa"
            })),
        });
        assert_eq!(transfer.status, 200);
        assert_eq!(transfer.body["updated"], 2);
        let gap = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/gaps/GAP1".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(gap.body["gap"]["node_id"], "remote-qa");

        let renamed = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/nodes/remote-qa".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"display_name": "Remote QA Renamed"})),
        });
        assert_eq!(renamed.status, 200);
        assert!(
            renamed.body["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["display_name"] == "Remote QA Renamed")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_manages_cluster_node_registry() {
        let temp_root = unique_temp_dir("http-cluster-registry");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        let registered = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/cluster/nodes".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "id": "node-1",
                "display_name": "Node One",
                "ssh_host": "example.com",
                "target_app_path": "/srv/app"
            })),
        });
        assert_eq!(registered.status, 200);
        assert_eq!(registered.body["enabled"], true);
        assert_eq!(registered.body["nodes"][0]["ssh_host"], "example.com");
        assert!(durable_root.join("cluster.json").exists());

        let disabled = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/cluster/nodes/node-1".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"enabled": false, "ssh_port": 2222})),
        });
        assert_eq!(disabled.status, 200);
        assert_eq!(disabled.body["nodes"][0]["enabled"], false);
        assert_eq!(disabled.body["nodes"][0]["ssh_port"], 2222);

        let bootstrap = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/cluster/nodes/node-1/bootstrap".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"dry_run": true})),
        });
        assert_eq!(bootstrap.status, 200);
        assert_eq!(bootstrap.body["ok"], true);
        assert_eq!(bootstrap.body["dry_run"], true);
        assert!(
            bootstrap.body["result"]["command"]
                .as_str()
                .unwrap()
                .contains("ssh -p 2222")
        );
        assert_eq!(
            bootstrap.body["cluster"]["nodes"][0]["health"]["status"],
            "ready"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_serves_source_file_tree_read_and_search() {
        let temp_root = unique_temp_dir("http-files");
        let durable_root = temp_root.join(".refine");
        fs::create_dir_all(temp_root.join("src")).unwrap();
        fs::create_dir_all(&durable_root).unwrap();
        fs::write(temp_root.join("README.md"), "hello\nworld\n").unwrap();
        fs::write(temp_root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(durable_root.join("settings.json"), "{}").unwrap();
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        let tree = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/files/tree?path=&recursive=1&max_depth=2&max_entries=20".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(tree.status, 200);
        let root_entries = tree.body["entries_by_path"][""].as_array().unwrap();
        assert!(
            root_entries
                .iter()
                .any(|entry| entry["path"] == "README.md")
        );
        assert!(root_entries.iter().any(|entry| entry["path"] == "src"));
        assert!(!root_entries.iter().any(|entry| entry["path"] == ".refine"));
        assert!(
            tree.body["entries_by_path"]["src"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "src/main.rs")
        );

        let read = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/files/read?path=README.md&offset=0&limit=6".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(read.status, 200);
        assert_eq!(read.body["previewable"], true);
        assert_eq!(read.body["content"], "hello\n");
        assert_eq!(read.body["has_more"], true);
        assert_eq!(read.body["next_offset"], 6);

        let search = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/files/search?q=main&max_entries=5".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(search.status, 200);
        assert_eq!(search.body["entries"][0]["path"], "src/main.rs");

        let traversal = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/files/read?path=../Cargo.toml".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(traversal.status, 400);
        assert_eq!(traversal.body["error"]["code"], "invalid_input");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_serves_project_utility_upgrade_health_and_sse_routes() {
        let temp_root = unique_temp_dir("http-project-utils");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(temp_root.join("child")).unwrap();
        fs::create_dir_all(&durable_root).unwrap();
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let path = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!(
                "/api/project/path?path={}",
                percent_encode_for_test(temp_root.to_str().unwrap())
            ),
            auth_token: None,
            body: None,
        });
        assert_eq!(path.status, 200);
        assert_eq!(path.body["exists"], true);
        assert_eq!(path.body["is_dir"], true);

        let directories = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!(
                "/api/project/directories?path={}&max_entries=10",
                percent_encode_for_test(temp_root.to_str().unwrap())
            ),
            auth_token: None,
            body: None,
        });
        assert_eq!(directories.status, 200);
        assert!(
            directories.body["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["name"] == "child")
        );

        let upgrade = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/upgrade".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(upgrade.status, 200);
        assert_eq!(upgrade.body["upgrade"]["available"], false);
        assert_eq!(upgrade.body["upgrade"]["upgrade_available"], false);
        assert_eq!(upgrade.body["upgrade"]["local_development"], true);

        let install = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/system/install".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"target": "linux-cli-web", "version": "1.0.0"})),
        });
        assert_eq!(install.status, 200);
        assert_eq!(install.body["install"]["installed"], true);
        assert_eq!(install.body["install"]["target"], "linux_cli_web");

        let install_status = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/system/install".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(install_status.status, 200);
        assert_eq!(install_status.body["install"]["installed"], true);

        let update = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/system/update".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"version": "1.1.0"})),
        });
        assert_eq!(update.status, 200);
        assert_eq!(update.body["install"]["version"], "1.1.0");

        let rollback = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/system/rollback".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(rollback.status, 200);
        assert_eq!(rollback.body["install"]["version"], "1.0.0");

        let uninstall = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/system/uninstall".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(uninstall.status, 200);
        assert_eq!(uninstall.body["uninstalled"], true);

        let health = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/target-app/health".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(health.status, 200);
        assert_eq!(health.body["last_check_ok"], true);

        let job_registry = FileJobRegistry::new(&runtime_root);
        let job = job_registry.register("sse-job").unwrap();
        job_registry
            .append_log(
                &job.id,
                LogEntry {
                    datetime: String::new(),
                    severity: "info".to_string(),
                    category: "job".to_string(),
                    message: "SSE job progress".to_string(),
                    details: None,
                    actions: Vec::new(),
                    actor: None,
                    gap_id: None,
                },
            )
            .unwrap();
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        let stdout_path = runtime_root.join("sse.stdout.log");
        fs::write(&stdout_path, "SSE process output\n").unwrap();
        supervisor
            .register(ManagedProcess {
                id: "sse-process".to_string(),
                owner: crate::core::host::process_supervision::ProcessOwner::UserHelper,
                pid: None,
                state: "completed".to_string(),
                label: Some("sse".to_string()),
                details: None,
                stdout_path: Some(stdout_path.display().to_string()),
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: Some(0),
            })
            .unwrap();
        let chat = FileChatService::new(&durable_root);
        let session = chat
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();
        chat.interrupt(&session.id, "SSE chat event").unwrap();

        let daemon = LocalHttpDaemon {
            server,
            static_root: None,
        };
        let sse = daemon.handle_wire_request(HttpRequest {
            method: "GET".to_string(),
            path: "/api/sse".to_string(),
            headers: BTreeMap::new(),
            body: None,
        });
        assert_eq!(sse.status, 200);
        assert_eq!(sse.content_type, "text/event-stream");
        let sse_body = String::from_utf8(sse.body).unwrap();
        assert!(sse_body.contains("event: ready"));
        assert!(sse_body.contains("event: project_updated"));
        assert!(sse_body.contains("event: status_change"));
        assert!(sse_body.contains("event: system_operation"));
        assert!(sse_body.contains("event: process_output"));
        assert!(sse_body.contains("SSE process output"));
        assert!(sse_body.contains("event: job_progress"));
        assert!(sse_body.contains("SSE job progress"));
        assert!(sse_body.contains("event: chat_event"));
        assert!(sse_body.contains("SSE chat event"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_reads_and_cancels_runtime_jobs() {
        let temp_root = unique_temp_dir("http-jobs");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&durable_root).unwrap();
        let registry = FileJobRegistry::new(&runtime_root);
        let job = registry.register("bulk_update_gaps").unwrap();
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let status = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/jobs/{}", job.id),
            auth_token: None,
            body: None,
        });
        assert_eq!(status.status, 200);
        assert_eq!(status.body["job"]["status"], "running");
        let cached = FileProjectStateStore::new(&durable_root)
            .load_projection_snapshot(&runtime_root.join("cache"))
            .unwrap()
            .unwrap();
        assert_eq!(cached.runtime.background_jobs[0]["id"], job.id);
        assert_eq!(cached.runtime.background_jobs[0]["status"], "running");

        let cancel = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/jobs/{}/cancel", job.id),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(cancel.status, 200);
        assert_eq!(cancel.body["job"]["status"], "cancelled");
        let logs = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/jobs/{}/logs?limit=10", job.id),
            auth_token: None,
            body: None,
        });
        assert_eq!(logs.status, 200);
        assert_eq!(logs.body["total"], 2);
        assert!(
            logs.body["logs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["message"] == "Job cancelled")
        );
        let cached = FileProjectStateStore::new(&durable_root)
            .load_projection_snapshot(&runtime_root.join("cache"))
            .unwrap()
            .unwrap();
        assert_eq!(cached.runtime.background_jobs[0]["status"], "cancelled");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_retries_scheduler_jobs_and_reads_retry_logs() {
        let temp_root = unique_temp_dir("http-job-retry");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&durable_root).unwrap();
        let scheduler = FileSchedulingService::new(&runtime_root);
        let reservation_id = scheduler.reserve("GAP1").unwrap();
        let job_id = scheduler.dispatch(&reservation_id).unwrap();
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let retry = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/jobs/{job_id}/retry"),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(retry.status, 200);
        assert_eq!(retry.body["retried_from"], job_id);
        assert_eq!(retry.body["job"]["owner"], "gap:GAP1");
        assert_ne!(retry.body["job"]["id"], job_id);

        let logs = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/jobs/{job_id}/logs?limit=1"),
            auth_token: None,
            body: None,
        });
        assert_eq!(logs.status, 200);
        assert_eq!(logs.body["log_count"], 1);
        assert_eq!(logs.body["has_more"], true);
        assert_eq!(logs.body["total"], 2);

        let cached = FileProjectStateStore::new(&durable_root)
            .load_projection_snapshot(&runtime_root.join("cache"))
            .unwrap()
            .unwrap();
        assert!(
            cached
                .runtime
                .background_jobs
                .iter()
                .any(|job| job["id"] == retry.body["job"]["id"])
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_lists_processes_and_updates_pause_controls() {
        let temp_root = unique_temp_dir("http-processes");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&durable_root).unwrap();
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        supervisor
            .launch(crate::core::host::process_supervision::ManagedProcessSpec {
                owner: crate::core::host::process_supervision::ProcessOwner::Agent,
                command: if cfg!(windows) { "cmd" } else { "sh" }.to_string(),
                args: if cfg!(windows) {
                    vec!["/C".to_string(), "echo agent".to_string()]
                } else {
                    vec!["-c".to_string(), "echo agent".to_string()]
                },
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
            })
            .unwrap();
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let listed = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/processes".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(listed.status, 200);
        assert_eq!(listed.body["processes"][0]["kind"], "agent");
        assert_eq!(listed.body["runner_reachable"], true);
        let cached = FileProjectStateStore::new(&durable_root)
            .load_projection_snapshot(&runtime_root.join("cache"))
            .unwrap()
            .unwrap();
        assert_eq!(cached.runtime.processes[0]["kind"], "agent");
        assert_eq!(
            cached.runtime.supervisor.unwrap()["runner_reachable"],
            json!(true)
        );

        let stdout_path = runtime_root.join("stream.stdout.log");
        let stderr_path = runtime_root.join("stream.stderr.log");
        fs::write(&stdout_path, "hello stdout\n").unwrap();
        fs::write(&stderr_path, "warn stderr\n").unwrap();
        supervisor
            .register(crate::core::host::process_supervision::ManagedProcess {
                id: "stream-test".to_string(),
                owner: crate::core::host::process_supervision::ProcessOwner::UserHelper,
                pid: None,
                state: "completed".to_string(),
                label: Some("stream".to_string()),
                details: None,
                stdout_path: Some(stdout_path.display().to_string()),
                stderr_path: Some(stderr_path.display().to_string()),
                stdin_path: None,
                limits: None,
                started_at: String::new(),
                exit_code: Some(0),
            })
            .unwrap();
        let stream = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/processes/stream-test/stream".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(stream.status, 200);
        assert_eq!(stream.body["process_id"], "stream-test");
        assert!(
            stream.body["output"]
                .as_str()
                .unwrap()
                .contains("hello stdout")
        );
        assert!(
            stream.body["output"]
                .as_str()
                .unwrap()
                .contains("warn stderr")
        );

        let background = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/processes/background".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"stopped": true})),
        });
        assert_eq!(background.status, 200);
        assert_eq!(background.body["background_processes_stopped"], true);

        let agents = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/processes/agents".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"paused": true})),
        });
        assert_eq!(agents.status, 200);
        assert_eq!(agents.body["agents_paused"], true);
        assert!(runtime_root.join("process-control.json").exists());
        let cached = FileProjectStateStore::new(&durable_root)
            .load_projection_snapshot(&runtime_root.join("cache"))
            .unwrap()
            .unwrap();
        assert_eq!(
            cached.runtime.supervisor.unwrap()["agents_paused"],
            json!(true)
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_reports_provider_diagnostics_for_agents_and_recheck() {
        let server = server_with_projection();

        let agents = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/agents".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(agents.status, 200);
        assert!(agents.body["providers"].as_array().unwrap().len() >= 5);
        assert_eq!(agents.body["stage"], "provider_detection");

        let diagnostics = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/agents/smoke-ai/diagnostics".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(diagnostics.status, 200);
        assert_eq!(diagnostics.body["provider"], "smoke-ai");
        assert!(
            diagnostics.body["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry.as_str().unwrap_or("").contains("Smoke AI"))
        );

        let configured = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/agents/smoke-ai/configure".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(configured.status, 200);
        assert_eq!(configured.body["configured"], true);

        let auth = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/agents/smoke-ai/auth".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert!(auth.status == 200 || auth.status == 503);
        if auth.status == 503 {
            assert_eq!(auth.body["error"]["code"], "degraded");
        }

        let invalid = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/agents/not-a-provider/diagnostics".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(invalid.status, 400);
        assert_eq!(invalid.body["error"]["code"], "invalid_input");

        let recheck = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/settings/recheck-auth".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(recheck.status, 200);
        assert!(recheck.body["message"].as_str().unwrap().contains("CLI"));
    }

    #[test]
    fn web_server_manages_quality_settings_and_regressions() {
        let temp_root = unique_temp_dir("http-quality");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        write_fake_playwright(&temp_root, 0);
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let app_settings = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/settings".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"target_app_url": "http://127.0.0.1:3000"})),
        });
        assert_eq!(app_settings.status, 200);

        let initial = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/quality".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(initial.status, 200);
        assert_eq!(initial.body["enabled"], "0");
        assert_eq!(initial.body["timing"], "pre_merge");

        let saved = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/quality".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "enabled": "1",
                "timing": "post_rebuild",
                "regressions_enabled": true,
                "business_requirements": "Dashboard must render",
                "instructions": "Run focused checks"
            })),
        });
        assert_eq!(saved.status, 200);
        assert_eq!(saved.body["enabled"], "1");
        assert_eq!(saved.body["timing"], "post_rebuild");
        assert_eq!(saved.body["regressions_enabled"], "1");
        assert_eq!(saved.body["configured"], true);

        let checks = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/quality/checks".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "owner_id": "GAP1",
                "command": "printf quality-ok"
            })),
        });
        assert_eq!(checks.status, 200);
        assert_eq!(checks.body["ok"], true);
        assert_eq!(checks.body["result"]["owner_id"], "GAP1");
        assert_eq!(checks.body["job"]["owner"], "quality:GAP1");
        assert_eq!(checks.body["job"]["status"], "complete");
        assert!(
            checks.body["result"]["diagnostics"][0]
                .as_str()
                .unwrap()
                .contains("quality-ok")
        );
        let quality_job_id = checks.body["job"]["id"].as_str().unwrap();
        let quality_job_logs = FileJobRegistry::new(&runtime_root)
            .page_logs(quality_job_id, 10, 0)
            .unwrap()
            .0;
        assert!(
            quality_job_logs
                .iter()
                .any(|log| log.message == "Quality checks passed")
        );

        let created = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/quality/regressions".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "title": "Dashboard smoke",
                "prompt": "Open the dashboard",
                "description": "Dashboard scenario"
            })),
        });
        assert_eq!(created.status, 201);
        assert_eq!(created.body["regression"]["id"], "dashboard-smoke");
        assert!(
            durable_root
                .join("regressions/specs/dashboard-smoke.js")
                .exists()
        );

        let run = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/quality/regressions/run".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({})),
        });
        assert_eq!(run.status, 200);
        assert_eq!(run.body["ok"], true);
        assert_eq!(run.body["runs"].as_array().unwrap().len(), 1);
        assert_eq!(
            run.body["runs"][0]["message"],
            "Playwright regression passed"
        );
        assert!(
            run.body["runs"][0]["json_report_path"]
                .as_str()
                .unwrap()
                .ends_with("report.json")
        );

        let screenshots = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/quality/screenshots?owner_id=GAP1".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(screenshots.status, 200);
        assert_eq!(screenshots.body["owner_id"], "GAP1");
        assert_eq!(screenshots.body["screenshot_count"], 1);
        assert!(
            screenshots.body["screenshots"][0]
                .as_str()
                .unwrap()
                .ends_with("screenshot.png")
        );

        let listed = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/quality".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(listed.status, 200);
        assert_eq!(listed.body["regressions"][0]["latest_run"]["ok"], true);

        let disabled = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/quality/regressions/dashboard-smoke".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"enabled": false})),
        });
        assert_eq!(disabled.status, 200);
        assert_eq!(disabled.body["regression"]["enabled"], false);

        let deleted = server.handle(ApiRequest {
            method: "DELETE".to_string(),
            path: "/api/quality/regressions/dashboard-smoke".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(deleted.status, 200);
        assert_eq!(deleted.body["ok"], true);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_manages_durable_chat_sessions() {
        let temp_root = unique_temp_dir("http-chat");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        write_fake_provider(
            &durable_root,
            "smoke-ai",
            0,
            "{\"message\":\"web provider output\",\"importable_artifacts\":[{\"type\":\"round\",\"round\":{\"reporter\":\"QA\",\"actual\":\"Broken\",\"target\":\"Fixed\"}}]}",
        );
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let started = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/chat/start".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"gap_id": "GAP1", "provider": "smoke-ai"})),
        });
        assert_eq!(started.status, 201);
        let session_id = started.body["session_id"].as_str().unwrap().to_string();
        assert_eq!(started.body["mode"], "gap");
        assert!(
            durable_root
                .join(format!("chat/sessions/{session_id}.json"))
                .exists()
        );

        let input = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/chat/{session_id}/input"),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"text": "What should I test?"})),
        });
        assert_eq!(input.status, 200);

        let read = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/chat/{session_id}/read"),
            auth_token: None,
            body: None,
        });
        assert_eq!(read.status, 200);
        assert_eq!(read.body["alive"], true);
        assert!(
            read.body["lines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap_or("").contains("What should I test?"))
        );
        assert!(
            read.body["progress_lines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line
                    .as_str()
                    .unwrap_or("")
                    .contains("Provider turn completed"))
        );
        assert!(
            read.body["lines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap_or("").contains("web provider output"))
        );
        assert_eq!(
            read.body["importable_artifacts"].as_array().unwrap().len(),
            1
        );
        assert_eq!(read.body["importable_artifacts"][0]["type"], "round");
        assert_eq!(
            read.body["importable_artifacts"][0]["round"]["reporter"],
            "QA"
        );
        let jobs = FileJobRegistry::new(&runtime_root).recover().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].owner, format!("chat:{session_id}"));
        assert_eq!(jobs[0].state, JobState::Succeeded);
        let job = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/jobs/{}", jobs[0].id),
            auth_token: None,
            body: None,
        });
        assert_eq!(job.status, 200);
        assert_eq!(job.body["job"]["owner"], format!("chat:{session_id}"));
        assert_eq!(job.body["job"]["status"], "complete");

        let stopped = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/chat/{session_id}/stop"),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(stopped.status, 200);
        assert_eq!(stopped.body["alive"], false);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn local_http_daemon_recovers_stale_chat_turns_before_serving() {
        let temp_root = unique_temp_dir("http-chat-recovery");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let chat = FileChatService::with_runtime_root(&durable_root, &runtime_root);
        let session = chat
            .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
            .unwrap();
        let job = FileJobRegistry::new(&runtime_root)
            .register(&format!("chat:{}", session.id))
            .unwrap();
        let session_path = durable_root.join(format!("chat/sessions/{}.json", session.id));

        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());
        let daemon = LocalHttpDaemon {
            server,
            static_root: None,
        };
        daemon.recover_runtime_state().unwrap();

        let recovered: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&session_path).unwrap()).unwrap();
        assert!(recovered.get("in_flight").is_none());
        assert!(recovered.get("last_turn_started_at").is_none());
        assert_eq!(recovered["interrupted"], true);
        assert!(
            recovered["interruption_detail"]
                .as_str()
                .unwrap_or("")
                .contains("Daemon restarted")
        );
        assert_eq!(
            FileJobRegistry::new(&runtime_root)
                .status(&job.id)
                .unwrap()
                .state,
            JobState::Interrupted
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_reports_project_registry_and_updates_settings() {
        let temp_root = unique_temp_dir("http-project-settings");
        let app_root = temp_root.join("app");
        let durable_root = app_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&durable_root).unwrap();
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());

        let status = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/project/status".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(status.status, 200);
        assert_eq!(status.body["attached"], true);
        assert_eq!(status.body["client_repo"], app_root.display().to_string());
        assert_eq!(status.body["apps"].as_array().unwrap().len(), 1);
        assert!(runtime_root.join("apps.json").exists());

        let app_status = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/apps/status".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(app_status.status, 200);
        assert_eq!(app_status.body["attached"], true);

        let other_app = temp_root.join("other");
        fs::create_dir_all(&other_app).unwrap();
        let attached = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/project/attach".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"path": other_app.display().to_string()})),
        });
        assert_eq!(attached.status, 200);
        assert_eq!(
            attached.body["client_repo"],
            other_app.display().to_string()
        );

        let switched = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/apps/switch".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"path": app_root.display().to_string()})),
        });
        assert_eq!(switched.status, 200);
        assert_eq!(switched.body["client_repo"], app_root.display().to_string());

        let third_app = temp_root.join("third");
        fs::create_dir_all(&third_app).unwrap();
        let registered = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/apps/register".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "name": "third-app",
                "path": third_app.display().to_string()
            })),
        });
        assert_eq!(registered.status, 201);
        assert_eq!(registered.body["apps"].as_array().unwrap().len(), 3);

        let switched_by_name = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/apps/switch".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"name": "third-app"})),
        });
        assert_eq!(switched_by_name.status, 200);
        assert_eq!(
            switched_by_name.body["client_repo"],
            third_app.display().to_string()
        );

        let detached = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/apps/detach".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(detached.status, 200);
        assert_eq!(detached.body["attached"], false);
        assert_eq!(detached.body["client_repo"], serde_json::Value::Null);

        let listed = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/apps".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(listed.status, 200);
        assert_eq!(listed.body["apps"].as_array().unwrap().len(), 3);
        assert_eq!(listed.body["current"], "");

        let settings = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/settings".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(settings.status, 200);
        assert_eq!(settings.body["settings"]["agent_cli"], "claude");
        assert_eq!(settings.body["runtime"]["paused"], false);

        let updated = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/settings".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "agent_cli": "smoke-ai",
                "parallel_run_cap": 3,
                "paused": true
            })),
        });
        assert_eq!(updated.status, 200);
        assert_eq!(updated.body["settings"]["agent_cli"], "smoke-ai");
        assert_eq!(updated.body["settings"]["parallel_run_cap"], "3");
        assert_eq!(updated.body["settings"]["paused"], "1");
        assert_eq!(updated.body["runtime"]["agents_paused"], true);
        assert_eq!(
            updated.body["runtime"]["background_processes_stopped"],
            true
        );
        assert!(runtime_root.join("process-control.json").exists());
        assert!(durable_root.join("settings.json").exists());

        let removed = server.handle(ApiRequest {
            method: "DELETE".to_string(),
            path: "/api/apps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"path": other_app.display().to_string()})),
        });
        assert_eq!(removed.status, 200);
        assert_eq!(removed.body["apps"].as_array().unwrap().len(), 2);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_manages_governance_guidance_and_reporters() {
        let temp_root = unique_temp_dir("http-project-config");
        let durable_root = temp_root.join(".refine");
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());

        let governance = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/governance".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "product": "Refine",
                "constitution": "Be useful",
                "rules": [{"text": "No regressions"}]
            })),
        });
        assert_eq!(governance.status, 200);
        assert_eq!(governance.body["configured"], true);
        assert_eq!(governance.body["rules"].as_array().unwrap().len(), 1);

        let generated = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/governance/generate-rules".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"product": "Refine", "constitution": "Be useful"})),
        });
        assert_eq!(generated.status, 200);
        assert_eq!(generated.body["ok"], true);
        assert!(generated.body["rules"].as_array().unwrap().len() >= 2);

        let guidance = server.handle(ApiRequest {
            method: "PUT".to_string(),
            path: "/api/guidance".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"guidance": [{
                "name": "Accessibility",
                "rule": "When UI changes",
                "instructions": "Check keyboard behavior",
                "enabled": true
            }]})),
        });
        assert_eq!(guidance.status, 200);
        assert_eq!(guidance.body["guidance"].as_array().unwrap().len(), 1);

        let reporter_one = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/reporters".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"name": "Buddy"})),
        });
        assert_eq!(reporter_one.status, 201);
        let reporter_one_id = reporter_one.body["reporter"]["id"].as_u64().unwrap();
        let reporter_two = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/reporters".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"name": "Alex"})),
        });
        let reporter_two_id = reporter_two.body["reporter"]["id"].as_u64().unwrap();

        let renamed = server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: format!("/api/reporters/{reporter_one_id}"),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"name": "Buddy Williams"})),
        });
        assert_eq!(renamed.status, 200);
        assert_eq!(renamed.body["new"], "Buddy Williams");

        let merged = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/reporters/{reporter_one_id}/merge"),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"target_id": reporter_two_id})),
        });
        assert_eq!(merged.status, 200);
        assert_eq!(merged.body["ok"], true);

        let listed = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/reporters".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(listed.status, 200);
        assert_eq!(listed.body["reporters"].as_array().unwrap().len(), 1);
        assert!(durable_root.join("governance.json").exists());
        assert!(durable_root.join("guidance.json").exists());
        assert!(durable_root.join("reporters.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn web_server_reports_dashboard_diagnostics_target_app_nodes_and_cluster() {
        let temp_root = unique_temp_dir("http-status-surfaces");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&temp_root).unwrap();
        fs::write(
            temp_root.join("package.json"),
            r#"{"scripts":{"dev":"vite","build":"vite build"}}"#,
        )
        .unwrap();
        let mut server = server_with_projection();
        server.durable_root = Some(durable_root.clone());
        server.runtime_root = Some(runtime_root.clone());
        server.handle(ApiRequest {
            method: "PATCH".to_string(),
            path: "/api/settings".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({
                "target_app_url": "http://127.0.0.1:3000",
                "target_app_start_command": "npm run dev",
                "target_app_auto_rebuild": "never"
            })),
        });
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/gaps".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"id": "GAP1", "name": "Dashboard Gap"})),
        });
        FileActivityService::new(&durable_root)
            .append(ActivityEntry {
                id: "act-dashboard".to_string(),
                datetime: "2026-06-05T00:00:00Z".to_string(),
                severity: "info".to_string(),
                category: "state".to_string(),
                message: "Dashboard activity".to_string(),
                gap_id: Some("GAP1".to_string()),
                actor: Some("system".to_string()),
                details: None,
                actions: Vec::new(),
            })
            .unwrap();

        let dashboard = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/dashboard?node=current".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(dashboard.status, 200);
        assert_eq!(dashboard.body["counts"]["backlog"], 1);
        assert_eq!(dashboard.body["active_node_id"], "default");
        assert_eq!(dashboard.body["activity"][0]["id"], "act-dashboard");
        let cached = FileProjectStateStore::new(&durable_root)
            .load_projection_snapshot(&runtime_root.join("cache"))
            .unwrap()
            .unwrap();
        assert_eq!(
            cached.runtime.target_app.unwrap()["app_url"],
            "http://127.0.0.1:3000"
        );
        assert_eq!(
            cached.runtime.preflight.unwrap()["stage"],
            "provider_detection"
        );

        let diagnostics = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/diagnostics".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(diagnostics.status, 200);
        assert_eq!(diagnostics.body["reachable"], true);
        for key in [
            "daemon",
            "install",
            "os_backend",
            "target_app",
            "git",
            "provider",
            "browser",
            "docker",
            "storage",
        ] {
            assert!(
                diagnostics.body["doctor"][key]
                    .as_array()
                    .map(|items| !items.is_empty())
                    .unwrap_or(false),
                "missing doctor section {key}"
            );
        }
        assert!(
            diagnostics.body["doctor"]["target_app"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry
                    .as_str()
                    .unwrap_or("")
                    .contains("supervised by the native daemon"))
        );
        assert!(
            diagnostics.body["doctor"]["storage"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry
                    .as_str()
                    .unwrap_or("")
                    .contains("runtime_root_exists="))
        );

        let target = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/target-app/status".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(target.status, 200);
        assert_eq!(target.body["app_url"], "http://127.0.0.1:3000");
        assert_eq!(target.body["has_start_command"], true);

        let generated = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/target-app/generate-instructions".to_string(),
            auth_token: Some("secret".to_string()),
            body: Some(json!({"kind": "all"})),
        });
        assert_eq!(generated.status, 200);
        assert_eq!(generated.body["config"]["start_command"], "npm run dev");
        assert_eq!(
            generated.body["settings"]["target_app_rebuild_command"],
            "npm run build"
        );
        assert_eq!(generated.body["config"]["tcp_check_port"], "3000");

        let rebuild = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/runner-workers/target-app-rebuilder/rebuild".to_string(),
            auth_token: Some("secret".to_string()),
            body: None,
        });
        assert_eq!(rebuild.status, 200);
        assert_eq!(rebuild.body["queued"], false);

        let nodes = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/nodes".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(nodes.status, 200);
        assert_eq!(nodes.body["nodes"][0]["id"], "default");

        let cluster = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: "/api/cluster".to_string(),
            auth_token: None,
            body: None,
        });
        assert_eq!(cluster.status, 200);
        assert_eq!(cluster.body["enabled"], false);

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn server_with_projection() -> InProcessWebServer {
        let mut gaps = BTreeMap::new();
        gaps.insert(
            "GAP1".to_string(),
            GapSummaryProjection {
                gap: GapIndexProjection {
                    id: "GAP1".to_string(),
                    name: "Projection route".to_string(),
                    status: GapStatus::Todo,
                    priority: GapPriority::Medium,
                    reporter: Some("Buddy".to_string()),
                    round_count: 1,
                    created: "created".to_string(),
                    updated: "updated".to_string(),
                    branch_name: None,
                    node_id: Some("default".to_string()),
                    feature_id: None,
                    feature_order: None,
                    json_path: "gaps/01/GAP1/gap.json".to_string(),
                },
                node_display_name: None,
                searchable_text: "Projection route".to_string(),
                activity_ids: Vec::new(),
            },
        );

        InProcessWebServer {
            status: DaemonStatus {
                port: 8080,
                daemon_healthy: true,
                web_available: true,
                worker_state: "idle".to_string(),
                target_app_state: "detached".to_string(),
                active_operations: Vec::new(),
                degraded_integrations: Vec::new(),
            },
            projection: ProjectionSnapshot {
                version: PROJECTION_SNAPSHOT_VERSION,
                generated_at: "now".to_string(),
                source_fingerprints: BTreeMap::new(),
                gaps,
                features: BTreeMap::new(),
                activity: BTreeMap::new(),
                changes: BTreeMap::new(),
                dashboard: DashboardProjection::default(),
                runtime: RuntimeProjection::default(),
            },
            auth_token: Some("secret".to_string()),
            durable_root: None,
            runtime_root: None,
        }
    }

    fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RefineError::Conflict(
                format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )
                .trim()
                .to_string(),
            ))
        }
    }

    fn write_fake_playwright(root: &Path, exit_code: i32) {
        let bin_dir = root.join("node_modules/.bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join("playwright");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' '{{\"status\":\"passed\"}}'\nif [ -n \"$REFINE_REGRESSION_SCREENSHOT\" ]; then printf 'png' > \"$REFINE_REGRESSION_SCREENSHOT\"; fi\nexit {exit_code}"
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }

    fn write_fake_provider(durable_root: &Path, name: &str, exit_code: i32, output: &str) {
        let bin_dir = durable_root.join("provider-bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let path = bin_dir.join(name);
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "#!/bin/sh\nprintf '%s\\n' {output:?}\nexit {exit_code}"
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn percent_encode_for_test(value: &str) -> String {
        value
            .bytes()
            .flat_map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    vec![byte as char]
                }
                _ => format!("%{byte:02X}").chars().collect(),
            })
            .collect()
    }
}
