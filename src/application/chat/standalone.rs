use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::application::projects::projection::GoalSummaryProjection;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::with_repository_git_lock;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::infrastructure::storage::project_layout::target_root_for_refine_dir;
use crate::model::workflow::GoalStatus;

use super::{
    ChatAttachment, ChatService, ChatSessionRecord, ChatSessionWorktree, FileChatService,
    StandaloneQualityRequest, StandaloneQualityResult, derive_standalone_goal_name,
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

    pub fn submit_standalone_quality(
        &self,
        session_id: &str,
        request: StandaloneQualityRequest,
    ) -> RefineResult<StandaloneQualityResult> {
        let session = self.load_record(session_id)?;
        if !matches!(session.attachment, ChatAttachment::Standalone) {
            return Err(RefineError::InvalidInput(
                "only standalone chat sessions can be submitted to Quality".to_string(),
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
                "wait for the standalone chat to finish before submitting to Quality".to_string(),
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
            let candidate_commit = with_repository_git_lock(&target_root, || {
                match worktree_git.commit(&format!("Submit {goal_id} from standalone chat"), &[]) {
                    Ok(commit) => Ok(commit),
                    Err(error) => {
                        if !worktree_git.has_commits_since(target_branch)? {
                            return Err(error);
                        }
                        worktree_git.resolve_commit("HEAD")
                    }
                }
            })?;
            let target_git =
                FileGitWorktreeService::with_runtime_root(&target_root, &self.runtime_root);
            let base_commit = target_git.resolve_commit(target_branch)?;
            work_items.update_goal_git_refs(
                &goal_id,
                &worktree.branch,
                target_branch,
                &base_commit,
                Some(&candidate_commit),
            )?;
            work_items.update_latest_goal_round_evaluation_summary(
                &goal_id,
                &serde_json::json!({
                    "imported_candidate": {
                        "source": "standalone",
                        "session_id": session_id,
                        "branch": worktree.branch,
                        "candidate_commit": candidate_commit
                    },
                    "implementation_report": "Imported from a completed Standalone worktree for independent Quality and Governance."
                }),
            )?;
            work_items.transition_goal_status(&goal_id, GoalStatus::Todo)?;
            let goal = work_items.advance_automated_goal_status(&goal_id, GoalStatus::Quality)?;
            self.mark_worktree_submitted(session_id, &goal_id)?;
            self.interrupt(session_id, "submitted to quality")?;
            Ok(goal)
        })();
        match submit_result {
            Ok(goal) => Ok(StandaloneQualityResult { goal, worktree }),
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
