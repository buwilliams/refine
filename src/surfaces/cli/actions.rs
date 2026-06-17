use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::model::workflow::GapStatus;
use crate::tools::host::installation::InstallTarget;

#[derive(Debug, Parser)]
#[command(name = "refine")]
#[command(about = "Refine - Your team's agentic software delivery system.")]
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
    Website {
        #[arg(long, default_value_t = 8099)]
        port: u16,
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
        bind_address: IpAddr,
        #[arg(long, default_value = ".")]
        static_root: PathBuf,
        #[arg(long)]
        once: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectAction {
    Status {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Attach {
        path: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Switch {
        name: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Detach {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Register {
        name: String,
        path: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
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
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Remove {
        name: String,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Migrate {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Sync {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    Doctor {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
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
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
    },
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Edit {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        priority: Option<String>,
    },
    Note {
        id: String,
        body: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long, default_value = "")]
        author: String,
    },
    NoteEdit {
        id: String,
        note_id: String,
        body: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    NoteDelete {
        id: String,
        note_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Round {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
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
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Cancel {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Retry {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long, default_value = "quality")]
        stage: String,
    },
    Verify {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Merge {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Undo {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Delete {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    AssignFeature {
        id: String,
        feature_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    RemoveFeature {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FeatureAction {
    Create {
        name: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        reporter: Option<String>,
    },
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Edit {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
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
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    RemoveGap {
        id: String,
        gap_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    ReorderGap {
        id: String,
        gap_id: String,
        order: i64,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    OrderGap {
        id: String,
        gap_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    UnorderGap {
        id: String,
        gap_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Move {
        id: String,
        target: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Transfer {
        id: String,
        node_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Cancel {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Delete {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Import {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
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
    Pause {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Resume {
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CliGapStatus {
    Backlog,
    Todo,
    InProgress,
    Qa,
    ReadyMerge,
    Build,
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
            CliGapStatus::Build => Self::Build,
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
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Create {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Activate {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Archive {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Rename {
        id: String,
        name: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Settings {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Transfer {
        id: String,
        item_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ClusterAction {
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Show {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    AddNode {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
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
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    EnableNode {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    DisableNode {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    RemoveNode {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Bootstrap {
        id: String,
        #[arg(long)]
        dry_run: bool,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Sync {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Run {
        id: String,
        command: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Transfer {
        id: String,
        item_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Maintenance {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum LogAction {
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    Tail {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Show {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
    },
    Query {
        q: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
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
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    Bundle {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
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
        #[arg(long)]
        port: u16,
        #[arg(long, value_enum, default_value_t = CliInstallTarget::Auto)]
        target: CliInstallTarget,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Repair {
        #[arg(long)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Update {
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Rollback {
        #[arg(long)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Uninstall {
        #[arg(long)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    Start {
        #[arg(long, default_value_t = 8082)]
        port: u16,
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
        bind_address: IpAddr,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        #[arg(long)]
        static_root: Option<PathBuf>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        once: bool,
        #[arg(long)]
        foreground: bool,
    },
    Stop {
        #[arg(long, default_value_t = 8082)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Restart {
        #[arg(long, default_value_t = 8082)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Status {
        #[arg(long, default_value_t = 8082)]
        port: u16,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    Ps {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long)]
        stop: Option<String>,
        #[arg(long, default_value = "terminate")]
        signal: String,
    },
    Doctor {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
    ApiGroups,
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
