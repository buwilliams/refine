use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use crate::core::product::project_state::{GapSummaryProjection, ProjectionSnapshot};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::workflow::GapStatus;

const TERMINAL_OUTPUT_LIMIT: usize = 64_000;
const TERMINAL_COMMAND_LIMIT: usize = 8_000;

#[derive(Clone, Debug)]
struct TerminalWorktree {
    path: PathBuf,
    branch: Option<String>,
    head: Option<String>,
    current: bool,
}

pub(in crate::surfaces::web_server) fn terminal_worktrees_response(
    source_root: &Path,
    projection: &ProjectionSnapshot,
) -> RefineResult<Value> {
    let worktrees = terminal_worktrees(source_root)?;
    Ok(json!({
        "source_root": source_root.display().to_string(),
        "worktrees": worktrees
            .iter()
            .map(|worktree| terminal_worktree_value(worktree, projection))
            .collect::<Vec<_>>()
    }))
}

pub(in crate::surfaces::web_server) fn terminal_run_response(
    source_root: &Path,
    requested_path: &str,
    command: &str,
) -> RefineResult<Value> {
    let command = command.trim();
    if command.is_empty() {
        return Err(RefineError::InvalidInput(
            "terminal command is required".to_string(),
        ));
    }
    if command.chars().count() > TERMINAL_COMMAND_LIMIT {
        return Err(RefineError::InvalidInput(format!(
            "terminal command is limited to {TERMINAL_COMMAND_LIMIT} characters"
        )));
    }
    let worktree = resolve_terminal_worktree(source_root, requested_path)?;
    let output = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(&worktree.path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run terminal command: {error}")))?;
    Ok(json!({
        "ok": output.status.success(),
        "code": output.status.code(),
        "worktree_path": worktree.path.display().to_string(),
        "command": command,
        "stdout": truncate_terminal_output(&String::from_utf8_lossy(&output.stdout)),
        "stderr": truncate_terminal_output(&String::from_utf8_lossy(&output.stderr))
    }))
}

fn terminal_worktree_value(worktree: &TerminalWorktree, projection: &ProjectionSnapshot) -> Value {
    let branch = worktree.branch.as_deref().unwrap_or("");
    let gap = terminal_gap_for_worktree(worktree, projection);
    json!({
        "path": worktree.path.display().to_string(),
        "label": terminal_worktree_label(worktree, gap),
        "branch": branch,
        "head": worktree.head.as_deref().unwrap_or(""),
        "current": worktree.current,
        "gap_id": gap.map(|gap| gap.gap.id.as_str()).unwrap_or(""),
        "gap_name": gap.map(|gap| gap.gap.name.as_str()).unwrap_or(""),
        "gap_status": gap.map(|gap| gap.gap.status.as_str()).unwrap_or(""),
        "can_submit_merge": gap
            .map(|gap| can_submit_gap_for_merge(&gap.gap.status))
            .unwrap_or(false)
    })
}

fn terminal_worktree_label(
    worktree: &TerminalWorktree,
    gap: Option<&GapSummaryProjection>,
) -> String {
    let name = worktree
        .path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_else(|| worktree.path.to_str().unwrap_or("worktree"));
    let mut parts = vec![name.to_string()];
    if let Some(branch) = &worktree.branch
        && !branch.is_empty()
    {
        parts.push(branch.clone());
    }
    if let Some(gap) = gap {
        parts.push(format!("Gap {}", gap.gap.id));
    }
    parts.join(" - ")
}

fn terminal_gap_for_worktree<'a>(
    worktree: &TerminalWorktree,
    projection: &'a ProjectionSnapshot,
) -> Option<&'a GapSummaryProjection> {
    if let Some(branch) = worktree.branch.as_deref() {
        if let Some(gap) = projection.gaps.values().find(|gap| {
            gap.gap
                .branch_name
                .as_deref()
                .map(normalize_branch_name)
                .as_deref()
                == Some(branch)
        }) {
            return Some(gap);
        }
    }
    let path = worktree.path.display().to_string();
    projection
        .gaps
        .values()
        .find(|gap| !gap.gap.id.is_empty() && path.contains(&gap.gap.id))
}

