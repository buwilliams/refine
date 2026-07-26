use super::*;

#[derive(Debug, Subcommand)]
pub enum AgentAction {
    /// Open a native agent TUI or attach to a Goal instance.
    Open {
        /// Goal id whose running Goal Agent should be opened.
        goal_id: Option<String>,
        /// Agent role to open.
        #[arg(long, value_enum, default_value_t = CliAgentProfile::Agent)]
        profile: CliAgentProfile,
        /// Optional starting context for Plan Mode.
        #[arg(long)]
        prompt: Option<String>,
    },
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CliAgentProfile {
    Agent,
    Plan,
    Standalone,
    Goal,
}

impl CliAgentProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
            Self::Standalone => "standalone",
            Self::Goal => "goal",
        }
    }
}
