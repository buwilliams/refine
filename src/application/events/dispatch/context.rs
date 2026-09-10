//! Explicit manual context and execution authority shared by lifecycle dispatch.
use super::*;

impl FileEventService {
    pub fn manual_context(
        &self,
        target_root: &Path,
        body: &Value,
    ) -> RefineResult<InvocationContext> {
        let runtime = self.runtime()?;
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, runtime)
            .active_node_id()?;
        if body
            .get("node_id")
            .and_then(Value::as_str)
            .is_some_and(|n| !n.eq_ignore_ascii_case(&node))
        {
            return Err(RefineError::Conflict(
                "trigger this Event through its selected node's daemon".into(),
            ));
        }
        let settings = FileSettingsService::with_active_root(&self.refine_dir, runtime).load()?;
        let provider = settings
            .get("agent_cli")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("claude")
            .to_string();
        let goal_id = body
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut goal = Value::Null;
        let mut cwd = target_root.to_path_buf();
        let mut round_idx = None;
        let mut candidate_commit = None;
        if let Some(id) = &goal_id {
            goal = FileWorkItemService::new(&self.refine_dir).show_goal_detail(id)?;
            let owner = goal
                .get("node_id")
                .and_then(Value::as_str)
                .unwrap_or("default");
            if !owner.eq_ignore_ascii_case(&node) {
                return Err(RefineError::Conflict(format!(
                    "Goal {id} is owned by node {owner}"
                )));
            }
            round_idx = goal
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|r| r.len().checked_sub(1));
            candidate_commit = goal
                .get("candidate_commit")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(branch) = goal.get("branch_name").and_then(Value::as_str) {
                use crate::infrastructure::git::worktrees::FileGitWorktreeService;
                let git = FileGitWorktreeService::with_runtime_root(target_root, runtime);
                if let Some(worktree) = git.existing_worktree_for_branch(branch)? {
                    cwd = crate::infrastructure::git::worktrees::agent_worktree_cwd(
                        &worktree.to_string_lossy(),
                        settings["agent_subpath"].as_str().unwrap_or_default(),
                    )?;
                }
            }
        }
        Ok(InvocationContext {
            node_id: node.clone(),
            target_root: target_root.into(),
            cwd: cwd.clone(),
            workspace: None,
            lifecycle: None,
            provider,
            goal_id,
            round_idx,
            workflow_revision: None,
            candidate_commit,
            data: json!({"goal": super::super::execution::goal_context(&goal), "system": {"node_id": node, "project_root": target_root, "workspace": cwd, "runtime_root": runtime, "refine_executable": std::env::current_exe().ok(), "refine_checkout": crate::infrastructure::runtime::checkout::active_refine_paths().ok().map(|(_, checkout)| checkout)}}),
            metadata: Default::default(),
        })
    }

    pub fn validate_manual_authority(&self, invocation: &EventInvocation) -> RefineResult<()> {
        if self.invocation(&invocation.id)?.state == InvocationState::Cancelled {
            return Err(RefineError::Conflict(
                "Event invocation was cancelled".into(),
            ));
        }
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, self.runtime()?)
            .active_node_id()?;
        if node != invocation.context.node_id {
            return Err(RefineError::Conflict("Event execution node changed".into()));
        }
        let node = FileNodeRegistryService::with_active_root(&self.refine_dir, self.runtime()?)
            .active_node_id()?;
        if node != invocation.context.node_id {
            return Err(RefineError::Conflict("Event execution node changed".into()));
        }
        if let Some(id) = &invocation.context.goal_id {
            let goal = FileWorkItemService::new(&self.refine_dir).show_goal_detail(id)?;
            let pinned = invocation.context.data.get("goal").unwrap_or(&Value::Null);
            for key in ["status", "node_id", "candidate_commit", "event_generation"] {
                if invocation.context.data.get("occurrence").is_some()
                    && ["status", "event_generation"].contains(&key)
                {
                    continue;
                }
                if goal.get(key) != pinned.get(key) {
                    return Err(RefineError::Conflict(format!(
                        "Goal {id} {key} changed after this Event was requested"
                    )));
                }
            }
            let current_request = goal
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|r| r.last())
                .and_then(|r| r.get("prompt"));
            let pinned_request = pinned
                .get("rounds")
                .and_then(Value::as_array)
                .and_then(|r| r.last())
                .and_then(|r| r.get("prompt"));
            if current_request != pinned_request {
                return Err(RefineError::Conflict(
                    "Goal request changed after this Event was requested".into(),
                ));
            }
            if goal.get("rounds").and_then(Value::as_array).map(Vec::len)
                != pinned.get("rounds").and_then(Value::as_array).map(Vec::len)
            {
                return Err(RefineError::Conflict("Goal Round changed".into()));
            }
        }
        Ok(())
    }
}
