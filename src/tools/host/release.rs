use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::tools::host::project_layout::{prepare_refine_dir, refine_dir_for_target_root};
use crate::tools::product::work_items::FileWorkItemService;

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
pub struct TrustedPreparation {
    pub preparation_id: String,
    pub goal_id: String,
    pub version: String,
    pub tag: String,
    pub branch: String,
    pub target_branch: String,
    pub candidate_commit: String,
    pub release_notes: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicationPreflight {
    pub main_commit: String,
    pub remote: String,
    pub branch: String,
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
    Prepare {
        plan: Box<ReleasePlan>,
        goal_id: Option<String>,
    },
    Publish {
        preparation_id: String,
    },
}

/// External publication is split into idempotent stages so a retry can inspect
/// already-created state, reject conflicts, and continue at the first missing stage.
pub trait ReleaseHost {
    fn plan(&mut self, bump: ReleaseBump) -> RefineResult<ReleasePlan>;
    fn preflight(&mut self, preparation: &TrustedPreparation)
    -> RefineResult<PublicationPreflight>;
    fn ensure_local_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()>;
    fn ensure_remote_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()>;
    fn ensure_github_release(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String>;
    fn observe_delivery(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String>;
    fn verify(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String>;
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
        ShellReleaseHost::new(&self.repo_root).plan(bump)
    }

    pub fn status(&self) -> RefineResult<Value> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let mut operations = Vec::new();
        for operation in registry
            .recover()?
            .into_iter()
            .filter(|operation| operation.owner.starts_with("release:"))
        {
            let operation_id = operation.id.clone();
            let (logs, _, _) = registry.page_logs(&operation.id, 100, 0)?;
            let mut value = operation_json(operation);
            value["logs"] = json!(logs);
            let preparation_id = value["result"]["preparation_id"]
                .as_str()
                .map(ToString::to_string)
                .or_else(|| match self.load_request(&operation_id).ok()? {
                    ReleaseRequest::Prepare { .. } => Some(operation_id.clone()),
                    ReleaseRequest::Publish { preparation_id } => Some(preparation_id),
                });
            if let Some(preparation_id) = preparation_id
                && let Ok(preparation) = self.preparation_status(&preparation_id)
            {
                value["preparation"] = preparation;
            }
            operations.push(value);
        }
        Ok(json!({"operations": operations}))
    }

    pub fn start_prepare(&self, bump: ReleaseBump) -> RefineResult<OperationHandle> {
        let plan = self.plan(bump)?;
        let operation = self.register_request(ReleaseRequest::Prepare {
            plan: Box::new(plan),
            goal_id: None,
        })?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn prepare_blocking(&self, bump: ReleaseBump) -> RefineResult<OperationHandle> {
        let plan = self.plan(bump)?;
        let operation = self.register_request(ReleaseRequest::Prepare {
            plan: Box::new(plan),
            goal_id: None,
        })?;
        self.run_or_fail(&operation.id)?;
        FileOperationRegistry::new(&self.runtime_root).status(&operation.id)
    }

    pub fn start_publish(
        &self,
        preparation_id: &str,
        confirmed: bool,
    ) -> RefineResult<OperationHandle> {
        self.require_confirmation(confirmed)?;
        self.resolve_trusted_preparation(preparation_id)?;
        let operation = self.register_request(ReleaseRequest::Publish {
            preparation_id: preparation_id.to_string(),
        })?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn publish_blocking(
        &self,
        preparation_id: &str,
        confirmed: bool,
    ) -> RefineResult<OperationHandle> {
        self.require_confirmation(confirmed)?;
        self.resolve_trusted_preparation(preparation_id)?;
        let operation = self.register_request(ReleaseRequest::Publish {
            preparation_id: preparation_id.to_string(),
        })?;
        self.run_or_fail(&operation.id)?;
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
        if matches!(request, ReleaseRequest::Publish { .. }) {
            self.require_confirmation(confirmed)?;
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
            ReleaseRequest::Prepare { plan, goal_id } => {
                let goal_id = self.queue_preparation_goal(operation_id, &plan, goal_id)?;
                json!({
                    "preparation_id": operation_id,
                    "goal_id": goal_id,
                    "plan": plan,
                    "review_required": true
                })
            }
            ReleaseRequest::Publish { preparation_id } => {
                let preparation = self.resolve_trusted_preparation(&preparation_id)?;
                let published = run_publication(&registry, operation_id, host, &preparation)?;
                json!({
                    "preparation_id": preparation_id,
                    "goal_id": preparation.goal_id,
                    "published": published
                })
            }
        };
        registry.finish_with_result(operation_id, OperationState::Succeeded, result)
    }

    fn queue_preparation_goal(
        &self,
        operation_id: &str,
        plan: &ReleasePlan,
        existing_goal_id: Option<String>,
    ) -> RefineResult<String> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let work_items = self.work_items()?;
        if let Some(goal_id) = existing_goal_id {
            let goal = work_items.show_goal_summary(&goal_id)?;
            match goal.goal.status {
                GoalStatus::Failed => {
                    stage(
                        &registry,
                        operation_id,
                        "queue_goal",
                        "Re-queueing the linked release preparation Goal",
                        Some(&goal_id),
                    )?;
                    work_items.transition_goal_status(&goal_id, GoalStatus::Todo)?;
                    return Ok(goal_id);
                }
                GoalStatus::Backlog | GoalStatus::Todo => {
                    work_items.start_goal_workflow(&goal_id)?;
                    return Ok(goal_id);
                }
                _ => {
                    return Err(RefineError::Conflict(format!(
                        "release preparation Goal {goal_id} is {}; retry it through its normal workflow",
                        goal.goal.status.as_str()
                    )));
                }
            }
        }

        stage(
            &registry,
            operation_id,
            "queue_goal",
            "Creating a normal Goal for agent-operated release preparation",
            None,
        )?;
        let name = format!("Prepare {}", plan.proposed_tag);
        let goal = work_items.create_goal_summary(&name, None)?;
        let goal_id = goal.goal.id.clone();
        let prompt = release_goal_prompt(plan);
        if let Err(error) = work_items
            .append_goal_round_summary(&goal_id, "Release workflow", &prompt)
            .and_then(|_| work_items.start_goal_workflow(&goal_id))
        {
            let _ = work_items.delete_goal_record(&goal_id);
            return Err(error);
        }
        self.write_request(
            operation_id,
            &ReleaseRequest::Prepare {
                plan: Box::new(plan.clone()),
                goal_id: Some(goal_id.clone()),
            },
        )?;
        stage(
            &registry,
            operation_id,
            "queued",
            "Release preparation Goal queued for the configured agent",
            Some(&goal_id),
        )?;
        Ok(goal_id)
    }

    fn preparation_status(&self, preparation_id: &str) -> RefineResult<Value> {
        let request = self.load_request(preparation_id)?;
        let ReleaseRequest::Prepare { plan, goal_id } = request else {
            return Err(RefineError::InvalidInput(format!(
                "operation {preparation_id} is not a release preparation"
            )));
        };
        let Some(goal_id) = goal_id else {
            return Ok(json!({"preparation_id": preparation_id, "plan": plan}));
        };
        let detail = self.work_items()?.show_goal_detail(&goal_id)?;
        let status = detail.get("status").cloned().unwrap_or(Value::Null);
        let branch = detail.get("branch_name").cloned().unwrap_or(Value::Null);
        let candidate_commit = detail
            .get("candidate_commit")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(json!({
            "preparation_id": preparation_id,
            "goal_id": goal_id,
            "plan": plan,
            "status": status,
            "branch": branch,
            "candidate_commit": candidate_commit,
            "rounds": detail.get("rounds").cloned().unwrap_or_else(|| json!([])),
            "review_url": format!("#/goals/{goal_id}"),
            "publishable": status == "done" && !candidate_commit.is_null()
        }))
    }

    fn resolve_trusted_preparation(
        &self,
        preparation_id: &str,
    ) -> RefineResult<TrustedPreparation> {
        let request = self.load_request(preparation_id)?;
        let ReleaseRequest::Prepare { plan, goal_id } = request else {
            return Err(RefineError::InvalidInput(
                "publication requires a persisted preparation operation id".to_string(),
            ));
        };
        let goal_id = goal_id.ok_or_else(|| {
            RefineError::Conflict("release preparation has not created its Goal yet".to_string())
        })?;
        let work_items = self.work_items()?;
        let goal = work_items.show_goal_summary(&goal_id)?;
        if goal.goal.status != GoalStatus::Done {
            return Err(RefineError::Conflict(format!(
                "release preparation Goal {goal_id} must be approved and done before publication; it is {}",
                goal.goal.status.as_str()
            )));
        }
        let detail = work_items.show_goal_detail(&goal_id)?;
        let required = |field: &str| {
            detail
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "release preparation Goal {goal_id} has no {field}"
                    ))
                })
        };
        Ok(TrustedPreparation {
            preparation_id: preparation_id.to_string(),
            goal_id: goal_id.clone(),
            version: plan.proposed_version,
            tag: plan.proposed_tag,
            branch: required("branch_name")?,
            target_branch: required("target_branch")?,
            candidate_commit: required("candidate_commit")?,
            release_notes: "RELEASE_NOTES.md".to_string(),
        })
    }

    fn work_items(&self) -> RefineResult<FileWorkItemService> {
        let refine_dir = prepare_refine_dir(&self.repo_root)?;
        Ok(FileWorkItemService::with_projection_cache(
            refine_dir,
            self.runtime_root.join("cache"),
        ))
    }

    fn require_confirmation(&self, confirmed: bool) -> RefineResult<()> {
        if confirmed {
            Ok(())
        } else {
            Err(RefineError::InvalidInput(
                "publishing is externally mutating and requires confirmed=true".to_string(),
            ))
        }
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
        self.write_request(&operation.id, &request)?;
        Ok(operation)
    }

    fn request_path(&self, operation_id: &str) -> PathBuf {
        self.runtime_root
            .join(RELEASE_REQUESTS_DIR)
            .join(format!("{operation_id}.json"))
    }

    fn write_request(&self, operation_id: &str, request: &ReleaseRequest) -> RefineResult<()> {
        let path = self.request_path(operation_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error("create release request directory"))?;
        }
        fs::write(
            &path,
            serde_json::to_vec_pretty(request).map_err(|error| {
                RefineError::Serialization(format!("failed to encode release request: {error}"))
            })?,
        )
        .map_err(io_error("write release request"))
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

    fn run_or_fail(&self, operation_id: &str) -> RefineResult<()> {
        let mut host = ShellReleaseHost::new(&self.repo_root);
        if let Err(error) = self.run_with_host(operation_id, &mut host) {
            let registry = FileOperationRegistry::new(&self.runtime_root);
            let _ = registry.fail_with_error(
                operation_id,
                json!({"code": "release_operation_failed", "message": error.to_string()}),
            );
            return Err(error);
        }
        Ok(())
    }

    fn spawn(&self, operation_id: String) {
        let service = self.clone();
        std::thread::spawn(move || {
            let _ = service.run_or_fail(&operation_id);
        });
    }
}

