use super::*;

impl WorkflowEngine {
    pub fn policy(&self) -> RefineResult<WorkflowPolicy> {
        let Some(target_root) = &self.target_root else {
            return Ok(WorkflowPolicy::default());
        };
        let refine_dir = prepare_refine_dir(target_root)?;
        self.policy_for_refine_dir(&refine_dir)
    }

    /// Resolves the workflow/agent capacity policy for an already-established target-app state
    /// directory. Manual agent capabilities use this entry point so they share the scheduler's
    /// exact limits without rediscovering or relocating state.
    pub fn policy_for_refine_dir(&self, refine_dir: &Path) -> RefineResult<WorkflowPolicy> {
        let active_node_id =
            FileNodeRegistryService::with_active_root(refine_dir, &self.runtime_root)
                .active_node_id()?;
        self.policy_for_refine_dir_and_node(refine_dir, &active_node_id)
    }

    /// Loads Node-scoped workflow policy using a previously resolved ownership identity.
    pub fn policy_for_refine_dir_and_node(
        &self,
        refine_dir: &Path,
        node_id: &str,
    ) -> RefineResult<WorkflowPolicy> {
        let mut policy = WorkflowPolicy::default();
        if let Some(target_root) = &self.target_root {
            let settings = FileSettingsService::for_node(refine_dir, node_id).load()?;
            policy.global_limit = setting_usize(&settings, "parallel_run_cap", policy.global_limit);
            policy.per_node_limit = setting_cap_with_default_values(
                &settings,
                "parallel_per_node_cap",
                policy.global_limit,
                &[1, 2],
            );
            policy.per_provider_limit = setting_cap_with_default_values(
                &settings,
                "parallel_per_provider_cap",
                policy.global_limit,
                &[2],
            );
            policy.per_target_app_limit = setting_cap_with_default_values(
                &settings,
                "parallel_per_target_app_cap",
                policy.global_limit,
                &[2],
            );
            policy.provider = setting_string(&settings, "agent_cli", &policy.provider);
            policy.target_app_id = target_root.display().to_string();
            policy.active_node_id = node_id.to_string();
        }
        Ok(policy)
    }

    pub fn apply_runtime_settings(&self) -> RefineResult<usize> {
        let runnable = {
            let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
            let _state_lock = self.acquire_state_mutation_lock()?;
            let mut state = self.load_state()?;
            state.policy = self.policy()?;
            let runnable = match self.ensure_automation_running(&state) {
                Ok(()) => true,
                Err(RefineError::Conflict(_)) => false,
                Err(error) => return Err(error),
            };
            self.save_state(&mut state)?;
            runnable
        };
        if runnable { self.promote() } else { Ok(0) }
    }

