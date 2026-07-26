use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::tools::host::project_layout::target_root_for_refine_dir;
use crate::tools::product::project_state::GoalSummaryProjection;
use crate::tools::product::work_items::FileWorkItemService;

use super::{
    ChatAttachment, ChatService, ChatSessionRecord, ChatSessionWorktree, FileChatService,
    StandaloneReadyMergeRequest, StandaloneReadyMergeResult, derive_standalone_goal_name,
};

impl FileChatService {
    pub fn start_standalone_with_options(
        &self,
        provider: Option<&str>,
        mode: Option<&str>,
    ) -> RefineResult<ChatSessionRecord> {
        let mut session =
            self.start_record_with_options(ChatAttachment::Standalone, provider, mode)?;
        match self
            .create_standalone_worktree(&session.id)
            .and_then(|worktree| self.attach_worktree(&session.id, worktree))
        {
            Ok(updated) => {
                session = updated;
                Ok(session)
            }
            Err(error) => {
                let _ = self.interrupt(&session.id, "standalone worktree setup failed");
                Err(error)
            }
        }
    }

    pub fn stop_with_standalone_cleanup(
        &self,
        session_id: &str,
    ) -> RefineResult<ChatSessionRecord> {
        let existing = self.load_record(session_id)?;
        if matches!(existing.attachment, ChatAttachment::Standalone)
            && existing
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.submitted_goal_id.as_deref())
                .is_none()
            && let Some(worktree) = existing.worktree.as_ref()
        {
            self.cleanup_standalone_worktree(worktree)?;
        }
        self.stop(session_id)
    }

    pub fn submit_standalone_ready_merge(
        &self,
        session_id: &str,
        request: StandaloneReadyMergeRequest,
    ) -> RefineResult<StandaloneReadyMergeResult> {
        let session = self.load_record(session_id)?;
        if !matches!(session.attachment, ChatAttachment::Standalone) {
            return Err(RefineError::InvalidInput(
                "only standalone chat sessions can be submitted for merge".to_string(),
            ));
        }
        if session.closed {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} is closed"
            )));
        }
        let read_state = self.read(session_id)?;
        if session.in_flight
            || session.queue_dispatching
            || !session.queued_messages.is_empty()
            || read_state.in_flight
            || !read_state.queued_messages.is_empty()
        {
            return Err(RefineError::Conflict(
                "wait for the standalone chat to finish before submitting for merge".to_string(),
            ));
        }
        let Some(worktree) = session.worktree.clone() else {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} has no standalone worktree"
            )));
        };
        if worktree.submitted_goal_id.is_some() {
            return Err(RefineError::Conflict(format!(
                "Chat session {session_id} was already submitted"
            )));
        }

        let prompt = request.prompt.trim();
        let reporter = request.reporter.trim();
        let priority = request.priority.trim();
        let name = request
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| derive_standalone_goal_name(prompt))
            .ok_or_else(|| {
                RefineError::InvalidInput("body.name or body.prompt is required".to_string())
            })?;
        if reporter.is_empty() || prompt.is_empty() {
            return Err(RefineError::InvalidInput(
                "reporter and prompt are required".to_string(),
            ));
        }
        if !matches!(priority, "low" | "medium" | "high") {
            return Err(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }

        let settings =
            FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root).load()?;
        let target_branch = settings
            .get("merge_target_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("main");
        let worktree_git =
            FileGitWorktreeService::with_runtime_root(&worktree.path, &self.runtime_root);
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let goal = work_items.create_goal_summary(&name, None)?;
        let goal_id = goal.goal.id.clone();
        let submit_result = (|| -> RefineResult<GoalSummaryProjection> {
            work_items.append_goal_round_summary(&goal_id, reporter, prompt)?;
            if priority != "low" {
                work_items.update_goal_metadata_summary(
                    &goal_id,
                    None,
                    Some(priority),
                    None,
                    None,
                )?;
            }
            with_repository_git_lock(&target_root, || {
                match worktree_git.commit(&format!("Submit {goal_id} from standalone chat"), &[]) {
                    Ok(_) => {}
                    Err(error) => {
                        if !worktree_git.has_commits_since(target_branch)? {
                            return Err(error);
                        }
                    }
                }
                Ok(())
            })?;
            work_items.set_goal_branch_name(&goal_id, &worktree.branch)?;
            work_items.transition_goal_status(&goal_id, GoalStatus::Todo)?;
            work_items.advance_automated_goal_status(&goal_id, GoalStatus::InProgress)?;
            let goal =
                work_items.advance_automated_goal_status(&goal_id, GoalStatus::ReadyMerge)?;
            self.mark_worktree_submitted(session_id, &goal_id)?;
            self.interrupt(session_id, "submitted for ready-merge")?;
            Ok(goal)
        })();
        match submit_result {
            Ok(goal) => Ok(StandaloneReadyMergeResult { goal, worktree }),
            Err(error) => {
                let _ = work_items.delete_goal_record(&goal_id);
                Err(error)
            }
        }
    }

    pub(super) fn create_standalone_worktree(
        &self,
        session_id: &str,
    ) -> RefineResult<ChatSessionWorktree> {
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let branch = format!("refine/standalone/{session_id}");
        let git = FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
        let target = git
            .git_path("refine-standalone-worktrees")?
            .join(session_id);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create standalone worktree directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let path =
            with_repository_git_lock(&target_root, || git.ensure_worktree(&branch, &target))?;
        Ok(ChatSessionWorktree {
            branch,
            path,
            submitted_goal_id: None,
        })
    }

    pub(super) fn cleanup_standalone_worktree(
        &self,
        worktree: &ChatSessionWorktree,
    ) -> RefineResult<()> {
        let target_root = target_root_for_refine_dir(&self.refine_dir)?;
        let git = FileGitWorktreeService::new(&target_root);
        let path = PathBuf::from(&worktree.path);
        with_repository_git_lock(&target_root, || {
            if path.exists() {
                git.remove_worktree(&path, true)?;
            }
            let _ = git.delete_branch(&worktree.branch, true);
            Ok(())
        })
    }
}
