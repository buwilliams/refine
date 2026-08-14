mod agents;
mod config;
mod config_input;
#[cfg(test)]
pub(crate) use config::dispatch_config;
mod daemon_transport;
mod discovery;
mod features;
mod fleet;
mod goals;
mod logs;
mod nodes;
mod projects;
mod system;
mod todos;
mod website;
mod workflow;
use daemon_transport::*;
use system::{port_status_with_processes, selected_process_ports, stop_system_process};

#[cfg(not(test))]
use agents::dispatch_agent_daemon;
#[cfg(not(test))]
use config::dispatch_config as dispatch_config_daemon;
#[cfg(not(test))]
use features::dispatch_feature_daemon;
#[cfg(not(test))]
use fleet::dispatch_fleet_daemon;
#[cfg(not(test))]
use goals::dispatch_goal_daemon;
#[cfg(not(test))]
use logs::dispatch_log_daemon;
#[cfg(not(test))]
use nodes::dispatch_node_daemon;
#[cfg(not(test))]
use projects::dispatch_project_daemon;
#[cfg(not(test))]
use todos::dispatch_todo;
#[cfg(not(test))]
use workflow::dispatch_workflow_daemon;

use std::fs;
#[cfg(not(test))]
use std::io::IsTerminal;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::process::Command;
use std::sync::Arc;
#[cfg(not(test))]
use std::sync::mpsc;
#[cfg(not(test))]
use std::thread;
#[cfg(not(test))]
use std::time::{Duration, Instant};

use clap::Parser;
use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::runner::run_worker;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    BackgroundDaemonConfig, DaemonStatus, FileDaemonLifecycleService, current_launch_executable,
    current_launch_mode,
};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::surfaces::web_server::{
    API_CONTRACT_VERSION, API_GROUPS, InProcessWebServer, LocalHttpDaemon,
};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::checkout::RefineCheckoutPaths;
use crate::tools::host::daemon_lifecycle::{
    DaemonLifecycleAction, FileHostDaemonLifecycleService, execute_daemon_lifecycle,
    uninstall_daemon_installation,
};
use crate::tools::host::deployed_update::{
    DeployedUpdateOptions, FileDeployedUpdateHost, discover_refine_checkout,
    resolve_agent_provider, run_deployed_update,
};
use crate::tools::host::fleet::{
    FLEET_RUNBOOK_PATH, FileFleetService, FleetService, NodeRemoteUpdate, fleet_manage_prompt,
};
use crate::tools::host::git_sync::FileGitSyncService;
use crate::tools::host::installation::{FileInstallationService, InstallationService};
use crate::tools::host::node_init::{WorkerInitOptions, initialize_worker};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::host::release::{FileReleaseService, ReleaseBump};
use crate::tools::host::source_promotion::FileSourcePromotionService;
use crate::tools::observability::activity::{ActivityQuery, ActivityService, FileActivityService};
use crate::tools::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::tools::observability::processes::FileProcessStatusService;
use crate::tools::observability::support_bundle::{FileSupportBundleService, SupportBundleService};
use crate::tools::product::goal_exports::FileGoalExportService;
use crate::tools::product::governance_integration::FileGovernanceIntegrationService;
use crate::tools::product::imports::FileImportService;
use crate::tools::product::next_actions::FileNextActionsService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::process_control::FileProcessControlService;
use crate::tools::product::project_projection::{
    FileProjectProjectionStore, ProjectionQuery, ProjectionSnapshot,
};
use crate::tools::product::project_registry::{FileProjectRegistryService, ProjectRegistryService};
use crate::tools::product::todos::FileTodoService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::tools::product::worktree_cleanup::{FileWorktreeCleanupService, WorktreeCleanupOptions};

use super::actions::*;
use super::helpers::*;

pub fn run() -> RefineResult<()> {
    let cli = Cli::parse();
    dispatch(cli)
}

