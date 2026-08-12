use super::*;

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
        /// Agent provider that performs the update. Defaults to the target
        /// app's configured provider, then the first installed provider CLI
        /// in alphabetical order.
        #[arg(long)]
        provider: Option<String>,
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
    /// Complete a durable daemon stop or restart outside the managed daemon service.
    #[command(hide = true)]
    DaemonLifecycleHelper {
        /// Lifecycle action queued by the daemon.
        #[arg(long)]
        action: String,
        /// Refine daemon port controlled by the operation.
        #[arg(long)]
        port: u16,
        /// Canonical runtime directory containing port-scoped lifecycle state.
        #[arg(long)]
        runtime_root: PathBuf,
        /// Durable daemon-lifecycle operation identifier.
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
        /// Canonical runtime directory containing the shared project registry.
        #[arg(long)]
        project_registry_root: Option<PathBuf>,
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
    /// Start the Refine daemon (through its port-scoped systemd/launchd installation when present;
    /// command failures stay visible even when the daemon is already healthy).
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
    /// Stop the Refine daemon and its port-scoped systemd/launchd service; stopped is reported only
    /// after shutdown is confirmed.
    Stop {
        /// Port the daemon is listening on.
        #[arg(long, default_value_t = 8082)]
        port: u16,
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Restart the Refine daemon on the given port through its activated systemd/launchd service
    /// when present.
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
    pub(in crate::surfaces::cli) fn into_target(self) -> InstallTarget {
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
