use super::*;

#[derive(Debug, Subcommand)]
pub enum MissionAction {
    /// Create a new Mission in Draft. It consumes no agent or fleet capacity until started.
    Create {
        /// Human-readable Mission name.
        name: String,
        /// The desired outcome, as inline text (alternative to --file).
        #[arg(long)]
        intent: Option<String>,
        /// File containing the desired outcome (alternative to --intent).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Reporter who owns the Mission.
        #[arg(long)]
        reporter: Option<String>,
        /// Explicit Mission id (generated when omitted).
        #[arg(long)]
        id: Option<String>,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// List all Missions with their status and Round.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Show full detail for one Mission.
    Show {
        /// Mission id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit a Draft Mission's frame (name, intent).
    Edit {
        /// Mission id.
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        intent: Option<String>,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Append a new Mission Round, freezing the current charter.
    Round {
        /// Mission id.
        id: String,
        /// Who is authoring this Round.
        #[arg(long)]
        reporter: Option<String>,
        /// The authorizing request for this Round.
        #[arg(long)]
        prompt: Option<String>,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Start a Draft Mission: appends the first Round and moves to Investigate.
    Start {
        /// Mission id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Approve a Mission plan by its effective digest.
    ApprovePlan {
        /// Mission id.
        id: String,
        /// The effective plan digest to authorize.
        #[arg(long)]
        plan_digest: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Approve a reviewed Mission Outcome, authorizing consolidation.
    ApproveOutcome {
        /// Mission id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Cancel a Mission. Default cancellation does not cancel child Goals.
    Cancel {
        /// Mission id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Advance one Mission by one engine step (investigation, wave
    /// admission, reconciliation, synthesis, quality, governance, or
    /// consolidation).
    Advance {
        /// Mission id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Settle a Mission contribution for a Goal at Review with valid
    /// evidence.
    Contribute {
        /// Goal id.
        goal: String,
        /// File containing the contribution JSON.
        #[arg(long)]
        file: PathBuf,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Read a Mission's published Outcome.
    Outcome {
        /// Mission id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}
