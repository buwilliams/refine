use super::*;

#[derive(Debug, Subcommand)]
pub enum FleetAction {
    /// Anything that is not a fleet subcommand is treated as a plain-language
    /// request: `refine fleet "<request>"` opens the same runbook-guided
    /// agent session as `fleet manage`.
    #[command(external_subcommand)]
    Request(Vec<String>),
    /// Open an agent session that manages the fleet with runbook guidance:
    /// describe what you want, answer its questions, and it acts.
    Manage {
        /// What the fleet should do or look like, in plain language.
        request: String,
        /// Agent provider that runs the session. Defaults to the target
        /// app's configured provider, then the first installed provider CLI
        /// in alphabetical order.
        #[arg(long)]
        provider: Option<String>,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// List the fleet: every fleet node with its enablement, connection, and health details.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Show one fleet node's full fleet record.
    Show {
        /// Node id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Register a new node in the fleet so it can be configured and receive distributed work.
    AddNode {
        /// Node id to add.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit a fleet node's connection settings: SSH details, paths, and ports.
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
    /// Remove a node from the fleet registry.
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
    /// converges reviewable Goals home with --converge --to <node>, or delegates
    /// to an agent when given plain-language instructions.
    Distribute {
        /// Plain-language distribution instructions. When given, an agent
        /// session guided by the manage-fleet runbook plans and applies the
        /// Goal assignments instead of the built-in spread.
        #[arg(value_name = "INSTRUCTIONS", conflicts_with_all = ["to", "converge", "dry_run"])]
        instructions: Option<String>,
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
    /// Transfer ownership of a Goal or Feature (by item id) to the given node, updating fleet records.
    Transfer {
        /// Destination node id.
        id: String,
        /// Goal or Feature id to transfer.
        item_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Put the fleet into maintenance mode and report the updated fleet state.
    Maintenance {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}