pub fn dispatch(cli: Cli) -> RefineResult<()> {
    #[cfg(not(test))]
    if let Some(path) = explicit_target_root_path(&cli.command) {
        return Err(RefineError::InvalidInput(format!(
            "direct --target-root CLI dispatch is not supported in normal operation; use the daemon API for target state mutations instead ({})",
            path.display()
        )));
    }

    #[cfg(not(test))]
    let cli = match cli.command {
        Commands::Config { action } => {
            let result = dispatch_config_daemon(action).map_err(config::structured_config_error)?;
            print_json(&result);
            return Ok(());
        }
        Commands::Project { action } => return dispatch_project_daemon(action),
        Commands::Goal { action } => return dispatch_goal_daemon(action),
        Commands::Feature { action } => return dispatch_feature_daemon(action),
        Commands::Todo { action } => return dispatch_todo(action),
        Commands::Workflow { action } => return dispatch_workflow_daemon(action),
        Commands::Node { action } => return dispatch_node_daemon(action),
        Commands::Fleet {
            action: FleetAction::Request(words),
        } => return fleet::dispatch_request(words),
        Commands::Fleet {
            action:
                FleetAction::Manage {
                    request,
                    provider,
                    runtime_root,
                    port,
                },
        } => return fleet::dispatch_manage(request, provider, runtime_root, port),
        Commands::Fleet {
            action:
                FleetAction::Distribute {
                    instructions: Some(instructions),
                    ..
                },
        } => return fleet::dispatch_distribute_instructions(instructions),
        Commands::Fleet { action } => return dispatch_fleet_daemon(action),
        Commands::Log { action } => return dispatch_log_daemon(action),
        Commands::Agent { action } => return dispatch_agent_daemon(action),
        Commands::System {
            action: SystemAction::Doctor { .. },
        } => {
            let response = daemon_json("GET", "/diagnostics", None)?;
            print_json(&response);
            return Ok(());
        }
        other => Cli { command: other },
    };

    match cli.command {
        command @ Commands::Config { .. } => config::dispatch_command(command),
        command @ Commands::Website { .. } => website::dispatch_command(command),
        command @ Commands::System { .. } => system::dispatch_command(command),
        command @ Commands::Node { .. } => nodes::dispatch_command(command),
        command @ Commands::Fleet { .. } => fleet::dispatch_command(command),
        command @ Commands::Next { .. } => discovery::dispatch_command(command),
        command @ Commands::Commands => discovery::dispatch_command(command),
        command @ Commands::Log { .. } => logs::dispatch_command(command),
        command @ Commands::Agent { .. } => agents::dispatch_command(command),
        command @ Commands::Project { .. } => projects::dispatch_command(command),
        command @ Commands::Goal { .. } => goals::dispatch_command(command),
        command @ Commands::Feature { .. } => features::dispatch_command(command),
        command @ Commands::Todo { .. } => todos::dispatch_command(command),
        command @ Commands::Workflow { .. } => workflow::dispatch_command(command),
    }
}

