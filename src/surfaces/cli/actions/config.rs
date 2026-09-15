use super::*;

#[derive(Clone, Debug, ValueEnum)]
pub enum ConfigDomain {
    Settings,
    Skills,
    Providers,
}

#[derive(Debug, clap::Args)]
pub struct ConfigPayload {
    /// Inline JSON object payload.
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    /// Read a JSON object payload from a file.
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,
    /// Read a JSON object payload from standard input.
    #[arg(long)]
    pub stdin: bool,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Read all configuration domains, or one named domain.
    Show {
        #[arg(value_enum)]
        domain: Option<ConfigDomain>,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Inspect or patch ordinary project and runtime settings.
    Settings {
        #[command(subcommand)]
        action: ConfigSettingsAction,
    },
    /// Read or replace the shared AI provider catalog; revisions fence edits.
    Providers {
        #[command(subcommand)]
        action: ConfigProvidersAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigSettingsAction {
    /// Read ordinary settings.
    Show {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Apply a validated partial settings patch.
    Set {
        /// Set one key to a JSON scalar or string; repeat for multiple keys.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        values: Vec<String>,
        #[command(flatten)]
        payload: ConfigPayload,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigProvidersAction {
    Show,
    /// Save a complete catalog containing the revision returned by show.
    Save {
        #[command(flatten)]
        payload: ConfigPayload,
    },
    /// Set the node provider override; omit ID to inherit the system default.
    Select {
        provider: Option<String>,
    },
}
