use super::*;

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