pub(super) fn run_system_start(
    port: u16,
    bind_address: std::net::IpAddr,
    cache_dir: Option<PathBuf>,
    static_root: Option<PathBuf>,
    runtime_root: PathBuf,
    once: bool,
    foreground: bool,
) -> RefineResult<()> {
    #[cfg(not(test))]
    let product_paths = RefineCheckoutPaths::discover()?;
    #[cfg(test)]
    let product_paths: Option<RefineCheckoutPaths> = None;
    #[cfg(not(test))]
    let runtime_root = resolve_system_runtime_root_for_paths(runtime_root, &product_paths)?;
    #[cfg(test)]
    let runtime_root = resolve_system_runtime_root(runtime_root)?;
    let cache_dir = cache_dir.map(absolute_cli_path).transpose()?;
    let static_root = static_root.map(absolute_cli_path).transpose()?;
    if !foreground && !once {
        let lifecycle = FileHostDaemonLifecycleService::new(
            RuntimeRoot {
                root: runtime_root.clone(),
            },
            env!("CARGO_PKG_VERSION"),
        );
        let status = execute_daemon_lifecycle(
            &lifecycle,
            DaemonLifecycleAction::Start,
            BackgroundDaemonConfig {
                port,
                bind_address,
                cache_dir,
                static_root,
            },
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&web_response(status)).unwrap()
        );
        return Ok(());
    }
    let listener = LocalHttpDaemon::bind_address(bind_address, port)?;
    let addr = LocalHttpDaemon::local_addr(&listener)?;
    let actual_port = addr.port();
    let port_runtime_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(actual_port);
    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.clone(),
    });
    lifecycle.begin_start(actual_port)?;
    // Each milestone is published as well as printed. A service-managed daemon's
    // stderr goes to the journal, where its readiness waiter cannot see it, so
    // without this a slow start is indistinguishable from a hung one.
    lifecycle.record_startup_progress(actual_port, "preparing");
    eprintln!("refine: preparing daemon at http://{addr}");
    let project_preparation = (|| -> RefineResult<_> {
        lifecycle.record_startup_progress(actual_port, "loading-project-registry");
        eprintln!("refine: loading active project registry");
        let project_registry = FileProjectRegistryService::new(&runtime_root, None);
        let legacy_project_registry = FileProjectRegistryService::new(&port_runtime_root, None);
        if !project_registry.path().exists() && legacy_project_registry.path().exists() {
            eprintln!(
                "refine: migrating port-scoped project registry to {}",
                project_registry.path().display()
            );
            project_registry.save(&legacy_project_registry.load()?)?;
        }
        let registry = project_registry.load()?;
        let mut project_degradation = None;
        let target_root = match registry.active_app {
            None => None,
            Some(target_root) if !Path::new(&target_root).is_dir() => {
                let message = format!(
                    "active project {target_root} is unavailable; detaching it for daemon startup"
                );
                eprintln!("refine: {message}");
                project_registry.detach()?;
                project_degradation = Some("active-project-unavailable".to_string());
                None
            }
            Some(_) => match project_registry.status() {
                Ok(status) => status.target_root,
                Err(RefineError::InvalidInput(error))
                    if error.contains("must be a Git worktree") =>
                {
                    eprintln!(
                        "refine: active project is not a usable Git worktree ({error}); detaching it for daemon startup"
                    );
                    project_registry.detach()?;
                    project_degradation = Some("active-project-unavailable".to_string());
                    None
                }
                Err(error) => return Err(error),
            },
        };
        let snapshot = if let Some(target_root) = target_root {
            // The longest step by far on a large project, and the one whose
            // slowness a fixed deadline used to misread as failure.
            lifecycle.record_startup_progress(actual_port, "warming-project-cache");
            eprintln!("refine: warming project cache for {target_root}");
            let target_root = PathBuf::from(target_root);
            let refine_dir = refine_dir_for_target_root(&target_root)?;
            let cache_root = cache_dir
                .clone()
                .unwrap_or_else(|| port_runtime_root.join("cache"));
            let store =
                FileProjectProjectionStore::with_runtime_root(&refine_dir, &port_runtime_root);
            store.load_or_refresh_projection(&cache_root)?
        } else {
            eprintln!("refine: no active project; using empty project cache");
            ProjectionSnapshot::default()
        };
        Ok((snapshot, project_degradation))
    })();
    let (snapshot, project_degradation) = match project_preparation {
        Ok(preparation) => preparation,
        Err(error) => {
            let _ = lifecycle.mark_start_failed(actual_port, &error);
            return Err(error);
        }
    };
    let mut status = match lifecycle.prepare_start(actual_port) {
        Ok(status) => status,
        Err(error) => {
            let _ = lifecycle.mark_start_failed(actual_port, &error);
            return Err(error);
        }
    };
    if let Some(degradation) = project_degradation {
        status.degraded_integrations.push(degradation);
    }
    let mut daemon = LocalHttpDaemon::new(
        InProcessWebServer {
            status: status.clone(),
            projection: snapshot,
            target_root: None,
            app_registry_root: Some(runtime_root.clone()),
            runtime_root: Some(port_runtime_root),
            product_paths: {
                #[cfg(not(test))]
                {
                    Some(product_paths)
                }
                #[cfg(test)]
                {
                    product_paths
                }
            },
        },
        static_root.or_else(default_static_root),
    );
    if let Err(error) = daemon.recover_runtime_state_with_progress(|message| {
        lifecycle.record_startup_progress(actual_port, message);
        eprintln!("refine: {message}");
    }) {
        let _ = lifecycle.mark_start_failed(actual_port, &error);
        return Err(error);
    }
    let status = lifecycle.mark_ready(status)?;
    Arc::make_mut(&mut daemon.server).status = status;
    daemon.warm_caches_in_background();
    eprintln!("running foreground Refine daemon at http://{addr}");
    if once {
        daemon.serve_once(listener)?;
    } else {
        daemon.serve_until_unhealthy(listener, lifecycle, actual_port)?;
    }
    Ok(())
}

