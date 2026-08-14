use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub use crate::model::goal::MergeResult;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::host_resources::HostResources;

pub const GIT_AUDIT_FILE: &str = "refine-audit.jsonl";
/// How long a Git command may go without output before it is stopped.
///
/// Generous, because this only has to catch a genuinely wedged command: Git
/// reports progress throughout a slow fetch or checkout, so legitimate slowness
/// on a constrained host keeps resetting the budget.
const GIT_STALL_TIMEOUT_SECONDS: u64 = 300;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitStatus {
    pub root: String,
    pub branch: Option<String>,
    pub dirty_user_changes: bool,
    pub refine_owned_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitChange {
    pub commit: String,
    pub committed_time: String,
    pub subject: String,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitHeadRef {
    pub branch: Option<String>,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitLinkedWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitCommitOutcome {
    pub commit: String,
    pub has_changes_since_base: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergedBranchCleanup {
    pub branch: String,
    pub worktree_path: Option<String>,
    pub worktree_removed: bool,
    pub branch_deleted: bool,
}

pub trait GitWorktreeService {
    fn inspect(&self, path: &str) -> RefineResult<GitStatus>;
    fn branch(&self, name: &str) -> RefineResult<String>;
    fn switch(&self, branch: &str) -> RefineResult<String>;
    fn worktree(&self, branch: &str) -> RefineResult<String>;
    fn ensure_branch_from_head(&self, name: &str) -> RefineResult<String>;
    fn ensure_worktree(&self, branch: &str, target: &Path) -> RefineResult<String>;
    fn diff(&self, pathspecs: &[String]) -> RefineResult<String>;
    fn merge(&self, branch: &str) -> RefineResult<MergeResult>;
    fn merge_no_ff(&self, branch: &str) -> RefineResult<MergeResult>;
    fn rebase(&self, branch: &str) -> RefineResult<MergeResult>;
    fn commit(&self, message: &str, pathspecs: &[String]) -> RefineResult<String>;
    fn commit_or_current_if_clean_since(
        &self,
        message: &str,
        pathspecs: &[String],
        base_branch: &str,
    ) -> RefineResult<String>;
    fn commit_allow_empty(&self, message: &str, pathspecs: &[String]) -> RefineResult<String>;
    fn push(&self, remote: &str, branch: &str) -> RefineResult<()>;
    fn hard_reset(&self) -> RefineResult<MergeResult>;
    fn recover(&self) -> RefineResult<MergeResult>;
}

#[derive(Clone, Debug)]
pub struct FileGitWorktreeService {
    pub root: PathBuf,
    pub runtime_root: Option<PathBuf>,
    operation_id: Option<String>,
    process_metadata: Map<String, Value>,
}

impl FileGitWorktreeService {}

fn create_worktree_parent(target: &Path) -> RefineResult<()> {
    let Some(parent) = target.parent() else {
        return Ok(());
    };
    // Checking out a worktree writes a full copy of the target application. On a
    // volume already at its reserve that fails partway through and leaves a
    // half-written tree that later runs then have to reason about. Refusing to
    // start is backpressure; a partial worktree is corruption.
    //
    // This does not attempt to predict the checkout's size — that would mean
    // measuring the working tree on every creation. It refuses only when the
    // volume has no headroom left at all, which is the case that actually
    // corrupts.
    if !HostResources::sample(parent).has_disk_headroom(0) {
        return Err(RefineError::Degraded(format!(
            "not enough free disk space under {} to check out a worktree",
            parent.display()
        )));
    }
    fs::create_dir_all(parent).map_err(|error| {
        RefineError::Io(format!(
            "failed to create worktree parent {}: {error}",
            parent.display()
        ))
    })
}

fn relative_child_path(root: &Path, child: &Path) -> Option<String> {
    let root = fs::canonicalize(root).ok()?;
    let child = fs::canonicalize(child).ok()?;
    let relative = child.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    Some(relative.to_string_lossy().replace('\\', "/"))
}

fn same_existing_path(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left)
        .ok()
        .zip(fs::canonicalize(right).ok())
        .is_some_and(|(left, right)| left == right)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HostCommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn stdout(output: HostCommandOutput) -> RefineResult<String> {
    String::from_utf8(output.stdout)
        .map_err(|error| RefineError::Serialization(format!("git output was not UTF-8: {error}")))
}

fn trimmed_command_text(output: &HostCommandOutput) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}\n{}", stdout.trim(), stderr.trim())
        .trim()
        .to_string()
}

fn is_nothing_to_commit_error(error: &RefineError) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("nothing to commit") || message.contains("nothing added to commit")
}

fn parse_git_change(raw: &str) -> Option<GitChange> {
    let raw = raw.trim_matches('\n').trim_matches('\r');
    if raw.trim().is_empty() {
        return None;
    }
    let mut parts = raw.splitn(3, '\x1f');
    Some(GitChange {
        commit: parts.next()?.trim().to_string(),
        committed_time: parts.next()?.trim().to_string(),
        subject: parts.next().unwrap_or("").trim().to_string(),
        branch: None,
    })
}

#[derive(Clone, Debug)]
struct GitChangeGraphNode {
    change: GitChange,
    parents: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct GitChangeGraph {
    nodes: BTreeMap<String, GitChangeGraphNode>,
    order: Vec<String>,
}

fn parse_git_change_graph(output: &str) -> GitChangeGraph {
    let mut graph = GitChangeGraph::default();
    for raw in output.split('\x1e') {
        let raw = raw.trim_matches(['\n', '\r']);
        if raw.trim().is_empty() {
            continue;
        }
        let mut parts = raw.splitn(4, '\x1f');
        let Some(commit) = parts
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let parents = parts
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let committed_time = parts.next().unwrap_or("").trim().to_string();
        let subject = parts.next().unwrap_or("").trim().to_string();
        graph.order.push(commit.to_string());
        graph.nodes.insert(
            commit.to_string(),
            GitChangeGraphNode {
                change: GitChange {
                    commit: commit.to_string(),
                    committed_time,
                    subject,
                    branch: None,
                },
                parents,
            },
        );
    }
    graph
}

fn reachable_commits(
    graph: &BTreeMap<String, GitChangeGraphNode>,
    start: &str,
) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![start.to_string()];
    while let Some(commit) = pending.pop() {
        if !reachable.insert(commit.clone()) {
            continue;
        }
        if let Some(node) = graph.get(&commit) {
            pending.extend(node.parents.iter().cloned());
        }
    }
    reachable
}

fn validate_commitish(commit: &str) -> RefineResult<()> {
    let commit = commit.trim();
    if commit.is_empty()
        || commit.starts_with('-')
        || commit.contains("..")
        || !commit
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.'))
    {
        return Err(RefineError::InvalidInput(
            "commit must be a git revision".to_string(),
        ));
    }
    Ok(())
}

fn validate_branch_name(branch: &str) -> RefineResult<()> {
    let branch = branch.trim();
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.contains("..")
        || branch.contains("//")
        || !branch
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.'))
    {
        return Err(RefineError::InvalidInput(
            "branch name is invalid".to_string(),
        ));
    }
    Ok(())
}

fn is_read_only_git_command(args: &[&str]) -> bool {
    match args {
        ["branch", "--show-current"] => true,
        ["worktree", "list", "--porcelain"] => true,
        ["remote"] | ["remote", "get-url", ..] => true,
        [command, ..] => matches!(
            *command,
            "cat-file" | "diff" | "log" | "merge-base" | "rev-list" | "rev-parse" | "show"
                | "status"
        ),
        [] => false,
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn is_refine_owned_artifact(path: &str) -> bool {
    path.starts_with("run/") || path.starts_with("target/")
}

fn sanitize_runtime_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "root".to_string()
    } else {
        out.to_string()
    }
}

mod changes;
mod commands;
mod integration;
mod service_adapter;
#[cfg(test)]
mod tests;
mod worktrees;
