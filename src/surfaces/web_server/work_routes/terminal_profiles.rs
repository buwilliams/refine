use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::process::supervisor::errors::RefineError;
use crate::prompts::{PromptEngine, PromptTemplate};
use crate::tools::host::deployed_update::active_refine_paths;
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::product::work_items::FileWorkItemService;

use super::InProcessWebServer;

const GENERAL_AGENT_WORKFLOW_CONTRACT: &str = concat!(
    "Treat Refine as the execution path for repository changes. You may inspect source, ",
    "runtime state, logs, Git history, and other evidence directly to understand the request, ",
    "answer conversational questions, and prepare precise work. When the request calls for ",
    "implementation or another repository change, do not modify the repository ad hoc in this ",
    "session. Autonomously translate the desired outcome and your findings into a complete Refine ",
    "Goal with appropriate metadata and an actionable Round containing the relevant behavior, ",
    "constraints, evidence, and verification, then use supported Refine interfaces to make the ",
    "Goal eligible for workflow execution; do not require the user to recite lifecycle commands. ",
    "When continuing work after an unsuccessful recorded attempt, preserve that attempt, append a ",
    "new Round containing the relevant findings and actionable next-step guidance, and use supported ",
    "Refine interfaces to return the Goal to an eligible workflow state. Honor Refine's confirmation ",
    "and audit boundaries: never directly edit durable Goal state, conceal failures, approve or merge ",
    "on the user's behalf, destructively discard retained work, or begin ongoing supervision unless ",
    "the user requests it.",
);

pub(crate) fn terminal_profile_prompt(
    server: &InProcessWebServer,
    profile: &str,
    goal_id: Option<&str>,
    feature_id: Option<&str>,
    supplemental_prompt: Option<&str>,
) -> Result<String, RefineError> {
    let template = match profile {
        "agent" => PromptTemplate::ChatAgent,
        "plan" => PromptTemplate::ChatPlan,
        "goal" => PromptTemplate::ChatGoal,
        "standalone" => PromptTemplate::ChatStandalone,
        _ => {
            return Err(RefineError::InvalidInput(format!(
                "terminal profile {profile} does not launch an agent"
            )));
        }
    };
    let mut sections = vec![PromptEngine::load(template).trim().to_string()];
    if profile == "agent" {
        sections.push(GENERAL_AGENT_WORKFLOW_CONTRACT.to_string());
        let (executable, checkout) = active_refine_paths()?;
        sections.push(format!(
            "Active Refine executable: `{}`. Resolved Refine source checkout: `{}`. If `refine` is absent from PATH, run the checkout-local `./r` from that checkout.",
            executable.display(),
            checkout.display(),
        ));
    }
    let projection = (goal_id.is_some() || feature_id.is_some())
        .then(|| server.current_projection())
        .transpose()?;
    if let Some(goal_id) = goal_id {
        let goal = projection
            .as_ref()
            .expect("projection loaded for attached Goal")
            .goals
            .get(goal_id)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} was not found")))?;
        let context_value = match server.current_refine_dir()? {
            Some(refine_dir) => FileWorkItemService::new(refine_dir).show_goal_detail(goal_id)?,
            None => json!({
                "id": goal.goal.id,
                "name": goal.goal.name,
                "status": goal.goal.status,
                "priority": goal.goal.priority,
                "reporter": goal.goal.reporter,
                "assignee": goal.goal.assignee,
                "round_count": goal.goal.round_count,
                "feature_id": goal.goal.feature_id,
                "node_id": goal.goal.node_id,
                "updated": goal.goal.updated,
            }),
        };
        let context = serde_json::to_string_pretty(&context_value).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Goal context: {error}"))
        })?;
        sections.push(format!("Attached Refine Goal context:\n{context}"));
    }
    if let Some(feature_id) = feature_id {
        let feature = projection
            .as_ref()
            .expect("projection loaded for attached Feature")
            .features
            .get(feature_id)
            .ok_or_else(|| RefineError::NotFound(format!("Feature {feature_id} was not found")))?;
        let context = serde_json::to_string_pretty(&json!({
            "id": feature.feature.id,
            "name": feature.feature.name,
            "description": feature.feature.description,
            "status": feature.status,
            "goal_ids": feature.goal_ids,
            "updated": feature.feature.updated,
        }))
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode Feature context: {error}"))
        })?;
        sections.push(format!("Attached Refine Feature context:\n{context}"));
    }
    if profile == "plan" {
        sections.push(
            "Use Refine's CLI when the user asks you to persist the resulting Feature or Goals."
                .to_string(),
        );
    } else if profile == "goal" {
        sections.push(
            "This is a diagnostic session, not the Goal's workflow-owned implementation process. Inspect the attached Goal's recorded rounds, logs, failure, Governance, Quality, Git, and repository evidence before drawing conclusions. Do not change durable Goal state, submit a recovery round, or modify source unless the user explicitly asks you to do so."
                .to_string(),
        );
    }
    if let Some(prompt) = supplemental_prompt {
        sections.push(format!("User-provided starting context:\n{prompt}"));
    }
    Ok(sections.join("\n\n"))
}

