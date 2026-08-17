//! In-memory three-way tree merge. The clean path never materializes a
//! worktree: `git merge-tree --write-tree` produces the merged tree directly,
//! and conflicted paths are reported structurally for the state driver or a
//! conflict report to consume. A vendored Git older than 2.38 falls back to a
//! per-path blob-level three-way over the same operands.

use std::sync::OnceLock;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::git::repo::command_failed;
use crate::tools::host::git_sync::FileGitSyncService;

/// Result of merging two commits from their common history: the merged tree
/// (conflicted paths carry a side-arbitrary or marker blob and are only
/// meaningful once replaced) and the paths that did not merge cleanly.
#[derive(Debug)]
pub struct TreeMerge {
    pub tree: String,
    pub conflicted_paths: Vec<String>,
}

pub fn merge_commits(
    repo: &FileGitSyncService,
    root: &std::path::Path,
    merge_base: &str,
    ours: &str,
    theirs: &str,
) -> RefineResult<TreeMerge> {
    // An empty-tree base marks unrelated histories being joined: `merge-tree`
    // refuses commits without a common ancestor, so that base always takes
    // the per-path route, which reads any tree-ish base with `ls-tree`.
    if merge_tree_supported(repo) && merge_base != empty_tree_id(repo, root)? {
        merge_with_merge_tree(repo, root, ours, theirs)
    } else {
        merge_per_path(repo, root, merge_base, ours, theirs)
    }
}

/// The repository's empty tree object: the merge base of two histories that
/// share no root (independent orphan bootstraps of the same branch).
pub fn empty_tree_id(repo: &FileGitSyncService, root: &std::path::Path) -> RefineResult<String> {
    repo.git_at_stdout(root, &["hash-object", "-t", "tree", "/dev/null"])
}

fn merge_tree_supported(repo: &FileGitSyncService) -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        let Ok(version) = repo.git_stdout(&["version"]) else {
            return false;
        };
        let mut numbers = version
            .split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .split('.')
            .filter_map(|part| part.parse::<u32>().ok());
        let major = numbers.next().unwrap_or(0);
        let minor = numbers.next().unwrap_or(0);
        major > 2 || (major == 2 && minor >= 38)
    })
}

fn merge_with_merge_tree(
    repo: &FileGitSyncService,
    root: &std::path::Path,
    ours: &str,
    theirs: &str,
) -> RefineResult<TreeMerge> {
    let output = repo.git_at(
        root,
        &["merge-tree", "--write-tree", "--name-only", ours, theirs],
    )?;
    parse_merge_tree_output(output.success, &String::from_utf8_lossy(&output.stdout)).ok_or_else(
        || {
            command_failed(
                &format!("git merge-tree --write-tree {ours} {theirs}"),
                &output,
            )
        },
    )
}

