use super::*;

#[derive(Debug, Subcommand)]
pub enum TodoAction {
    /// List all Todo lists and their items for one Reporter.
    List {
        /// Reporter whose Todo lists to return.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Create a named Todo list for one Reporter.
    CreateList {
        /// Name for the new list.
        name: String,
        /// Reporter who owns the new list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Rename a Reporter-owned Todo list.
    RenameList {
        /// Todo list id.
        list_id: String,
        /// New name for the list.
        name: String,
        /// Reporter who owns the list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Delete a Reporter-owned Todo list and its items.
    DeleteList {
        /// Todo list id.
        list_id: String,
        /// Reporter who owns the list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Add an item to a Reporter-owned Todo list.
    Add {
        /// Todo list id.
        list_id: String,
        /// Text for the new item.
        text: String,
        /// Reporter who owns the list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Edit the text of a Reporter-owned Todo item.
    Edit {
        /// Todo list id.
        list_id: String,
        /// Todo item id.
        item_id: String,
        /// Replacement item text.
        text: String,
        /// Reporter who owns the list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Delete an item from a Reporter-owned Todo list.
    Delete {
        /// Todo list id.
        list_id: String,
        /// Todo item id.
        item_id: String,
        /// Reporter who owns the list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Mark a Reporter-owned Todo item done.
    Done {
        /// Todo list id.
        list_id: String,
        /// Todo item id.
        item_id: String,
        /// Reporter who owns the list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
    /// Restore a Reporter-owned Todo item to not done.
    Undo {
        /// Todo list id.
        list_id: String,
        /// Todo item id.
        item_id: String,
        /// Reporter who owns the list.
        #[arg(long)]
        reporter: String,
        #[cfg_attr(test, arg(long, hide = true))]
        #[cfg_attr(not(test), arg(skip = None))]
        target_root: Option<PathBuf>,
    },
}