fn can_submit_gap_for_merge(status: &GapStatus) -> bool {
    matches!(
        status,
        GapStatus::InProgress | GapStatus::Qa | GapStatus::ReadyMerge | GapStatus::Failed
    )
}

fn resolve_terminal_worktree(
    source_root: &Path,
    requested_path: &str,
) -> RefineResult<TerminalWorktree> {
    let requested_path = requested_path.trim();
    let worktrees = terminal_worktrees(source_root)?;
    let source_canonical = canonical_path(source_root)?;
    let requested_canonical = if requested_path.is_empty() {
        source_canonical
    } else {
        canonical_path(Path::new(requested_path))?
    };
    worktrees
        .into_iter()
        .find(|worktree| {
            canonical_path(&worktree.path)
                .map(|path| path == requested_canonical)
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            RefineError::InvalidInput(
                "terminal worktree must be one of the attached repository worktrees".to_string(),
            )
        })
}

fn terminal_worktrees(source_root: &Path) -> RefineResult<Vec<TerminalWorktree>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(source_root)
        .args(["worktree", "list", "--porcelain"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| RefineError::Io(format!("failed to list Git worktrees: {error}")))?;
    if !output.status.success() {
        return Ok(vec![TerminalWorktree {
            path: source_root.to_path_buf(),
            branch: None,
            head: None,
            current: true,
        }]);
    }
    let current = canonical_path(source_root)?;
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::new();
    let mut pending: Option<TerminalWorktree> = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(worktree) = pending.take() {
                push_terminal_worktree(worktree, &mut parsed, &mut seen);
            }
            let path = PathBuf::from(path);
            let is_current = canonical_path(&path)
                .map(|path| path == current)
                .unwrap_or(false);
            pending = Some(TerminalWorktree {
                path,
                branch: None,
                head: None,
                current: is_current,
            });
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            if let Some(worktree) = pending.as_mut() {
                worktree.head = Some(head.to_string());
            }
        } else if let Some(branch) = line.strip_prefix("branch ") {
            if let Some(worktree) = pending.as_mut() {
                worktree.branch = Some(normalize_branch_name(branch));
            }
        }
    }
    if let Some(worktree) = pending.take() {
        push_terminal_worktree(worktree, &mut parsed, &mut seen);
    }
    if parsed.is_empty() {
        parsed.push(TerminalWorktree {
            path: source_root.to_path_buf(),
            branch: None,
            head: None,
            current: true,
        });
    }
    parsed.sort_by(|a, b| b.current.cmp(&a.current).then_with(|| a.path.cmp(&b.path)));
    Ok(parsed)
}

fn push_terminal_worktree(
    worktree: TerminalWorktree,
    parsed: &mut Vec<TerminalWorktree>,
    seen: &mut BTreeSet<PathBuf>,
) {
    let key = canonical_path(&worktree.path).unwrap_or_else(|_| worktree.path.clone());
    if seen.insert(key) {
        parsed.push(worktree);
    }
}

fn normalize_branch_name(branch: &str) -> String {
    branch
        .trim()
        .strip_prefix("refs/heads/")
        .unwrap_or_else(|| branch.trim())
        .to_string()
}

fn canonical_path(path: &Path) -> RefineResult<PathBuf> {
    fs::canonicalize(path).map_err(|error| {
        RefineError::InvalidInput(format!("path {} is not available: {error}", path.display()))
    })
}

fn truncate_terminal_output(text: &str) -> String {
    if text.len() <= TERMINAL_OUTPUT_LIMIT {
        return text.to_string();
    }
    let mut truncated = text.chars().take(TERMINAL_OUTPUT_LIMIT).collect::<String>();
    truncated.push_str("\n[output truncated]");
    truncated
}
