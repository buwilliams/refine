use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::supervisor::errors::{RefineError, RefineResult};

pub const GIT_AUDIT_FILE: &str = "refine-audit.jsonl";

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
pub struct MergeResult {
    pub ok: bool,
    pub conflicts: Vec<String>,
    pub message: Option<String>,
}

pub trait GitWorktreeService {
    fn inspect(&self, path: &str) -> RefineResult<GitStatus>;
    fn branch(&self, name: &str) -> RefineResult<String>;
    fn worktree(&self, branch: &str) -> RefineResult<String>;
    fn diff(&self, pathspecs: &[String]) -> RefineResult<String>;
    fn merge(&self, branch: &str) -> RefineResult<MergeResult>;
    fn rebase(&self, branch: &str) -> RefineResult<MergeResult>;
    fn commit(&self, message: &str, pathspecs: &[String]) -> RefineResult<String>;
    fn push(&self, remote: &str, branch: &str) -> RefineResult<()>;
    fn hard_reset(&self) -> RefineResult<MergeResult>;
    fn recover(&self) -> RefineResult<MergeResult>;
}

#[derive(Clone, Debug)]
pub struct FileGitWorktreeService {
    pub root: PathBuf,
}

impl FileGitWorktreeService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn audit_path(&self) -> RefineResult<PathBuf> {
        stdout(self.git_output(&["rev-parse", "--git-path", GIT_AUDIT_FILE])?).map(|path| {
            let path = PathBuf::from(path.trim());
            if path.is_absolute() {
                path
            } else {
                self.root.join(path)
            }
        })
    }

    pub fn recent_changes(&self, limit: usize) -> RefineResult<Vec<GitChange>> {
        let output = self.git_output(&[
            "log",
            "--date=iso-strict",
            "--pretty=format:%H%x1f%cI%x1f%s%x1e",
            "-n",
            &limit.max(1).min(1000).to_string(),
        ])?;
        let text = stdout(output)?;
        Ok(text
            .split('\x1e')
            .filter_map(parse_git_change)
            .collect::<Vec<_>>())
    }

    pub fn revert_commit(&self, commit: &str) -> RefineResult<MergeResult> {
        validate_commitish(commit)?;
        let output = self.git_raw(&["revert", "--no-edit", commit])?;
        if output.status.success() {
            let result = MergeResult {
                ok: true,
                conflicts: Vec::new(),
                message: Some(trimmed_command_text(&output)),
            };
            self.audit("revert", "ok", json!({"commit": commit, "result": &result}))?;
            return Ok(result);
        }
        let conflicts = self.conflicts().unwrap_or_default();
        let result = MergeResult {
            ok: false,
            conflicts,
            message: Some(trimmed_command_text(&output)),
        };
        let _ = self.audit(
            "revert",
            "conflict",
            json!({"commit": commit, "result": &result}),
        );
        Ok(result)
    }

    fn root_for(&self, path: &str) -> PathBuf {
        let path = path.trim();
        if path.is_empty() {
            self.root.clone()
        } else {
            PathBuf::from(path)
        }
    }

    fn git_output(&self, args: &[&str]) -> RefineResult<Output> {
        let output = self.git_raw(args)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(RefineError::Conflict(trimmed_command_text(&output)))
        }
    }

    fn git_raw(&self, args: &[&str]) -> RefineResult<Output> {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to run git in {}: {error}",
                    self.root.display()
                ))
            })
    }

    fn conflicts(&self) -> RefineResult<Vec<String>> {
        let output = self.git_output(&["diff", "--name-only", "--diff-filter=U"])?;
        Ok(stdout(output)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    fn audit(&self, action: &str, status: &str, details: serde_json::Value) -> RefineResult<()> {
        let path = self.audit_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create Git audit directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let event = json!({
            "action": action,
            "status": status,
            "details": details,
            "created_at": now_timestamp()
        });
        let line = serde_json::to_string(&event).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Git audit event: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open Git audit {}: {error}",
                    path.display()
                ))
            })?;
        writeln!(file, "{line}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append Git audit {}: {error}",
                path.display()
            ))
        })
    }
}

