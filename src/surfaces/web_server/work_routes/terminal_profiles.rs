use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::work_items::FileWorkItemService;
use crate::error::RefineError;
use crate::infrastructure::git::with_repository_git_lock;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::infrastructure::runtime::checkout::active_refine_paths;

use super::InProcessWebServer;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TerminalSessionLaunchSurface {
    Toolbar,
    #[default]
    Cli,
}

impl TerminalSessionLaunchSurface {
    pub(crate) fn from_request(value: Option<&Value>) -> Result<Self, RefineError> {
        match value {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(Value::String(value)) if value.trim().eq_ignore_ascii_case("toolbar") => {
                Ok(Self::Toolbar)
            }
            Some(Value::String(value)) if value.trim().eq_ignore_ascii_case("cli") => Ok(Self::Cli),
            Some(Value::String(value)) => Err(RefineError::InvalidInput(format!(
                "unknown terminal session surface {}",
                value.trim()
            ))),
            Some(_) => Err(RefineError::InvalidInput(
                "terminal session surface must be toolbar or cli".to_string(),
            )),
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Toolbar => "toolbar",
            Self::Cli => "cli",
        }
    }
}

pub(crate) fn terminal_profile_prompt(
    server: &InProcessWebServer,
    profile: &str,
    surface: TerminalSessionLaunchSurface,
    goal_id: Option<&str>,
    feature_id: Option<&str>,
    supplemental_prompt: Option<&str>,
) -> Result<String, RefineError> {
    use crate::application::templates::{TemplateScope, TemplateValue};
    let refine_dir = server.current_refine_dir()?;
    let _templates = TemplateScope::inherit_or_root(refine_dir.as_deref())?;
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
    let fragment = |template: PromptTemplate| {
        TemplateValue::Template(format!("{{{{templates.{}}}}}", template.id()))
    };
    let mut values = TemplateScope::literals(&[
        ("workflow_context", ""),
        ("active_refine", ""),
        ("goal_attachment", ""),
        ("feature_attachment", ""),
        ("profile_context", ""),
        ("supplemental_attachment", ""),
    ]);
    values.insert("instructions".into(), fragment(template));
    if surface == TerminalSessionLaunchSurface::Toolbar && matches!(profile, "agent" | "plan") {
        values.insert(
            "workflow_context".into(),
            fragment(PromptTemplate::TerminalProfileToolbarAgentWorkflow),
        );
    } else if surface == TerminalSessionLaunchSurface::Cli && profile == "agent" {
        values.insert(
            "workflow_context".into(),
            fragment(PromptTemplate::TerminalProfileGeneralAgentWorkflow),
        );
    }
    if profile == "agent" {
        let (executable, checkout) = active_refine_paths()?;
        values.extend(TemplateScope::literals(&[
            ("executable", &executable.display().to_string()),
            ("checkout", &checkout.display().to_string()),
        ]));
        values.insert(
            "active_refine".into(),
            fragment(PromptTemplate::TerminalProfileActiveRefine),
        );
    }
    let projection = (goal_id.is_some() || feature_id.is_some())
        .then(|| server.current_projection_shared())
        .transpose()?;
    if let Some(goal_id) = goal_id {
        let goal = projection
            .as_ref()
            .expect("loaded")
            .goals
            .get(goal_id)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} was not found")))?;
        let context = if let Some(refine_dir) = &refine_dir {
            FileWorkItemService::new(refine_dir).show_goal_detail(goal_id)?
        } else {
            json!({"id":goal.goal.id,"name":goal.goal.name,"status":goal.goal.status})
        };
        values.extend(TemplateScope::context_values(&json!({"goal":context})));
        values.insert(
            "goal_context".into(),
            TemplateValue::Literal(
                serde_json::to_string_pretty(&context)
                    .map_err(|e| RefineError::Serialization(e.to_string()))?,
            ),
        );
        values.insert(
            "goal_attachment".into(),
            fragment(PromptTemplate::TerminalProfileAttachedGoal),
        );
    }
    if let Some(feature_id) = feature_id {
        let feature = projection
            .as_ref()
            .expect("loaded")
            .features
            .get(feature_id)
            .ok_or_else(|| RefineError::NotFound(format!("Feature {feature_id} was not found")))?;
        let context = json!({"id":feature.feature.id,"name":feature.feature.name,"description":feature.feature.description,"status":feature.status,"goal_ids":feature.goal_ids,"updated":feature.feature.updated});
        values.insert(
            "feature_context".into(),
            TemplateValue::Literal(serde_json::to_string_pretty(&context).unwrap()),
        );
        values.insert(
            "feature_attachment".into(),
            fragment(PromptTemplate::TerminalProfileAttachedFeature),
        );
    }
    if profile == "plan" {
        values.insert(
            "profile_context".into(),
            fragment(PromptTemplate::TerminalProfilePlan),
        );
    } else if profile == "goal" {
        values.insert(
            "profile_context".into(),
            fragment(PromptTemplate::TerminalProfileGoalDiagnostic),
        );
    }
    if let Some(prompt) = supplemental_prompt {
        values.insert(
            "supplemental_context".into(),
            TemplateValue::Literal(prompt.into()),
        );
        values.insert(
            "supplemental_attachment".into(),
            fragment(PromptTemplate::TerminalProfileSupplementalContext),
        );
    }
    TemplateScope::render(&PromptTemplate::TerminalSession.id(), values)
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
