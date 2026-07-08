use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::model::workflow::GapStatus;
use crate::tools::host::installation::InstallTarget;

/// Refine: agent fleet software delivery — track Gaps, run agent workflows, and operate a fleet of nodes.
#[derive(Debug, Parser)]
#[command(name = "refine")]
#[command(about = "Refine - Your team's agentic software delivery system.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Manage which target application this Refine instance operates on.
    /// Attach, clone, switch, register, and diagnose target app repositories.
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Create and drive Gaps — units of work capturing the difference between actual and desired app behavior.
    /// Covers the full lifecycle: create, round, start, retry, verify, merge, undo.
    Gap {
        #[command(subcommand)]
        action: GapAction,
    },
    /// Manage Features — named groups of ordered Gaps delivered together.
    /// Group, order, move, transfer, and bulk-import Gaps under a Feature.
    Feature {
        #[command(subcommand)]
        action: FeatureAction,
    },
    /// Control the agent automation engine that advances Gaps through their workflow (pause/resume).
    Workflow {
        #[command(subcommand)]
        action: WorkflowAction,
    },
    /// Manage nodes — the machines that own active work — including turning this machine into a fleet node.
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// Operate the cluster (the fleet of nodes): register, provision, and bootstrap nodes,
    /// distribute unclaimed Gap ownership, and run remote commands.
    Cluster {
        #[command(subcommand)]
        action: ClusterAction,
    },
    /// Inspect the activity log: list, tail, query, and export entries, or build a support bundle.
    Log {
        #[command(subcommand)]
        action: LogAction,
    },
    /// Manage coding agent providers (e.g. claude): detect, configure, authenticate, diagnose, and invoke directly.
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Install, update, and operate the Refine daemon and service on this machine.
    System {
        #[command(subcommand)]
        action: SystemAction,
    },
    /// Recommend the next operations from current project and fleet state, each with the exact command to run.
    /// Start here when unsure what to do.
    Next {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Print a machine-readable JSON catalog of every CLI command with descriptions.
    /// Load this once instead of exploring --help per subcommand.
    Commands,
    /// Serve the Refine website as a local static file server (no daemon or project state required).
    Website {
        /// Port to listen on.
        #[arg(long, default_value_t = 8099)]
        port: u16,
        /// IP address to bind the listener to.
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
        bind_address: IpAddr,
        /// Directory containing the static website files to serve.
        #[arg(long, default_value = ".")]
        static_root: PathBuf,
        /// Serve a single request then exit (useful for smoke tests).
        #[arg(long)]
        once: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectAction {
    /// Show which target app is currently attached and the state of the project registry.
    Status {
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Attach an existing local repository as the current target app.
    /// The path is registered and becomes the app Refine operates on.
    Attach {
        /// Filesystem path to the target app repository.
        path: String,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Switch the current target app to another registered project by name.
    /// Migrates the project's Refine state if it uses an older schema.
    Switch {
        /// Registered project name to make current.
        name: String,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Detach the current target app so no project is active.
    /// Registered projects are kept; nothing is deleted from disk.
    Detach {
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Register a local repository as a named project without making it current.
    Register {
        /// Project name to register under.
        name: String,
        /// Filesystem path to the target app repository.
        path: String,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Clone a git repository to a local destination and register it as a project.
    /// Use --make-current to also attach it as the current target app.
    Clone {
        /// Git URL or path to clone from.
        source: String,
        /// Local directory to clone into.
        destination: String,
        /// Project name to register (derived from the source when omitted).
        #[arg(long)]
        name: Option<String>,
        /// Also switch to the cloned project as the current target app.
        #[arg(long)]
        make_current: bool,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a project from the registry by name. Files on disk are not deleted.
    Remove {
        /// Registered project name to remove.
        name: String,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Migrate the current project's on-disk Refine state to the latest schema and report what changed.
    Migrate {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Rebuild the project projection (Gap/Feature caches) from on-disk state.
    /// Run after external edits to .refine data; optionally persists the snapshot to a cache directory.
    Sync {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Cache directory to persist the rebuilt projection snapshot into.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    /// Run project-level diagnostics against the attached target app and report problems.
    Doctor {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Path to the Refine checkout used for repository diagnostics.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum GapAction {
    /// Create a new Gap — a unit of work describing the difference between actual and desired behavior.
    /// It starts in the backlog; add a round to describe the behavior, then `gap start` to begin work.
    Create {
        /// Human-readable Gap name.
        name: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Explicit Gap id (generated when omitted).
        #[arg(long)]
        id: Option<String>,
    },
    /// List all Gaps with their status and ownership.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Show full detail for one Gap: status, rounds, notes, and ownership.
    Show {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit a Gap's metadata (name and/or priority). Only valid while the Gap's status allows editing.
    Edit {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// New Gap name.
        #[arg(long)]
        name: Option<String>,
        /// New priority value.
        #[arg(long)]
        priority: Option<String>,
    },
    /// Append a free-form note to a Gap for context that agents and humans should see.
    Note {
        /// Gap id.
        id: String,
        /// Note text.
        body: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Author label recorded on the note.
        #[arg(long, default_value = "")]
        author: String,
    },
    /// Replace the body of an existing note on a Gap.
    NoteEdit {
        /// Gap id.
        id: String,
        /// Id of the note to edit.
        note_id: String,
        /// Replacement note text.
        body: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Delete a note from a Gap.
    NoteDelete {
        /// Gap id.
        id: String,
        /// Id of the note to delete.
        note_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Record a round on a Gap: a reporter's statement of actual vs target behavior.
    /// Requires --reporter, --actual, and --target unless --edit-latest amends the newest round.
    Round {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Who is reporting this round.
        #[arg(long)]
        reporter: Option<String>,
        /// The actual (current) behavior observed.
        #[arg(long)]
        actual: Option<String>,
        /// The target (desired) behavior.
        #[arg(long)]
        target: Option<String>,
        /// Edit the most recent round instead of appending a new one.
        #[arg(long)]
        edit_latest: bool,
    },
    /// Start work on a Gap: moves it from backlog/todo to in-progress so the agent workflow picks it up.
    Start {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Cancel a Gap: any not-yet-done Gap becomes cancelled. Done Gaps cannot be cancelled (use undo first).
    Cancel {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Retry a failed stage for a Gap: --stage quality returns it to QA, --stage merge to ready-merge.
    Retry {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Stage to retry: "quality" (back to QA) or "merge" (back to ready-merge).
        #[arg(long, default_value = "quality")]
        stage: String,
    },
    /// Approve a Gap that is in review: marks it done after the change has been verified.
    Verify {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Merge a ready-merge Gap and mark it done. Requires the Gap to be in the ready-merge status.
    Merge {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Walk a Gap's status backwards: done goes to review; review or cancelled goes to todo.
    Undo {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Permanently delete a Gap record from project state. Irreversible; prefer cancel to keep history.
    Delete {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Assign a Gap to a Feature so it is grouped and ordered with related work.
    AssignFeature {
        /// Gap id.
        id: String,
        /// Feature id to assign the Gap to.
        feature_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a Gap from its Feature. The Gap itself is kept.
    RemoveFeature {
        /// Gap id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FeatureAction {
    /// Create a Feature — a named group of ordered Gaps delivered together.
    Create {
        /// Human-readable Feature name.
        name: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Explicit Feature id (generated when omitted).
        #[arg(long)]
        id: Option<String>,
        /// Feature description.
        #[arg(long)]
        description: Option<String>,
        /// Reporter recorded on the Feature.
        #[arg(long)]
        reporter: Option<String>,
    },
    /// List all Features with their rollup status.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Show one Feature with its Gaps and rollup status.
    Show {
        /// Feature id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit a Feature's metadata: name, description, or reporter.
    Edit {
        /// Feature id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// New Feature name.
        #[arg(long)]
        name: Option<String>,
        /// New Feature description.
        #[arg(long)]
        description: Option<String>,
        /// New reporter value.
        #[arg(long)]
        reporter: Option<String>,
    },
    /// Add an existing Gap to a Feature.
    AddGap {
        /// Feature id.
        id: String,
        /// Gap id to add to the Feature.
        gap_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a Gap from a Feature. The Gap itself is kept.
    RemoveGap {
        /// Feature id.
        id: String,
        /// Gap id to remove from the Feature.
        gap_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Set a Gap's position within the Feature's ordered delivery sequence.
    ReorderGap {
        /// Feature id.
        id: String,
        /// Gap id to reposition.
        gap_id: String,
        /// New position in the Feature's ordered Gap sequence.
        order: i64,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Add a Gap to the Feature's ordered delivery sequence.
    OrderGap {
        /// Feature id.
        id: String,
        /// Gap id to add to the ordered sequence.
        gap_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a Gap from the Feature's ordered delivery sequence while keeping it in the Feature.
    UnorderGap {
        /// Feature id.
        id: String,
        /// Gap id to remove from the ordered sequence.
        gap_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Move all of a Feature's eligible Gaps to a workflow stage (backlog or todo).
    Move {
        /// Feature id.
        id: String,
        /// Target status for the Feature's Gaps: "backlog" or "todo".
        target: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Transfer ownership of a Feature and its Gaps to another node in the fleet.
    Transfer {
        /// Feature id.
        id: String,
        /// Destination node id.
        node_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Cancel a Feature: its cancellable Gaps are cancelled as well.
    Cancel {
        /// Feature id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Permanently delete a Feature and its Gaps. Irreversible; prefer cancel to keep history.
    Delete {
        /// Feature id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Bulk-import Gap drafts from text, structured JSON, or CSV, optionally attaching them to a Feature.
    Import {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
        /// Inline import source text (alternative to --file).
        #[arg(long)]
        text: Option<String>,
        /// File to read the import source from (alternative to --text).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Parse the input as CSV instead of structured or free text.
        #[arg(long)]
        csv: bool,
        /// Reporter recorded on the imported Gaps.
        #[arg(long)]
        reporter: Option<String>,
        /// Feature id to attach the imported Gaps to.
        #[arg(long)]
        feature_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkflowAction {
    /// Pause the agent automation engine: no new Gap work is claimed until resumed.
    Pause {
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Resume the agent automation engine after a pause so agents claim Gap work again.
    Resume {
        /// Runtime directory where Refine keeps daemon state.
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
    /// List all nodes in the registry and show which one is active on this machine.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Turn this machine into a working fleet node: clone or attach the target repo (from env or flags),
    /// activate the node identity, and select an agent provider. Runs at worker boot; idempotent.
    Init {
        /// Node identity to activate for this machine.
        #[arg(long)]
        node_id: Option<String>,
        /// Git URL of the target app repository to clone.
        #[arg(long)]
        repo_url: Option<String>,
        /// Local path for the target app checkout.
        #[arg(long)]
        target_path: Option<PathBuf>,
        /// Comma-separated agent providers to enable (e.g. "claude").
        #[arg(long)]
        agent_providers: Option<String>,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Daemon port for this node.
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// Show one node's record and whether it is the active node on this machine.
    Show {
        /// Node id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Create a new node record in the registry with default settings. Fails if the id already exists.
    Create {
        /// Node id to create.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Set the given node as this machine's active node identity. The node must exist and not be archived.
    Activate {
        /// Node id to activate.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Archive a node so it can no longer be activated or receive work. The active node cannot be archived.
    Archive {
        /// Node id to archive.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Change a node's display name.
    Rename {
        /// Node id.
        id: String,
        /// New display name.
        name: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Print a node's settings object.
    Settings {
        /// Node id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Transfer ownership of a Gap or Feature (by item id) to the given node.
    Transfer {
        /// Destination node id.
        id: String,
        /// Gap or Feature id to transfer.
        item_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ClusterAction {
    /// List the cluster: every fleet node with its enablement, connection, and health details.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Show one fleet node's full cluster record.
    Show {
        /// Node id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Register a new node in the cluster so it can be configured, provisioned, and receive distributed work.
    AddNode {
        /// Node id to add.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit a cluster node's connection and provisioning settings: SSH details, paths, ports, and provider.
    EditNode {
        /// Node id to edit.
        id: String,
        /// New display name.
        #[arg(long)]
        display_name: Option<String>,
        /// SSH hostname or address for reaching the node.
        #[arg(long)]
        ssh_host: Option<String>,
        /// SSH username.
        #[arg(long)]
        ssh_user: Option<String>,
        /// Path to the SSH identity (private key) file.
        #[arg(long)]
        ssh_identity_path: Option<String>,
        /// SSH port.
        #[arg(long)]
        ssh_port: Option<u16>,
        /// Path to the Refine checkout on the node.
        #[arg(long)]
        refine_checkout: Option<String>,
        /// Path to the target app checkout on the node.
        #[arg(long)]
        target_app_path: Option<String>,
        /// Port the node's Refine daemon listens on.
        #[arg(long)]
        refine_port: Option<u16>,
        /// Provisioning provider for this node (e.g. "fly").
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, help = "JSON object of provider provisioning overrides")]
        provisioning: Option<String>,
        /// Enable or disable the node for work distribution.
        #[arg(long)]
        enabled: Option<bool>,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Enable a node so distribute can assign it work.
    EnableNode {
        /// Node id to enable.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Disable a node so it stops receiving distributed work.
    DisableNode {
        /// Node id to disable.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a node from the cluster registry.
    RemoveNode {
        /// Node id to remove.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// SSH-bootstrap a manually configured node by git-pulling its Refine checkout.
    /// Requires the node's SSH settings to be configured; use --dry-run to preview the commands.
    Bootstrap {
        /// Node id to bootstrap.
        id: String,
        /// Print the commands that would run without executing them.
        #[arg(long)]
        dry_run: bool,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// List configured fleet provisioning providers (built-in fly plus .refine/fleet.json entries).
    Providers {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Create and deploy a cloud worker machine for a registered node via its provider (e.g. Fly.io).
    /// Use --dry-run to preview the provider commands without running them.
    Provision {
        /// Node id to provision.
        id: String,
        /// Provider to provision with (defaults to the node's configured provider).
        #[arg(long)]
        provider: Option<String>,
        /// Print the provider commands that would run without executing them.
        #[arg(long)]
        dry_run: bool,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Destroy the node's cloud app via its provider and disable the node.
    Deprovision {
        /// Node id to deprovision.
        id: String,
        /// Print the provider commands that would run without executing them.
        #[arg(long)]
        dry_run: bool,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Run the provider's status command for a node and refresh its recorded health.
    ProvisionStatus {
        /// Node id to check.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Reassign eligible unclaimed Gap ownership across the fleet.
    /// Spreads across enabled healthy nodes by default, fills one node with --to,
    /// or converges reviewable Gaps home with --converge --to <node>.
    Distribute {
        /// Send all moves to this node instead of spreading across the fleet.
        #[arg(long)]
        to: Option<String>,
        /// Converge reviewable Gaps back to the node given by --to.
        #[arg(long)]
        converge: bool,
        /// Plan the moves without applying them.
        #[arg(long)]
        dry_run: bool,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Reload the cluster registry (merging any legacy records) and report the enabled node count.
    Sync {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Run an authorized command on a node over SSH or the provider exec channel and print the result.
    Run {
        /// Node id to run the command on.
        id: String,
        /// Command line to execute on the node.
        command: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Transfer ownership of a Gap or Feature (by item id) to the given node, updating cluster records.
    Transfer {
        /// Destination node id.
        id: String,
        /// Gap or Feature id to transfer.
        item_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Put the cluster into maintenance mode and report the updated cluster state.
    Maintenance {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum LogAction {
    /// List recent activity log entries.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
        /// Maximum number of entries to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Show the most recent activity log entries (a short tail of the log).
    Tail {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
        /// Maximum number of entries to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show one activity log entry by id.
    Show {
        /// Activity log entry id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
    },
    /// Search the activity log with a text query and optional filters, with pagination.
    Query {
        /// Text to search for.
        q: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
        /// Maximum number of entries to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Number of matching entries to skip (for pagination).
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Only return entries for this Gap id.
        #[arg(long)]
        gap_id: Option<String>,
        /// Only return entries with this severity.
        #[arg(long)]
        severity: Option<String>,
        /// Only return entries in this category.
        #[arg(long)]
        category: Option<String>,
        /// Only return entries recorded by this actor.
        #[arg(long)]
        actor: Option<String>,
    },
    /// Export activity log entries as JSON with an exported count.
    Export {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Build a support bundle of diagnostics and logs for troubleshooting, redacting secrets by default.
    Bundle {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = PathBuf::new()))]
        target_root: PathBuf,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Path to the Refine checkout to include repository diagnostics from.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        /// Redact secrets from bundle contents.
        #[arg(long, default_value_t = true)]
        redact_secrets: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentAction {
    /// Detect which agent provider CLIs are installed and available on this host.
    Detect,
    /// Configure an agent provider so workflows can invoke it.
    Configure {
        /// Agent provider name (e.g. "claude").
        #[arg(long, default_value = "claude")]
        provider: String,
    },
    /// Check or initiate authentication for an agent provider.
    Auth {
        /// Agent provider name (e.g. "claude").
        #[arg(long, default_value = "claude")]
        provider: String,
    },
    /// Run diagnostics for an agent provider and report configuration or auth problems.
    Diagnose {
        /// Agent provider name (e.g. "claude").
        #[arg(long, default_value = "claude")]
        provider: String,
    },
    /// Invoke an agent once with a prompt and print the result. Useful for testing provider setup.
    Invoke {
        /// Prompt text to send to the agent.
        prompt: String,
        /// Agent provider name (e.g. "claude").
        #[arg(long, default_value = "claude")]
        provider: String,
        /// Working directory for the agent run.
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Resume a previous agent session by session id, keeping its context.
    Resume {
        /// Agent session id to resume.
        session_id: String,
        /// Agent provider name (e.g. "claude").
        #[arg(long, default_value = "claude")]
        provider: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SystemAction {
    /// Install Refine on this machine (macOS app bundle, Windows installer, or Linux CLI/web).
    Install {
        /// Daemon port to configure for the installation.
        #[arg(long)]
        port: u16,
        /// Install target; auto-detects the operating system by default.
        #[arg(long, value_enum, default_value_t = CliInstallTarget::Auto)]
        target: CliInstallTarget,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Version string to record for the installation.
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    /// Repair an existing installation: recreate launchers and services for the recorded version.
    Repair {
        /// Daemon port the installation is configured for.
        #[arg(long)]
        port: u16,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Version string to record for the installation.
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    /// Self-update Refine to the latest available version.
    Update {
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Roll the installation back to a previously installed version.
    Rollback {
        /// Daemon port the installation is configured for.
        #[arg(long)]
        port: u16,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Version string to roll back around.
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    /// Uninstall Refine from this machine.
    Uninstall {
        /// Daemon port the installation is configured for.
        #[arg(long)]
        port: u16,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Version string of the installation to remove.
        #[arg(long, default_value = env!("CARGO_PKG_VERSION"))]
        version: String,
    },
    /// Start the Refine daemon (background by default; --foreground or --once run it in-process).
    Start {
        /// Port for the daemon to listen on.
        #[arg(long, default_value_t = 8082)]
        port: u16,
        /// IP address to bind the listener to.
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
        bind_address: IpAddr,
        /// Directory for the projection cache.
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Directory of static web assets to serve.
        #[arg(long)]
        static_root: Option<PathBuf>,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Serve a single request then exit (useful for smoke tests).
        #[arg(long)]
        once: bool,
        /// Run in the foreground instead of spawning a background daemon.
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the Refine daemon running on the given port.
    Stop {
        /// Port the daemon is listening on.
        #[arg(long, default_value_t = 8082)]
        port: u16,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Restart the Refine daemon on the given port.
    Restart {
        /// Port the daemon is listening on.
        #[arg(long, default_value_t = 8082)]
        port: u16,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Report daemon status for the given port: health, worker state, and target app state.
    Status {
        /// Port the daemon is listening on.
        #[arg(long, default_value_t = 8082)]
        port: u16,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// List running Refine daemon processes; optionally stop one with --stop.
    Ps {
        /// Only inspect the daemon on this port.
        #[arg(long)]
        port: Option<u16>,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Identifier of the process to stop.
        #[arg(long)]
        stop: Option<String>,
        /// Signal to send when stopping ("terminate" or "kill").
        #[arg(long, default_value = "terminate")]
        signal: String,
    },
    /// Run system-level diagnostics covering the daemon, runtime, and repository, and report problems.
    Doctor {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        /// Path to the Refine checkout used for repository diagnostics.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
    /// Print the daemon HTTP API groups and the capability each one requires.
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
