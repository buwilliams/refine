use super::*;

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
