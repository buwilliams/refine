use super::*;

#[derive(Debug, Subcommand)]
pub enum WorkflowAction {
    /// Pause the agent automation engine: no new Goal work starts until resumed. Active local work continues to completion and can be stopped separately.
    Pause {
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Resume the agent automation engine after a pause so agents start eligible Goal work again.
    Resume {
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub enum CliGoalStatus {
    Backlog,
    Todo,
    InProgress,
    Qa,
    ReadyMerge,
    Build,
    Review,
    Done,
    Failed,
    Cancelled,
}

impl From<CliGoalStatus> for GoalStatus {
    fn from(value: CliGoalStatus) -> Self {
        match value {
            CliGoalStatus::Backlog => Self::Backlog,
            CliGoalStatus::Todo => Self::Todo,
            CliGoalStatus::InProgress => Self::InProgress,
            CliGoalStatus::Qa => Self::Qa,
            CliGoalStatus::ReadyMerge => Self::ReadyMerge,
            CliGoalStatus::Build => Self::Build,
            CliGoalStatus::Review => Self::Review,
            CliGoalStatus::Done => Self::Done,
            CliGoalStatus::Failed => Self::Failed,
            CliGoalStatus::Cancelled => Self::Cancelled,
        }
    }
}
