use super::*;

#[derive(Debug, Subcommand)]
pub enum DefinitionAction {
    /// List definitions and their configuration revision.
    List {
        #[arg(long)]
        node_id: Option<String>,
    },
    /// Read a definition and its configuration revision.
    Show { id: String },
    /// Create or replace a definition from a JSON object, using an observed revision.
    Save {
        id: String,
        #[arg(long)]
        revision: u64,
        #[command(flatten)]
        payload: ConfigPayload,
    },
    /// Enable a definition, preserving concurrent edits through revision fencing.
    Enable { id: String },
    /// Disable a definition.
    Disable { id: String },
    /// Remove a definition using an observed revision.
    Remove {
        id: String,
        #[arg(long)]
        revision: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum EventAction {
    /// Discover all standard event sources and Skill result roles.
    Catalog,
    /// List applicable Event definitions.
    List {
        #[arg(long)]
        node_id: Option<String>,
    },
    Show {
        id: String,
    },
    Save {
        id: String,
        #[arg(long)]
        revision: u64,
        #[command(flatten)]
        payload: ConfigPayload,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Remove {
        id: String,
        #[arg(long)]
        revision: u64,
    },
    /// Add or replace an ordered Skill binding from JSON.
    Bind {
        id: String,
        #[arg(long)]
        revision: u64,
        #[command(flatten)]
        payload: ConfigPayload,
    },
    /// Remove a Skill binding.
    Unbind {
        id: String,
        binding_id: String,
        #[arg(long)]
        revision: u64,
    },
    /// Trigger a custom Event, prompting for missing required values in an interactive terminal.
    Trigger {
        id: String,
        #[arg(long)]
        goal_id: Option<String>,
        #[arg(long)]
        node_id: Option<String>,
        #[arg(long = "param", value_name = "NAME=VALUE")]
        parameters: Vec<String>,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// List invocation evidence.
    Runs {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Read one invocation and its collected Skill results.
    Status {
        id: String,
    },
    Cancel {
        id: String,
    },
}