/// Parse `git merge-tree --write-tree --name-only` output: the merged tree id
/// on the first line, then — for a conflicted merge (non-zero exit) — the
/// conflicted names until a blank line separates the informational messages.
/// `None` means the output is not a merge result but a real command failure.
fn parse_merge_tree_output(success: bool, stdout: &str) -> Option<TreeMerge> {
    let mut lines = stdout.lines();
    let tree = lines.next().unwrap_or_default().trim().to_string();
    if tree.len() < 40 || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if success {
        return Some(TreeMerge {
            tree,
            conflicted_paths: Vec::new(),
        });
    }
    let conflicted_paths = lines
        .take_while(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    Some(TreeMerge {
        tree,
        conflicted_paths,
    })
}

/// Blob-identity three-way for Git without `merge-tree --write-tree`: a path
/// changed on one side takes that side; changed on both sides it is
/// conflicted. Content-level line merging is deliberately absent — contested
/// state files go to the structural driver, which is byte-exact anyway.
fn merge_per_path(
    repo: &FileGitSyncService,
    root: &std::path::Path,
    merge_base: &str,
    ours: &str,
    theirs: &str,
) -> RefineResult<TreeMerge> {
    let base = tree_blobs(repo, root, merge_base)?;
    let ours_blobs = tree_blobs(repo, root, ours)?;
    let theirs_blobs = tree_blobs(repo, root, theirs)?;
    let paths = base
        .keys()
        .chain(ours_blobs.keys())
        .chain(theirs_blobs.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut operations = Vec::new();
    let mut conflicted_paths = Vec::new();
    for path in paths {
        let base_blob = base.get(&path);
        let our_blob = ours_blobs.get(&path);
        let their_blob = theirs_blobs.get(&path);
        if our_blob == their_blob || their_blob == base_blob {
            continue; // ours already carries the merged value
        }
        if our_blob == base_blob {
            operations.push(match their_blob {
                Some(blob) => TreeOperation::Set {
                    path: path.clone(),
                    blob: blob.clone(),
                },
                None => TreeOperation::Remove { path: path.clone() },
            });
            continue;
        }
        conflicted_paths.push(path);
    }
    let ours_tree = repo.git_at_stdout(root, &["rev-parse", &format!("{ours}^{{tree}}")])?;
    let tree = build_tree(repo, root, &ours_tree, &operations)?;
    Ok(TreeMerge {
        tree,
        conflicted_paths,
    })
}

fn tree_blobs(
    repo: &FileGitSyncService,
    root: &std::path::Path,
    commitish: &str,
) -> RefineResult<std::collections::BTreeMap<String, String>> {
    let listing = repo.git_at_stdout(root, &["ls-tree", "-r", commitish])?;
    let mut blobs = std::collections::BTreeMap::new();
    for line in listing.lines() {
        // `<mode> SP <type> SP <oid> TAB <path>`
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        let mut fields = meta.split_whitespace();
        let _mode = fields.next();
        let kind = fields.next().unwrap_or_default();
        let oid = fields.next().unwrap_or_default();
        if kind == "blob" {
            blobs.insert(path.to_string(), oid.to_string());
        }
    }
    Ok(blobs)
}

/// One path-level edit applied on top of a start tree.
#[derive(Debug)]
pub enum TreeOperation {
    Set { path: String, blob: String },
    Remove { path: String },
}

/// Build a tree from `start_tree` plus path-level edits, using the given
/// worktree's index as scratch space and restoring it to HEAD afterwards. An
/// interruption leaves only a staged diff that the worktree recovery step
/// already clears.
pub fn build_tree(
    repo: &FileGitSyncService,
    root: &std::path::Path,
    start_tree: &str,
    operations: &[TreeOperation],
) -> RefineResult<String> {
    let result = (|| {
        repo.git_at_checked(root, &["read-tree", start_tree])?;
        for operation in operations {
            match operation {
                TreeOperation::Set { path, blob } => {
                    repo.git_at_checked(
                        root,
                        &[
                            "update-index",
                            "--add",
                            "--cacheinfo",
                            &format!("100644,{blob},{path}"),
                        ],
                    )?;
                }
                TreeOperation::Remove { path } => {
                    repo.git_at_checked(root, &["update-index", "--force-remove", "--", path])?;
                }
            }
        }
        repo.git_at_stdout(root, &["write-tree"])
    })();
    let restore = repo.git_at_checked(root, &["read-tree", "HEAD"]);
    let tree = result?;
    restore?;
    Ok(tree)
}

/// Store bytes as a blob object and return its id.
pub fn write_blob(
    repo: &FileGitSyncService,
    root: &std::path::Path,
    bytes: &[u8],
) -> RefineResult<String> {
    let staging = std::env::temp_dir().join(format!(
        "refine-merge-blob-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&staging, bytes).map_err(|error| {
        RefineError::Io(format!(
            "failed to stage merge blob {}: {error}",
            staging.display()
        ))
    })?;
    let staged = staging.display().to_string();
    let result = repo.git_at_stdout(root, &["hash-object", "-w", "--", &staged]);
    let _ = std::fs::remove_file(&staging);
    result
}

/// Create a commit object for a tree with explicit parents; no ref moves.
pub fn commit_tree(
    repo: &FileGitSyncService,
    root: &std::path::Path,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> RefineResult<String> {
    let mut args = vec!["commit-tree".to_string(), tree.to_string()];
    for parent in parents {
        args.push("-p".to_string());
        args.push((*parent).to_string());
    }
    args.push("-m".to_string());
    args.push(message.to_string());
    let arguments = args.iter().map(String::as_str).collect::<Vec<_>>();
    repo.git_at_stdout(root, &arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

    #[test]
    fn clean_merge_output_parses_to_a_tree_without_conflicts() {
        let merged = parse_merge_tree_output(true, &format!("{TREE}\n")).unwrap();
        assert_eq!(merged.tree, TREE);
        assert!(merged.conflicted_paths.is_empty());
    }

    #[test]
    fn conflicted_merge_output_parses_names_until_the_message_separator() {
        let stdout = format!(
            "{TREE}\n.refine/goals/GO/ALA/goal.json\n.refine/nodes.json\n\nAuto-merging .refine/nodes.json\nCONFLICT (content): Merge conflict in .refine/nodes.json\n"
        );
        let merged = parse_merge_tree_output(false, &stdout).unwrap();
        assert_eq!(merged.tree, TREE);
        assert_eq!(
            merged.conflicted_paths,
            vec![".refine/goals/GO/ALA/goal.json", ".refine/nodes.json"]
        );
    }

    #[test]
    fn non_merge_failures_do_not_parse() {
        assert!(parse_merge_tree_output(false, "fatal: not a git repository\n").is_none());
        assert!(parse_merge_tree_output(false, "").is_none());
    }
}
