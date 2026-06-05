use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::core::host::installation::InstallTarget;
use crate::model::workflow::GapStatus;

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
    Clone {
        source: String,
        destination: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        make_current: bool,
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
        durable_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Create {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Activate {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Archive {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Rename {
        id: String,
        name: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Settings {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
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
        durable_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    AddNode {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    EditNode {
        id: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        ssh_host: Option<String>,
        #[arg(long)]
        ssh_user: Option<String>,
        #[arg(long)]
        ssh_identity_path: Option<String>,
        #[arg(long)]
        ssh_port: Option<u16>,
        #[arg(long)]
        refine_checkout: Option<String>,
        #[arg(long)]
        target_app_path: Option<String>,
        #[arg(long)]
        refine_port: Option<u16>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    EnableNode {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    DisableNode {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    RemoveNode {
        id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Sync {
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Run {
        id: String,
        command: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Transfer {
        id: String,
        item_id: String,
        #[arg(long)]
        durable_root: Option<PathBuf>,
    },
    Maintenance {
        #[arg(long)]
        durable_root: Option<PathBuf>,
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
    #[command(name = "web")]
    Web {
        #[arg(long, default_value_t = 8080)]
        port: u16,
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
        #[arg(long, hide = true)]
        foreground: bool,
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
    pub(super) fn into_target(self) -> InstallTarget {
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
