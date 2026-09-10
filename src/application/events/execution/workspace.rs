//! Goal-bound Skill workspace admission. Custom Skills without an explicit Goal
//! keep their independent execution context.
use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::infrastructure::git::worktrees::{FileGitWorktreeService, ManagedWorktree};
use std::path::Path;

fn unavailable(reason: &str) -> RefineError {
    RefineError::Degraded(format!(
        "Goal Skill workspace unavailable: {reason}; existing work was preserved. Recover the current Round workspace before retrying."
    ))
}

impl InvocationContext {
    fn goal_branch(&self, refine_dir: &Path) -> RefineResult<Option<String>> {
        let Some(goal_id) = self.goal_id.as_deref() else {
            return Ok(None);
        };
        let detail = FileWorkItemService::new(refine_dir).show_goal_detail(goal_id)?;
        let rounds = detail["rounds"]
            .as_array()
            .ok_or_else(|| unavailable("missing Round evidence"))?;
        if self.round_idx != rounds.len().checked_sub(1) {
            return Err(unavailable("the Goal Round changed"));
        }
        let owner = detail["node_id"].as_str().unwrap_or("default");
        if !crate::application::fleet::nodes::node_ids_match(owner, &self.node_id) {
            return Err(unavailable("the Goal node changed"));
        }
        let branch = detail["branch_name"]
            .as_str()
            .ok_or_else(|| unavailable("missing Round branch"))?;
        let mut reconciliation = false;
        if let Some(workspace) = &self.workspace {
            let checkout =
                &rounds[self.round_idx.unwrap()]["workflow_reconciliation"]["quality_checkout"];
            reconciliation = workspace.commit.as_deref() == self.candidate_commit.as_deref()
                && workspace.commit.is_some()
                && checkout["branch"].as_str() == Some(workspace.branch.as_str())
                && checkout["path"].as_str() == workspace.path.to_str()
                && checkout["candidate_commit"].as_str() == workspace.commit.as_deref()
                && rounds[self.round_idx.unwrap()]["workflow_integration"]["candidate_commit"]
                    .as_str()
                    == workspace.commit.as_deref();
            if workspace.branch != branch && !reconciliation {
                return Err(unavailable(
                    "workspace does not belong to the recorded Round branch or exact reconciliation candidate",
                ));
            }
        }
        if !reconciliation {
            use crate::infrastructure::process::supervisor::config::{
                ConfigService, FileSettingsService,
            };
            let settings = FileSettingsService::for_node(refine_dir, &self.node_id).load()?;
            crate::application::workflow::engine::context::validate_round_workspace_branch(
                &detail,
                goal_id,
                self.round_idx.unwrap(),
                branch,
                settings
                    .get("branch_name_pattern")
                    .and_then(Value::as_str)
                    .unwrap_or("refine/{goal_id}"),
            )?;
        }
        Ok(Some(branch.into()))
    }

    pub(super) fn admit_workspace(&mut self, refine_dir: &Path) -> RefineResult<()> {
        let Some(branch) = self.goal_branch(refine_dir)? else {
            return Ok(());
        };
        if self.workspace.is_none() {
            let git = FileGitWorktreeService::new(&self.target_root);
            self.workspace = Some(
                ManagedWorktree {
                    repository: self.target_root.clone(),
                    path: git.managed_worktree_path(&branch)?,
                    branch,
                    commit: if self.data["verification_only"] == true {
                        self.candidate_commit.clone()
                    } else {
                        None
                    },
                    allow_rebase: false,
                    registration: None,
                }
                .pin()?,
            );
        }
        self.validate_workspace(refine_dir)?;
        self.cwd = std::fs::canonicalize(&self.cwd).map_err(|e| unavailable(&e.to_string()))?;
        self.data["system"]["workspace"] = json!(self.cwd);
        Ok(())
    }

    pub(crate) fn validate_workspace(&self, refine_dir: &Path) -> RefineResult<()> {
        if let Some(owner) = &self.lifecycle {
            return owner.validate_workspace(self, refine_dir);
        }
        if self.goal_branch(refine_dir)?.is_none() {
            return Ok(());
        }
        let workspace = self
            .workspace
            .as_ref()
            .ok_or_else(|| unavailable("the retained invocation has no admitted workspace"))?;
        if workspace.registration.is_none()
            || std::fs::canonicalize(&workspace.repository).ok()
                != std::fs::canonicalize(&self.target_root).ok()
        {
            return Err(unavailable("the retained repository commitment changed"));
        }
        workspace.validate_cwd(&self.cwd)
    }

    pub(super) fn resolve_workspace_parameters(
        &self,
        bindings: &mut [PinnedBinding],
        inputs: &BTreeMap<String, Value>,
    ) -> RefineResult<()> {
        for pinned in bindings {
            let mut mapped = pinned.parameters.clone();
            for (name, path) in &pinned.binding.inputs {
                if !inputs.contains_key(name)
                    && (path == "system" || path.starts_with("system."))
                    && let Some(value) = field(&self.data, path)
                {
                    mapped.insert(name.clone(), value.clone());
                }
            }
            pinned.parameters =
                resolve_parameters(&pinned.skill.parameters, &BTreeMap::new(), &mapped)?;
        }
        Ok(())
    }

    pub(super) fn process_workspace(&self, metadata: &mut serde_json::Map<String, Value>) {
        // Current process authority may change on recovery; the pinned workspace may not.
        metadata.remove("managed_worktree");
        if let Some(workspace) = &self.workspace {
            metadata.insert("managed_worktree".into(), json!(workspace));
        }
    }
}