    pub fn promote_backlog_to_todo(&self) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(0);
        };
        self.promote_backlog_to_todo_for_refine_dir(&refine_dir)
    }

    pub(super) fn promote_backlog_to_todo_for_refine_dir(
        &self,
        refine_dir: &Path,
    ) -> RefineResult<usize> {
        BacklogPromotionService::new(refine_dir, &self.runtime_root).promote_backlog_to_todo()
    }

    pub fn set_workflow_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        FileProcessSupervisor::new(&self.runtime_root).set_workflow_paused(paused)
    }

    pub fn fail_interrupted_goals(&self, detail: &str) -> RefineResult<usize> {
        if let Some(target_root) = &self.target_root {
            return with_repository_git_lock(target_root, || {
                self.fail_interrupted_goals_locked(detail)
            });
        }
        self.fail_interrupted_goals_locked(detail)
    }

    pub(super) fn fail_interrupted_goals_locked(&self, detail: &str) -> RefineResult<usize> {
        let Some(refine_dir) = self.refine_dir()? else {
            return Ok(0);
        };
        let snapshot = self.projection_snapshot(&refine_dir)?;
        let active_node_id = FileNodeRegistryService::new(&refine_dir).active_node_id()?;
        let goal_ids = snapshot
            .goals
            .values()
            .filter(|projection| {
                matches!(
                    projection.goal.status,
                    GoalStatus::InProgress
                        | GoalStatus::ReadyMerge
                        | GoalStatus::Build
                        | GoalStatus::Qa
                )
            })
            .filter(|projection| {
                projection.goal.node_id.as_deref().unwrap_or("default") == active_node_id
            })
            .map(|projection| projection.goal.id.clone())
            .collect::<Vec<_>>();
        if goal_ids.is_empty() {
            return Ok(0);
        }

        let detail = detail.trim();
        let detail = if detail.is_empty() {
            "workflow runner stopped before the Goal completed"
        } else {
            detail
        };
        let work_items = FileWorkItemService::new(&refine_dir);
        let logs = FileLogService::new(&refine_dir);
        for goal_id in &goal_ids {
            work_items.advance_automated_goal_status(goal_id, GoalStatus::Failed)?;
            let round_idx = ensure_workflow_round(&work_items, goal_id)?;
            logs.append_round_log(
                goal_id,
                round_idx,
                LogEntry {
                    datetime: now_timestamp(),
                    severity: "error".to_string(),
                    category: "workflow".to_string(),
                    message: format!("Workflow interrupted: {detail}"),
                    details: Some(json_object(json!({"reason": detail}))),
                    actions: Vec::new(),
                    actor: Some("refine".to_string()),
                    goal_id: Some(goal_id.clone()),
                },
            )?;
        }
        self.interrupt_active_claims(&goal_ids)?;
        Ok(goal_ids.len())
    }

    pub(super) fn signal_workflow_subprocesses(
        &self,
        execution_id: &str,
        signal: &str,
    ) -> RefineResult<usize> {
        let mut signalled = 0;
        // Current providers register under the managed-agent root. The legacy port root remains
        // observable during migration so a daemon upgrade can still stop an older process.
        for process_root in [self.runtime_root.join("agents"), self.runtime_root.clone()] {
            let supervisor = FileProcessSupervisor::new(process_root);
            for process in supervisor.list()? {
                let matches_execution = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                    .and_then(|details| {
                        details
                            .get("execution_id")
                            .and_then(|value| value.as_str())
                            .map(|value| value == execution_id)
                    })
                    .unwrap_or(false);
                if matches_execution {
                    supervisor.request_termination(&process.id, signal)?;
                    signalled += 1;
                }
            }
        }
        Ok(signalled)
    }

    /// True when a managed agent process for `execution_id` is still running.
    pub(super) fn workflow_execution_process_alive(
        &self,
        execution_id: &str,
    ) -> RefineResult<bool> {
        for process_root in [self.runtime_root.join("agents"), self.runtime_root.clone()] {
            let supervisor = FileProcessSupervisor::new(process_root);
            for process in supervisor.list()? {
                if process.state != "running" {
                    continue;
                }
                let matches_execution = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                    .and_then(|details| {
                        details
                            .get("execution_id")
                            .and_then(|value| value.as_str())
                            .map(|value| value == execution_id)
                    })
                    .unwrap_or(false);
                if matches_execution {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Settle `Running` claims whose executor no longer exists, releasing the
    /// concurrency slot each one was holding.
    ///
    /// A claim reaches `Running` only after its capacity lease is acquired, and a
    /// lease records the pid of the process that took it, so within one daemon
    /// generation a `Running` claim always holds a live lease. When a daemon dies
    /// between starting a claim and recording its terminal state, the worker that
    /// would have settled the claim dies with it: the lease is pruned as dead on
    /// the next capacity read, but the claim stays `Running` on disk forever.
    ///
    /// Nothing else reconciled that. Completion reports are matched by
    /// `execution_id` and never arrive, and `interrupt_active_claims` only runs
    /// against an explicit stop list. Admission counts every `Running` claim, so
    /// each orphan permanently consumed a slot and effective parallelism drifted
    /// below the configured cap with every mid-flight daemon death.
    ///
    /// Both conditions are required, and the order matters. Absence of a lease
    /// alone would also match the brief window in which settlement has released
    /// capacity but not yet persisted the terminal claim state, so a live process
    /// for the execution vetoes the sweep.
    pub(super) fn reconcile_orphaned_running_claims(&self) -> RefineResult<usize> {
        // Reading the capacity snapshot prunes leases whose holder is gone, which
        // is what makes a missing lease meaningful here.
        let held = self
            .capacity_service()
            .snapshot()?
            .leases
            .into_iter()
            .map(|lease| lease.owner_id)
            .collect::<BTreeSet<_>>();
        let candidates = self
            .load_state()?
            .claims
            .into_iter()
            .filter(|claim| claim.state == WorkflowClaimState::Running)
            .filter(|claim| !held.contains(&format!("workflow:{}", claim.claim_id)))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(0);
        }

        let mut orphaned = BTreeSet::new();
        for claim in candidates {
            let alive = match claim.execution_id.as_deref() {
                Some(execution_id) => self.workflow_execution_process_alive(execution_id)?,
                None => false,
            };
            if !alive {
                orphaned.insert(claim.claim_id);
            }
        }
        if orphaned.is_empty() {
            return Ok(0);
        }

        let _coordination = acquire_workflow_coordination(&self.coordination_root()?)?;
        let _state_lock = self.acquire_state_mutation_lock()?;
        let mut state = self.load_state()?;
        let mut released_claim_ids = Vec::new();
        let now = now_timestamp();
        for claim in &mut state.claims {
            // Re-check under the lock: a settlement may have landed since the scan.
            if claim.state == WorkflowClaimState::Running && orphaned.contains(&claim.claim_id) {
                claim.decision_version = claim.decision_version.saturating_add(1);
                claim.state = WorkflowClaimState::Interrupted;
                claim.updated_at = now.clone();
                released_claim_ids.push(claim.claim_id.clone());
            }
        }
        if released_claim_ids.is_empty() {
            return Ok(0);
        }
        self.save_state(&mut state)?;
        for claim_id in &released_claim_ids {
            self.release_claim_capacity(claim_id)?;
        }
        Ok(released_claim_ids.len())
    }

    pub(super) fn workflow_paused(&self) -> RefineResult<bool> {
        let pause_state = FileProcessSupervisor::new(&self.runtime_root).pause_state()?;
        Ok(pause_state.workflow_paused)
    }

    pub(super) fn ensure_automation_running(
        &self,
        _state: &WorkflowAutomationState,
    ) -> RefineResult<()> {
        if self.workflow_paused()? {
            return Err(RefineError::Conflict(
                "workflow automation is paused".to_string(),
            ));
        }
        Ok(())
    }

    pub(super) fn active_claim<'a>(
        state: &'a WorkflowAutomationState,
        goal_id: &str,
    ) -> Option<&'a WorkflowClaim> {
        state.active_claim(goal_id)
    }

    pub(super) fn claim_load(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
    ) -> ClaimLoad {
        Self::claim_load_excluding(state, policy, None)
    }

    pub(super) fn claim_load_excluding(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
        excluded_index: Option<usize>,
    ) -> ClaimLoad {
        let mut load = ClaimLoad::default();
        for claim in state
            .claims
            .iter()
            .enumerate()
            .filter(|(index, claim)| {
                Some(*index) != excluded_index
                    && matches!(
                        claim.state,
                        WorkflowClaimState::Claimed | WorkflowClaimState::Running
                    )
            })
            .map(|(_, claim)| claim)
        {
            load.global += 1;
            *load.by_node.entry(claim.node_id.clone()).or_default() += 1;
            *load.by_provider.entry(claim.provider.clone()).or_default() += 1;
            *load
                .by_target_app
                .entry(claim.target_app_id.clone())
                .or_default() += 1;
        }
        load.ensure_policy_keys(policy);
        load
    }

    pub(super) fn capacity_available(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
        node_id: &str,
        provider: &str,
        target_app_id: &str,
    ) -> bool {
        let load = Self::claim_load(state, policy);
        Self::capacity_available_for_load(&load, policy, node_id, provider, target_app_id)
    }

    pub(super) fn capacity_available_for_load(
        load: &ClaimLoad,
        policy: &WorkflowPolicy,
        node_id: &str,
        provider: &str,
        target_app_id: &str,
    ) -> bool {
        load.global < policy.global_limit
            && load.by_node.get(node_id).copied().unwrap_or(0) < policy.per_node_limit
            && load.by_provider.get(provider).copied().unwrap_or(0) < policy.per_provider_limit
            && load.by_target_app.get(target_app_id).copied().unwrap_or(0)
                < policy.per_target_app_limit
    }

    pub(super) fn record_claim_load(load: &mut ClaimLoad, claim: &WorkflowClaim) {
        load.global += 1;
        *load.by_node.entry(claim.node_id.clone()).or_default() += 1;
        *load.by_provider.entry(claim.provider.clone()).or_default() += 1;
        *load
            .by_target_app
            .entry(claim.target_app_id.clone())
            .or_default() += 1;
    }

    pub(super) fn running_claim_load(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
    ) -> ClaimLoad {
        let mut load = ClaimLoad::default();
        for claim in state
            .claims
            .iter()
            .filter(|claim| claim.state == WorkflowClaimState::Running)
        {
            Self::record_claim_load(&mut load, claim);
        }
        load.ensure_policy_keys(policy);
        load
    }

    pub(super) fn launchable_claim_ids(
        state: &WorkflowAutomationState,
        policy: &WorkflowPolicy,
    ) -> Vec<String> {
        let mut load = Self::running_claim_load(state, policy);
        let mut claim_ids = Vec::new();
        for claim in state
            .claims
            .iter()
            .filter(|claim| claim.state == WorkflowClaimState::Claimed)
        {
            if Self::capacity_available_for_load(
                &load,
                policy,
                &claim.node_id,
                &claim.provider,
                &claim.target_app_id,
            ) {
                Self::record_claim_load(&mut load, claim);
                claim_ids.push(claim.claim_id.clone());
            }
        }
        claim_ids
    }

    pub(super) fn claim_metadata(
        &self,
        goal: Option<&GoalSummaryProjection>,
        policy: &WorkflowPolicy,
    ) -> RefineResult<ClaimMetadata> {
        let node_id = goal
            .and_then(|goal| goal.goal.node_id.clone())
            .unwrap_or_else(default_node_id);
        if node_id != policy.active_node_id {
            let goal_id = goal
                .map(|goal| goal.goal.id.as_str())
                .unwrap_or("requested Goal");
            return Err(RefineError::Conflict(format!(
                "{goal_id} is owned by node {node_id}, not active node {}",
                policy.active_node_id
            )));
        }
        Ok(ClaimMetadata {
            node_id,
            provider: policy.provider.clone(),
            target_app_id: policy.target_app_id.clone(),
        })
    }

    pub(super) fn projection_snapshot(
        &self,
        refine_dir: &Path,
    ) -> RefineResult<ProjectionSnapshot> {
        FileProjectStateStore::with_runtime_root(refine_dir, &self.runtime_root)
            .load_or_refresh_projection(&self.runtime_root.join("cache"))
    }

    pub(super) fn feature_claim_eligible(
        snapshot: &ProjectionSnapshot,
        goal: &GoalSummaryProjection,
    ) -> bool {
        let Some(feature_id) = goal.goal.feature_id.as_deref() else {
            return true;
        };
        let Some(feature_order) = goal.goal.feature_order else {
            return true;
        };
        let node_id = goal.goal.node_id.as_deref().unwrap_or("default");
        !snapshot.goals.values().any(|other| {
            other.goal.feature_id.as_deref() == Some(feature_id)
                && other.goal.node_id.as_deref().unwrap_or("default") == node_id
                && other
                    .goal
                    .feature_order
                    .is_some_and(|order| order < feature_order)
                && !matches!(
                    other.goal.status,
                    GoalStatus::Review | GoalStatus::Done | GoalStatus::Cancelled
                )
        }) && !snapshot.goals.values().any(|other| {
            other.goal.id != goal.goal.id
                && other.goal.feature_id.as_deref() == Some(feature_id)
                && other.goal.node_id.as_deref().unwrap_or("default") == node_id
                && is_ordered_feature_goal(goal.goal.feature_order)
                && is_ordered_feature_goal(other.goal.feature_order)
                && matches!(
                    other.goal.status,
                    GoalStatus::InProgress
                        | GoalStatus::ReadyMerge
                        | GoalStatus::Build
                        | GoalStatus::Qa
                )
        })
    }

    pub(super) fn priority_claim_eligible(
        snapshot: &ProjectionSnapshot,
        goal: &GoalSummaryProjection,
    ) -> bool {
        let node_id = goal.goal.node_id.as_deref().unwrap_or("default");
        !snapshot.goals.values().any(|other| {
            other.goal.id != goal.goal.id
                && other.goal.status == GoalStatus::Todo
                && other.goal.node_id.as_deref().unwrap_or("default") == node_id
                && priority_rank(&other.goal.priority) > priority_rank(&goal.goal.priority)
                && Self::feature_claim_eligible(snapshot, other)
        })
    }
}