pub(super) fn create_terminal_standalone_worktree(
    target_root: &Path,
    runtime_root: &Path,
) -> Result<Value, RefineError> {
    let worktree_id = Uuid::new_v4().to_string();
    let branch = format!("refine/standalone/{worktree_id}");
    let git = FileGitWorktreeService::with_runtime_root(target_root, runtime_root);
    let target = git
        .git_path("refine-standalone-worktrees")?
        .join(&worktree_id);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create standalone worktree directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let path = with_repository_git_lock(target_root, || git.ensure_worktree(&branch, &target))?;
    Ok(json!({"branch": branch, "path": path}))
}

pub(super) fn resume_terminal_standalone_worktree(
    target_root: &Path,
    runtime_root: &Path,
    worktree: &Value,
) -> Result<Value, RefineError> {
    let branch = worktree
        .get("branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("refine/standalone/"))
        .ok_or_else(|| {
            RefineError::InvalidInput(
                "standalone worktree branch must be owned by Refine".to_string(),
            )
        })?;
    let requested = worktree
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            RefineError::InvalidInput("standalone worktree path is required".to_string())
        })?;
    let git = FileGitWorktreeService::with_runtime_root(target_root, runtime_root);
    let allowed_root = git.git_path("refine-standalone-worktrees")?;
    let canonical = requested.canonicalize().map_err(|error| {
        RefineError::NotFound(format!(
            "standalone worktree {} is not available: {error}",
            requested.display()
        ))
    })?;
    let canonical_allowed_root = allowed_root.canonicalize().map_err(|error| {
        RefineError::NotFound(format!(
            "standalone worktree root {} is not available: {error}",
            allowed_root.display()
        ))
    })?;
    if !canonical.starts_with(&canonical_allowed_root) {
        return Err(RefineError::InvalidInput(format!(
            "standalone worktree {} is outside Refine's worktree root",
            canonical.display()
        )));
    }
    let status = git.inspect(canonical.to_str().ok_or_else(|| {
        RefineError::InvalidInput("standalone worktree path is not valid UTF-8".to_string())
    })?)?;
    let inspected_root = PathBuf::from(&status.root)
        .canonicalize()
        .map_err(|error| {
            RefineError::NotFound(format!(
                "standalone worktree root {} is not available: {error}",
                status.root
            ))
        })?;
    if inspected_root != canonical {
        return Err(RefineError::InvalidInput(format!(
            "standalone worktree path {} is not the worktree root",
            canonical.display()
        )));
    }
    if status.branch.as_deref() != Some(branch) {
        return Err(RefineError::InvalidInput(format!(
            "standalone worktree {} is checked out on {}, not {branch}",
            canonical.display(),
            status.branch.as_deref().unwrap_or("a detached HEAD")
        )));
    }
    Ok(json!({"branch": branch, "path": canonical.display().to_string()}))
}

pub(super) fn cleanup_failed_terminal_worktree(target_root: &Path, worktree: &Value) {
    let Some(path) = worktree.get("path").and_then(Value::as_str) else {
        return;
    };
    let Some(branch) = worktree.get("branch").and_then(Value::as_str) else {
        return;
    };
    let git = FileGitWorktreeService::new(target_root);
    let path = PathBuf::from(path);
    let _ = with_repository_git_lock(target_root, || {
        if path.exists() {
            git.remove_worktree(&path, true)?;
        }
        let _ = git.delete_branch(branch, true);
        Ok(())
    });
}
