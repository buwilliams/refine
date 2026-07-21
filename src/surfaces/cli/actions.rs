use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::model::workflow::GoalStatus;
use crate::tools::host::installation::InstallTarget;

/// Refine: agent fleet software delivery — track Goals, run agent workflows, and operate a fleet of nodes.
#[derive(Debug, Parser)]
#[command(name = "refine", version)]
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
    /// Create and drive Goals — prompt-driven units of work for the active app.
    /// Covers the full lifecycle: create, round, start, retry, approve, and undo.
    Goal {
        #[command(subcommand)]
        action: GoalAction,
    },
    /// Manage Features — named groups of ordered Goals delivered together.
    /// Group, order, move, transfer, and bulk-import Goals under a Feature.
    Feature {
        #[command(subcommand)]
        action: FeatureAction,
    },
    /// Control the agent automation engine that advances Goals through their workflow (pause/resume).
    Workflow {
        #[command(subcommand)]
        action: WorkflowAction,
    },
    /// Manage nodes — the machines that own active work — including turning this machine into a fleet node.
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// Operate the cluster (the fleet of nodes): register and bootstrap nodes,
    /// distribute unclaimed Goal ownership, and run remote commands.
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
    /// Older semantic schemas remain detached until a migration agent handles them.
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
    /// Report schema migration requirements. Semantic migrations are agent-operated.
    Migrate {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Runtime directory where Refine keeps daemon and registry state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Publish and pull Refine control state now.
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
pub enum GoalAction {
    /// Create a new prompt-driven Goal.
    /// It starts in the backlog; add a round to describe the behavior, then `goal start` to begin work.
    Create {
        /// Human-readable Goal name.
        name: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Explicit Goal id (generated when omitted).
        #[arg(long)]
        id: Option<String>,
    },
    /// List all Goals with their status and ownership.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Show full detail for one Goal: status, rounds, notes, and ownership.
    Show {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit a Goal's metadata (name and/or priority). Only valid while the Goal's status allows editing.
    Edit {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// New Goal name.
        #[arg(long)]
        name: Option<String>,
        /// New priority value.
        #[arg(long)]
        priority: Option<String>,
    },
    /// Append a free-form note to a Goal for context that agents and humans should see.
    Note {
        /// Goal id.
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
    /// Replace the body of an existing note on a Goal.
    NoteEdit {
        /// Goal id.
        id: String,
        /// Id of the note to edit.
        note_id: String,
        /// Replacement note text.
        body: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Delete a note from a Goal.
    NoteDelete {
        /// Goal id.
        id: String,
        /// Id of the note to delete.
        note_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Record an actionable prompt as a round on a Goal.
    /// Requires --reporter and --prompt unless --edit-latest amends the newest round.
    Round {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Who is reporting this round.
        #[arg(long)]
        reporter: Option<String>,
        /// The work prompt for the agent.
        #[arg(long)]
        prompt: Option<String>,
        /// Edit the most recent round instead of appending a new one.
        #[arg(long)]
        edit_latest: bool,
    },
    /// Queue a Goal for the agent workflow: moves backlog work to todo so automation can claim it.
    Start {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Cancel a Goal: any not-yet-done Goal becomes cancelled. Done Goals cannot be cancelled (use undo first).
    Cancel {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Retry a failed stage for a Goal: --stage quality returns it to QA, --stage merge to ready-merge.
    Retry {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Stage to retry: "quality" (back to QA) or "merge" (back to ready-merge).
        #[arg(long, default_value = "quality")]
        stage: String,
    },
    /// Approve a reviewed Goal and mark it done.
    Approve {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Internal verification alias retained for QA and compatibility.
    #[command(hide = true)]
    Verify {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Deprecated alias for approving a reviewed Goal.
    #[command(hide = true)]
    Merge {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Walk a Goal's status backwards: done goes to review; review or cancelled goes to todo.
    Undo {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Permanently delete a Goal record from project state. Irreversible; prefer cancel to keep history.
    Delete {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Assign a Goal to a Feature so it is grouped and ordered with related work.
    AssignFeature {
        /// Goal id.
        id: String,
        /// Feature id to assign the Goal to.
        feature_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a Goal from its Feature. The Goal itself is kept.
    RemoveFeature {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum FeatureAction {
    /// Create a Feature — a named group of ordered Goals delivered together.
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
    /// Show one Feature with its Goals and rollup status.
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
    /// Add an existing Goal to a Feature.
    AddGoal {
        /// Feature id.
        id: String,
        /// Goal id to add to the Feature.
        goal_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a Goal from a Feature. The Goal itself is kept.
    RemoveGoal {
        /// Feature id.
        id: String,
        /// Goal id to remove from the Feature.
        goal_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Set a Goal's position within the Feature's ordered delivery sequence.
    ReorderGoal {
        /// Feature id.
        id: String,
        /// Goal id to reposition.
        goal_id: String,
        /// New position in the Feature's ordered Goal sequence.
        order: i64,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Add a Goal to the Feature's ordered delivery sequence.
    OrderGoal {
        /// Feature id.
        id: String,
        /// Goal id to add to the ordered sequence.
        goal_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a Goal from the Feature's ordered delivery sequence while keeping it in the Feature.
    UnorderGoal {
        /// Feature id.
        id: String,
        /// Goal id to remove from the ordered sequence.
        goal_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Move all of a Feature's eligible Goals to a workflow stage (backlog or todo).
    Move {
        /// Feature id.
        id: String,
        /// Target status for the Feature's Goals: "backlog" or "todo".
        target: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Transfer ownership of a Feature and its Goals to another node in the fleet.
    Transfer {
        /// Feature id.
        id: String,
        /// Destination node id.
        node_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Cancel a Feature: its cancellable Goals are cancelled as well.
    Cancel {
        /// Feature id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Permanently delete a Feature and its Goals. Irreversible; prefer cancel to keep history.
    Delete {
        /// Feature id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Bulk-import Goal drafts from text, structured JSON, or CSV, optionally attaching them to a Feature.
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
        /// Reporter recorded on the imported Goals.
        #[arg(long)]
        reporter: Option<String>,
        /// Feature id to attach the imported Goals to.
        #[arg(long)]
        feature_id: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorkflowAction {
    /// Pause the agent automation engine: no new Goal work is claimed until resumed.
    Pause {
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Resume the agent automation engine after a pause so agents claim Goal work again.
    Resume {
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CliGoalStatus {
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

impl From<CliGoalStatus> for GoalStatus {
    fn from(value: CliGoalStatus) -> Self {
        match value {
            CliGoalStatus::Backlog => Self::Backlog,
            CliGoalStatus::Todo => Self::Todo,
            CliGoalStatus::InProgress => Self::InProgress,
            CliGoalStatus::Qa => Self::Qa,
            CliGoalStatus::ReadyMerge => Self::ReadyMerge,
            CliGoalStatus::Build => Self::Build,
            CliGoalStatus::Review => Self::Review,
            CliGoalStatus::Done => Self::Done,
            CliGoalStatus::Failed => Self::Failed,
            CliGoalStatus::Cancelled => Self::Cancelled,
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
    /// Transfer ownership of a Goal or Feature (by item id) to the given node.
    Transfer {
        /// Destination node id.
        id: String,
        /// Goal or Feature id to transfer.
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
    /// Register a new node in the cluster so it can be configured and receive distributed work.
    AddNode {
        /// Node id to add.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit a cluster node's connection settings: SSH details, paths, and ports.
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
    /// Reassign eligible unclaimed Goal ownership across the fleet.
    /// Spreads across enabled healthy nodes by default, fills one node with --to,
    /// or converges reviewable Goals home with --converge --to <node>.
    Distribute {
        /// Send all moves to this node instead of spreading across the fleet.
        #[arg(long)]
        to: Option<String>,
        /// Converge reviewable Goals back to the node given by --to.
        #[arg(long)]
        converge: bool,
        /// Plan the moves without applying them.
        #[arg(long)]
        dry_run: bool,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Publish and pull this node's Refine control state now.
    Sync {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Run an authorized command on a node over SSH and print the result.
    Run {
        /// Node id to run the command on.
        id: String,
        /// Command line to execute on the node.
        command: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Transfer ownership of a Goal or Feature (by item id) to the given node, updating cluster records.
    Transfer {
        /// Destination node id.
        id: String,
        /// Goal or Feature id to transfer.
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
        /// Only return entries for this Goal id.
        #[arg(long)]
        goal_id: Option<String>,
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
    /// Preview a semantic release without changing files.
    ReleasePlan {
        /// Semantic version increment: major, minor, or patch.
        #[arg(long)]
        bump: String,
        /// Git checkout to release.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        /// Runtime directory where durable release operations are stored.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Queue an agent-operated Goal to prepare a reviewable semantic release.
    ReleasePrepare {
        /// Semantic version increment: major, minor, or patch.
        #[arg(long)]
        bump: String,
        /// Git checkout to release.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        /// Runtime directory where durable release operations are stored.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Publish an approved preparation by persisted id. Requires explicit --confirm.
    ReleasePublish {
        /// Persisted release preparation operation id returned by release-prepare.
        #[arg(long)]
        preparation_id: String,
        /// Confirm creation and push of the tag and external GitHub publication.
        #[arg(long)]
        confirm: bool,
        /// Git checkout whose synchronized main will be published.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
        /// Runtime directory where durable release operations are stored.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Inspect the running source checkout and its configured upstream branch.
    SourceStatus {
        /// Refine source checkout; auto-discovered when omitted.
        #[arg(long)]
        checkout: Option<PathBuf>,
        /// Fetch the configured upstream before reporting status.
        #[arg(long)]
        fetch: bool,
        /// Port of the running Refine daemon.
        #[arg(long, default_value_t = 8082)]
        port: u16,
        /// Directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Build, fast-forward, and restart a running Refine source checkout.
    SourcePromote {
        /// Refine source checkout; auto-discovered when omitted.
        #[arg(long)]
        checkout: Option<PathBuf>,
        /// Port of the running Refine daemon.
        #[arg(long, default_value_t = 8082)]
        port: u16,
        /// Directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Continue source promotion outside the daemon process.
    #[command(hide = true)]
    SourcePromoteHelper {
        /// Refine controller checkout selected by the initiating request.
        #[arg(long)]
        checkout: PathBuf,
        /// Port-scoped runtime directory containing durable operation state.
        #[arg(long)]
        port_runtime_root: PathBuf,
        /// Refine daemon port to stop, restart, and verify.
        #[arg(long)]
        port: u16,
        /// Durable source-promotion operation identifier.
        #[arg(long)]
        operation_id: String,
    },
    /// Run a supervised background worker outside the daemon process.
    #[command(hide = true)]
    RunnerWorker {
        /// Worker implementation to run.
        #[arg(long)]
        kind: String,
        /// Port-scoped runtime directory shared with the daemon.
        #[arg(long)]
        port_runtime_root: PathBuf,
        /// Target repository for one-shot project operations.
        #[arg(long)]
        target_root: Option<PathBuf>,
        /// Durable operation identifier for one-shot work.
        #[arg(long)]
        operation_id: Option<String>,
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
