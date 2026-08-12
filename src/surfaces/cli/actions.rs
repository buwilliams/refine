use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::model::workflow::GoalStatus;
use crate::tools::host::installation::InstallTarget;

mod agents;
mod features;
mod fleet;
mod goals;
mod logs;
mod nodes;
mod projects;
mod system;
mod todos;
mod workflow;

pub use agents::{AgentAction, CliAgentProfile};
pub use features::FeatureAction;
pub use fleet::FleetAction;
pub use goals::GoalAction;
pub use logs::LogAction;
pub use nodes::NodeAction;
pub use projects::ProjectAction;
pub use system::{CliInstallTarget, SystemAction};
pub use todos::TodoAction;
pub use workflow::{CliGoalStatus, WorkflowAction};

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
    /// Manage Reporter-owned Todo lists and items.
    /// Uses the same durable capability as the web UI and daemon API.
    Todo {
        #[command(subcommand)]
        action: TodoAction,
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
    /// Operate the fleet (the fleet of nodes): register and bootstrap nodes,
    /// distribute queued Goal ownership, and run remote commands.
    Fleet {
        #[command(subcommand)]
        action: FleetAction,
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
