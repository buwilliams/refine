use super::*;

#[derive(Clone, Debug, ValueEnum)]
pub enum ConfigDomain {
    Settings,
    Quality,
    Governance,
    Guidance,
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
    /// Inspect or patch Quality requirements, instructions, and plain-text tests.
    Quality {
        #[command(subcommand)]
        action: ConfigQualityAction,
    },
    /// Inspect or patch Governance context and rules, or generate rules.
    Governance {
        #[command(subcommand)]
        action: ConfigGovernanceAction,
    },
    /// List and mutate Guidance entries by stable id.
    Guidance {
        #[command(subcommand)]
        action: ConfigGuidanceAction,
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
pub enum ConfigQualityAction {
    /// Read Quality configuration.
    Show {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Apply a partial Quality configuration patch.
    Set {
        /// Business requirements, including multiline text.
        #[arg(long)]
        business_requirements: Option<String>,
        /// Quality-agent instructions, including multiline text.
        #[arg(long)]
        instructions: Option<String>,
        /// Replace plain-text Quality tests; repeat once per test.
        #[arg(long = "test")]
        tests: Vec<String>,
        #[command(flatten)]
        payload: ConfigPayload,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigGovernanceAction {
    /// Read Governance configuration.
    Show {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Apply a partial Governance patch; rule replacement is revision-fenced.
    Set {
        /// Product intent, including multiline text.
        #[arg(long)]
        product: Option<String>,
        /// Project constitution, including multiline text.
        #[arg(long)]
        constitution: Option<String>,
        /// Maximum automatic Governance recovery Rounds.
        #[arg(long)]
        max_automatic_round_retries: Option<u32>,
        /// Replace Governance rules; repeat once per rule.
        #[arg(long = "rule")]
        rules: Vec<String>,
        #[command(flatten)]
        payload: ConfigPayload,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Generate Governance rules from saved or supplied Product and Constitution.
    GenerateRules {
        #[arg(long)]
        product: Option<String>,
        #[arg(long)]
        constitution: Option<String>,
        #[command(flatten)]
        payload: ConfigPayload,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigGuidanceAction {
    /// List Guidance entries and the observed collection revision.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Add a Guidance entry.
    Add {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rule: Option<String>,
        #[arg(long)]
        instructions: Option<String>,
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: Option<bool>,
        #[command(flatten)]
        payload: ConfigPayload,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit fields on one Guidance entry.
    Edit {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rule: Option<String>,
        #[arg(long)]
        instructions: Option<String>,
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: Option<bool>,
        #[command(flatten)]
        payload: ConfigPayload,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Enable one Guidance entry.
    Enable {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Disable one Guidance entry.
    Disable {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove one Guidance entry.
    Remove {
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}
