use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use crate::core::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::core::host::cluster::{ClusterService, FileClusterRegistryService};
use crate::core::host::installation::{
    FileInstallationService, InstallTarget, InstallationService,
};
use crate::core::host::process_supervision::FileProcessSupervisor;
use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::core::observability::support_bundle::{FileSupportBundleService, SupportBundleService};
use crate::core::product::imports::FileImportService;
use crate::core::product::nodes::FileNodeRegistryService;
use crate::core::product::project_registry::{FileProjectRegistryService, ProjectRegistryService};
use crate::core::product::project_state::{
    FileProjectStateStore, ProjectStateStore, ProjectionQuery,
};
use crate::core::product::scheduling::{
    FileSchedulingService, SchedulerControl, SchedulingService,
};
use crate::core::product::work_items::{
    BulkGapFilter, BulkGapSelection, BulkGapUpdate, FileWorkItemService,
};
use crate::core::supervisor::errors::RefineResult;
use crate::core::supervisor::lifecycle::{DaemonLifecycleService, FileDaemonLifecycleService};
use crate::core::supervisor::runtime::RuntimeRoot;
use crate::model::project::RegisteredApp;
use crate::model::workflow::{GapStatus, user_status_transition};
use crate::surfaces::web_server::{API_GROUPS, InProcessWebServer, LocalHttpDaemon};

