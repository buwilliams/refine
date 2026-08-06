use super::*;

#[derive(Debug, Subcommand)]
pub enum WorkflowAction {
    /// Quiesce autonomous workers while keeping the daemon/API available. Active Goal agents drain and can be stopped separately.
    Pause {
        /// Runtime directory where Refine keeps daemon state.
        #[arg(long, default_value = "run")]
        runtime_root: PathBuf,
    },
    /// Resume autonomous workers and allow agents to claim Goal work again.
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
