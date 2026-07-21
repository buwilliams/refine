use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};

const RELEASE_REQUESTS_DIR: &str = "releases/requests";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseBump {
    Major,
    Minor,
    Patch,
}

impl ReleaseBump {
    pub fn parse(value: &str) -> RefineResult<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "major" => Ok(Self::Major),
            "minor" => Ok(Self::Minor),
            "patch" => Ok(Self::Patch),
            _ => Err(RefineError::InvalidInput(
                "release bump must be major, minor, or patch".to_string(),
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseChange {
    pub commit: String,
    pub summary: String,
    pub breaking: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleasePlan {
    pub current_version: String,
    pub proposed_version: String,
    pub proposed_tag: String,
    pub previous_tag: Option<String>,
    pub bump: ReleaseBump,
    pub changes: Vec<ReleaseChange>,
    pub completed_goals: Vec<String>,
    pub breaking_changes: Vec<String>,
    pub version_files: Vec<String>,
    pub documentation_files: Vec<String>,
    pub gates: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedRelease {
    pub version: String,
    pub tag: String,
    pub branch: String,
    pub commit: String,
    pub worktree: String,
    pub release_notes: String,
    pub changed_files: Vec<String>,
    pub gates: Vec<ReleaseGateResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseGateResult {
    pub command: String,
    pub success: bool,
    pub output: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublishedRelease {
    pub version: String,
    pub tag: String,
    pub commit: String,
    pub remote: String,
    pub deployment: String,
    pub release_url: String,
    pub verified: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ReleaseRequest {
    Prepare { bump: ReleaseBump },
    Publish { candidate: PreparedRelease },
}

pub trait ReleaseHost {
    fn plan(&mut self, bump: ReleaseBump) -> RefineResult<ReleasePlan>;
    fn prepare(&mut self, plan: &ReleasePlan) -> RefineResult<PreparedRelease>;
    fn publish(&mut self, candidate: &PreparedRelease) -> RefineResult<PublishedRelease>;
}

#[derive(Clone, Debug)]
pub struct FileReleaseService {
    pub repo_root: PathBuf,
    pub runtime_root: PathBuf,
}

impl FileReleaseService {
    pub fn new(repo_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn plan(&self, bump: ReleaseBump) -> RefineResult<ReleasePlan> {
        ShellReleaseHost::new(&self.repo_root, &self.runtime_root).plan(bump)
    }

    pub fn status(&self) -> RefineResult<Value> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let mut operations = Vec::new();
        for operation in registry
            .recover()?
            .into_iter()
            .filter(|operation| operation.owner.starts_with("release:"))
        {
            let (logs, _, _) = registry.page_logs(&operation.id, 100, 0)?;
            let mut value = operation_json(operation);
            value["logs"] = json!(logs);
            operations.push(value);
        }
        Ok(json!({"operations": operations}))
    }

    pub fn start_prepare(&self, bump: ReleaseBump) -> RefineResult<OperationHandle> {
        let operation = self.register_request(ReleaseRequest::Prepare { bump })?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn prepare_blocking(&self, bump: ReleaseBump) -> RefineResult<OperationHandle> {
        let operation = self.register_request(ReleaseRequest::Prepare { bump })?;
        let mut host = ShellReleaseHost::new(&self.repo_root, &self.runtime_root);
        if let Err(error) = self.run_with_host(&operation.id, &mut host) {
            let registry = FileOperationRegistry::new(&self.runtime_root);
            let _ = registry.fail_with_error(
                &operation.id,
                json!({"code": "release_operation_failed", "message": error.to_string()}),
            );
            return Err(error);
        }
        FileOperationRegistry::new(&self.runtime_root).status(&operation.id)
    }

    pub fn start_publish(
        &self,
        candidate: PreparedRelease,
        confirmed: bool,
    ) -> RefineResult<OperationHandle> {
        if !confirmed {
            return Err(RefineError::InvalidInput(
                "publishing is externally mutating and requires confirmed=true".to_string(),
            ));
        }
        let operation = self.register_request(ReleaseRequest::Publish { candidate })?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn publish_blocking(
        &self,
        candidate: PreparedRelease,
        confirmed: bool,
    ) -> RefineResult<OperationHandle> {
        if !confirmed {
            return Err(RefineError::InvalidInput(
                "publishing is externally mutating and requires confirmed=true".to_string(),
            ));
        }
        let operation = self.register_request(ReleaseRequest::Publish { candidate })?;
        let mut host = ShellReleaseHost::new(&self.repo_root, &self.runtime_root);
        if let Err(error) = self.run_with_host(&operation.id, &mut host) {
            let registry = FileOperationRegistry::new(&self.runtime_root);
            let _ = registry.fail_with_error(
                &operation.id,
                json!({"code": "release_operation_failed", "message": error.to_string()}),
            );
            return Err(error);
        }
        FileOperationRegistry::new(&self.runtime_root).status(&operation.id)
    }

    pub fn retry(&self, operation_id: &str, confirmed: bool) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let prior = registry.status(operation_id)?;
        if !prior.owner.starts_with("release:") {
            return Err(RefineError::InvalidInput(
                "only release operations can be retried here".to_string(),
            ));
        }
        if matches!(
            prior.state,
            OperationState::Running | OperationState::Pending
        ) {
            return Err(RefineError::Conflict(format!(
                "release operation {operation_id} is still active"
            )));
        }
        let request = self.load_request(operation_id)?;
        if matches!(request, ReleaseRequest::Publish { .. }) && !confirmed {
            return Err(RefineError::InvalidInput(
                "retrying publication requires confirmed=true".to_string(),
            ));
        }
        let operation = self.register_request(request)?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn run_with_host(
        &self,
        operation_id: &str,
        host: &mut dyn ReleaseHost,
    ) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let request = self.load_request(operation_id)?;
        let result = match request {
            ReleaseRequest::Prepare { bump } => {
                stage(
                    &registry,
                    operation_id,
                    "analyze",
                    "Analyzing completed work and commits",
                )?;
                let plan = host.plan(bump)?;
                stage(
                    &registry,
                    operation_id,
                    "prepare",
                    "Updating versions, notes, and documentation",
                )?;
                let candidate = host.prepare(&plan)?;
                json!({"plan": plan, "candidate": candidate, "review_required": true})
            }
            ReleaseRequest::Publish { candidate } => {
                stage(
                    &registry,
                    operation_id,
                    "preflight",
                    "Checking synchronized main, tag, credentials, and remote",
                )?;
                let published = host.publish(&candidate)?;
                json!({"candidate": candidate, "published": published})
            }
        };
        registry.finish_with_result(operation_id, OperationState::Succeeded, result)
    }

    fn register_request(&self, request: ReleaseRequest) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        if registry.recover()?.iter().any(|operation| {
            operation.owner.starts_with("release:")
                && matches!(
                    operation.state,
                    OperationState::Running | OperationState::Pending
                )
        }) {
            return Err(RefineError::Conflict(
                "another release operation is already active".to_string(),
            ));
        }
        let owner = match request {
            ReleaseRequest::Prepare { .. } => "release:prepare",
            ReleaseRequest::Publish { .. } => "release:publish",
        };
        let operation = registry.register(owner)?;
        let path = self.request_path(&operation.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error("create release request directory"))?;
        }
        fs::write(
            &path,
            serde_json::to_vec_pretty(&request).map_err(|error| {
                RefineError::Serialization(format!("failed to encode release request: {error}"))
            })?,
        )
        .map_err(io_error("write release request"))?;
        Ok(operation)
    }

    fn request_path(&self, operation_id: &str) -> PathBuf {
        self.runtime_root
            .join(RELEASE_REQUESTS_DIR)
            .join(format!("{operation_id}.json"))
    }

    fn load_request(&self, operation_id: &str) -> RefineResult<ReleaseRequest> {
        let path = self.request_path(operation_id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RefineError::NotFound(format!("release request {operation_id} was not found"))
            } else {
                RefineError::Io(format!("failed to read {}: {error}", path.display()))
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
        })
    }

    fn spawn(&self, operation_id: String) {
        let service = self.clone();
        std::thread::spawn(move || {
            let mut host = ShellReleaseHost::new(&service.repo_root, &service.runtime_root);
            if let Err(error) = service.run_with_host(&operation_id, &mut host) {
                let registry = FileOperationRegistry::new(&service.runtime_root);
                let _ = registry.fail_with_error(
                    &operation_id,
                    json!({"code": "release_operation_failed", "message": error.to_string()}),
                );
            }
        });
    }
}

fn stage(
    registry: &FileOperationRegistry,
    operation_id: &str,
    name: &str,
    message: &str,
) -> RefineResult<()> {
    registry.update_progress(operation_id, json!({"stage": name, "message": message}))?;
    registry.append_log(
        operation_id,
        LogEntry {
            datetime: String::new(),
            severity: "info".to_string(),
            category: "release".to_string(),
            message: message.to_string(),
            actor: Some("release-agent".to_string()),
            goal_id: None,
            actions: Vec::new(),
            details: Some(json!({"stage": name}).as_object().unwrap().clone()),
        },
    )?;
    Ok(())
}

fn operation_json(operation: OperationHandle) -> Value {
    json!({
        "id": operation.id,
        "owner": operation.owner,
        "status": operation.state.as_api_status(),
        "progress": operation.progress,
        "result": operation.result,
        "error": operation.error,
    })
}

#[derive(Clone, Debug)]
pub struct ShellReleaseHost {
    repo_root: PathBuf,
    runtime_root: PathBuf,
}

impl ShellReleaseHost {
    pub fn new(repo_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    fn git(&self, args: &[&str]) -> RefineResult<String> {
        command_text(&self.repo_root, "git", args)
    }
}

impl ReleaseHost for ShellReleaseHost {
    fn plan(&mut self, bump: ReleaseBump) -> RefineResult<ReleasePlan> {
        ensure_git_checkout(&self.repo_root)?;
        let current_version = read_package_version(&self.repo_root.join("Cargo.toml"))?;
        let proposed_version = bump_version(&current_version, bump)?;
        let previous_tag = latest_semver_tag(&self.repo_root)?;
        let range = previous_tag
            .as_ref()
            .map(|tag| format!("{tag}..HEAD"))
            .unwrap_or_else(|| "HEAD".to_string());
        let log = self.git(&["log", "--format=%H%x09%s", &range])?;
        let changes = log
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(commit, summary)| ReleaseChange {
                commit: commit.to_string(),
                summary: summary.to_string(),
                breaking: summary.contains("BREAKING CHANGE")
                    || summary.contains("!:")
                    || summary.starts_with("breaking:"),
            })
            .collect::<Vec<_>>();
        let breaking_changes = changes
            .iter()
            .filter(|change| change.breaking)
            .map(|change| change.summary.clone())
            .collect();
        let completed_goals = completed_goal_summaries(&self.repo_root)?;
        let mut version_files = vec!["Cargo.toml".to_string(), "Cargo.lock".to_string()];
        version_files.retain(|path| self.repo_root.join(path).is_file());
        let documentation_files = ["RELEASE_NOTES.md", "docs/story.md"]
            .into_iter()
            .filter(|path| self.repo_root.join(path).exists() || *path == "RELEASE_NOTES.md")
            .map(str::to_string)
            .collect();
        let gates = release_gate_commands(&self.repo_root);
        let tag_prefix = previous_tag
            .as_deref()
            .filter(|tag| tag.starts_with('v'))
            .map(|_| "v")
            .unwrap_or("");
        Ok(ReleasePlan {
            proposed_tag: format!("{tag_prefix}{proposed_version}"),
            current_version,
            proposed_version,
            previous_tag,
            bump,
            changes,
            completed_goals,
            breaking_changes,
            version_files,
            documentation_files,
            gates,
        })
    }

    fn prepare(&mut self, plan: &ReleasePlan) -> RefineResult<PreparedRelease> {
        if !self.git(&["status", "--porcelain"])?.is_empty() {
            return Err(RefineError::Conflict(
                "release preparation requires a clean checkout".to_string(),
            ));
        }
        let branch = format!("release/v{}", plan.proposed_version);
        let worktree = self
            .runtime_root
            .join("releases/worktrees")
            .join(format!("v{}", plan.proposed_version));
        if let Some(parent) = worktree.parent() {
            fs::create_dir_all(parent).map_err(io_error("create release worktree directory"))?;
        }
        let base_commit = self.git(&["rev-parse", "HEAD"])?;
        if worktree.exists() {
            let existing_branch =
                command_text(&worktree, "git", &["symbolic-ref", "--short", "HEAD"])?;
            if existing_branch != branch {
                return Err(RefineError::Conflict(format!(
                    "release worktree {} belongs to {existing_branch}, not {branch}",
                    worktree.display()
                )));
            }
        } else if command_optional(
            &self.repo_root,
            "git",
            &["show-ref", "--verify", &format!("refs/heads/{branch}")],
        )?
        .is_some()
        {
            command_text(
                &self.repo_root,
                "git",
                &["worktree", "add", &worktree.display().to_string(), &branch],
            )?;
        } else {
            command_text(
                &self.repo_root,
                "git",
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    &worktree.display().to_string(),
                    "HEAD",
                ],
            )?;
        }
        command_text(&worktree, "git", &["reset", "--hard", &base_commit])?;
        command_text(&worktree, "git", &["clean", "-fd"])?;
        update_version_file(
            &worktree.join("Cargo.toml"),
            &plan.current_version,
            &plan.proposed_version,
        )?;
        update_lockfile(
            &worktree.join("Cargo.lock"),
            &plan.current_version,
            &plan.proposed_version,
        )?;
        let notes = render_release_notes(plan);
        prepend_file(&worktree.join("RELEASE_NOTES.md"), &notes)?;
        if worktree.join("docs/story.md").is_file() {
            append_story(&worktree.join("docs/story.md"), plan)?;
        }
        let mut gates = Vec::new();
        for command in &plan.gates {
            let output = shell_command(&worktree, command)?;
            let result = ReleaseGateResult {
                command: command.clone(),
                success: output.status.success(),
                output: combined_output(&output),
            };
            let success = result.success;
            gates.push(result);
            if !success {
                return Err(RefineError::Degraded(format!(
                    "release gate failed: {command}\n{}",
                    gates
                        .last()
                        .map(|gate| gate.output.as_str())
                        .unwrap_or_default()
                )));
            }
        }
        command_text(&worktree, "git", &["add", "--all"])?;
        command_text(
            &worktree,
            "git",
            &["commit", "-m", &format!("Prepare {}", plan.proposed_tag)],
        )?;
        let commit = command_text(&worktree, "git", &["rev-parse", "HEAD"])?;
        let changed = command_text(
            &worktree,
            "git",
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        )?;
        Ok(PreparedRelease {
            version: plan.proposed_version.clone(),
            tag: plan.proposed_tag.clone(),
            branch,
            commit,
            worktree: worktree.display().to_string(),
            release_notes: "RELEASE_NOTES.md".to_string(),
            changed_files: changed.lines().map(str::to_string).collect(),
            gates,
        })
    }

    fn publish(&mut self, candidate: &PreparedRelease) -> RefineResult<PublishedRelease> {
        ensure_git_checkout(&self.repo_root)?;
        let branch = self.git(&["symbolic-ref", "--short", "HEAD"])?;
        if branch != "main" {
            return Err(RefineError::Conflict(format!(
                "publication requires main; current branch is {branch}"
            )));
        }
        if !self.git(&["status", "--porcelain"])?.is_empty() {
            return Err(RefineError::Conflict(
                "publication requires clean main".to_string(),
            ));
        }
        self.git(&["fetch", "--tags", "origin"])?;
        let local = self.git(&["rev-parse", "HEAD"])?;
        let remote = self.git(&["rev-parse", "origin/main"])?;
        if local != remote || local != candidate.commit {
            return Err(RefineError::Conflict(
                "publication requires synchronized main at the reviewed candidate commit"
                    .to_string(),
            ));
        }
        let version = read_package_version(&self.repo_root.join("Cargo.toml"))?;
        if version != candidate.version
            || candidate.tag.strip_prefix('v').unwrap_or(&candidate.tag) != version
        {
            return Err(RefineError::Conflict(
                "version, candidate, and semantic tag are not aligned".to_string(),
            ));
        }
        let local_tag = command_optional(
            &self.repo_root,
            "git",
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("refs/tags/{}^{{}}", candidate.tag),
            ],
        )?;
        if local_tag
            .as_deref()
            .is_some_and(|commit| commit != candidate.commit)
        {
            return Err(RefineError::Conflict(format!(
                "tag {} points at a different commit",
                candidate.tag
            )));
        }
        command_text(&self.repo_root, "gh", &["auth", "status"])?;
        if local_tag.is_none() {
            self.git(&[
                "tag",
                "-a",
                &candidate.tag,
                "-m",
                &format!("Release {}", candidate.version),
            ])?;
        }
        let remote_tag = self.git(&[
            "ls-remote",
            "origin",
            &format!("refs/tags/{}^{{}}", candidate.tag),
        ])?;
        if remote_tag.is_empty() {
            self.git(&["push", "origin", &candidate.tag])?;
        } else if !remote_tag
            .lines()
            .any(|line| line.starts_with(&candidate.commit))
        {
            return Err(RefineError::Conflict(format!(
                "remote tag {} points at a different commit",
                candidate.tag
            )));
        }
        let release_args = [
            "release",
            "view",
            &candidate.tag,
            "--json",
            "url",
            "--jq",
            ".url",
        ];
        let release_url = match command_optional(&self.repo_root, "gh", &release_args)? {
            Some(url) => url,
            None => {
                command_text(
                    &self.repo_root,
                    "gh",
                    &[
                        "release",
                        "create",
                        &candidate.tag,
                        "--title",
                        &candidate.tag,
                        "--notes-file",
                        &candidate.release_notes,
                        "--verify-tag",
                    ],
                )?;
                command_text(&self.repo_root, "gh", &release_args)?
            }
        };
        let delivery = command_text(
            &self.repo_root,
            "gh",
            &[
                "run",
                "list",
                "--commit",
                &candidate.commit,
                "--limit",
                "20",
                "--json",
                "name,status,conclusion,url",
            ],
        )?;
        Ok(PublishedRelease {
            version: candidate.version.clone(),
            tag: candidate.tag.clone(),
            commit: candidate.commit.clone(),
            remote: "origin".to_string(),
            deployment: if delivery.is_empty() {
                "tag push observed; no deployment or package workflow was reported yet".to_string()
            } else {
                delivery
            },
            release_url,
            verified: true,
        })
    }
}

pub fn bump_version(current: &str, bump: ReleaseBump) -> RefineResult<String> {
    let parts = current
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RefineError::InvalidInput(format!("invalid semantic version: {current}")))?;
    if parts.len() != 3 {
        return Err(RefineError::InvalidInput(format!(
            "invalid semantic version: {current}"
        )));
    }
    let (major, minor, patch) = (parts[0], parts[1], parts[2]);
    Ok(match bump {
        ReleaseBump::Major => format!("{}.0.0", major + 1),
        ReleaseBump::Minor => format!("{}.{}.0", major, minor + 1),
        ReleaseBump::Patch => format!("{}.{}.{}", major, minor, patch + 1),
    })
}

fn read_package_version(path: &Path) -> RefineResult<String> {
    let text = fs::read_to_string(path).map_err(io_error("read package manifest"))?;
    text.lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("version = \"")
                .and_then(|value| value.strip_suffix('"'))
        })
        .map(str::to_string)
        .ok_or_else(|| {
            RefineError::InvalidInput(format!("package version not found in {}", path.display()))
        })
}

fn latest_semver_tag(root: &Path) -> RefineResult<Option<String>> {
    let tags = command_text(root, "git", &["tag", "--merged", "HEAD"])?;
    let mut versions = tags
        .lines()
        .filter_map(|tag| {
            let raw = tag.strip_prefix('v').unwrap_or(tag);
            let parts = raw
                .split('.')
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            (parts.len() == 3).then_some(((parts[0], parts[1], parts[2]), tag.to_string()))
        })
        .collect::<Vec<_>>();
    versions.sort_by_key(|(version, _)| *version);
    Ok(versions.pop().map(|(_, tag)| tag))
}

fn release_gate_commands(root: &Path) -> Vec<String> {
    let mut gates = vec!["git diff --check".to_string()];
    if root.join("Cargo.toml").is_file() {
        gates.extend([
            "cargo fmt --all -- --check".to_string(),
            "cargo test --lib --bins -- --test-threads=1".to_string(),
            "cargo build --release --locked".to_string(),
        ]);
    }
    gates
}

fn render_release_notes(plan: &ReleasePlan) -> String {
    let changes = if plan.changes.is_empty() {
        "- No commits since the previous semantic release.\n".to_string()
    } else {
        plan.changes
            .iter()
            .map(|change| {
                format!(
                    "- {} ({})\n",
                    change.summary,
                    &change.commit[..change.commit.len().min(12)]
                )
            })
            .collect()
    };
    let breaking = if plan.breaking_changes.is_empty() {
        "- None identified.\n".to_string()
    } else {
        plan.breaking_changes
            .iter()
            .map(|change| format!("- {change}\n"))
            .collect()
    };
    let goals = if plan.completed_goals.is_empty() {
        "- No completed Goal records found in the local state view.\n".to_string()
    } else {
        plan.completed_goals
            .iter()
            .map(|goal| format!("- {goal}\n"))
            .collect()
    };
    format!(
        "# {}\n\n## Completed Goals\n\n{goals}\n## Changes\n\n{changes}\n## Breaking changes\n\n{breaking}\n",
        plan.proposed_tag
    )
}

fn completed_goal_summaries(root: &Path) -> RefineResult<Vec<String>> {
    let candidates = [
        root.join(".refine/goals"),
        root.join(".git/refine-live-state/goals"),
    ];
    let Some(goals_root) = candidates.into_iter().find(|path| path.is_dir()) else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
    collect_named_files(&goals_root, "goal.json", &mut files)?;
    let mut goals = Vec::new();
    for path in files {
        let value: Value =
            serde_json::from_slice(&fs::read(&path).map_err(io_error("read Goal record"))?)
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to parse {}: {error}",
                        path.display()
                    ))
                })?;
        if value.get("status").and_then(Value::as_str) == Some("done") {
            let id = value.get("id").and_then(Value::as_str).unwrap_or("Goal");
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            goals.push(format!("{id}: {name}"));
        }
    }
    goals.sort();
    Ok(goals)
}

