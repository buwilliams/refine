use super::*;

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
    /// Inspect or remove clean terminal Goal worktrees for the attached target app.
    ///
    /// Dry-run is the default. Use --apply to remove eligible worktrees while preserving branches.
    CleanupWorktrees {
        /// Remove eligible worktrees instead of only reporting them.
        #[arg(long)]
        apply: bool,
        /// Preserve terminal worktrees newer than this many seconds.
        #[arg(long, default_value_t = 0)]
        older_than_seconds: u64,
        /// Runtime directory where Refine keeps daemon and process state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
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
