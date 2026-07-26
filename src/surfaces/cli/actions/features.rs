use super::*;

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