pub(super) fn system_status_response(runtime_root: PathBuf) -> RefineResult<serde_json::Value> {
    let runtime = RuntimeRoot {
        root: runtime_root.clone(),
    };
    let lifecycle = FileDaemonLifecycleService::new(runtime.clone());
    let ports = lifecycle.running_statuses()?;
    let running_ports: Vec<u16> = ports.iter().map(|status| status.port).collect();
    let ports = ports
        .iter()
        .map(|status| port_status_with_processes(&runtime, status))
        .collect::<Vec<_>>();
    Ok(json!({
        "product": "refine",
        "version": env!("CARGO_PKG_VERSION"),
        "current_version": env!("CARGO_PKG_VERSION"),
        "launch_mode": current_launch_mode(),
        "executable_path": current_launch_executable(),
        "runtime_root": runtime_root,
        "api_contract_version": API_CONTRACT_VERSION,
        "running_ports": running_ports,
        "ports": ports,
    }))
}

pub(super) fn system_ps_response(
    runtime_root: PathBuf,
    port: Option<u16>,
    stop: Option<&str>,
    signal: &str,
) -> RefineResult<serde_json::Value> {
    let runtime = RuntimeRoot {
        root: runtime_root.clone(),
    };
    if let Some(process_id) = stop {
        return stop_system_process(&runtime, port, process_id, signal);
    }
    let ports = selected_process_ports(&runtime, port)?;
    let mut flattened = Vec::new();
    let mut port_values = Vec::new();
    for port in ports {
        let port_root = runtime.port_root(port);
        let summary = FileProcessStatusService::new(&port_root).summary()?;
        if let Some(processes) = summary.get("processes").and_then(|value| value.as_array()) {
            for process in processes {
                let mut process = process.clone();
                if let Some(object) = process.as_object_mut() {
                    object.insert("port".to_string(), json!(port));
                    object.insert(
                        "runtime_root".to_string(),
                        json!(port_root.display().to_string()),
                    );
                }
                flattened.push(process);
            }
        }
        port_values.push(json!({
            "port": port,
            "runtime_root": port_root.display().to_string(),
            "process_count": summary.get("processes").and_then(|value| value.as_array()).map(|processes| processes.len()).unwrap_or(0),
            "process_summary": summary
        }));
    }
    Ok(json!({
        "product": "refine",
        "runtime_root": runtime_root,
        "ports": port_values,
        "process_count": flattened.len(),
        "processes": flattened
    }))
}

pub(super) fn absolute_cli_path(path: PathBuf) -> RefineResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        let cwd = std::env::current_dir().map_err(|error| {
            RefineError::Io(format!("failed to resolve current directory: {error}"))
        })?;
        Ok(cwd.join(path))
    }
}

pub(super) fn resolve_system_runtime_root(path: PathBuf) -> RefineResult<PathBuf> {
    #[cfg(test)]
    if path.is_absolute() {
        return Ok(path);
    }
    let paths = RefineCheckoutPaths::discover()?;
    resolve_system_runtime_root_for_paths(path, &paths)
}

pub(super) fn resolve_system_runtime_root_for_paths(
    path: PathBuf,
    paths: &RefineCheckoutPaths,
) -> RefineResult<PathBuf> {
    paths.resolve_runtime_compatibility(&path)
}

#[cfg(not(test))]
struct CliTerminalMode {
    previous: Option<String>,
}

#[cfg(not(test))]
impl CliTerminalMode {
    fn enter() -> Self {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return Self { previous: None };
        }
        let previous = Command::new("stty")
            .arg("-g")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty());
        if previous.is_some() {
            let _ = Command::new("stty").args(["raw", "-echo"]).status();
        }
        Self { previous }
    }
}

#[cfg(not(test))]
impl Drop for CliTerminalMode {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            let _ = Command::new("stty").arg(previous).status();
        }
    }
}

pub(super) fn plan_goal_draft_body(
    text: Option<String>,
    file: Option<PathBuf>,
    reporter: Option<String>,
    provider: Option<String>,
) -> RefineResult<Value> {
    if text.is_some() && file.is_some() {
        return Err(RefineError::InvalidInput(
            "goal draft accepts either --text or --file, not both".to_string(),
        ));
    }
    let source = match (text, file) {
        (Some(text), None) => text,
        (None, Some(file)) => fs::read_to_string(&file).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Plan transcript {}: {error}",
                file.display()
            ))
        })?,
        (None, None) => {
            return Err(RefineError::InvalidInput(
                "goal draft requires --text or --file".to_string(),
            ));
        }
        (Some(_), Some(_)) => unreachable!("validated above"),
    };
    if source.trim().is_empty() {
        return Err(RefineError::InvalidInput(
            "goal draft Plan transcript cannot be empty".to_string(),
        ));
    }
    Ok(json!({
        "text": source,
        "purpose": "plan_goal",
        "reporter": reporter,
        "provider": provider
    }))
}