fn collect_named_files(root: &Path, name: &str, files: &mut Vec<PathBuf>) -> RefineResult<()> {
    for entry in fs::read_dir(root).map_err(io_error("read Goal state directory"))? {
        let entry = entry.map_err(io_error("inspect Goal state entry"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, name, files)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            files.push(path);
        }
    }
    Ok(())
}

fn update_version_file(path: &Path, old: &str, new: &str) -> RefineResult<()> {
    let text = fs::read_to_string(path).map_err(io_error("read version file"))?;
    let needle = format!("version = \"{old}\"");
    if !text.contains(&needle) {
        return Err(RefineError::InvalidInput(format!(
            "expected {needle} in {}",
            path.display()
        )));
    }
    fs::write(
        path,
        text.replacen(&needle, &format!("version = \"{new}\""), 1),
    )
    .map_err(io_error("write version file"))
}

fn update_lockfile(path: &Path, old: &str, new: &str) -> RefineResult<()> {
    if !path.is_file() {
        return Ok(());
    }
    let text = fs::read_to_string(path).map_err(io_error("read lockfile"))?;
    let package = format!("name = \"refine\"\nversion = \"{old}\"");
    if text.contains(&package) {
        fs::write(
            path,
            text.replacen(
                &package,
                &format!("name = \"refine\"\nversion = \"{new}\""),
                1,
            ),
        )
        .map_err(io_error("write lockfile"))?;
    }
    Ok(())
}

