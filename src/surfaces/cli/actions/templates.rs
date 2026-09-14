use super::*;

#[derive(Debug, Subcommand)]
pub enum TemplateAction {
    /// List system Templates, including defaults without saved overrides.
    List,
    /// Read a Template, its default, and available variables.
    Show { id: String },
    /// Discover supported template variables.
    Variables,
    /// Edit a fixed Template using an observed revision and a JSON {"prompt":"..."} payload.
    Save {
        id: String,
        #[arg(long)]
        revision: u64,
        #[command(flatten)]
        payload: ConfigPayload,
    },
    /// Render a draft without launching an agent. Accepts {"prompt":"...","values":{...}}.
    Preview {
        id: String,
        #[command(flatten)]
        payload: ConfigPayload,
    },
}
