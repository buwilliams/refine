use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::errors::{RefineError, RefineResult};

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
pub struct GitHeadRef {
    pub branch: Option<String>,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MergeResult {
    pub ok: bool,
    pub conflicts: Vec<String>,
    pub message: Option<String>,
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
}

impl FileGitWorktreeService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime_root: Some(runtime_root.into()),
        }
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

    pub fn remote_exists(&self, remote: &str) -> RefineResult<bool> {
        if remote.trim().is_empty() {
            return Ok(false);
        }
        Ok(self.git_raw(&["remote", "get-url", remote])?.success)
    }

    pub fn fetch_branch(&self, remote: &str, branch: &str) -> RefineResult<()> {
        validate_branch_name(branch)?;
        if !self.remote_exists(remote)? {
            return Err(RefineError::NotFound(format!(
                "Git remote {remote} was not found"
            )));
        }
        self.git_output(&["fetch", remote, branch])?;
        self.audit(
            "branch_fetch",
            "ok",
            json!({"remote": remote, "branch": branch}),
        )
    }

    pub fn fast_forward_from_remote(&self, remote: &str, branch: &str) -> RefineResult<()> {
        self.fetch_branch(remote, branch)?;
        let remote_branch = format!("{remote}/{branch}");
        self.git_output(&["merge", "--ff-only", &remote_branch])?;
        self.audit(
            "branch_fast_forward",
            "ok",
            json!({"remote": remote, "branch": branch}),
        )
    }

    pub fn resolve_commit(&self, commitish: &str) -> RefineResult<String> {
        validate_commitish(commitish)?;
        stdout(self.git_output(&["rev-parse", "--verify", &format!("{commitish}^{{commit}}")])?)
            .map(|value| value.trim().to_string())
    }

    pub fn merge_commit_no_ff(&self, commit: &str) -> RefineResult<MergeResult> {
        validate_commitish(commit)?;
        let output = self.git_raw(&["merge", "--no-ff", "--no-edit", commit])?;
        if output.success {
            let result = MergeResult {
                ok: true,
                conflicts: Vec::new(),
                message: Some(trimmed_command_text(&output)),
            };
            self.audit(
                "merge_commit_no_ff",
                "ok",
                json!({"commit": commit, "result": &result}),
            )?;
            return Ok(result);
        }
        let result = MergeResult {
            ok: false,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        let _ = self.audit(
            "merge_commit_no_ff",
            "conflict",
            json!({"commit": commit, "result": &result}),
        );
        Ok(result)
    }

    pub fn ensure_branch_from_remote(&self, remote: &str, branch: &str) -> RefineResult<()> {
        validate_branch_name(branch)?;
        if self.branch_exists(branch)? {
            return Ok(());
        }
        if !self.remote_exists(remote)? {
            return Err(RefineError::NotFound(format!(
                "Git remote {remote} was not found"
            )));
        }
        self.fetch_branch(remote, branch)?;
        let remote_branch = format!("{remote}/{branch}");
        self.git_output(&["branch", branch, &remote_branch])?;
        self.audit(
            "branch_fetch",
            "ok",
            json!({"remote": remote, "branch": branch}),
        )
    }

    pub fn head_ref(&self) -> RefineResult<GitHeadRef> {
        let branch_output = self.git_raw(&["branch", "--show-current"])?;
        let branch = if branch_output.success {
            let branch = stdout(branch_output)?.trim().to_string();
            if branch.is_empty() {
                None
            } else {
                Some(branch)
            }
        } else {
            None
        };

        let commit_output = self.git_raw(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        let commit = if commit_output.success {
            let commit = stdout(commit_output)?.trim().to_string();
            if commit.is_empty() {
                None
            } else {
                Some(commit)
            }
        } else {
            None
        };

        Ok(GitHeadRef { branch, commit })
    }

    pub fn git_path(&self, path: &str) -> RefineResult<PathBuf> {
        let resolved = stdout(self.git_output(&["rev-parse", "--git-path", path])?)?;
        let resolved = PathBuf::from(resolved.trim());
        let path = if resolved.is_absolute() {
            resolved
        } else {
            self.root.join(resolved)
        };
        self.audit(
            "git_path",
            "ok",
            json!({"path": path.display().to_string()}),
        )?;
        Ok(path)
    }

    pub fn revert_commit(&self, commit: &str) -> RefineResult<MergeResult> {
        validate_commitish(commit)?;
        let output = self.git_raw(&["revert", "--no-edit", commit])?;
        if output.success {
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

    pub fn remove_worktree(&self, path: &Path, force: bool) -> RefineResult<()> {
        let target = path.to_str().unwrap_or("");
        if target.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "worktree path is required".to_string(),
            ));
        }
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(target);
        self.git_output(&args)?;
        self.audit(
            "worktree_remove",
            "ok",
            json!({"target": path.display().to_string(), "force": force}),
        )
    }

    pub fn delete_branch(&self, branch: &str, force: bool) -> RefineResult<()> {
        validate_branch_name(branch)?;
        self.git_output(&["branch", if force { "-D" } else { "-d" }, branch])?;
        self.audit(
            "branch_delete",
            "ok",
            json!({"branch": branch, "force": force}),
        )
    }

    pub fn cleanup_merged_branch(&self, branch: &str) -> RefineResult<MergedBranchCleanup> {
        validate_branch_name(branch)?;
        let worktree_path = self.worktree_for_branch(branch)?;
        let mut worktree_removed = false;
        if let Some(path) = worktree_path.as_deref() {
            if same_existing_path(path, &self.root) {
                return Err(RefineError::Conflict(format!(
                    "refusing to remove primary worktree {} for branch {branch}",
                    path.display()
                )));
            }
            self.remove_worktree(path, true)?;
            worktree_removed = true;
        }

        let mut branch_deleted = false;
        if self.branch_exists(branch)? {
            self.delete_branch(branch, false)?;
            branch_deleted = true;
        }

        let cleanup = MergedBranchCleanup {
            branch: branch.to_string(),
            worktree_path: worktree_path.map(|path| path.display().to_string()),
            worktree_removed,
            branch_deleted,
        };
        self.audit("merged_branch_cleanup", "ok", json!({"cleanup": &cleanup}))?;
        Ok(cleanup)
    }

    pub fn has_commits_since(&self, base_branch: &str) -> RefineResult<bool> {
        validate_branch_name(base_branch)?;
        let output =
            stdout(self.git_output(&["rev-list", "--count", &format!("{base_branch}..HEAD")])?)?;
        Ok(output.trim().parse::<usize>().unwrap_or(0) > 0)
    }

    fn head_commit(&self) -> RefineResult<String> {
        stdout(self.git_output(&["rev-parse", "HEAD"])?).map(|commit| commit.trim().to_string())
    }

    fn is_clean(&self) -> RefineResult<bool> {
        stdout(self.git_output(&["status", "--porcelain=v1"])?)
            .map(|status| status.trim().is_empty())
    }

    fn current_clean_commit_since(&self, base_branch: &str) -> RefineResult<Option<String>> {
        if self.is_clean()? && self.has_commits_since(base_branch)? {
            return self.head_commit().map(Some);
        }
        Ok(None)
    }

    pub fn commit_or_clean_noop_since(
        &self,
        message: &str,
        pathspecs: &[String],
        base_branch: &str,
    ) -> RefineResult<GitCommitOutcome> {
        if let Some(commit) = self.current_clean_commit_since(base_branch)? {
            self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
            return Ok(GitCommitOutcome {
                commit,
                has_changes_since_base: true,
            });
        }
        match self.commit_inner(message, pathspecs, false) {
            Ok(commit) => Ok(GitCommitOutcome {
                commit,
                has_changes_since_base: true,
            }),
            Err(error) if is_nothing_to_commit_error(&error) => {
                if let Some(commit) = self.current_clean_commit_since(base_branch)? {
                    self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
                    Ok(GitCommitOutcome {
                        commit,
                        has_changes_since_base: true,
                    })
                } else if self.is_clean()? {
                    let commit = self.head_commit()?;
                    self.audit(
                        "commit_noop",
                        "ok",
                        json!({
                            "commit": commit,
                            "message": message,
                            "pathspecs": pathspecs,
                            "base_branch": base_branch
                        }),
                    )?;
                    Ok(GitCommitOutcome {
                        commit,
                        has_changes_since_base: false,
                    })
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn audit_existing_commit(
        &self,
        commit: &str,
        message: &str,
        pathspecs: &[String],
        base_branch: &str,
    ) -> RefineResult<()> {
        self.audit(
            "commit_existing",
            "ok",
            json!({
                "commit": commit,
                "message": message,
                "pathspecs": pathspecs,
                "base_branch": base_branch
            }),
        )
    }

    fn root_for(&self, path: &str) -> PathBuf {
        let path = path.trim();
        if path.is_empty() {
            self.root.clone()
        } else {
            PathBuf::from(path)
        }
    }

    fn git_output(&self, args: &[&str]) -> RefineResult<HostCommandOutput> {
        let output = self.git_raw_with_env(args, &[])?;
        if output.success {
            Ok(output)
        } else {
            Err(RefineError::Conflict(trimmed_command_text(&output)))
        }
    }

    fn git_raw(&self, args: &[&str]) -> RefineResult<HostCommandOutput> {
        self.git_raw_with_env(args, &[])
    }

    fn git_output_with_env(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> RefineResult<HostCommandOutput> {
        let output = self.git_raw_with_env(args, env)?;
        if output.success {
            Ok(output)
        } else {
            Err(RefineError::Conflict(trimmed_command_text(&output)))
        }
    }

    fn git_raw_with_env(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> RefineResult<HostCommandOutput> {
        if is_read_only_git_command(args) {
            return self.git_raw_untracked(args, env);
        }
        let mut process_args = vec!["-C".to_string(), self.root.display().to_string()];
        process_args.extend(args.iter().map(|arg| arg.to_string()));
        let output = FileProcessSupervisor::new(self.process_runtime_root()).run_to_completion(
            ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: "git".to_string(),
                args: process_args,
                cwd: None,
                env: env
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
                stdin: None,
                limits: None,
                authorization_command: Some(format!("git {}", args.join(" "))),
                sensitive: false,
                metadata: Default::default(),
            },
        )?;
        Ok(HostCommandOutput {
            success: output.success(),
            stdout: output.stdout.into_bytes(),
            stderr: output.stderr.into_bytes(),
        })
    }

    fn git_raw_untracked(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> RefineResult<HostCommandOutput> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .envs(env.iter().map(|(key, value)| (*key, *value)))
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        Ok(HostCommandOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn head_commit_exists(&self) -> RefineResult<bool> {
        Ok(self
            .git_raw(&["rev-parse", "--verify", "HEAD^{commit}"])?
            .success)
    }

    fn ensure_head_commit(&self) -> RefineResult<()> {
        if self.head_commit_exists()? {
            return Ok(());
        }
        self.git_output_with_env(
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "--only",
                "--no-verify",
                "-m",
                "Initialize Refine workspace",
            ],
            &[
                ("GIT_AUTHOR_NAME", "Refine"),
                ("GIT_AUTHOR_EMAIL", "refine@example.invalid"),
                ("GIT_COMMITTER_NAME", "Refine"),
                ("GIT_COMMITTER_EMAIL", "refine@example.invalid"),
            ],
        )?;
        let commit = stdout(self.git_output(&["rev-parse", "HEAD"])?)?
            .trim()
            .to_string();
        self.audit(
            "bootstrap_head",
            "ok",
            json!({"commit": commit, "message": "Initialize Refine workspace"}),
        )
    }

    fn process_runtime_root(&self) -> PathBuf {
        self.runtime_root.clone().unwrap_or_else(|| {
            std::env::temp_dir()
                .join("refine")
                .join("git-worktrees")
                .join(sanitize_runtime_component(&self.root.display().to_string()))
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

    fn branch_exists(&self, branch: &str) -> RefineResult<bool> {
        validate_branch_name(branch)?;
        Ok(self
            .git_raw(&["rev-parse", "--verify", &format!("refs/heads/{branch}")])?
            .success)
    }

    fn worktree_for_branch(&self, branch: &str) -> RefineResult<Option<PathBuf>> {
        validate_branch_name(branch)?;
        let output = stdout(self.git_output(&["worktree", "list", "--porcelain"])?)?;
        let mut current_path: Option<PathBuf> = None;
        for line in output.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(path));
            } else if let Some(head_branch) = line.strip_prefix("branch refs/heads/")
                && head_branch == branch
            {
                return Ok(current_path);
            }
        }
        Ok(None)
    }

    fn commit_inner(
        &self,
        message: &str,
        pathspecs: &[String],
        allow_empty: bool,
    ) -> RefineResult<String> {
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
        let mut commit_args = vec!["commit"];
        if allow_empty {
            commit_args.push("--allow-empty");
        }
        commit_args.extend(["-m", message]);
        self.git_output(&commit_args)?;
        let commit = stdout(self.git_output(&["rev-parse", "HEAD"])?)?
            .trim()
            .to_string();
        self.audit(
            "commit",
            "ok",
            json!({"commit": &commit, "message": message, "pathspecs": pathspecs, "allow_empty": allow_empty}),
        )?;
        Ok(commit)
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
        let service = FileGitWorktreeService {
            root: self.root_for(path),
            runtime_root: self.runtime_root.clone(),
        };
        let root = stdout(service.git_output(&["rev-parse", "--show-toplevel"])?)?
            .trim()
            .to_string();
        let branch_output = service.git_raw(&["branch", "--show-current"])?;
        let branch = if branch_output.success {
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

    fn switch(&self, branch: &str) -> RefineResult<String> {
        validate_branch_name(branch)?;
        self.git_output(&["switch", branch])?;
        self.audit("switch", "ok", json!({"branch": branch}))?;
        Ok(branch.to_string())
    }

    fn worktree(&self, branch: &str) -> RefineResult<String> {
        validate_branch_name(branch)?;
        self.ensure_head_commit()?;
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

    fn ensure_branch_from_head(&self, name: &str) -> RefineResult<String> {
        validate_branch_name(name)?;
        self.ensure_head_commit()?;
        if !self.branch_exists(name)? {
            self.git_output(&["branch", name])?;
            self.audit("branch", "ok", json!({"name": name, "reused": false}))?;
        } else {
            self.audit("branch", "ok", json!({"name": name, "reused": true}))?;
        }
        Ok(name.to_string())
    }

    fn ensure_worktree(&self, branch: &str, target: &Path) -> RefineResult<String> {
        validate_branch_name(branch)?;
        if let Some(existing) = self.worktree_for_branch(branch)? {
            self.audit(
                "worktree",
                "ok",
                json!({"branch": branch, "target": existing.display().to_string(), "reused": true}),
            )?;
            return Ok(existing.display().to_string());
        }
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            self.root.join(target)
        };
        if !self.branch_exists(branch)? {
            self.ensure_head_commit()?;
            self.git_output(&[
                "worktree",
                "add",
                "-b",
                branch,
                target.to_str().unwrap_or(""),
            ])?;
        } else {
            self.git_output(&["worktree", "add", target.to_str().unwrap_or(""), branch])?;
        }
        self.audit(
            "worktree",
            "ok",
            json!({"branch": branch, "target": target.display().to_string(), "reused": false}),
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
            ok: output.success,
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

    fn merge_no_ff(&self, branch: &str) -> RefineResult<MergeResult> {
        validate_branch_name(branch)?;
        let output = self.git_raw(&["merge", "--no-ff", "--no-edit", branch])?;
        let result = MergeResult {
            ok: output.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(trimmed_command_text(&output)),
        };
        if result.ok {
            self.audit(
                "merge_no_ff",
                "ok",
                json!({"branch": branch, "result": &result}),
            )?;
        } else {
            let _ = self.audit(
                "merge_no_ff",
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
            ok: output.success,
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
        self.commit_inner(message, pathspecs, false)
    }

    fn commit_or_current_if_clean_since(
        &self,
        message: &str,
        pathspecs: &[String],
        base_branch: &str,
    ) -> RefineResult<String> {
        if let Some(commit) = self.current_clean_commit_since(base_branch)? {
            self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
            return Ok(commit);
        }
        match self.commit_inner(message, pathspecs, false) {
            Ok(commit) => Ok(commit),
            Err(error) if is_nothing_to_commit_error(&error) => {
                if let Some(commit) = self.current_clean_commit_since(base_branch)? {
                    self.audit_existing_commit(&commit, message, pathspecs, base_branch)?;
                    Ok(commit)
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn commit_allow_empty(&self, message: &str, pathspecs: &[String]) -> RefineResult<String> {
        self.commit_inner(message, pathspecs, true)
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
        let reset = self.git_raw(&["reset", "--hard", "HEAD"])?;
        let mut clean_args = vec![
            "clean".to_string(),
            "-fd".to_string(),
            "-e".to_string(),
            "run".to_string(),
            "-e".to_string(),
            "run/**".to_string(),
            "-e".to_string(),
            "target".to_string(),
            "-e".to_string(),
            "target/**".to_string(),
        ];
        if let Some(runtime_root) = &self.runtime_root
            && let Some(relative_runtime) = relative_child_path(&self.root, runtime_root)
        {
            clean_args.extend([
                "-e".to_string(),
                relative_runtime.clone(),
                "-e".to_string(),
                format!("{relative_runtime}/**"),
            ]);
        }
        let clean_refs = clean_args.iter().map(String::as_str).collect::<Vec<_>>();
        let clean = self.git_raw(&clean_refs)?;
        let result = MergeResult {
            ok: reset.success && clean.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(
                [trimmed_command_text(&reset), trimmed_command_text(&clean)]
                    .into_iter()
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
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
        let revert = self.git_raw(&["revert", "--abort"])?;
        let result = MergeResult {
            ok: merge.success || rebase.success || revert.success,
            conflicts: self.conflicts().unwrap_or_default(),
            message: Some(format!(
                "{}\n{}\n{}",
                trimmed_command_text(&merge),
                trimmed_command_text(&rebase),
                trimmed_command_text(&revert)
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

fn is_read_only_git_command(args: &[&str]) -> bool {
    match args {
        ["branch", "--show-current"] => true,
        ["worktree", "list", "--porcelain"] => true,
        [command, ..] => matches!(
            *command,
            "diff" | "log" | "rev-list" | "rev-parse" | "show" | "status"
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
    fn file_git_worktree_service_treats_primary_worktree_refine_state_as_user_change() {
        let temp_root = unique_temp_dir("git-status");
        let repo = temp_root.join("repo");
        fs::create_dir_all(repo.join(".refine")).unwrap();
        git(&repo, &["init"]).unwrap();
        fs::write(repo.join(".refine/state.json"), "{}\n").unwrap();
        fs::write(repo.join("user.txt"), "user\n").unwrap();

        let status = FileGitWorktreeService::new(&repo).inspect("").unwrap();
        assert!(status.dirty_user_changes);
        assert!(status.refine_owned_artifacts.is_empty());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_hard_resets_tracked_changes() {
        let temp_root = unique_temp_dir("git-hard-reset");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        fs::write(repo.join("app.txt"), "committed\n").unwrap();
        git(&repo, &["add", "app.txt"]).unwrap();
        git(&repo, &["commit", "-m", "initial"]).unwrap();
        fs::write(repo.join("app.txt"), "dirty\n").unwrap();
        fs::write(repo.join("untracked.txt"), "keep\n").unwrap();
        fs::create_dir_all(repo.join(".refine")).unwrap();
        fs::write(repo.join(".refine/state.json"), "{}\n").unwrap();

        let service = FileGitWorktreeService::new(&repo);
        let reset = service.hard_reset().unwrap();
        assert!(reset.ok);
        assert_eq!(
            fs::read_to_string(repo.join("app.txt")).unwrap(),
            "committed\n"
        );
        assert!(!repo.join("untracked.txt").exists());
        assert!(!repo.join(".refine").exists());
        let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
        assert!(audit.contains("\"action\":\"hard_reset\""));
        assert!(audit.contains("\"status\":\"ok\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes() {
        let temp_root = unique_temp_dir("git-workflow-happy");
        let repo = temp_root.join("repo");
        let remote = temp_root.join("remote.git");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        git(
            &temp_root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        commit_file(&repo, "base.txt", "base\n", "initial");

        let service = FileGitWorktreeService::new(&repo);
        assert_eq!(
            service.branch("feature/pathspec").unwrap(),
            "feature/pathspec"
        );
        assert_eq!(current_branch(&repo), "feature/pathspec");
        commit_file(
            &repo,
            "tracked.txt",
            "base tracked\n",
            "track selected file",
        );
        fs::write(repo.join("tracked.txt"), "tracked\n").unwrap();
        fs::write(repo.join("ignored.txt"), "ignored\n").unwrap();
        let diff = service.diff(&["tracked.txt".to_string()]).unwrap();
        assert!(diff.contains("tracked"));
        assert!(!diff.contains("ignored"));

        let commit = service
            .commit("commit selected path", &["tracked.txt".to_string()])
            .unwrap();
        assert_eq!(
            git_stdout(&repo, &["show", "--pretty=format:", "--name-only", &commit]),
            "tracked.txt"
        );
        assert!(git_stdout(&repo, &["status", "--porcelain=v1"]).contains("?? ignored.txt"));
        service.push("origin", "feature/pathspec").unwrap();
        assert_eq!(
            git_stdout(&repo, &["rev-parse", "origin/feature/pathspec^{commit}"]),
            commit
        );

        git(&repo, &["switch", "main"]).unwrap();
        let worktree_path = PathBuf::from(service.worktree("feature/worktree").unwrap());
        assert!(worktree_path.join(".git").exists());
        assert_eq!(current_branch(&worktree_path), "feature/worktree");
        let worktree_status = service.inspect(worktree_path.to_str().unwrap()).unwrap();
        assert_eq!(worktree_status.root, worktree_path.display().to_string());
        assert_eq!(worktree_status.branch.as_deref(), Some("feature/worktree"));
        let linked_service = FileGitWorktreeService::new(&worktree_path);
        fs::write(worktree_path.join("base.txt"), "linked change\n").unwrap();
        assert!(
            linked_service
                .diff(&["base.txt".to_string()])
                .unwrap()
                .contains("linked change")
        );
        let linked_audit_path = linked_service.audit_path().unwrap();
        assert!(linked_audit_path.exists());
        assert_ne!(
            linked_audit_path,
            worktree_path.join(".git").join(GIT_AUDIT_FILE)
        );

        let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
        for action in ["branch", "diff", "commit", "push", "worktree"] {
            assert!(audit.contains(&format!("\"action\":\"{action}\"")));
        }

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_bootstraps_unborn_repo_for_worktree() {
        let temp_root = unique_temp_dir("git-worktree-unborn");
        let repo = temp_root.join("repo");
        let target = temp_root.join("standalone");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();
        git(&repo, &["add", "staged.txt"]).unwrap();

        let service = FileGitWorktreeService::new(&repo);
        let path = PathBuf::from(
            service
                .ensure_worktree("refine/standalone/test", &target)
                .unwrap(),
        );

        assert_eq!(path, target);
        assert!(path.join(".git").exists());
        assert_eq!(current_branch(&path), "refine/standalone/test");
        assert_eq!(
            git_stdout(&repo, &["log", "--pretty=%s", "-1"]),
            "Initialize Refine workspace"
        );
        assert_eq!(
            git_stdout(&repo, &["show", "--pretty=format:", "--name-only", "HEAD"]),
            ""
        );
        assert!(git_stdout(&repo, &["status", "--porcelain=v1"]).contains("A  staged.txt"));
        assert!(git_stdout(&repo, &["status", "--porcelain=v1"]).contains("?? untracked.txt"));
        assert!(!path.join("staged.txt").exists());
        assert!(!path.join("untracked.txt").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_cleans_merged_branch_worktree() {
        let temp_root = unique_temp_dir("git-worktree-cleanup");
        let repo = temp_root.join("repo");
        let worktree_path = temp_root.join("repo-refine-GOAL1-round-1");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        let branch = "refine/GOAL1/round-1";
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                branch,
                worktree_path.to_str().unwrap(),
            ],
        )
        .unwrap();
        commit_file(&worktree_path, "feature.txt", "change\n", "feature");
        git(&repo, &["merge", "--no-edit", branch]).unwrap();

        let service = FileGitWorktreeService::new(&repo);
        let cleanup = service.cleanup_merged_branch(branch).unwrap();
        assert_eq!(cleanup.branch, branch);
        assert_eq!(
            cleanup.worktree_path.as_deref(),
            Some(worktree_path.to_str().unwrap())
        );
        assert!(cleanup.worktree_removed);
        assert!(cleanup.branch_deleted);
        assert!(!worktree_path.exists());
        assert!(!git_stdout(&repo, &["worktree", "list", "--porcelain"]).contains(branch));
        assert!(!git_succeeds(
            &repo,
            &["rev-parse", "--verify", &format!("refs/heads/{branch}")]
        ));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_does_not_track_read_only_git_probes() {
        let temp_root = unique_temp_dir("git-read-only-untracked");
        let repo = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        let service = FileGitWorktreeService::with_runtime_root(&repo, &runtime_root);
        service.inspect("").unwrap();
        service.recent_changes(10).unwrap();
        service.diff(&["app.txt".to_string()]).unwrap();

        let process_count = fs::read_dir(runtime_root.join("processes"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(process_count, 0);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_merges_rebases_and_recovers_conflicts() {
        let temp_root = unique_temp_dir("git-conflicts");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");

        git(&repo, &["switch", "-c", "merge-side"]).unwrap();
        commit_file(&repo, "app.txt", "merge side\n", "merge side");
        git(&repo, &["switch", "main"]).unwrap();
        commit_file(&repo, "app.txt", "main side\n", "main side");

        let service = FileGitWorktreeService::new(&repo);
        let merge = service.merge("merge-side").unwrap();
        assert!(!merge.ok);
        assert_eq!(merge.conflicts, vec!["app.txt"]);
        assert!(
            fs::read_to_string(repo.join("app.txt"))
                .unwrap()
                .contains("<<<<<<<")
        );
        let recovered = service.recover().unwrap();
        assert!(recovered.ok);
        assert_eq!(service.conflicts().unwrap(), Vec::<String>::new());

        git(&repo, &["switch", "-c", "rebase-side", "HEAD~1"]).unwrap();
        commit_file(&repo, "app.txt", "rebase side\n", "rebase side");
        let rebase = service.rebase("main").unwrap();
        assert!(!rebase.ok);
        assert_eq!(rebase.conflicts, vec!["app.txt"]);
        assert!(
            fs::read_to_string(repo.join("app.txt"))
                .unwrap()
                .contains("<<<<<<<")
        );
        let recovered = service.recover().unwrap();
        assert!(recovered.ok);
        assert_eq!(service.conflicts().unwrap(), Vec::<String>::new());

        let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
        assert!(audit.contains("\"action\":\"merge\""));
        assert!(audit.contains("\"action\":\"rebase\""));
        assert!(audit.contains("\"action\":\"recover\""));
        assert!(audit.contains("\"status\":\"conflict\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_merges_and_rebases_cleanly() {
        let temp_root = unique_temp_dir("git-clean-integrations");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "base.txt", "base\n", "initial");

        git(&repo, &["switch", "-c", "merge-clean"]).unwrap();
        commit_file(&repo, "merge.txt", "merge\n", "merge clean");
        git(&repo, &["switch", "main"]).unwrap();
        let service = FileGitWorktreeService::new(&repo);
        let merge = service.merge("merge-clean").unwrap();
        assert!(merge.ok);
        assert!(repo.join("merge.txt").exists());

        git(&repo, &["switch", "-c", "rebase-clean"]).unwrap();
        commit_file(&repo, "rebase.txt", "rebase\n", "rebase clean");
        git(&repo, &["switch", "main"]).unwrap();
        commit_file(&repo, "main.txt", "main\n", "main clean");
        git(&repo, &["switch", "rebase-clean"]).unwrap();
        let rebase = service.rebase("main").unwrap();
        assert!(rebase.ok);
        assert!(repo.join("main.txt").exists());
        assert!(repo.join("rebase.txt").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_rejects_invalid_names_and_reports_git_failures() {
        let temp_root = unique_temp_dir("git-invalid");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");
        let service = FileGitWorktreeService::new(&repo);

        for name in ["", "-bad", "bad..name", "bad//name", "bad name"] {
            assert!(matches!(
                service.branch(name),
                Err(RefineError::InvalidInput(_))
            ));
            assert!(matches!(
                service.worktree(name),
                Err(RefineError::InvalidInput(_))
            ));
            assert!(matches!(
                service.merge(name),
                Err(RefineError::InvalidInput(_))
            ));
            assert!(matches!(
                service.rebase(name),
                Err(RefineError::InvalidInput(_))
            ));
            assert!(matches!(
                service.push("origin", name),
                Err(RefineError::InvalidInput(_))
            ));
        }
        assert!(matches!(
            service.push("", "main"),
            Err(RefineError::InvalidInput(_))
        ));
        assert!(matches!(
            service.revert_commit("bad ref!"),
            Err(RefineError::InvalidInput(_))
        ));
        assert!(matches!(
            service.branch("main"),
            Err(RefineError::Conflict(_))
        ));
        assert!(matches!(
            service.worktree("main"),
            Err(RefineError::Conflict(_))
        ));
        assert!(matches!(
            service.push("missing-remote", "main"),
            Err(RefineError::Conflict(_))
        ));
        let missing_revert = service.revert_commit("deadbeef").unwrap();
        assert!(!missing_revert.ok);
        assert!(
            missing_revert
                .message
                .unwrap_or_default()
                .contains("deadbeef")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_reports_dirty_worktree_merge_failure() {
        let temp_root = unique_temp_dir("git-dirty-merge");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "initial");
        git(&repo, &["switch", "-c", "incoming"]).unwrap();
        commit_file(&repo, "app.txt", "incoming\n", "incoming");
        git(&repo, &["switch", "main"]).unwrap();
        fs::write(repo.join("app.txt"), "dirty local\n").unwrap();

        let result = FileGitWorktreeService::new(&repo)
            .merge("incoming")
            .unwrap();
        assert!(!result.ok);
        assert!(result.message.unwrap_or_default().contains("local changes"));
        assert_eq!(
            fs::read_to_string(repo.join("app.txt")).unwrap(),
            "dirty local\n"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_revert_conflict_and_recover_preserves_history() {
        let temp_root = unique_temp_dir("git-revert-conflict");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "one\n", "initial");
        commit_file(&repo, "app.txt", "two\n", "second");
        let second = git_stdout(&repo, &["rev-parse", "HEAD"]);
        commit_file(&repo, "app.txt", "three\n", "third");

        let service = FileGitWorktreeService::new(&repo);
        let reverted = service.revert_commit(&second).unwrap();
        assert!(!reverted.ok);
        assert_eq!(reverted.conflicts, vec!["app.txt"]);
        assert!(
            fs::read_to_string(repo.join("app.txt"))
                .unwrap()
                .contains("<<<<<<<")
        );
        let recovered = service.recover().unwrap();
        assert!(recovered.ok);
        assert_eq!(fs::read_to_string(repo.join("app.txt")).unwrap(), "three\n");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_accepts_clean_branch_commit_since_base() {
        let temp_root = unique_temp_dir("git-existing-commit");
        let repo = temp_root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "base.txt", "base\n", "initial");
        git(&repo, &["switch", "-c", "feature/precommitted"]).unwrap();
        commit_file(&repo, "agent.txt", "agent\n", "agent commit");
        let precommitted = git_stdout(&repo, &["rev-parse", "HEAD"]);

        let service = FileGitWorktreeService::new(&repo);
        let commit = service
            .commit_or_current_if_clean_since("Refine commit wrapper", &[], "main")
            .unwrap();
        assert_eq!(commit, precommitted);
        assert_eq!(
            git_stdout(&repo, &["log", "--pretty=%s", "-1"]),
            "agent commit"
        );
        let audit = fs::read_to_string(service.audit_path().unwrap()).unwrap();
        assert!(audit.contains("\"action\":\"commit_existing\""));
        assert!(audit.contains(&precommitted));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_git_worktree_service_hard_reset_preserves_runtime_and_removes_other_noise() {
        let temp_root = unique_temp_dir("git-hard-reset-runtime");
        let repo = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "committed\n", "initial");
        fs::write(repo.join("app.txt"), "dirty\n").unwrap();
        fs::write(repo.join("untracked.txt"), "remove\n").unwrap();
        fs::create_dir_all(runtime_root.join("processes")).unwrap();
        fs::write(runtime_root.join("processes/pid.json"), "{}\n").unwrap();
        fs::create_dir_all(repo.join("run/8080")).unwrap();
        fs::write(repo.join("run/8080/state.json"), "{}\n").unwrap();
        fs::create_dir_all(repo.join("target/tmp")).unwrap();
        fs::write(repo.join("target/tmp/build.txt"), "build\n").unwrap();

        let service = FileGitWorktreeService::with_runtime_root(&repo, &runtime_root);
        let status = service.inspect("").unwrap();
        assert!(status.dirty_user_changes);
        assert!(
            status
                .refine_owned_artifacts
                .iter()
                .any(|path| path == "run/")
        );
        assert!(
            status
                .refine_owned_artifacts
                .iter()
                .any(|path| path == "target/")
        );

        let reset = service.hard_reset().unwrap();
        assert!(reset.ok);
        assert_eq!(
            fs::read_to_string(repo.join("app.txt")).unwrap(),
            "committed\n"
        );
        assert!(!repo.join("untracked.txt").exists());
        assert!(runtime_root.join("processes/pid.json").exists());
        assert!(repo.join("run/8080/state.json").exists());
        assert!(repo.join("target/tmp/build.txt").exists());

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
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(RefineError::Conflict(
                format!("{}\n{}", stdout.trim(), stderr.trim())
                    .trim()
                    .to_string(),
            ))
        }
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn git_succeeds(repo: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    fn init_repo(repo: &Path) {
        git(repo, &["init", "-b", "main"]).unwrap();
        git(repo, &["config", "user.email", "test@example.com"]).unwrap();
        git(repo, &["config", "user.name", "Test User"]).unwrap();
    }

    fn commit_file(repo: &Path, path: &str, contents: &str, message: &str) {
        fs::write(repo.join(path), contents).unwrap();
        git(repo, &["add", path]).unwrap();
        git(repo, &["commit", "-m", message]).unwrap();
    }

    fn current_branch(repo: &Path) -> String {
        git_stdout(repo, &["branch", "--show-current"])
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        temp_root.join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
