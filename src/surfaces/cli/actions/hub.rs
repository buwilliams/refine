use super::*;
#[derive(Debug, Subcommand)]
pub enum HubAction {
    /// List Knowledge Hub sites.
    List,
    /// Inspect a site and its revision.
    Show { site: String },
    /// Create or edit a site from JSON (name, description, and revision for edits).
    Save {
        site: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Delete a site; retain its Git history.
    Delete {
        site: String,
        #[arg(long)]
        revision: String,
    },
    /// List a site's collections.
    Collections { site: String },
    /// Create or edit a collection from JSON (indexes, revision for edits).
    Collection {
        site: String,
        collection: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Delete a collection.
    DeleteCollection {
        site: String,
        collection: String,
        #[arg(long)]
        revision: String,
    },
    /// Read a record.
    Get {
        site: String,
        collection: String,
        id: String,
    },
    /// Write a record from JSON (data, request_id, revision for updates).
    Put {
        site: String,
        collection: String,
        id: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Delete a record.
    Remove {
        site: String,
        collection: String,
        id: String,
        #[arg(long)]
        revision: String,
    },
    /// Query indexed records, aggregations and search using a JSON query.
    Query {
        site: String,
        collection: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Rebuild a collection's disposable indexes.
    Index { site: String, collection: String },
    /// Import JSON or JSONL records in resumable batches (records require IDs).
    Import {
        site: String,
        collection: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// Export all records as JSONL.
    Export {
        site: String,
        collection: String,
        #[arg(long)]
        file: PathBuf,
    },
    /// List the asset manifest and its revision.
    Assets { site: String },
    /// Upload a site directory. Does not publish it.
    Upload { site: String, directory: PathBuf },
    /// Download the editable site assets to a directory.
    Download { site: String, directory: PathBuf },
    /// Remove one asset from the editable manifest.
    RemoveAsset {
        site: String,
        path: String,
        #[arg(long)]
        revision: String,
    },
    /// Publish assets and explicitly selected read-only collections.
    Publish {
        site: String,
        #[arg(long)]
        revision: String,
        #[arg(long)]
        collection: Vec<String>,
    },
    /// Withdraw public serving.
    Unpublish {
        site: String,
        #[arg(long)]
        revision: String,
    },
    /// Inspect local availability and publication status.
    Status { site: String },
}