fn run_publication(
    registry: &FileOperationRegistry,
    operation_id: &str,
    host: &mut dyn ReleaseHost,
    preparation: &TrustedPreparation,
) -> RefineResult<PublishedRelease> {
    stage(
        registry,
        operation_id,
        "preflight",
        "Checking clean synchronized main, merge ancestry, version, tags, remote, and credentials",
        Some(&preparation.goal_id),
    )?;
    let preflight = host.preflight(preparation)?;
    stage(
        registry,
        operation_id,
        "local_tag",
        "Creating or validating the local semantic tag",
        Some(&preparation.goal_id),
    )?;
    host.ensure_local_tag(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "remote_tag",
        "Pushing or validating the remote semantic tag",
        Some(&preparation.goal_id),
    )?;
    host.ensure_remote_tag(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "github_release",
        "Creating or validating the GitHub release",
        Some(&preparation.goal_id),
    )?;
    host.ensure_github_release(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "delivery",
        "Observing deployment and package workflows to a terminal result",
        Some(&preparation.goal_id),
    )?;
    let deployment = host.observe_delivery(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "verify",
        "Verifying the published tag and GitHub release",
        Some(&preparation.goal_id),
    )?;
    let release_url = host.verify(preparation, &preflight)?;
    Ok(PublishedRelease {
        version: preparation.version.clone(),
        tag: preparation.tag.clone(),
        commit: preflight.main_commit,
        remote: preflight.remote,
        deployment,
        release_url,
        verified: true,
    })
}