fn prepend_file(path: &Path, content: &str) -> RefineResult<()> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    fs::write(path, format!("{content}\n{existing}")).map_err(io_error("write release notes"))
}

fn append_story(path: &Path, plan: &ReleasePlan) -> RefineResult<()> {
    let mut text = fs::read_to_string(path).map_err(io_error("read story"))?;
    text.push_str(&format!(
        "\nRelease {} prepared with {} reviewed change(s); deterministic release gates passed.\n",
        plan.proposed_tag,
        plan.changes.len()
    ));
    fs::write(path, text).map_err(io_error("write story"))
}

fn ensure_git_checkout(root: &Path) -> RefineResult<()> {
    if !root.join(".git").exists() {
        return Err(RefineError::InvalidInput(format!(
            "{} is not a Git checkout",
            root.display()
        )));
    }
    Ok(())
}

fn shell_command(root: &Path, command: &str) -> RefineResult<Output> {
    Command::new("sh")
        .args(["-c", command])
        .current_dir(root)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run {command}: {error}")))
}

fn command_text(root: &Path, program: &str, args: &[&str]) -> RefineResult<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run {program}: {error}")))?;
    if !output.status.success() {
        return Err(RefineError::Degraded(format!(
            "{} {} failed: {}",
            program,
            args.join(" "),
            combined_output(&output)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn command_optional(root: &Path, program: &str, args: &[&str]) -> RefineResult<Option<String>> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run {program}: {error}")))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_string()
}

fn io_error(action: &'static str) -> impl FnOnce(std::io::Error) -> RefineError {
    move |error| RefineError::Io(format!("failed to {action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_bumps_are_explicit_and_deterministic() {
        assert_eq!(bump_version("4.2.9", ReleaseBump::Major).unwrap(), "5.0.0");
        assert_eq!(bump_version("4.2.9", ReleaseBump::Minor).unwrap(), "4.3.0");
        assert_eq!(bump_version("4.2.9", ReleaseBump::Patch).unwrap(), "4.2.10");
        assert!(bump_version("4.2", ReleaseBump::Patch).is_err());
    }

    #[derive(Default)]
    struct FakeHost {
        published: bool,
    }

    impl ReleaseHost for FakeHost {
        fn plan(&mut self, bump: ReleaseBump) -> RefineResult<ReleasePlan> {
            Ok(ReleasePlan {
                current_version: "1.0.0".into(),
                proposed_version: "1.1.0".into(),
                proposed_tag: "v1.1.0".into(),
                previous_tag: Some("v1.0.0".into()),
                bump,
                changes: vec![],
                completed_goals: vec!["GOAL1: Done".into()],
                breaking_changes: vec![],
                version_files: vec!["Cargo.toml".into()],
                documentation_files: vec!["RELEASE_NOTES.md".into()],
                gates: vec!["gate".into()],
            })
        }
        fn prepare(&mut self, plan: &ReleasePlan) -> RefineResult<PreparedRelease> {
            Ok(PreparedRelease {
                version: plan.proposed_version.clone(),
                tag: plan.proposed_tag.clone(),
                branch: "release/v1.1.0".into(),
                commit: "abc".into(),
                worktree: "/tmp/release".into(),
                release_notes: "RELEASE_NOTES.md".into(),
                changed_files: vec!["Cargo.toml".into()],
                gates: vec![],
            })
        }
        fn publish(&mut self, candidate: &PreparedRelease) -> RefineResult<PublishedRelease> {
            self.published = true;
            Ok(PublishedRelease {
                version: candidate.version.clone(),
                tag: candidate.tag.clone(),
                commit: candidate.commit.clone(),
                remote: "origin".into(),
                deployment: "observed".into(),
                release_url: "https://example.test/release".into(),
                verified: true,
            })
        }
    }

    #[test]
    fn operations_persist_and_publish_requires_confirmation() {
        let root = std::env::temp_dir().join(format!("refine-release-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let service = FileReleaseService::new(&root, &root);
        assert!(
            service
                .start_publish(
                    PreparedRelease {
                        version: "1.1.0".into(),
                        tag: "v1.1.0".into(),
                        branch: "release/v1.1.0".into(),
                        commit: "abc".into(),
                        worktree: "/tmp/release".into(),
                        release_notes: "RELEASE_NOTES.md".into(),
                        changed_files: vec![],
                        gates: vec![]
                    },
                    false
                )
                .is_err()
        );
        let operation = service
            .register_request(ReleaseRequest::Prepare {
                bump: ReleaseBump::Minor,
            })
            .unwrap();
        let mut host = FakeHost::default();
        let finished = service.run_with_host(&operation.id, &mut host).unwrap();
        assert_eq!(finished.state, OperationState::Succeeded);
        assert_eq!(finished.result["candidate"]["tag"], "v1.1.0");
        assert_eq!(
            service.status().unwrap()["operations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let candidate: PreparedRelease =
            serde_json::from_value(finished.result["candidate"].clone()).unwrap();
        let publish = service
            .register_request(ReleaseRequest::Publish { candidate })
            .unwrap();
        let published = service.run_with_host(&publish.id, &mut host).unwrap();
        assert!(host.published);
        assert_eq!(published.result["published"]["verified"], true);
        let _ = fs::remove_dir_all(root);
    }
}
