use super::*;

#[derive(Debug, Subcommand)]
pub enum SkillAction {
    /// List Skills and their configuration revision.
    List {
        #[arg(long)]
        node_id: Option<String>,
    },
    /// Read a Skill and its configuration revision.
    Show {
        id: String,
    },
    /// Create or replace a Skill from a JSON object, using an observed revision.
    Save {
        id: String,
        #[arg(long)]
        revision: u64,
        #[command(flatten)]
        payload: ConfigPayload,
    },
    /// Enable a Skill, preserving concurrent edits through revision fencing.
    Enable {
        id: String,
    },
    /// Disable a Skill.
    Disable {
        id: String,
    },
    /// Remove a Skill using an observed revision.
    Remove {
        id: String,
        #[arg(long)]
        revision: u64,
    },
    /// Copy a Skill and choose a different trigger without editing the original.
    Clone {
        id: String,
        new_id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        trigger: Option<String>,
    },
    /// List the available automatic and Custom trigger types.
    Triggers,
    /// Run a Skill with a Custom trigger, prompting for missing required values in an interactive terminal.
    Trigger {
        id: String,
        #[arg(long)]
        node_id: Option<String>,
        #[arg(long = "param", value_name = "NAME=VALUE")]
        parameters: Vec<String>,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// List Skill execution history.
    Runs {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Read one run and its collected results.
    Status {
        id: String,
    },
    Cancel {
        id: String,
    },
}