impl GitWorktreeService for FileGitWorktreeService {
    fn inspect(&self, path: &str) -> RefineResult<GitStatus> {
        let service = FileGitWorktreeService::new(self.root_for(path));
        let root = stdout(service.git_output(&["rev-parse", "--show-toplevel"])?)?
            .trim()
            .to_string();
        let branch_output = service.git_raw(&["branch", "--show-current"])?;
        let branch = if branch_output.status.success() {
            let branch = stdout(branch_output)?.trim().to_string();
            if branch.is_empty() {
                None
            } else {
                Some(branch)
            }
        } else {
            None
        };
        let status = stdout(service.git_output(&["status", "--porcelain=v1"])?).unwrap_or_default();
        let mut refine_owned_artifacts = Vec::new();
        let mut dirty_user_changes = false;
        for line in status.lines() {
            let path = line.get(3..).unwrap_or("").trim();
            if is_refine_owned_artifact(path) {
                refine_owned_artifacts.push(path.to_string());
            } else if !path.is_empty() {
                dirty_user_changes = true;
            }
        }
        Ok(GitStatus {
            root,
            branch,
            dirty_user_changes,
            refine_owned_artifacts,
        })
    }

    fn branch(&self, name: &str) -> RefineResult<String> {
        validate_branch_name(name)?;
        self.git_output(&["switch", "-c", name])?;
        self.audit("branch", "ok", json!({"name": name}))?;
        Ok(name.to_string())
    }

    fn worktree(&self, branch: &str) -> RefineResult<String> {
        validate_branch_name(branch)?;
        let parent = self.root.parent().unwrap_or_else(|| Path::new("."));
        let target = parent.join(format!(
            "{}-{}",
            self.root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("worktree"),
            branch.replace('/', "-")
        ));
        self.git_output(&[
            "worktree",
            "add",
            "-b",
            branch,
            target.to_str().unwrap_or(""),
        ])?;
        self.audit(
            "worktree",
            "ok",
            json!({"branch": branch, "target": target.display().to_string()}),
        )?;
        Ok(target.display().to_string())
    }

    fn diff(&self, pathspecs: &[String]) -> RefineResult<String> {
        let mut args = vec!["diff", "--"];
        for pathspec in pathspecs {
            args.push(pathspec.as_str());
        }
        let diff = stdout(self.git_output(&args)?)?;
        self.audit("diff", "ok", json!({"pathspecs": pathspecs}))?;
        Ok(diff)
    }

    fn merge(&self, branch: &str) -> RefineResult<MergeResult> {
        validate_branch_name(branch)?;
        let output = self.git_raw(&["merge", "--no-edit", branch])?;
        let result = MergeResult {
            ok: output.status.success(),
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit("merge", "ok", json!({"branch": branch, "result": &result}))?;
        } else {
            let _ = self.audit(
                "merge",
                "conflict",
                json!({"branch": branch, "result": &result}),
            );
        }
        Ok(result)
    }

    fn rebase(&self, branch: &str) -> RefineResult<MergeResult> {
        validate_branch_name(branch)?;
        let output = self.git_raw(&["rebase", branch])?;
        let result = MergeResult {
            ok: output.status.success(),
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit("rebase", "ok", json!({"branch": branch, "result": &result}))?;
        } else {
            let _ = self.audit(
                "rebase",
                "conflict",
                json!({"branch": branch, "result": &result}),
            );
        }
        Ok(result)
    }

    fn commit(&self, message: &str, pathspecs: &[String]) -> RefineResult<String> {
        if message.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "commit message is required".to_string(),
            ));
        }
        if pathspecs.is_empty() {
            self.git_output(&["add", "-A"])?;
        } else {
            let mut add_args = vec!["add", "--"];
            for pathspec in pathspecs {
                add_args.push(pathspec.as_str());
            }
            self.git_output(&add_args)?;
        }
        self.git_output(&["commit", "-m", message])?;
        let commit = stdout(self.git_output(&["rev-parse", "HEAD"])?)?
            .trim()
            .to_string();
        self.audit(
            "commit",
            "ok",
            json!({"commit": &commit, "message": message, "pathspecs": pathspecs}),
        )?;
        Ok(commit)
    }

    fn push(&self, remote: &str, branch: &str) -> RefineResult<()> {
        validate_branch_name(branch)?;
        if remote.trim().is_empty() {
            return Err(RefineError::InvalidInput("remote is required".to_string()));
        }
        self.git_output(&["push", remote, branch])?;
        self.audit("push", "ok", json!({"remote": remote, "branch": branch}))
    }

    fn hard_reset(&self) -> RefineResult<MergeResult> {
        let output = self.git_raw(&["reset", "--hard", "HEAD"])?;
        let result = MergeResult {
            ok: output.status.success(),
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit("hard_reset", "ok", json!({"result": &result}))?;
        } else {
            let _ = self.audit("hard_reset", "failed", json!({"result": &result}));
        }
        Ok(result)
    }

    fn recover(&self) -> RefineResult<MergeResult> {
        let merge = self.git_raw(&["merge", "--abort"])?;
        let rebase = self.git_raw(&["rebase", "--abort"])?;
        let result = MergeResult {
            ok: merge.status.success() || rebase.status.success(),
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(format!(
                "{}\n{}",
                trimmed_command_text(&merge),
                trimmed_command_text(&rebase)
            )),
        };
        if result.ok {
            self.audit("recover", "ok", json!({"result": &result}))?;
        } else {
            let _ = self.audit("recover", "failed", json!({"result": &result}));
        }
        Ok(result)
    }
}

