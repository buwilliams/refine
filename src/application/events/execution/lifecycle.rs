//! Application ownership and authority for Skills that precede Plan admission.
//! The invocation owns this workspace; it never supplies implementation Git refs.
use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, ManagedWorktree};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;

mod admission;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LifecycleWorkspace {
    pub invocation_id: String,
    pub occurrence_key: String,
    pub repository: PathBuf,
    pub goal_id: String,
    pub round_idx: Option<usize>,
    pub request: Value,
    pub node_id: String,
    pub source: String,
    pub authority: LifecycleAuthority,
    pub target_ref: String,
    pub source_commit: String,
    pub branch: String,
    pub path: PathBuf,
    pub agent_subpath: String,
    pub launch_ready: bool,
    pub inputs: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LifecycleAuthority {
    Transition {
        pending: Value,
    },
    Occurrence {
        occurrence: Value,
    },
    WorkflowTransition {
        from: String,
        to: String,
        generation: u64,
        workflow_revision: u64,
    },
}

pub(super) fn applicable(event: &EventDefinition, context: &InvocationContext) -> bool {
    if event.kind != EventKind::System || context.goal_id.is_none() {
        return false;
    }
    if event
        .source
        .as_deref()
        .is_some_and(|s| s.ends_with(".error") || s.ends_with(".success"))
        && context.data["goal"]["branch_name"].is_null()
    {
        return true;
    }
    match event.source.as_deref() {
        Some(
            "workflow.backlog.enter"
            | "workflow.backlog.exit"
            | "workflow.todo.enter"
            | "workflow.todo.exit",
        ) => true,
        Some(
            "workflow.done.enter"
            | "workflow.done.exit"
            | "workflow.review.enter"
            | "workflow.review.exit"
            | "workflow.failed.enter"
            | "workflow.failed.exit"
            | "workflow.cancelled.enter"
            | "workflow.cancelled.exit",
        ) => {
            // Stopping before Plan does not create an implementation workspace.
            // Its hooks have the same occurrence ownership as Backlog/Todo work.
            // Existing implementation evidence must still use normal admission.
            let goal = &context.data["goal"];
            ["branch_name", "base_commit", "candidate_commit"]
                .iter()
                .all(|key| goal[key].is_null())
                && goal["rounds"]
                    .as_array()
                    .and_then(|rounds| rounds.last())
                    .is_none_or(|round| round["implementation_plan"]["state"].is_null())
        }
        _ => false,
    }
}

fn unavailable(reason: &str) -> RefineError {
    RefineError::Degraded(format!(
        "lifecycle Skill workspace unavailable: {reason}; retained work and results were preserved. Recover with a new authorized invocation"
    ))
}

fn request(goal: &Value) -> Value {
    goal["rounds"].as_array().and_then(|rounds| rounds.last()).map(|round| {
        json!({"created":round["created"], "prompt":round["prompt"], "reporter":round["reporter"], "assignee":round["assignee"]})
    }).unwrap_or(Value::Null)
}

impl LifecycleWorkspace {
    pub(super) fn validate_authority(
        &self,
        context: &InvocationContext,
        root: &Path,
    ) -> RefineResult<()> {
        let goal = FileWorkItemService::new(root).show_goal_detail(&self.goal_id)?;
        let round = goal["rounds"]
            .as_array()
            .and_then(|rounds| rounds.len().checked_sub(1));
        if context.goal_id.as_deref() != Some(&self.goal_id)
            || context.round_idx != self.round_idx
            || round != self.round_idx
            || request(&goal) != self.request
        {
            return Err(unavailable("Goal Round or authored request changed"));
        }
        if context.node_id != self.node_id
            || !crate::application::fleet::nodes::node_ids_match(
                goal["node_id"].as_str().unwrap_or("default"),
                &self.node_id,
            )
        {
            return Err(unavailable("Goal node changed"));
        }
        if matches!(goal["status"].as_str(), Some("cancelled" | "failed"))
            && goal["status"] != context.data["goal"]["status"]
        {
            return Err(unavailable("Goal was cancelled or failed after admission"));
        }
        match &self.authority {
            LifecycleAuthority::WorkflowTransition {
                from,
                to,
                generation,
                workflow_revision,
            } => {
                if from != "todo"
                    || to != "plan"
                    || goal["status"] != *from
                    || goal["event_generation"].as_u64().unwrap_or(0) != *generation
                    || context.workflow_revision != Some(*workflow_revision)
                    || ![
                        "workflow.todo.enter",
                        "workflow.todo.success",
                        "workflow.todo.exit",
                    ]
                    .contains(&self.source.as_str())
                {
                    return Err(unavailable("claimed lifecycle transition changed"));
                }
                FileWorkItemService::new(root).verify_workflow_attempt(
                    &self.goal_id,
                    crate::application::work_items::WorkflowStepAuthority {
                        round_idx: self
                            .round_idx
                            .ok_or_else(|| unavailable("missing claimed Round"))?,
                        workflow_revision: *workflow_revision,
                        generation: *generation,
                    },
                    crate::model::workflow::GoalStatus::Todo,
                    &self.node_id,
                )?;
            }
            LifecycleAuthority::Transition { pending } => {
                if goal["pending_event_transition"] != *pending
                    || pending["state"] != "pending"
                    || pending["revision"] != goal["workflow_revision"]
                    || pending["generation"].as_u64().map_or(
                        pending["revision"] != goal["workflow_revision"],
                        |generation| {
                            generation != goal["event_generation"].as_u64().unwrap_or(0)
                                || pending["candidate_commit"] != goal["candidate_commit"]
                        },
                    )
                    || pending["from"] != goal["status"]
                    || ![
                        format!(
                            "workflow.{}.enter",
                            pending["from"].as_str().unwrap_or_default()
                        ),
                        format!(
                            "workflow.{}.exit",
                            pending["from"].as_str().unwrap_or_default()
                        ),
                        format!(
                            "workflow.{}.success",
                            pending["from"].as_str().unwrap_or_default()
                        ),
                    ]
                    .contains(&self.source)
                {
                    return Err(unavailable(
                        "transition authority changed or was superseded",
                    ));
                }
            }
            LifecycleAuthority::Occurrence { occurrence } => {
                let edge = self.source.rsplit('.').next().unwrap_or_default();
                if !["enter", "exit", "success", "error"].contains(&edge)
                    || (edge == "error" && occurrence["error"] != true)
                {
                    return Err(unavailable("invalid lifecycle outcome occurrence"));
                }
                let side = if edge == "enter" { "to" } else { "from" };
                let expected = format!(
                    "workflow.{}.{}",
                    occurrence[side].as_str().unwrap_or_default(),
                    edge
                );
                if self.source != expected
                    || occurrence["generation"].as_u64().unwrap_or(0)
                        != goal["event_generation"].as_u64().unwrap_or(0)
                    || !goal["workflow_events"]
                        .as_array()
                        .is_some_and(|items| items.contains(occurrence))
                {
                    return Err(unavailable(
                        "durable lifecycle occurrence is missing or changed",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn validate_workspace(
        &self,
        context: &InvocationContext,
        root: &Path,
    ) -> RefineResult<()> {
        self.validate_authority(context, root)?;
        let workspace = context
            .workspace
            .as_ref()
            .ok_or_else(|| unavailable("missing workspace commitment"))?;
        if workspace.repository != self.repository
            || std::fs::canonicalize(&context.target_root).ok().as_ref() != Some(&self.repository)
            || workspace.branch != self.branch
            || workspace.path != self.path
            || workspace.registration.is_none()
            || workspace.allow_rebase
            || workspace.commit.is_some()
        {
            return Err(unavailable(
                "repository, branch, path or physical registration commitment changed",
            ));
        }
        workspace.validate_cwd(&context.cwd)?;
        if context.cwd
            != crate::infrastructure::git::worktrees::agent_worktree_cwd(
                self.path
                    .to_str()
                    .ok_or_else(|| unavailable("non-UTF8 workspace path"))?,
                &self.agent_subpath,
            )?
            || context.data["system"]["workspace"] != json!(context.cwd)
        {
            return Err(unavailable("cwd and supplied workspace context disagree"));
        }
        // A Skill may commit in its own checkout, but cannot replace its pinned source history.
        let git = FileGitWorktreeService::new(&self.path);
        let head = git.resolve_commit("HEAD")?;
        if !git.commit_is_ancestor(&self.source_commit, &head)? {
            return Err(unavailable(
                "lifecycle branch no longer descends from its pinned source",
            ));
        }
        Ok(())
    }
}

impl FileEventService {
    pub(super) fn validate_lifecycle(&self, invocation: &EventInvocation) -> RefineResult<()> {
        if let Some(owner) = &invocation.context.lifecycle {
            if owner.invocation_id != invocation.id
                || invocation.id
                    != stable_id(&format!("{}:{}", owner.occurrence_key, invocation.event.id))
                || owner.branch != format!("refine/{}/lifecycle-{}", owner.goal_id, invocation.id)
                || owner.path
                    != FileGitWorktreeService::new(&owner.repository)
                        .managed_worktree_path(&owner.branch)?
                || invocation.event.source.as_deref() != Some(&owner.source)
            {
                return Err(unavailable("invocation or event identity changed"));
            }
            if self.invocation(&invocation.id)?.state == InvocationState::Cancelled {
                return Err(unavailable("invocation was cancelled"));
            }
            self.validate_manual_authority(invocation)?;
            owner.validate_authority(&invocation.context, &self.refine_dir)?;
        }
        Ok(())
    }

    pub(super) fn save_accepted_invocation(
        &self,
        invocation: &EventInvocation,
        validate_authority: impl Fn() -> RefineResult<()>,
    ) -> RefineResult<()> {
        let save = || {
            validate_authority()?;
            self.validate_lifecycle(invocation)?;
            if invocation
                .bindings
                .iter()
                .any(|binding| binding.binding.mode != BindingMode::Context)
            {
                invocation.context.validate_workspace(&self.refine_dir)?;
            }
            self.save_invocation(invocation)
        };
        if let Some(owner) = &invocation.context.lifecycle {
            with_record_lock(&self.refine_dir, &owner.goal_id, save)
        } else {
            save()
        }
    }

    /// Successful evidence must still name its original checkout and current authority.
    /// Failed/cancelled evidence stays terminal even after a workspace is removed.
    pub(crate) fn validate_retained_invocation(
        &self,
        invocation: &EventInvocation,
    ) -> RefineResult<()> {
        if matches!(
            invocation.state,
            InvocationState::Failed | InvocationState::Error | InvocationState::Cancelled
        ) {
            return Ok(());
        }
        if invocation.state == InvocationState::Succeeded {
            invocation.blocking_results()?;
        }
        self.validate_lifecycle(invocation)?;
        if invocation
            .bindings
            .iter()
            .any(|b| b.binding.mode != BindingMode::Context)
        {
            invocation.context.validate_workspace(&self.refine_dir)?;
        }
        Ok(())
    }
}