fn stage(
    registry: &FileOperationRegistry,
    operation_id: &str,
    name: &str,
    message: &str,
    goal_id: Option<&str>,
) -> RefineResult<()> {
    registry.update_progress(operation_id, json!({"stage": name, "message": message}))?;
    registry.append_log(
        operation_id,
        LogEntry {
            datetime: String::new(),
            severity: "info".to_string(),
            category: "release".to_string(),
            message: message.to_string(),
            actor: Some("release-service".to_string()),
            goal_id: goal_id.map(ToString::to_string),
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
}

impl ShellReleaseHost {
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
        }
    }

    fn git(&self, args: &[&str]) -> RefineResult<String> {
        command_text(&self.repo_root, "git", args)
    }

    fn local_tag_commit(&self, tag: &str) -> RefineResult<Option<String>> {
        command_optional(
            &self.repo_root,
            "git",
            &[
                "rev-parse",
                "-q",
                "--verify",
                &format!("refs/tags/{tag}^{{}}"),
            ],
        )
    }

    fn remote_tag_commit(&self, remote: &str, tag: &str) -> RefineResult<Option<String>> {
        let output = self.git(&[
            "ls-remote",
            remote,
            &format!("refs/tags/{tag}"),
            &format!("refs/tags/{tag}^{{}}"),
        ])?;
        Ok(output
            .lines()
            .find(|line| line.ends_with("^{}"))
            .or_else(|| output.lines().next())
            .and_then(|line| line.split_whitespace().next())
            .map(ToString::to_string))
    }

    fn github_release(&self, tag: &str) -> RefineResult<Option<Value>> {
        let Some(output) = command_optional(
            &self.repo_root,
            "gh",
            &["release", "view", tag, "--json", "url,tagName"],
        )?
        else {
            return Ok(None);
        };
        serde_json::from_str(&output).map(Some).map_err(|error| {
            RefineError::Serialization(format!("failed to parse GitHub release state: {error}"))
        })
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

    fn preflight(
        &mut self,
        preparation: &TrustedPreparation,
    ) -> RefineResult<PublicationPreflight> {
        ensure_git_checkout(&self.repo_root)?;
        let branch = self.git(&["symbolic-ref", "--short", "HEAD"])?;
        if branch != preparation.target_branch {
            return Err(RefineError::Conflict(format!(
                "publication requires {}; current branch is {branch}",
                preparation.target_branch
            )));
        }
        if !self.git(&["status", "--porcelain"])?.is_empty() {
            return Err(RefineError::Conflict(
                "publication requires a clean target branch".to_string(),
            ));
        }
        let upstream = self.git(&[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])?;
        let (remote, upstream_branch) = upstream.split_once('/').ok_or_else(|| {
            RefineError::Conflict(format!("configured upstream {upstream} has no remote"))
        })?;
        self.git(&["fetch", "--no-tags", remote, upstream_branch])?;
        let main_commit = self.git(&["rev-parse", "HEAD"])?;
        let remote_commit = self.git(&["rev-parse", &format!("{remote}/{upstream_branch}")])?;
        if main_commit != remote_commit {
            return Err(RefineError::Conflict(format!(
                "publication requires synchronized {branch} and {upstream}"
            )));
        }
        let ancestry = Command::new("git")
            .args([
                "merge-base",
                "--is-ancestor",
                &preparation.candidate_commit,
                &main_commit,
            ])
            .current_dir(&self.repo_root)
            .status()
            .map_err(|error| RefineError::Io(format!("failed to check merge ancestry: {error}")))?;
        if !ancestry.success() {
            return Err(RefineError::Conflict(format!(
                "approved preparation commit {} is not merged into {branch}",
                preparation.candidate_commit
            )));
        }
        let version = read_package_version(&self.repo_root.join("Cargo.toml"))?;
        if version != preparation.version
            || preparation
                .tag
                .strip_prefix('v')
                .unwrap_or(&preparation.tag)
                != version
        {
            return Err(RefineError::Conflict(
                "merged version and semantic tag are not aligned with the trusted preparation"
                    .to_string(),
            ));
        }
        if !self.repo_root.join(&preparation.release_notes).is_file() {
            return Err(RefineError::Conflict(format!(
                "merged release notes {} were not found",
                preparation.release_notes
            )));
        }
        if let Some(commit) = self.local_tag_commit(&preparation.tag)?
            && commit != main_commit
        {
            return Err(RefineError::Conflict(format!(
                "local tag {} points at {commit}, expected {main_commit}",
                preparation.tag
            )));
        }
        if let Some(commit) = self.remote_tag_commit(remote, &preparation.tag)?
            && commit != main_commit
        {
            return Err(RefineError::Conflict(format!(
                "remote tag {} points at {commit}, expected {main_commit}",
                preparation.tag
            )));
        }
        command_text(&self.repo_root, "gh", &["auth", "status"])?;
        if let Some(release) = self.github_release(&preparation.tag)?
            && release.get("tagName").and_then(Value::as_str) != Some(&preparation.tag)
        {
            return Err(RefineError::Conflict(format!(
                "GitHub release for {} has conflicting identity",
                preparation.tag
            )));
        }
        Ok(PublicationPreflight {
            main_commit,
            remote: remote.to_string(),
            branch,
        })
    }

    fn ensure_local_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()> {
        match self.local_tag_commit(&preparation.tag)? {
            Some(commit) if commit == preflight.main_commit => Ok(()),
            Some(commit) => Err(RefineError::Conflict(format!(
                "local tag {} points at {commit}, expected {}",
                preparation.tag, preflight.main_commit
            ))),
            None => self
                .git(&[
                    "tag",
                    "-a",
                    &preparation.tag,
                    &preflight.main_commit,
                    "-m",
                    &format!("Release {}", preparation.version),
                ])
                .map(|_| ()),
        }
    }

    fn ensure_remote_tag(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<()> {
        match self.remote_tag_commit(&preflight.remote, &preparation.tag)? {
            Some(commit) if commit == preflight.main_commit => Ok(()),
            Some(commit) => Err(RefineError::Conflict(format!(
                "remote tag {} points at {commit}, expected {}",
                preparation.tag, preflight.main_commit
            ))),
            None => self
                .git(&["push", &preflight.remote, &preparation.tag])
                .map(|_| ()),
        }
    }

    fn ensure_github_release(
        &mut self,
        preparation: &TrustedPreparation,
        _preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        if let Some(release) = self.github_release(&preparation.tag)? {
            return release
                .get("url")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .ok_or_else(|| {
                    RefineError::Conflict("existing GitHub release has no URL".to_string())
                });
        }
        command_text(
            &self.repo_root,
            "gh",
            &[
                "release",
                "create",
                &preparation.tag,
                "--title",
                &preparation.tag,
                "--notes-file",
                &preparation.release_notes,
                "--verify-tag",
            ],
        )?;
        self.github_release(&preparation.tag)?
            .and_then(|release| {
                release
                    .get("url")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .ok_or_else(|| {
                RefineError::Degraded("created GitHub release could not be read back".to_string())
            })
    }

    fn observe_delivery(
        &mut self,
        _preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        let list = || {
            command_text(
                &self.repo_root,
                "gh",
                &[
                    "run",
                    "list",
                    "--commit",
                    &preflight.main_commit,
                    "--limit",
                    "20",
                    "--json",
                    "databaseId,name,status,conclusion,url",
                ],
            )
        };
        let configured = delivery_workflows_configured(&self.repo_root)?;
        let mut runs: Vec<Value> = Vec::new();
        for attempt in 0..3 {
            runs = serde_json::from_str(&list()?).map_err(|error| {
                RefineError::Serialization(format!("failed to parse workflow runs: {error}"))
            })?;
            if !runs.is_empty() || !configured || attempt == 2 {
                break;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        if runs.is_empty() {
            if configured {
                return Err(RefineError::Degraded(
                    "delivery workflows are configured, but GitHub reported no run for the release commit"
                        .to_string(),
                ));
            }
            return Ok(
                "No deployment or package workflows are configured for this release commit."
                    .to_string(),
            );
        }
        for run in &runs {
            if run.get("status").and_then(Value::as_str) != Some("completed") {
                let id = run
                    .get("databaseId")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        RefineError::Serialization("workflow run has no databaseId".to_string())
                    })?;
                command_text(
                    &self.repo_root,
                    "gh",
                    &["run", "watch", &id.to_string(), "--exit-status"],
                )?;
            }
        }
        runs = serde_json::from_str(&list()?).map_err(|error| {
            RefineError::Serialization(format!("failed to parse terminal workflow runs: {error}"))
        })?;
        for run in &runs {
            let status = run.get("status").and_then(Value::as_str).unwrap_or("");
            let conclusion = run.get("conclusion").and_then(Value::as_str).unwrap_or("");
            if status != "completed" || !matches!(conclusion, "success" | "neutral" | "skipped") {
                return Err(RefineError::Degraded(format!(
                    "release workflow {} ended with status {status} and conclusion {conclusion}",
                    run.get("name").and_then(Value::as_str).unwrap_or("unknown")
                )));
            }
        }
        serde_json::to_string(&runs).map_err(|error| {
            RefineError::Serialization(format!("failed to encode workflow results: {error}"))
        })
    }

    fn verify(
        &mut self,
        preparation: &TrustedPreparation,
        preflight: &PublicationPreflight,
    ) -> RefineResult<String> {
        let remote = self
            .remote_tag_commit(&preflight.remote, &preparation.tag)?
            .ok_or_else(|| {
                RefineError::Degraded("published remote tag was not found".to_string())
            })?;
        if remote != preflight.main_commit {
            return Err(RefineError::Conflict(format!(
                "published tag resolves to {remote}, expected {}",
                preflight.main_commit
            )));
        }
        let release = self.github_release(&preparation.tag)?.ok_or_else(|| {
            RefineError::Degraded("published GitHub release was not found".to_string())
        })?;
        if release.get("tagName").and_then(Value::as_str) != Some(&preparation.tag) {
            return Err(RefineError::Conflict(
                "published GitHub release tag does not match".to_string(),
            ));
        }
        release
            .get("url")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| RefineError::Degraded("published GitHub release has no URL".to_string()))
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

fn release_goal_prompt(plan: &ReleasePlan) -> String {
    let changes = plan
        .changes
        .iter()
        .map(|change| format!("- {} {}", change.commit, change.summary))
        .collect::<Vec<_>>()
        .join("\n");
    let goals = plan.completed_goals.join("\n");
    format!(
        "Prepare the reviewable semantic release candidate described by this trusted ReleasePlan.\n\n\
Current version: {}\nProposed version: {}\nProposed tag: {}\nPrevious tag: {}\n\
Version-bearing files detected: {}\nDocumentation files detected: {}\n\n\
Completed Goals:\n{}\n\nCommits since the prior release:\n{}\n\n\
Analyze the completed Goals and commits. Update every applicable version-bearing file and lockfile, \
write release notes, preserve established documentation formats, and update story, runbooks, migration, \
or other affected documentation only where the actual changes require it. Identify breaking changes and \
write migration guidance when needed. Run `cargo run --manifest-path xtask/Cargo.toml -- release-check` \
and report deterministic command outcomes. Do not tag, push a tag, create a GitHub release, or publish externally. \
Use this normal Goal worktree and leave the candidate ready for the standard review and approval workflow.",
        plan.current_version,
        plan.proposed_version,
        plan.proposed_tag,
        plan.previous_tag.as_deref().unwrap_or("none"),
        plan.version_files.join(", "),
        plan.documentation_files.join(", "),
        if goals.is_empty() { "- None" } else { &goals },
        if changes.is_empty() {
            "- None"
        } else {
            &changes
        },
    )
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
            "cargo clippy --all-targets -- -D warnings".to_string(),
            "cargo test --lib --bins -- --test-threads=1".to_string(),
            "cargo build --release --locked".to_string(),
            "cargo run --manifest-path xtask/Cargo.toml -- release-check".to_string(),
        ]);
    }
    gates
}

fn completed_goal_summaries(root: &Path) -> RefineResult<Vec<String>> {
    let candidates = [
        root.join(".refine/goals"),
        refine_dir_for_target_root(root)?.join("goals"),
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

fn delivery_workflows_configured(root: &Path) -> RefineResult<bool> {
    let workflows = root.join(".github/workflows");
    if !workflows.is_dir() {
        return Ok(false);
    }
    for entry in fs::read_dir(workflows).map_err(io_error("read workflow directory"))? {
        let entry = entry.map_err(io_error("inspect workflow entry"))?;
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let body = fs::read_to_string(&path)
            .map_err(io_error("read workflow definition"))?
            .to_ascii_lowercase();
        if ["deploy", "publish", "package", "release"]
            .iter()
            .any(|term| name.contains(term) || body.contains(term))
        {
            return Ok(true);
        }
    }
    Ok(false)
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

fn ensure_git_checkout(root: &Path) -> RefineResult<()> {
    if !root.join(".git").exists() {
        return Err(RefineError::InvalidInput(format!(
            "{} is not a Git checkout",
            root.display()
        )));
    }
    Ok(())
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
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn semantic_bumps_are_explicit_and_deterministic() {
        assert_eq!(bump_version("4.2.9", ReleaseBump::Major).unwrap(), "5.0.0");
        assert_eq!(bump_version("4.2.9", ReleaseBump::Minor).unwrap(), "4.3.0");
        assert_eq!(bump_version("4.2.9", ReleaseBump::Patch).unwrap(), "4.2.10");
        assert!(bump_version("4.2", ReleaseBump::Patch).is_err());
    }

    #[derive(Default)]
    struct FakeHost {
        calls: Vec<&'static str>,
        fail_at: Option<&'static str>,
        main_commit: String,
    }

    impl FakeHost {
        fn call(&mut self, name: &'static str) -> RefineResult<()> {
            self.calls.push(name);
            if self.fail_at == Some(name) {
                Err(RefineError::Degraded(format!("{name} failed")))
            } else {
                Ok(())
            }
        }
    }

    impl ReleaseHost for FakeHost {
        fn plan(&mut self, _bump: ReleaseBump) -> RefineResult<ReleasePlan> {
            unreachable!()
        }

        fn preflight(
            &mut self,
            preparation: &TrustedPreparation,
        ) -> RefineResult<PublicationPreflight> {
            self.call("preflight")?;
            Ok(PublicationPreflight {
                main_commit: if self.main_commit.is_empty() {
                    format!("merge-of-{}", preparation.candidate_commit)
                } else {
                    self.main_commit.clone()
                },
                remote: "upstream".into(),
                branch: "main".into(),
            })
        }

        fn ensure_local_tag(
            &mut self,
            _preparation: &TrustedPreparation,
            _preflight: &PublicationPreflight,
        ) -> RefineResult<()> {
            self.call("local_tag")
        }

        fn ensure_remote_tag(
            &mut self,
            _preparation: &TrustedPreparation,
            _preflight: &PublicationPreflight,
        ) -> RefineResult<()> {
            self.call("remote_tag")
        }

        fn ensure_github_release(
            &mut self,
            _preparation: &TrustedPreparation,
            _preflight: &PublicationPreflight,
        ) -> RefineResult<String> {
            self.call("github_release")?;
            Ok("https://example.test/release".into())
        }

        fn observe_delivery(
            &mut self,
            _preparation: &TrustedPreparation,
            _preflight: &PublicationPreflight,
        ) -> RefineResult<String> {
            self.call("delivery")?;
            Ok("success".into())
        }

        fn verify(
            &mut self,
            _preparation: &TrustedPreparation,
            _preflight: &PublicationPreflight,
        ) -> RefineResult<String> {
            self.call("verify")?;
            Ok("https://example.test/release".into())
        }
    }

    fn trusted_preparation() -> TrustedPreparation {
        TrustedPreparation {
            preparation_id: "operation-prepare".into(),
            goal_id: "GOAL1".into(),
            version: "1.1.0".into(),
            tag: "v1.1.0".into(),
            branch: "refine/GOAL1/round-1".into(),
            target_branch: "main".into(),
            candidate_commit: "candidate".into(),
            release_notes: "RELEASE_NOTES.md".into(),
        }
    }

    fn release_plan() -> ReleasePlan {
        ReleasePlan {
            current_version: "1.0.0".into(),
            proposed_version: "1.1.0".into(),
            proposed_tag: "v1.1.0".into(),
            previous_tag: Some("v1.0.0".into()),
            bump: ReleaseBump::Minor,
            changes: vec![ReleaseChange {
                commit: "abc".into(),
                summary: "Reviewed change".into(),
                breaking: false,
            }],
            completed_goals: vec!["GOAL0: Reviewed work".into()],
            breaking_changes: vec![],
            version_files: vec!["Cargo.toml".into(), "Cargo.lock".into()],
            documentation_files: vec!["RELEASE_NOTES.md".into(), "docs/story.md".into()],
            gates: vec!["cargo run --manifest-path xtask/Cargo.toml -- release-check".into()],
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-release-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn test_registry(name: &str) -> (PathBuf, FileOperationRegistry, OperationHandle) {
        let root = std::env::temp_dir().join(format!(
            "refine-release-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        let registry = FileOperationRegistry::new(&root);
        let operation = registry.register("release:publish").unwrap();
        (root, registry, operation)
    }

    #[test]
    fn preparation_queues_a_normal_goal_with_the_trusted_plan() {
        let root = unique_temp_dir("goal-boundary");
        let repo = root.join("repo");
        let runtime = root.join("run");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        let service = FileReleaseService::new(&repo, &runtime);
        let operation = service
            .register_request(ReleaseRequest::Prepare {
                plan: Box::new(release_plan()),
                goal_id: None,
            })
            .unwrap();
        let mut host = FakeHost::default();

        let finished = service.run_with_host(&operation.id, &mut host).unwrap();

        let goal_id = finished.result["goal_id"].as_str().unwrap();
        let detail = service
            .work_items()
            .unwrap()
            .show_goal_detail(goal_id)
            .unwrap();
        assert_eq!(detail["status"], "todo");
        assert_eq!(detail["rounds"][0]["reporter"], "Release workflow");
        let prompt = detail["rounds"][0]["prompt"].as_str().unwrap();
        assert!(prompt.contains("Current version: 1.0.0"));
        assert!(prompt.contains("Proposed version: 1.1.0"));
        assert!(prompt.contains("GOAL0: Reviewed work"));
        assert!(prompt.contains("Do not tag, push a tag"));
        assert!(!runtime.join("releases/worktrees").exists());
        assert!(host.calls.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publication_uses_merge_commit_and_configured_remote() {
        let (root, registry, operation) = test_registry("merge");
        let mut host = FakeHost::default();
        let published =
            run_publication(&registry, &operation.id, &mut host, &trusted_preparation()).unwrap();
        assert_eq!(published.commit, "merge-of-candidate");
        assert_eq!(published.remote, "upstream");
        assert_eq!(
            host.calls,
            [
                "preflight",
                "local_tag",
                "remote_tag",
                "github_release",
                "delivery",
                "verify"
            ]
        );
        assert!(published.verified);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failure_after_tag_push_resumes_from_idempotent_stages() {
        let (root, registry, first) = test_registry("resume");
        let mut failed = FakeHost {
            fail_at: Some("github_release"),
            ..FakeHost::default()
        };
        assert!(
            run_publication(&registry, &first.id, &mut failed, &trusted_preparation()).is_err()
        );
        assert_eq!(
            failed.calls,
            ["preflight", "local_tag", "remote_tag", "github_release"]
        );
        let retry = registry.register("release:publish").unwrap();
        let mut resumed = FakeHost::default();
        let published =
            run_publication(&registry, &retry.id, &mut resumed, &trusted_preparation()).unwrap();
        assert!(published.verified);
        assert_eq!(resumed.calls.last(), Some(&"verify"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn conflicting_tag_and_failed_workflow_stop_verification() {
        for stage_name in ["local_tag", "remote_tag", "delivery"] {
            let (root, registry, operation) = test_registry(stage_name);
            let mut host = FakeHost {
                fail_at: Some(stage_name),
                ..FakeHost::default()
            };
            assert!(
                run_publication(&registry, &operation.id, &mut host, &trusted_preparation())
                    .is_err()
            );
            assert!(!host.calls.contains(&"verify"));
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn publication_requires_explicit_confirmation_before_identity_lookup() {
        let root = std::env::temp_dir().join(format!(
            "refine-release-confirmation-{}",
            std::process::id()
        ));
        let service = FileReleaseService::new(&root, &root);
        let error = service
            .start_publish("browser-controlled-value", false)
            .unwrap_err();
        assert!(error.to_string().contains("confirmed=true"));
        let _ = fs::remove_dir_all(root);
    }
}