fn stdout(output: Output) -> RefineResult<String> {
    String::from_utf8(output.stdout)
        .map_err(|error| RefineError::Serialization(format!("git output was not UTF-8: {error}")))
}

fn trimmed_command_text(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{}\n{}", stdout.trim(), stderr.trim())
        .trim()
        .to_string()
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

fn validate_commitish(commit: &str) -> RefineResult<()> {
    let commit = commit.trim();
    if commit.is_empty()
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

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn is_refine_owned_artifact(path: &str) -> bool {
    path.starts_with(".refine/")
        || path == ".refine"
        || path.starts_with("run/")
        || path.starts_with("rust/target/")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn file_git_worktree_service_lists_status_and_reverts_commits() {
        let temp_root = unique_temp_dir("git-worktree");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init"]).unwrap();
        git(&repo, &["config", "user.email", "test@example.com"]).unwrap();
        git(&repo, &["config", "user.name", "Test User"]).unwrap();
        fs::write(repo.join("app.txt"), "one\n").unwrap();
        git(&repo, &["add", "app.txt"]).unwrap();
        git(&repo, &["commit", "-m", "initial"]).unwrap();
        fs::write(repo.join("app.txt"), "two\n").unwrap();
        git(&repo, &["commit", "-am", "update app"]).unwrap();

        let service = FileGitWorktreeService::new(&repo);
        let changes = service.recent_changes(10).unwrap();
        assert_eq!(changes[0].subject, "update app");
        let status = service.inspect("").unwrap();
        assert!(matches!(status.branch.as_deref(), Some("main" | "master")));
        assert!(!status.dirty_user_changes);

        let reverted = service.revert_commit(&changes[0].commit).unwrap();
        assert!(reverted.ok);
        assert_eq!(fs::read_to_string(repo.join("app.txt")).unwrap(), "one\n");
        let audit_path = service.audit_path().unwrap();
        assert_eq!(audit_path, repo.join(".git").join(GIT_AUDIT_FILE));
        let audit = fs::read_to_string(audit_path).unwrap();
        assert!(audit.contains("\"action\":\"revert\""));
        assert!(audit.contains("\"status\":\"ok\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_separates_refine_artifacts_from_user_changes() {
        let temp_root = unique_temp_dir("git-status");
        let repo = temp_root.join("repo");
        fs::create_dir_all(repo.join(".refine")).unwrap();
        git(&repo, &["init"]).unwrap();
        fs::write(repo.join(".refine/state.json"), "{}\n").unwrap();
        fs::write(repo.join("user.txt"), "user\n").unwrap();

        let status = FileGitWorktreeService::new(&repo).inspect("").unwrap();
        assert!(status.dirty_user_changes);
        assert_eq!(status.refine_owned_artifacts, vec![".refine/"]);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_hard_resets_tracked_changes() {
        let temp_root = unique_temp_dir("git-hard-reset");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init"]).unwrap();
        git(&repo, &["config", "user.email", "test@example.com"]).unwrap();
        git(&repo, &["config", "user.name", "Test User"]).unwrap();
        fs::write(repo.join("app.txt"), "committed\n").unwrap();
        git(&repo, &["add", "app.txt"]).unwrap();
        git(&repo, &["commit", "-m", "initial"]).unwrap();
        fs::write(repo.join("app.txt"), "dirty\n").unwrap();
        fs::write(repo.join("untracked.txt"), "keep\n").unwrap();

        let service = FileGitWorktreeService::new(&repo);
        let reset = service.hard_reset().unwrap();
        assert!(reset.ok);
        assert_eq!(
            fs::read_to_string(repo.join("app.txt")).unwrap(),
            "committed\n"
        );
        assert_eq!(
            fs::read_to_string(repo.join("untracked.txt")).unwrap(),
            "keep\n"
        );
        let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
        assert!(audit.contains("\"action\":\"hard_reset\""));
        assert!(audit.contains("\"status\":\"ok\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RefineError::Conflict(trimmed_command_text(&output)))
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}
