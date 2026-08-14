use super::*;

#[derive(Debug, Subcommand)]
pub enum GoalAction {
    /// Create a new prompt-driven Goal.
    /// It starts in the backlog; add a round to describe the behavior, then `goal start` to begin work.
    Create {
        /// Human-readable Goal name.
        name: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Explicit Goal id (generated when omitted).
        #[arg(long)]
        id: Option<String>,
    },
    /// Draft exactly one reviewable Goal from a Plan transcript without persisting it.
    Draft {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Inline Plan transcript (alternative to --file).
        #[arg(long)]
        text: Option<String>,
        /// File containing the Plan transcript (alternative to --text).
        #[arg(long)]
        file: Option<PathBuf>,
        /// Reporter to include in the drafted Goal.
        #[arg(long)]
        reporter: Option<String>,
        /// Configured AI provider to use for extraction.
        #[arg(long)]
        provider: Option<String>,
    },
    /// List all Goals with their status and ownership.
    List {
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Show full detail for one Goal: status, rounds, notes, and ownership.
    Show {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Export a Jira-importable CSV containing the Goal's SOC 2 delivery evidence.
    Export {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Write the CSV to a file instead of standard output.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Edit a Goal's metadata (name and/or priority). Only valid while the Goal's status allows editing.
    Edit {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// New Goal name.
        #[arg(long)]
        name: Option<String>,
        /// New priority value.
        #[arg(long)]
        priority: Option<String>,
    },
    /// Append a free-form note to a Goal for context that agents and humans should see.
    Note {
        /// Goal id.
        id: String,
        /// Note text.
        body: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Author label recorded on the note.
        #[arg(long, default_value = "")]
        author: String,
    },
    /// Replace the body of an existing note on a Goal.
    NoteEdit {
        /// Goal id.
        id: String,
        /// Id of the note to edit.
        note_id: String,
        /// Replacement note text.
        body: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Delete a note from a Goal.
    NoteDelete {
        /// Goal id.
        id: String,
        /// Id of the note to delete.
        note_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Record an actionable prompt as a round on a Goal.
    /// Requires --reporter and --prompt unless --edit-latest amends the newest round.
    Round {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Who is reporting this round.
        #[arg(long)]
        reporter: Option<String>,
        /// The work prompt for the agent.
        #[arg(long)]
        prompt: Option<String>,
        /// Edit the most recent round instead of appending a new one.
        #[arg(long)]
        edit_latest: bool,
    },
    /// Queue a Goal for the agent workflow: moves backlog work to todo so automation can start it.
    Start {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Cancel a Goal: any not-yet-done Goal becomes cancelled. Done Goals cannot be cancelled (use undo first).
    Cancel {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Retry a failed stage for a Goal: --stage quality or --stage governance.
    Retry {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
        /// Stage to retry: "quality" or "governance".
        #[arg(long, default_value = "quality")]
        stage: String,
    },
    /// Approve a reviewed Goal and mark it done.
    Approve {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Walk a Goal's status backwards: done goes to review; cancelled goes to todo.
    ///
    /// To decline a reviewed Goal, submit a new auditable round instead.
    Undo {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Permanently delete a Goal record from project state. Irreversible; prefer cancel to keep history.
    Delete {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Assign a Goal to a Feature so it is grouped and ordered with related work.
    AssignFeature {
        /// Goal id.
        id: String,
        /// Feature id to assign the Goal to.
        feature_id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Remove a Goal from its Feature. The Goal itself is kept.
    RemoveFeature {
        /// Goal id.
        id: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}
