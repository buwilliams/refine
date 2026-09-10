//! Durable, invocation-owned creation using the shared Git worktree capability.
use super::*;
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};

impl FileEventService {
    pub(in super::super) fn pin_lifecycle_intent(
        &self,
        event: &EventDefinition,
        context: &mut InvocationContext,
        id: &str,
        occurrence_key: &str,
    ) -> RefineResult<()> {
        let goal = &context.data["goal"];
        let authority = if let Some(occurrence) = context.data.get("occurrence") {
            LifecycleAuthority::Occurrence {
                occurrence: occurrence.clone(),
            }
        } else if let Some(pending) = context.data.get("lifecycle_transition") {
            LifecycleAuthority::Transition {
                pending: pending.clone(),
            }
        } else if let Some(workflow_revision) = context.workflow_revision {
            LifecycleAuthority::WorkflowTransition {
                from: goal["status"].as_str().unwrap_or_default().into(),
                to: context.data["context"]["destination"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
                generation: goal["event_generation"].as_u64().unwrap_or(0),
                workflow_revision,
            }
        } else {
            return Err(unavailable(
                "no explicit lifecycle transition, workflow claim or durable occurrence",
            ));
        };
        let settings = FileSettingsService::for_node(&self.refine_dir, &context.node_id).load()?;
        let repository =
            std::fs::canonicalize(&context.target_root).map_err(|e| unavailable(&e.to_string()))?;
        let git = FileGitWorktreeService::new(&repository);
        let target_ref = goal["target_branch"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| settings["merge_target_branch"].as_str())
            .unwrap_or("main")
            .to_string();
        // Resolve the configured target explicitly. Never consult the developer's HEAD.
        let source_commit = git.resolve_commit(&target_ref)?;
        let goal_id = context
            .goal_id
            .clone()
            .ok_or_else(|| unavailable("missing Goal"))?;
        let branch = format!("refine/{goal_id}/lifecycle-{id}");
        let path = git.managed_worktree_path(&branch)?;
        let owner = LifecycleWorkspace {
            invocation_id: id.into(),
            occurrence_key: occurrence_key.into(),
            repository: repository.clone(),
            goal_id,
            round_idx: context.round_idx,
            request: request(goal),
            node_id: context.node_id.clone(),
            source: event
                .source
                .clone()
                .ok_or_else(|| unavailable("missing lifecycle source"))?,
            authority,
            target_ref,
            source_commit,
            branch: branch.clone(),
            path: path.clone(),
            launch_ready: false,
            inputs: BTreeMap::new(),
            agent_subpath: settings["agent_subpath"]
                .as_str()
                .unwrap_or_default()
                .into(),
        };
        owner.validate_authority(context, &self.refine_dir)?;
        context.workspace = Some(ManagedWorktree {
            repository,
            path,
            branch,
            commit: None,
            allow_rebase: false,
            registration: None,
        });
        context.lifecycle = Some(owner);
        Ok(())
    }

    pub(in super::super) fn materialize_lifecycle(
        &self,
        invocation: &mut EventInvocation,
    ) -> RefineResult<()> {
        let Some(owner) = invocation.context.lifecycle.clone() else {
            return Ok(());
        };
        if owner.launch_ready {
            return self.validate_retained_invocation(invocation);
        }
        self.validate_lifecycle(invocation)?;
        crate::infrastructure::git::with_repository_git_lock(&owner.repository, || {
            // Short Goal/record locks protect creation, never the running agent.
            with_record_lock(&self.refine_dir, &owner.goal_id, || {
                with_record_lock(
                    &self.refine_dir,
                    &format!("event-{}", invocation.id),
                    || {
                        *invocation = self.invocation(&invocation.id)?;
                        self.validate_lifecycle(invocation)?;
                        if invocation
                            .context
                            .lifecycle
                            .as_ref()
                            .is_some_and(|owner| owner.launch_ready)
                        {
                            return self.validate_retained_invocation(invocation);
                        }
                        let mut workspace = invocation
                            .context
                            .workspace
                            .clone()
                            .ok_or_else(|| unavailable("missing prepared workspace"))?;
                        if workspace.registration.is_none() {
                            FileGitWorktreeService::with_runtime_root(
                                &owner.repository,
                                self.runtime()?,
                            )
                            .create_worktree_from_base(
                                &owner.branch,
                                &owner.path,
                                &owner.source_commit,
                            )?;
                            workspace = workspace.pin()?;
                            // Persist the physical identity before resolving a possibly invalid subpath.
                            invocation.context.workspace = Some(workspace.clone());
                            self.save_invocation(invocation)?;
                        } else {
                            workspace.validate()?;
                        }
                        invocation.context.cwd =
                            crate::infrastructure::git::worktrees::agent_worktree_cwd(
                                owner
                                    .path
                                    .to_str()
                                    .ok_or_else(|| unavailable("non-UTF8 workspace path"))?,
                                &owner.agent_subpath,
                            )?;
                        invocation.context.data["system"]["workspace"] =
                            json!(invocation.context.cwd);
                        invocation.context.resolve_workspace_parameters(
                            &mut invocation.bindings,
                            &owner.inputs,
                        )?;
                        invocation.context.validate_workspace(&self.refine_dir)?;
                        invocation.context.lifecycle.as_mut().unwrap().launch_ready = true;
                        self.save_invocation(invocation)
                    },
                )
            })
        })
    }
}