#[derive(Debug, Parser)]
#[command(name = "refine")]
#[command(about = "Native Refine CLI surface")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    Gap {
        #[command(subcommand)]
        action: GapAction,
    },
    Feature {
        #[command(subcommand)]
        action: FeatureAction,
    },
    Workflow {
        #[command(subcommand)]
        action: WorkflowAction,
    },
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    Cluster {
        #[command(subcommand)]
        action: ClusterAction,
    },
    Log {
        #[command(subcommand)]
        action: LogAction,
    },
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    System {
        #[command(subcommand)]
        action: SystemAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectAction {
    Status {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Attach {
        path: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Switch {
        name: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Detach {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Register {
        name: String,
        path: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Remove {
        name: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Migrate {
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Sync {
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    Doctor {
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum GapAction {
    Create {
        name: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
    },
    List {
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Edit {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        priority: Option<String>,
    },
    Note {
        id: String,
        body: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long, default_value = "")]
        author: String,
    },
    Round {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long)]
        reporter: Option<String>,
        #[arg(long)]
        actual: Option<String>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        edit_latest: bool,
    },
    Start {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Cancel {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Retry {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long, default_value = "quality")]
        stage: String,
    },
    Verify {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Merge {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Undo {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Delete {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    AssignFeature {
        id: String,
        feature_id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    RemoveFeature {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FeatureAction {
    Create {
        name: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        reporter: Option<String>,
    },
    List {
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Edit {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        reporter: Option<String>,
    },
    AddGap {
        id: String,
        gap_id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    RemoveGap {
        id: String,
        gap_id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    ReorderGap {
        id: String,
        gap_id: String,
        order: i64,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Move {
        id: String,
        target: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Cancel {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Delete {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Import {
        #[arg(long)]
        durable_root: PathBuf,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        csv: bool,
        #[arg(long)]
        reporter: Option<String>,
        #[arg(long)]
        feature_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkflowAction {
    Allowed {
        from: CliGapStatus,
        to: CliGapStatus,
    },
    Transition {
        id: String,
        target: CliGapStatus,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    BulkTransition {
        target: CliGapStatus,
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long = "selected-id")]
        selected_ids: Vec<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        q: Option<String>,
    },
    Schedule {
        #[arg(long)]
        durable_root: PathBuf,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Pause {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Resume {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Restore {
        #[arg(long)]
        durable_root: PathBuf,
    },
    Enforce {
        #[arg(long)]
        durable_root: PathBuf,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CliGapStatus {
    Backlog,
    Todo,
    InProgress,
    Qa,
    ReadyMerge,
    AwaitingRebuild,
    Review,
    Done,
    Failed,
    Cancelled,
}

impl From<CliGapStatus> for GapStatus {
    fn from(value: CliGapStatus) -> Self {
        match value {
            CliGapStatus::Backlog => Self::Backlog,
            CliGapStatus::Todo => Self::Todo,
            CliGapStatus::InProgress => Self::InProgress,
            CliGapStatus::Qa => Self::Qa,
            CliGapStatus::ReadyMerge => Self::ReadyMerge,
            CliGapStatus::AwaitingRebuild => Self::AwaitingRebuild,
            CliGapStatus::Review => Self::Review,
            CliGapStatus::Done => Self::Done,
            CliGapStatus::Failed => Self::Failed,
            CliGapStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum NodeAction {
    List {
        #[arg(long)]
        durable_root: PathBuf,
    },
    Show {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Create {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Activate {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Archive {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Rename {
        id: String,
        name: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Settings {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Transfer {
        id: String,
        item_id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ClusterAction {
    List {
        #[arg(long)]
        durable_root: PathBuf,
    },
    Show {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    AddNode {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    EditNode {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    EnableNode {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    DisableNode {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    RemoveNode {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Sync {
        #[arg(long)]
        durable_root: PathBuf,
    },
    Run {
        id: String,
        command: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Transfer {
        id: String,
        item_id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Maintenance {
        #[arg(long)]
        durable_root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum LogAction {
    List {
        #[arg(long)]
        durable_root: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Tail {
        #[arg(long)]
        durable_root: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Show {
        id: String,
        #[arg(long)]
        durable_root: PathBuf,
    },
    Query {
        q: String,
        #[arg(long)]
        durable_root: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        gap_id: Option<String>,
        #[arg(long)]
        severity: Option<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        actor: Option<String>,
    },
    Export {
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Bundle {
        #[arg(long)]
        durable_root: PathBuf,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        #[arg(long, default_value_t = true)]
        redact_secrets: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentAction {
    Detect,
    Configure {
        #[arg(long, default_value = "claude")]
        provider: String,
    },
    Auth {
        #[arg(long, default_value = "claude")]
        provider: String,
    },
    Diagnose {
        #[arg(long, default_value = "claude")]
        provider: String,
    },
    Invoke {
        prompt: String,
        #[arg(long, default_value = "claude")]
        provider: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Resume {
        session_id: String,
        #[arg(long, default_value = "claude")]
        provider: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SystemAction {
    Install {
        #[arg(long, value_enum, default_value_t = CliInstallTarget::Auto)]
        target: CliInstallTarget,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Repair {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Update {
        version: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Rollback {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Uninstall {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Start {
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Stop {
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Restart {
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Status {
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Doctor {
        #[arg(long)]
        durable_root: Option<PathBuf>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
    ApiGroups,
    Serve {
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(long)]
        durable_root: PathBuf,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        static_root: Option<PathBuf>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        once: bool,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CliInstallTarget {
    Auto,
    MacosAppBundle,
    WindowsInstaller,
    LinuxCliWeb,
}

impl CliInstallTarget {
    fn into_target(self) -> InstallTarget {
        match self {
            CliInstallTarget::Auto => match std::env::consts::OS {
                "macos" => InstallTarget::MacOsAppBundle,
                "windows" => InstallTarget::WindowsInstaller,
                _ => InstallTarget::LinuxCliWeb,
            },
            CliInstallTarget::MacosAppBundle => InstallTarget::MacOsAppBundle,
            CliInstallTarget::WindowsInstaller => InstallTarget::WindowsInstaller,
            CliInstallTarget::LinuxCliWeb => InstallTarget::LinuxCliWeb,
        }
    }
}

pub fn run() -> RefineResult<()> {
    let cli = Cli::parse();
    dispatch(cli)
}

pub fn dispatch(cli: Cli) -> RefineResult<()> {
    match cli.command {
        Commands::Workflow {
            action: WorkflowAction::Allowed { from, to },
        } => {
            let from: GapStatus = from.into();
            let to: GapStatus = to.into();
            let decision = user_status_transition(&from, &to);
            println!("{}", serde_json::to_string_pretty(&decision).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action:
                WorkflowAction::Transition {
                    id,
                    target,
                    durable_root: Some(durable_root),
                },
        } => {
            let target: GapStatus = target.into();
            let gap = FileWorkItemService::new(durable_root).transition_gap_status(&id, target)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Workflow {
            action:
                WorkflowAction::BulkTransition {
                    target,
                    durable_root: Some(durable_root),
                    selected_ids,
                    status,
                    q,
                },
        } => {
            let target: GapStatus = target.into();
            let selection = BulkGapSelection {
                filter: BulkGapFilter {
                    status,
                    q,
                    ..Default::default()
                },
                selected_ids: if selected_ids.is_empty() {
                    None
                } else {
                    Some(selected_ids)
                },
                exclude_ids: Vec::new(),
            };
            let result = FileWorkItemService::new(durable_root).bulk_update_gaps(
                selection,
                BulkGapUpdate::Status(target.as_str().to_string()),
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action:
                WorkflowAction::Schedule {
                    durable_root,
                    runtime_root,
                },
        } => {
            let scheduler = FileSchedulingService::with_durable_root(runtime_root, durable_root);
            let promoted = scheduler.promote()?;
            let state = scheduler.load_state()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "promoted": promoted,
                    "reservations": state.reservations
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Pause { runtime_root },
        } => {
            let scheduler = FileSchedulingService::new(&runtime_root);
            let supervisor = FileProcessSupervisor::new(runtime_root);
            supervisor.set_agents_paused(true)?;
            let state = supervisor.set_background_processes_stopped(true)?;
            scheduler.pause(SchedulerControl::Agents)?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Resume { runtime_root },
        } => {
            let scheduler = FileSchedulingService::new(&runtime_root);
            let supervisor = FileProcessSupervisor::new(runtime_root);
            supervisor.set_background_processes_stopped(false)?;
            let state = supervisor.set_agents_paused(false)?;
            scheduler.resume(SchedulerControl::Agents)?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Restore { durable_root },
        } => {
            let result = FileWorkItemService::new(durable_root).bulk_update_gaps(
                BulkGapSelection {
                    filter: BulkGapFilter::default(),
                    selected_ids: None,
                    exclude_ids: Vec::new(),
                },
                BulkGapUpdate::Status("__last_workflow_state".to_string()),
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Enforce { durable_root },
        } => {
            let snapshot = FileProjectStateStore::new(durable_root).rebuild_projection()?;
            let automated: Vec<_> = snapshot
                .gaps
                .values()
                .filter(|gap| {
                    matches!(
                        gap.gap.status,
                        GapStatus::InProgress
                            | GapStatus::Qa
                            | GapStatus::ReadyMerge
                            | GapStatus::AwaitingRebuild
                    )
                })
                .map(|gap| gap.gap.id.clone())
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "ok": true,
                    "checked": snapshot.gaps.len(),
                    "automated": automated
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::System {
            action: SystemAction::ApiGroups,
        } => {
            let groups: Vec<_> = API_GROUPS
                .iter()
                .map(|group| json!({"prefix": group.prefix, "capability": group.capability}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&groups).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Install {
                    target,
                    runtime_root,
                    version,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, version)
                .install(target.into_target())?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Repair {
                    runtime_root,
                    version,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, version).repair()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Update {
                    version,
                    runtime_root,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION"))
                .update(&version)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Rollback {
                    runtime_root,
                    version,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, version).rollback()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Uninstall {
                    runtime_root,
                    version,
                },
        } => {
            FileInstallationService::new(runtime_root, version).uninstall()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"uninstalled": true})).unwrap()
            );
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Doctor {
                    durable_root,
                    runtime_root,
                    repo_root,
                },
        } => {
            let report =
                FileDiagnosticsService::new(durable_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::List { durable_root },
        } => {
            let nodes = FileNodeRegistryService::new(durable_root).list_response()?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Show { id, durable_root },
        } => {
            let node = FileNodeRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Create { id, durable_root },
        } => {
            let node = FileNodeRegistryService::new(durable_root).create(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Activate { id, durable_root },
        } => {
            let nodes = FileNodeRegistryService::new(durable_root).activate(&id)?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Archive { id, durable_root },
        } => {
            let node = FileNodeRegistryService::new(durable_root).archive(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Rename {
                    id,
                    name,
                    durable_root,
                },
        } => {
            let node = FileNodeRegistryService::new(durable_root).rename(&id, &name)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Settings { id, durable_root },
        } => {
            let settings = FileNodeRegistryService::new(durable_root).settings(&id)?;
            println!("{}", serde_json::to_string_pretty(&settings).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Transfer {
                    id,
                    item_id,
                    durable_root: Some(durable_root),
                },
        } => {
            FileNodeRegistryService::new(&durable_root).show(&id)?;
            let result = FileWorkItemService::new(durable_root).bulk_transfer_gaps_to_node(
                &id,
                BulkGapSelection {
                    filter: BulkGapFilter::default(),
                    selected_ids: Some(vec![item_id]),
                    exclude_ids: Vec::new(),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::List { durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).list_response()?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::Show { id, durable_root },
        } => {
            let node = FileClusterRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::AddNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).add_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::EditNode { id, durable_root },
        } => {
            let node = FileClusterRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::EnableNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).set_enabled(&id, true)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::DisableNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).set_enabled(&id, false)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::RemoveNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).remove_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::Sync { durable_root },
        } => {
            let sync = FileClusterRegistryService::new(durable_root).sync_response()?;
            println!("{}", serde_json::to_string_pretty(&sync).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Run {
                    id,
                    command,
                    durable_root,
                },
        } => {
            let result = FileClusterRegistryService::new(durable_root).run_remote(&id, &command)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Transfer {
                    id,
                    item_id,
                    durable_root,
                },
        } => {
            let service = FileClusterRegistryService::new(&durable_root);
            service.transfer(&item_id, &id)?;
            let result = FileWorkItemService::new(durable_root).bulk_transfer_gaps_to_node(
                &id,
                BulkGapSelection {
                    filter: BulkGapFilter::default(),
                    selected_ids: Some(vec![item_id]),
                    exclude_ids: Vec::new(),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::Maintenance { durable_root },
        } => {
            let maintenance =
                FileClusterRegistryService::new(durable_root).maintenance_response()?;
            println!("{}", serde_json::to_string_pretty(&maintenance).unwrap());
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::List {
                    durable_root,
                    limit,
                },
        } => {
            let entries = FileActivityService::new(durable_root).recent(limit)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Tail {
                    durable_root,
                    limit,
                },
        } => {
            let entries = FileActivityService::new(durable_root).recent(limit)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries, "tail": true})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action: LogAction::Show { id, durable_root },
        } => {
            let service = FileActivityService::new(durable_root);
            let limit = service.count()?.max(1);
            let Some(entry) = service
                .query(limit, 0, None, None, None, None, None)?
                .into_iter()
                .find(|entry| entry.id == id)
            else {
                return Err(crate::core::supervisor::errors::RefineError::NotFound(
                    format!("Log entry {id} was not found"),
                ));
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entry": entry})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Query {
                    q,
                    durable_root,
                    limit,
                    offset,
                    gap_id,
                    severity,
                    category,
                    actor,
                },
        } => {
            let entries = FileActivityService::new(durable_root).query(
                limit,
                offset,
                gap_id.as_deref(),
                severity.as_deref(),
                category.as_deref(),
                actor.as_deref(),
                Some(&q),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Export {
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileActivityService::new(durable_root);
            let limit = service.count()?;
            let entries = if limit == 0 {
                Vec::new()
            } else {
                service.query(limit, 0, None, None, None, None, None)?
            };
            let exported = entries.len();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries, "exported": exported}))
                    .unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Bundle {
                    durable_root,
                    runtime_root,
                    repo_root,
                    redact_secrets,
                },
        } => {
            let bundle = FileSupportBundleService::new(durable_root, runtime_root, repo_root)
                .export(redact_secrets)?;
            println!("{}", serde_json::to_string_pretty(&bundle).unwrap());
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Detect,
        } => {
            let providers = HostAgentProviderService::new().detect()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"providers": providers})).unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Configure { provider },
        } => {
            HostAgentProviderService::new().configure(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "configured": true})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Auth { provider },
        } => {
            HostAgentProviderService::new().authenticate(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "authenticated": true})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Diagnose { provider },
        } => {
            let diagnostics = HostAgentProviderService::new().diagnose(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "diagnostics": diagnostics})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action:
                AgentAction::Invoke {
                    prompt,
                    provider,
                    cwd,
                },
        } => {
            let output = HostAgentProviderService::new().invoke(ProviderInvocation {
                provider,
                prompt,
                session_id: None,
                cwd: cwd.map(|path| path.display().to_string()),
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": true, "output": output})).unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action:
                AgentAction::Resume {
                    session_id,
                    provider,
                },
        } => {
            let output = HostAgentProviderService::new().resume(&provider, &session_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": true, "output": output})).unwrap()
            );
            Ok(())
        }
        Commands::System {
            action: SystemAction::Start { port, runtime_root },
        } => {
            let status =
                FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root }).start(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Stop { port, runtime_root },
        } => {
            let status =
                FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root }).stop(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Restart { port, runtime_root },
        } => {
            let status = FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root })
                .restart(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Status { port, runtime_root },
        } => {
            let status =
                FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root }).status(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Status {
                    runtime_root,
                    durable_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root).status()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Attach {
                    path,
                    runtime_root,
                    durable_root,
                },
        } => {
            let status =
                FileProjectRegistryService::new(runtime_root, durable_root).attach(&path)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Switch {
                    name,
                    runtime_root,
                    durable_root,
                },
        } => {
            let status =
                FileProjectRegistryService::new(runtime_root, durable_root).switch(&name)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Detach {
                    runtime_root,
                    durable_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root).detach()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Register {
                    name,
                    path,
                    runtime_root,
                    durable_root,
                },
        } => {
            let path = absolutize_cli_path(&path)?;
            let timestamp = cli_timestamp();
            let registry = FileProjectRegistryService::new(runtime_root, durable_root).register(
                RegisteredApp {
                    name,
                    path: path.display().to_string(),
                    added_at: timestamp,
                    last_used_at: None,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Remove {
                    name,
                    runtime_root,
                    durable_root,
                },
        } => {
            let registry =
                FileProjectRegistryService::new(runtime_root, durable_root).remove(&name)?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Migrate {
                    durable_root,
                    runtime_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root).status()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "ok": true,
                    "migrated": false,
                    "schema": status.schema,
                    "message": "project schema is already compatible"
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Doctor {
                    durable_root,
                    runtime_root,
                    repo_root,
                },
        } => {
            let report =
                FileDiagnosticsService::new(durable_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Sync {
                    durable_root: Some(durable_root),
                    cache_dir,
                },
        } => {
            let store = FileProjectStateStore::new(&durable_root);
            let snapshot = store.rebuild_projection()?;
            if let Some(cache_dir) = &cache_dir {
                store.persist_projection_snapshot(&cache_dir, &snapshot)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "gaps": snapshot.gaps.len(),
                    "features": snapshot.features.len(),
                    "source_fingerprints": snapshot.source_fingerprints.len(),
                    "status_counts": snapshot.status_counts(),
                    "cache_updated": cache_dir.is_some()
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Create {
                    name,
                    durable_root: Some(durable_root),
                    id,
                },
        } => {
            let gap =
                FileWorkItemService::new(durable_root).create_gap_summary(&name, id.as_deref())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::List {
                    durable_root: Some(durable_root),
                },
        } => {
            let gaps: Vec<_> = FileWorkItemService::new(durable_root)
                .list_gap_summaries()?
                .into_iter()
                .map(|gap| gap.gap)
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gaps": gaps})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Show {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).show_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Edit {
                    id,
                    durable_root: Some(durable_root),
                    name,
                    priority,
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).update_gap_metadata_summary(
                &id,
                name.as_deref(),
                priority.as_deref(),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Note {
                    id,
                    body,
                    durable_root: Some(durable_root),
                    author,
                },
        } => {
            let gap =
                FileWorkItemService::new(durable_root).add_gap_note_summary(&id, &author, &body)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Round {
                    id,
                    durable_root: Some(durable_root),
                    reporter,
                    actual,
                    target,
                    edit_latest,
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
            let gap = if edit_latest {
                service.edit_latest_gap_round_summary(
                    &id,
                    reporter.as_deref(),
                    actual.as_deref(),
                    target.as_deref(),
                )?
            } else {
                let Some(reporter) = reporter.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "round reporter is required".to_string(),
                    ));
                };
                let Some(actual) = actual.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "round actual is required".to_string(),
                    ));
                };
                let Some(target) = target.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "round target is required".to_string(),
                    ));
                };
                service.append_gap_round_summary(&id, reporter, actual, target)?
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Delete {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            FileWorkItemService::new(durable_root).delete_gap_record(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"deleted": true, "id": id})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Cancel {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).cancel_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Start {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
            let current = service.show_gap_summary(&id)?;
            if current.gap.status == GapStatus::Backlog {
                service.transition_gap_status(&id, GapStatus::Todo)?;
            }
            let gap = service.start_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Retry {
                    id,
                    durable_root: Some(durable_root),
                    stage,
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
            let gap = match stage.as_str() {
                "quality" | "qa" => service.retry_gap_quality_summary(&id)?,
                "merge" => service.retry_gap_merge_summary(&id)?,
                _ => {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "retry stage must be quality or merge".to_string(),
                    ));
                }
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Verify {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).verify_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Merge {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).merge_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Undo {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).undo_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::AssignFeature {
                    id,
                    feature_id,
                    durable_root: Some(durable_root),
                },
        } => {
            let feature =
                FileWorkItemService::new(durable_root).assign_gap_to_feature(&feature_id, &id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::RemoveFeature {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
            let current = service.show_gap_summary(&id)?;
            let Some(feature_id) = current.gap.feature_id.clone() else {
                return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                    format!("Gap {id} is not assigned to a Feature"),
                ));
            };
            let feature = service.remove_gap_from_feature(&feature_id, &id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Create {
                    name,
                    durable_root: Some(durable_root),
                    id,
                    description,
                    reporter,
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).create_feature_summary(
                &name,
                id.as_deref(),
                description.as_deref(),
                reporter.as_deref(),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Edit {
                    id,
                    durable_root: Some(durable_root),
                    name,
                    description,
                    reporter,
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).update_feature_metadata_summary(
                &id,
                name.as_deref(),
                description.as_deref(),
                reporter.as_deref(),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::List {
                    durable_root: Some(durable_root),
                },
        } => {
            let features: Vec<_> = FileWorkItemService::new(durable_root)
                .list_feature_summaries()?
                .into_iter()
                .map(|feature| {
                    json!({
                        "feature": feature.feature,
                        "gap_ids": feature.gap_ids,
                        "rollup": feature.rollup
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"features": features})).unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Show {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).show_feature_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::AddGap {
                    id,
                    gap_id,
                    durable_root: Some(durable_root),
                },
        } => {
            let feature =
                FileWorkItemService::new(durable_root).assign_gap_to_feature(&id, &gap_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::RemoveGap {
                    id,
                    gap_id,
                    durable_root: Some(durable_root),
                },
        } => {
            let feature =
                FileWorkItemService::new(durable_root).remove_gap_from_feature(&id, &gap_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::ReorderGap {
                    id,
                    gap_id,
                    order,
                    durable_root: Some(durable_root),
                },
        } => {
            let feature = FileWorkItemService::new(durable_root)
                .reorder_gap_in_feature(&id, &gap_id, order)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Move {
                    id,
                    target,
                    durable_root: Some(durable_root),
                },
        } => {
            let Some(target) = GapStatus::parse_wire(&target) else {
                return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                    "target must be backlog or todo".to_string(),
                ));
            };
            let feature =
                FileWorkItemService::new(durable_root).move_feature_workflow(&id, target)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Cancel {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).cancel_feature_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Delete {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            FileWorkItemService::new(durable_root).delete_feature_record(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"deleted": true, "id": id})).unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Import {
                    durable_root,
                    text,
                    file,
                    csv,
                    reporter,
                    feature_id,
                },
        } => {
            let service = FileImportService::new(durable_root);
            let result = if let Some(file) = file {
                service.import_from_file(file, csv, reporter.as_deref(), feature_id.as_deref())?
            } else {
                let Some(text) = text.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "feature import requires --text or --file".to_string(),
                    ));
                };
                service.import_from_text(text, csv, reporter.as_deref(), feature_id.as_deref())?
            };
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Serve {
                    port,
                    durable_root,
                    cache_dir,
                    static_root,
                    runtime_root,
                    token,
                    once,
                },
        } => {
            let store = FileProjectStateStore::new(&durable_root);
            let snapshot = store.rebuild_projection()?;
            if let Some(cache_dir) = &cache_dir {
                store.persist_projection_snapshot(cache_dir, &snapshot)?;
            }
            let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
                root: runtime_root.clone(),
            });
            let status = lifecycle.start(port)?;
            let daemon = LocalHttpDaemon {
                server: InProcessWebServer {
                    status,
                    projection: snapshot,
                    auth_token: token,
                    durable_root: Some(durable_root),
                    runtime_root: Some(runtime_root),
                },
                static_root: static_root.or_else(default_static_root),
            };
            let listener = LocalHttpDaemon::bind_loopback(port)?;
            let addr = LocalHttpDaemon::local_addr(&listener)?;
            eprintln!("serving Refine daemon API at http://{addr}");
            if once {
                daemon.serve_next(&listener)?;
            } else {
                loop {
                    daemon.serve_next(&listener)?;
                }
            }
            Ok(())
        }
        other => Err(crate::core::supervisor::errors::RefineError::InvalidInput(
            format!(
                "CLI command is missing required options or cannot run in this mode: {other:?}"
            ),
        )),
    }
}

fn default_static_root() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("src/surfaces/web/static"),
        PathBuf::from("rust/src/surfaces/web/static"),
    ];
    candidates.into_iter().find(|path| path.is_dir())
}

fn absolutize_cli_path(path: &str) -> RefineResult<PathBuf> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
            "path is required".to_string(),
        ));
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| {
                crate::core::supervisor::errors::RefineError::Io(format!(
                    "failed to inspect cwd: {error}"
                ))
            })?
            .join(path))
    }
}

fn cli_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product::project_state::PROJECTION_SNAPSHOT_FILE;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn project_sync_rebuilds_projection_from_cli_surface() {
        let temp_root = unique_temp_dir("cli-project-sync");
        let durable_root = temp_root.join(".refine");
        let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
        let cache_dir = temp_root.join("run").join("8080").join("cache");
        fs::create_dir_all(&gap_dir).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "CLI visible Gap",
              "status": "done",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
        )
        .unwrap();

        let cli = Cli::try_parse_from([
            "refine",
            "project",
            "sync",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--cache-dir",
            cache_dir.to_str().unwrap(),
        ])
        .unwrap();
        dispatch(cli).unwrap();

        assert!(cache_dir.join(PROJECTION_SNAPSHOT_FILE).exists());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn project_registry_commands_use_shared_file_project_registry_service() {
        let temp_root = unique_temp_dir("cli-project-registry");
        let runtime_root = temp_root.join("run");
        let app_one = temp_root.join("app-one");
        let app_two = temp_root.join("app-two");
        fs::create_dir_all(app_one.join(".refine")).unwrap();
        fs::create_dir_all(app_two.join(".refine")).unwrap();

        dispatch(
            Cli::try_parse_from([
                "refine",
                "project",
                "status",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--durable-root",
                app_one.join(".refine").to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let registry_path = runtime_root.join("apps.json");
        let registry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
        assert_eq!(registry["active_app"], app_one.to_str().unwrap());

        dispatch(
            Cli::try_parse_from([
                "refine",
                "project",
                "detach",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let registry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
        assert!(registry["active_app"].is_null());

        dispatch(
            Cli::try_parse_from([
                "refine",
                "project",
                "attach",
                app_one.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "project",
                "register",
                "second",
                app_two.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "project",
                "switch",
                "second",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let registry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
        assert_eq!(registry["active_app"], app_two.to_str().unwrap());

        dispatch(
            Cli::try_parse_from([
                "refine",
                "project",
                "remove",
                "second",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let registry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
        assert!(registry["apps"].get(app_two.to_str().unwrap()).is_none());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn project_and_system_doctor_and_migrate_use_observability_services() {
        let temp_root = unique_temp_dir("cli-doctor-migrate");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run");
        fs::create_dir_all(&durable_root).unwrap();

        for argv in [
            vec![
                "refine",
                "project",
                "doctor",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--repo-root",
                temp_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "system",
                "doctor",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--repo-root",
                temp_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "project",
                "migrate",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ],
        ] {
            dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn system_install_update_rollback_and_uninstall_use_installation_service() {
        let temp_root = unique_temp_dir("cli-installation");
        let runtime_root = temp_root.join("run");

        for argv in [
            vec![
                "refine",
                "system",
                "install",
                "--target",
                "linux-cli-web",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--version",
                "1.0.0",
            ],
            vec![
                "refine",
                "system",
                "update",
                "1.1.0",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "system",
                "rollback",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--version",
                "1.1.0",
            ],
            vec![
                "refine",
                "system",
                "repair",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--version",
                "1.0.0",
            ],
            vec![
                "refine",
                "system",
                "uninstall",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--version",
                "1.0.0",
            ],
        ] {
            dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
        }

        let state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(runtime_root.join("install-state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["status"]["installed"], false);
        assert_eq!(state["status"]["version"], "1.0.0");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn gap_create_list_show_use_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-gap-create");
        let durable_root = temp_root.join(".refine");

        let create = Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "CLI Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap();
        dispatch(create).unwrap();

        let list = Cli::try_parse_from([
            "refine",
            "gap",
            "list",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap();
        dispatch(list).unwrap();

        let show = Cli::try_parse_from([
            "refine",
            "gap",
            "show",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap();
        dispatch(show).unwrap();

        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"name\": \"CLI Gap\""));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn gap_edit_note_delete_use_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-gap-edit-note-delete");
        let durable_root = temp_root.join(".refine");

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Original",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "edit",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--name",
                "Renamed",
                "--priority",
                "medium",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "note",
                "GAP1",
                "CLI note",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--author",
                "Reviewer",
            ])
            .unwrap(),
        )
        .unwrap();

        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"name\": \"Renamed\""));
        assert!(written.contains("\"priority\": \"medium\""));
        assert!(written.contains("\"body\": \"CLI note\""));

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "delete",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn gap_round_append_and_edit_use_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-gap-rounds");
        let durable_root = temp_root.join(".refine");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Round Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "round",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--reporter",
                "Reporter",
                "--actual",
                "Actual",
                "--target",
                "Target",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "round",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--edit-latest",
                "--reporter",
                "Reviewer",
                "--actual",
                "Revised",
            ])
            .unwrap(),
        )
        .unwrap();

        let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(written.contains("\"reporter\": \"Reviewer\""));
        assert!(written.contains("\"actual\": \"Revised\""));
        assert!(written.contains("\"target\": \"Target\""));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn gap_merge_and_undo_use_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-gap-merge-undo");
        let durable_root = temp_root.join(".refine");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Merge Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();
        let gap_path = durable_root.join("gaps/GA/P1/gap.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&gap_path).unwrap()).unwrap();
        value["status"] = serde_json::Value::String("ready-merge".to_string());
        fs::write(&gap_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "merge",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let written = fs::read_to_string(&gap_path).unwrap();
        assert!(written.contains("\"status\": \"done\""));

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "undo",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let written = fs::read_to_string(&gap_path).unwrap();
        assert!(written.contains("\"status\": \"review\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn workflow_schedule_uses_file_scheduler_service() {
        let temp_root = unique_temp_dir("cli-workflow-schedule");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Schedulable Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "workflow",
                "transition",
                "GAP1",
                "todo",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "workflow",
                "schedule",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();

        let scheduler_state =
            fs::read_to_string(runtime_root.join("scheduler-state.json")).unwrap();
        assert!(scheduler_state.contains("\"gap_id\": \"GAP1\""));
        assert!(scheduler_state.contains("\"state\": \"reserved\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn feature_create_list_show_and_membership_use_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-feature-membership");
        let durable_root = temp_root.join(".refine");

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Gap One",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "create",
                "Feature One",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "FEA1",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "add-gap",
                "FEA1",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "show",
                "FEA1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "list",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();

        let assigned = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(assigned.contains("\"feature_id\": \"FEA1\""));
        assert!(assigned.contains("\"feature_order\": 1"));

        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "remove-gap",
                "FEA1",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let removed = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(removed.contains("\"feature_id\": null"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn cli_gap_lifecycle_membership_and_feature_edit_use_core_services() {
        let temp_root = unique_temp_dir("cli-gap-lifecycle");
        let durable_root = temp_root.join(".refine");
        for (command, args) in [
            (
                "gap",
                vec![
                    "create",
                    "Lifecycle Gap",
                    "--durable-root",
                    durable_root.to_str().unwrap(),
                    "--id",
                    "GAP1",
                ],
            ),
            (
                "feature",
                vec![
                    "create",
                    "Feature One",
                    "--durable-root",
                    durable_root.to_str().unwrap(),
                    "--id",
                    "FEA1",
                ],
            ),
        ] {
            let mut argv = vec!["refine", command];
            argv.extend(args);
            dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
        }

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "assign-feature",
                "GAP1",
                "FEA1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"feature_id\": \"FEA1\"")
        );

        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "edit",
                "FEA1",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--name",
                "Renamed Feature",
                "--description",
                "Edited",
                "--reporter",
                "QA",
            ])
            .unwrap(),
        )
        .unwrap();
        let feature = fs::read_to_string(durable_root.join("features/FE/A1/feature.json")).unwrap();
        assert!(feature.contains("\"name\": \"Renamed Feature\""));
        assert!(feature.contains("\"reporter\": \"QA\""));

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "remove-feature",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"feature_id\": null")
        );

        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "start",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"status\": \"in-progress\"")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn feature_reorder_and_move_use_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-feature-reorder-move");
        let durable_root = temp_root.join(".refine");
        for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
            dispatch(
                Cli::try_parse_from([
                    "refine",
                    "gap",
                    "create",
                    name,
                    "--durable-root",
                    durable_root.to_str().unwrap(),
                    "--id",
                    id,
                ])
                .unwrap(),
            )
            .unwrap();
        }
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "create",
                "Feature One",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "FEA1",
            ])
            .unwrap(),
        )
        .unwrap();
        for gap_id in ["GAP1", "GAP2"] {
            dispatch(
                Cli::try_parse_from([
                    "refine",
                    "feature",
                    "add-gap",
                    "FEA1",
                    gap_id,
                    "--durable-root",
                    durable_root.to_str().unwrap(),
                ])
                .unwrap(),
            )
            .unwrap();
        }
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "reorder-gap",
                "FEA1",
                "GAP2",
                "1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P2/gap.json"))
                .unwrap()
                .contains("\"feature_order\": 1")
        );

        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "move",
                "FEA1",
                "todo",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"status\": \"todo\"")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn feature_cancel_and_delete_use_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-feature-cancel-delete");
        let durable_root = temp_root.join(".refine");
        for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
            dispatch(
                Cli::try_parse_from([
                    "refine",
                    "gap",
                    "create",
                    name,
                    "--durable-root",
                    durable_root.to_str().unwrap(),
                    "--id",
                    id,
                ])
                .unwrap(),
            )
            .unwrap();
        }
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "create",
                "Feature One",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "FEA1",
            ])
            .unwrap(),
        )
        .unwrap();
        for gap_id in ["GAP1", "GAP2"] {
            dispatch(
                Cli::try_parse_from([
                    "refine",
                    "feature",
                    "add-gap",
                    "FEA1",
                    gap_id,
                    "--durable-root",
                    durable_root.to_str().unwrap(),
                ])
                .unwrap(),
            )
            .unwrap();
        }

        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "cancel",
                "FEA1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"status\": \"cancelled\"")
        );

        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "delete",
                "FEA1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(!durable_root.join("features/FE/A1/feature.json").exists());
        assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
        assert!(!durable_root.join("gaps/GA/P2/gap.json").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn feature_import_uses_shared_import_service() {
        let temp_root = unique_temp_dir("cli-feature-import");
        let durable_root = temp_root.join(".refine");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "create",
                "Imported Feature",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "FEA1",
            ])
            .unwrap(),
        )
        .unwrap();
        let csv = temp_root.join("import.csv");
        fs::write(
            &csv,
            "actual,target,reporter,priority\nBroken flow,Fixed flow,QA,high\n",
        )
        .unwrap();

        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "import",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--file",
                csv.to_str().unwrap(),
                "--csv",
                "--feature-id",
                "FEA1",
            ])
            .unwrap(),
        )
        .unwrap();

        let snapshot = FileProjectStateStore::new(&durable_root)
            .rebuild_projection()
            .unwrap();
        let gap = snapshot.gaps.values().next().unwrap();
        assert_eq!(gap.gap.feature_id.as_deref(), Some("FEA1"));
        assert_eq!(gap.gap.priority.as_str(), "high");
        assert_eq!(gap.gap.reporter.as_deref(), Some("QA"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn workflow_transition_uses_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-workflow-transition");
        let durable_root = temp_root.join(".refine");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Workflow Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();

        dispatch(
            Cli::try_parse_from([
                "refine",
                "workflow",
                "transition",
                "GAP1",
                "todo",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"status\": \"todo\"")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn workflow_bulk_transition_uses_shared_file_work_item_service() {
        let temp_root = unique_temp_dir("cli-workflow-bulk");
        let durable_root = temp_root.join(".refine");
        for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
            dispatch(
                Cli::try_parse_from([
                    "refine",
                    "gap",
                    "create",
                    name,
                    "--durable-root",
                    durable_root.to_str().unwrap(),
                    "--id",
                    id,
                ])
                .unwrap(),
            )
            .unwrap();
        }

        dispatch(
            Cli::try_parse_from([
                "refine",
                "workflow",
                "bulk-transition",
                "todo",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--selected-id",
                "GAP1",
                "--selected-id",
                "GAP2",
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"status\": \"todo\"")
        );
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P2/gap.json"))
                .unwrap()
                .contains("\"status\": \"todo\"")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn workflow_control_commands_use_core_state() {
        let temp_root = unique_temp_dir("cli-workflow-control");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Workflow Control Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "workflow",
                "transition",
                "GAP1",
                "todo",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();

        for argv in [
            vec![
                "refine",
                "workflow",
                "schedule",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "workflow",
                "pause",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "workflow",
                "resume",
                "--runtime-root",
                runtime_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "workflow",
                "enforce",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
        ] {
            dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
        }

        let pause_state: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(runtime_root.join("process-control.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(pause_state["agents_paused"], false);
        assert_eq!(pause_state["background_processes_stopped"], false);

        dispatch(
            Cli::try_parse_from([
                "refine",
                "workflow",
                "bulk-transition",
                "failed",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--selected-id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from([
                "refine",
                "workflow",
                "restore",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert!(
            fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
                .unwrap()
                .contains("\"status\": \"todo\"")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn log_commands_use_shared_activity_service() {
        let temp_root = unique_temp_dir("cli-log-activity");
        let durable_root = temp_root.join(".refine");
        let service = FileActivityService::new(&durable_root);
        let first = service.new_entry(
            "Build failed",
            "error",
            "quality",
            Some("GAP1".to_string()),
            Some("agent".to_string()),
        );
        let first_id = first.id.clone();
        service.append(first).unwrap();
        service
            .append(service.new_entry("Build passed", "info", "quality", None, None))
            .unwrap();

        for argv in [
            vec![
                "refine",
                "log",
                "list",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--limit",
                "2",
            ],
            vec![
                "refine",
                "log",
                "tail",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--limit",
                "1",
            ],
            vec![
                "refine",
                "log",
                "query",
                "failed",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--severity",
                "error",
                "--gap-id",
                "GAP1",
            ],
            vec![
                "refine",
                "log",
                "show",
                first_id.as_str(),
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "log",
                "export",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
        ] {
            dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn log_bundle_exports_redacted_support_bundle() {
        let temp_root = unique_temp_dir("cli-log-bundle");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run");
        fs::create_dir_all(&durable_root).unwrap();
        fs::write(
            durable_root.join("settings.json"),
            r#"{"provider_token":"secret-value","visible":"ok"}"#,
        )
        .unwrap();

        dispatch(
            Cli::try_parse_from([
                "refine",
                "log",
                "bundle",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--repo-root",
                temp_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();

        let bundle_dir = durable_root.join("support-bundles");
        let bundle_path = fs::read_dir(&bundle_dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let body = fs::read_to_string(bundle_path).unwrap();
        assert!(body.contains("[redacted]"));
        assert!(!body.contains("secret-value"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn node_commands_use_shared_node_registry_service() {
        let temp_root = unique_temp_dir("cli-node-registry");
        let durable_root = temp_root.join(".refine");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Owned Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();

        for argv in [
            vec![
                "refine",
                "node",
                "list",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "node",
                "create",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "node",
                "rename",
                "node-1",
                "Node One",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "node",
                "activate",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "node",
                "settings",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "node",
                "transfer",
                "node-1",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "node",
                "activate",
                "default",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "node",
                "archive",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
        ] {
            dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
        }

        let gap = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(gap.contains("\"node_id\": \"node-1\""));
        let nodes: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(durable_root.join("nodes.json")).unwrap())
                .unwrap();
        assert_eq!(nodes["nodes"][1]["display_name"], "Node One");
        assert_eq!(nodes["nodes"][1]["archived"], true);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn cluster_commands_use_shared_cluster_registry_service() {
        let temp_root = unique_temp_dir("cli-cluster-registry");
        let durable_root = temp_root.join(".refine");
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                "Cluster Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ])
            .unwrap(),
        )
        .unwrap();

        for argv in [
            vec![
                "refine",
                "cluster",
                "list",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "add-node",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "show",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "edit-node",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "disable-node",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "enable-node",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "transfer",
                "node-1",
                "GAP1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "sync",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "maintenance",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
            vec![
                "refine",
                "cluster",
                "remove-node",
                "node-1",
                "--durable-root",
                durable_root.to_str().unwrap(),
            ],
        ] {
            dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
        }

        let gap = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
        assert!(gap.contains("\"node_id\": \"node-1\""));
        let cluster: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(durable_root.join("cluster.json")).unwrap())
                .unwrap();
        assert_eq!(cluster["nodes"].as_array().unwrap().len(), 0);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn agent_configure_and_diagnose_use_provider_service() {
        dispatch(
            Cli::try_parse_from(["refine", "agent", "configure", "--provider", "smoke-ai"])
                .unwrap(),
        )
        .unwrap();
        dispatch(
            Cli::try_parse_from(["refine", "agent", "diagnose", "--provider", "smoke-ai"]).unwrap(),
        )
        .unwrap();
        let invalid = dispatch(
            Cli::try_parse_from(["refine", "agent", "configure", "--provider", "nope"]).unwrap(),
        );
        assert!(invalid.is_err());
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
}
