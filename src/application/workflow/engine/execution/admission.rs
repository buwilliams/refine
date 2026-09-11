use super::*;

impl WorkflowEngine {
    pub(super) fn target_still_attached(
        &self,
        registry: Option<&std::path::Path>,
    ) -> RefineResult<bool> {
        // The worker supplies its launch context explicitly. Descendant agents inherit the
        // ownership token too; it must not turn their CLI or test engines into daemon workers.
        let Some(registry) = registry else {
            return Ok(true);
        };
        let attached =
            crate::application::projects::registry::FileProjectRegistryService::new(registry, None)
                .load()?
                .active_app
                .map(std::path::PathBuf::from);
        let canonical = |path: Option<&std::path::Path>| {
            path.map(std::path::Path::canonicalize)
                .transpose()
                .map_err(|e| RefineError::Io(e.to_string()))
        };
        Ok(canonical(attached.as_deref())? == canonical(self.target_root.as_deref())?)
    }

    /// Admission remains responsive while child Goal executions are running.
    pub(super) fn service_pending_skills(&self, limit: usize) {
        let Some(target) = self.target_root.as_ref() else {
            return;
        };
        let result = (|| -> RefineResult<()> {
            let events = crate::application::events::FileEventService::with_runtime_root(
                prepare_refine_dir(target)?,
                &self.runtime_root,
            );
            events.dispatch_outcomes(target)?;
            if let Err(error) = events.dispatch_goal_events(target) {
                eprintln!("refine Goal event materialization: {error}");
            }
            if limit > 0 {
                events.dispatch_pending_limit(target, limit)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("refine Skill admission: {error}");
        }
    }

    pub(super) fn reserve_goal(
        &self,
        goal_id: &str,
    ) -> RefineResult<Option<super::super::admission::AdmissionLease>> {
        let policy = self.policy()?;
        super::super::admission::reserve(
            self,
            &policy,
            format!(
                "{}:{}:goal:{goal_id}",
                self.runtime_root.display(),
                policy.target_app_id
            ),
            super::super::admission::ExecutionReservation {
                runtime: self.runtime_root.clone(),
                invocation_id: None,
                goal_id: Some(goal_id.into()),
                node: policy.active_node_id.clone(),
                provider: policy.provider.clone(),
                target: policy.target_app_id.clone(),
            },
        )
    }

    pub(super) fn launchable_goals(&self, active: &BTreeSet<String>) -> RefineResult<Vec<String>> {
        let target_root = self.target_root.as_ref().ok_or_else(|| {
            RefineError::InvalidInput(
                "target root is required to execute workflow work".to_string(),
            )
        })?;
        let refine_dir = prepare_refine_dir(target_root)?;
        ActiveGoalIndex::ensure_built(&refine_dir)?;
        let index = ActiveGoalIndex::load_or_rebuild(&refine_dir)?;
        let policy = self.policy()?;
        let events = crate::application::events::FileEventService::new(&refine_dir);
        let items = FileWorkItemService::new(&refine_dir);
        let mut observed = BTreeSet::new();
        for root in [self.runtime_root.clone(), self.runtime_root.join("agents")] {
            let supervisor =
                crate::infrastructure::process::subprocess::FileProcessSupervisor::new(root);
            for process in supervisor.capacity_processes()? {
                if !supervisor.group_pending(&process)? {
                    continue;
                }
                if let Some(goal) = process
                    .details
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v["goal_id"].as_str().map(str::to_string))
                {
                    observed.insert(goal);
                }
            }
        }
        let eligibility = SchedulingEligibility::new(index.goals());
        let mut goals = index
            .goals()
            .filter(|goal| {
                matches!(
                    goal.status,
                    GoalStatus::Todo
                        | GoalStatus::Plan
                        | GoalStatus::Implement
                        | GoalStatus::Governance
                        | GoalStatus::Quality
                )
            })
            .filter(|goal| {
                crate::application::fleet::nodes::node_ids_match(
                    goal.node_id.as_deref().unwrap_or("default"),
                    &policy.active_node_id,
                )
            })
            .filter(|goal| goal.round_count > 0)
            .filter(|goal| !active.contains(&goal.id) && !observed.contains(&goal.id))
            .filter(|goal| eligibility.feature_eligible(&goal.id))
            .filter(|goal| eligibility.priority_eligible(goal))
            .cloned()
            .collect::<Vec<_>>();
        goals.sort_by(|a, b| {
            priority_rank(&b.priority)
                .cmp(&priority_rank(&a.priority))
                .then_with(|| compare_feature_goal_order(a.feature_order, b.feature_order))
                .then_with(|| a.created.cmp(&b.created))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut load = self.observed_execution_load()?;
        let mut result = Vec::new();
        for goal in goals {
            let detail = items.show_goal_detail(&goal.id)?;
            if self.failed_attempt_is_fenced(&goal.id, &detail) {
                continue;
            }
            if detail["workflow_integration_control"]["state"] == "pending"
                || detail["pending_workflow_outcome"]["state"] == "pending"
                || detail["pending_event_transition"]["state"] == "pending"
            {
                continue;
            }
            if goal.status == GoalStatus::Todo
                && events
                    .missing_workflow_requirement(
                        &items.show_goal_detail(&goal.id)?,
                        &policy.active_node_id,
                    )?
                    .is_some()
            {
                continue;
            }
            if !load.available(
                &policy,
                &policy.active_node_id,
                &policy.provider,
                &policy.target_app_id,
            ) {
                break;
            }
            load.record(
                &policy.active_node_id,
                &policy.provider,
                &policy.target_app_id,
            );
            result.push(goal.id);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_worker_target_fence_keeps_child_engines_independent() {
        let root = std::env::temp_dir().join(format!(
            "refine-worker-target-fence-{}",
            uuid::Uuid::new_v4()
        ));
        let target = root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        let engine = WorkflowEngine::with_target_root(root.join("run"), &target);
        let registry =
            crate::application::projects::registry::FileProjectRegistryService::new(&root, None);
        assert!(engine.target_still_attached(None).unwrap());
        assert!(!engine.target_still_attached(Some(&root)).unwrap());
        let mut apps = registry.load().unwrap();
        apps.active_app = Some(target.display().to_string());
        registry.save(&apps).unwrap();
        assert!(engine.target_still_attached(Some(&root)).unwrap());
        apps.active_app = Some(root.display().to_string());
        registry.save(&apps).unwrap();
        assert!(!engine.target_still_attached(Some(&root)).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