fn skipped_target_root(path: &Path) -> bool {
    path.as_os_str().is_empty()
}

fn refine_dir_for_target_root(target_root: &Path) -> RefineResult<PathBuf> {
    #[cfg(test)]
    if !target_root.join(".git").exists() {
        return Ok(target_root.join(".refine"));
    }
    prepare_refine_dir(target_root)
}

/// Work-item service for direct (daemonless) CLI commands. Backed by the
/// snapshot cache under the state directory's `runtime/` so a single command
/// parses one snapshot instead of rebuilding the projection from every record.
fn direct_work_item_service(target_root: &Path) -> RefineResult<FileWorkItemService> {
    let refine_dir = refine_dir_for_target_root(target_root)?;
    let cache_dir = refine_dir.join("runtime/cache");
    Ok(FileWorkItemService::with_snapshot_cache(refine_dir, cache_dir))
}

pub(super) fn explicit_target_root_path(command: &Commands) -> Option<&PathBuf> {
    match command {
        Commands::Config { action } => match action {
            ConfigAction::Show { target_root, .. } => target_root.as_ref(),
            ConfigAction::Settings { action } => match action {
                ConfigSettingsAction::Show { target_root }
                | ConfigSettingsAction::Set { target_root, .. } => target_root.as_ref(),
            },
            ConfigAction::Quality { action } => match action {
                ConfigQualityAction::Show { target_root }
                | ConfigQualityAction::Set { target_root, .. } => target_root.as_ref(),
            },
            ConfigAction::Governance { action } => match action {
                ConfigGovernanceAction::Show { target_root }
                | ConfigGovernanceAction::Set { target_root, .. }
                | ConfigGovernanceAction::GenerateRules { target_root, .. } => target_root.as_ref(),
            },
            ConfigAction::Guidance { action } => match action {
                ConfigGuidanceAction::List { target_root }
                | ConfigGuidanceAction::Add { target_root, .. }
                | ConfigGuidanceAction::Edit { target_root, .. }
                | ConfigGuidanceAction::Enable { target_root, .. }
                | ConfigGuidanceAction::Disable { target_root, .. }
                | ConfigGuidanceAction::Remove { target_root, .. } => target_root.as_ref(),
            },
        },
        Commands::Project { action } => match action {
            ProjectAction::Status { target_root, .. }
            | ProjectAction::Attach { target_root, .. }
            | ProjectAction::Switch { target_root, .. }
            | ProjectAction::Detach { target_root, .. }
            | ProjectAction::Register { target_root, .. }
            | ProjectAction::Clone { target_root, .. }
            | ProjectAction::Remove { target_root, .. }
            | ProjectAction::Migrate { target_root, .. }
            | ProjectAction::Sync { target_root, .. }
            | ProjectAction::CleanupWorktrees { target_root, .. }
            | ProjectAction::Doctor { target_root, .. } => target_root.as_ref(),
        },
        Commands::Goal { action } => match action {
            GoalAction::Create { target_root, .. }
            | GoalAction::Draft { target_root, .. }
            | GoalAction::List { target_root }
            | GoalAction::Show { target_root, .. }
            | GoalAction::Export { target_root, .. }
            | GoalAction::Edit { target_root, .. }
            | GoalAction::Note { target_root, .. }
            | GoalAction::NoteEdit { target_root, .. }
            | GoalAction::NoteDelete { target_root, .. }
            | GoalAction::Round { target_root, .. }
            | GoalAction::Start { target_root, .. }
            | GoalAction::Cancel { target_root, .. }
            | GoalAction::Retry { target_root, .. }
            | GoalAction::Approve { target_root, .. }
            | GoalAction::Verify { target_root, .. }
            | GoalAction::Undo { target_root, .. }
            | GoalAction::Delete { target_root, .. }
            | GoalAction::AssignFeature { target_root, .. }
            | GoalAction::RemoveFeature { target_root, .. } => target_root.as_ref(),
        },
        Commands::Feature { action } => match action {
            FeatureAction::Create { target_root, .. }
            | FeatureAction::List { target_root }
            | FeatureAction::Show { target_root, .. }
            | FeatureAction::Edit { target_root, .. }
            | FeatureAction::AddGoal { target_root, .. }
            | FeatureAction::RemoveGoal { target_root, .. }
            | FeatureAction::ReorderGoal { target_root, .. }
            | FeatureAction::OrderGoal { target_root, .. }
            | FeatureAction::UnorderGoal { target_root, .. }
            | FeatureAction::Move { target_root, .. }
            | FeatureAction::Transfer { target_root, .. }
            | FeatureAction::Cancel { target_root, .. }
            | FeatureAction::Delete { target_root, .. } => target_root.as_ref(),
            FeatureAction::Import { target_root, .. } => Some(target_root),
        },
        Commands::Todo { action } => match action {
            TodoAction::List { target_root, .. }
            | TodoAction::CreateList { target_root, .. }
            | TodoAction::RenameList { target_root, .. }
            | TodoAction::DeleteList { target_root, .. }
            | TodoAction::Add { target_root, .. }
            | TodoAction::Edit { target_root, .. }
            | TodoAction::Delete { target_root, .. }
            | TodoAction::Done { target_root, .. }
            | TodoAction::Undo { target_root, .. } => target_root.as_ref(),
        },
        Commands::Workflow { action } => match action {
            WorkflowAction::Pause { .. } | WorkflowAction::Resume { .. } => None,
        },
        Commands::Node { action } => match action {
            NodeAction::List { target_root }
            | NodeAction::Show { target_root, .. }
            | NodeAction::Create { target_root, .. }
            | NodeAction::Activate { target_root, .. }
            | NodeAction::Archive { target_root, .. }
            | NodeAction::Rename { target_root, .. }
            | NodeAction::Settings { target_root, .. }
            | NodeAction::Transfer { target_root, .. } => target_root.as_ref(),
            NodeAction::Init { .. } => None,
        },
        Commands::Fleet { action } => match action {
            FleetAction::List { target_root }
            | FleetAction::Show { target_root, .. }
            | FleetAction::AddNode { target_root, .. }
            | FleetAction::EditNode { target_root, .. }
            | FleetAction::EnableNode { target_root, .. }
            | FleetAction::DisableNode { target_root, .. }
            | FleetAction::RemoveNode { target_root, .. }
            | FleetAction::Bootstrap { target_root, .. }
            | FleetAction::Distribute { target_root, .. }
            | FleetAction::Sync { target_root }
            | FleetAction::Run { target_root, .. }
            | FleetAction::Transfer { target_root, .. }
            | FleetAction::Maintenance { target_root } => target_root.as_ref(),
            FleetAction::Manage { .. } | FleetAction::Request(_) => None,
        },
        Commands::Log { action } => match action {
            LogAction::List { target_root, .. }
            | LogAction::Tail { target_root, .. }
            | LogAction::Show { target_root, .. }
            | LogAction::Query { target_root, .. }
            | LogAction::Bundle { target_root, .. } => Some(target_root),
            LogAction::Export { target_root } => target_root.as_ref(),
        },
        Commands::Agent { .. } => None,
        Commands::Website { .. } => None,
        Commands::Next { target_root } => target_root.as_ref(),
        Commands::Commands => None,
        Commands::System { action } => match action {
            SystemAction::Doctor { target_root, .. } => target_root.as_ref(),
            SystemAction::Install { .. }
            | SystemAction::Build
            | SystemAction::Clean
            | SystemAction::Performance { .. }
            | SystemAction::Repair { .. }
            | SystemAction::Update { .. }
            | SystemAction::ReleasePlan { .. }
            | SystemAction::ReleasePrepare { .. }
            | SystemAction::ReleasePublish { .. }
            | SystemAction::SourceStatus { .. }
            | SystemAction::SourcePromote { .. }
            | SystemAction::SourcePromoteHelper { .. }
            | SystemAction::SourceUpgradeCapability { .. }
            | SystemAction::SourceCheckWorker { .. }
            | SystemAction::DaemonLifecycleHelper { .. }
            | SystemAction::RunnerWorker { .. }
            | SystemAction::Rollback { .. }
            | SystemAction::Uninstall { .. }
            | SystemAction::Start { .. }
            | SystemAction::Stop { .. }
            | SystemAction::Restart { .. }
            | SystemAction::Status { .. }
            | SystemAction::Ps { .. }
            | SystemAction::ApiGroups => None,
        },
    }
    .filter(|path| !skipped_target_root(path))
}
