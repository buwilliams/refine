use super::*;
#[derive(Debug, Subcommand)]
pub enum PlanningCliAction {
    /// Read all shared boards, cards and pending actions.
    List,
    /// Inspect an action submitted from any surface.
    Action { id: String },
    /// Cancel a pending action, retaining completed transitions and evidence.
    Cancel { id: String },
    /// Submit a board, lane or card operation. Use a stable request id for retries.
    Apply {
        /// board.create/update/archive, lane.create/update/reorder/delete,
        /// card.create/attach/update/move/archive/detach/apply, or migrate.
        operation: String,
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        board_id: Option<String>,
        #[arg(long)]
        lane_id: Option<String>,
        #[arg(long)]
        goal_id: Option<String>,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long, default_value = "operator")]
        actor: String,
        /// JSON editable fields, e.g. '{"name":"Ideas"}'.
        #[arg(long, default_value = "{}")]
        data: String,
    },
}
