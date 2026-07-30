use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::model::goal::GoalIndexProjection;
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{FileProcessSupervisor, ProcessPauseState};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::host_resources::{HostResources, observed_agent_memory_bytes};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_state::{
    FileProjectStateStore, GoalSummaryProjection, ProjectionSnapshot,
};
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::promotion::BacklogPromotionService;

use super::{
    ClaimLoad, ClaimMetadata, WorkflowAutomation, WorkflowAutomationState, WorkflowClaim,
    WorkflowClaimState, WorkflowEngine, WorkflowPolicy, default_node_id, ensure_workflow_round,
    json_object, now_timestamp, priority_rank, setting_cap_with_default_values, setting_string,
    setting_usize,
};

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
            // Concurrency defaults to what this host can actually support rather
            // than to a fixed number. The same constant previously applied to a
            // two-core node and a thirty-two-core one, wasting the capable host
            // and overcommitting the constrained one. An explicit setting still
            // wins: the governor supplies the fallback, not an override.
            let governed_limit = HostResources::current(&self.runtime_root)
                .recommended_agent_concurrency(observed_agent_memory_bytes(&self.runtime_root));
            policy.global_limit = setting_usize(&settings, "parallel_run_cap", governed_limit);
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

}

/// A Goal past implementation no longer holds its Feature's ordering queue.
fn releases_feature_order(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::Review | GoalStatus::Done | GoalStatus::Cancelled
    )
}

/// Statuses that occupy a Feature's single in-flight slot.
fn occupies_feature_slot(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::InProgress | GoalStatus::ReadyMerge | GoalStatus::Build | GoalStatus::Qa
    )
}

/// Claim eligibility for every Goal in one projection snapshot, decided up front.
///
/// Both predicates used to be answered by rescanning the whole snapshot per
/// candidate, and the priority scan called the Feature scan inside its own loop.
/// That made promotion quadratic in Goal count at rest and cubic once a batch of
/// Todo Goals shared a Feature and a priority band — the shape a real backlog
/// takes. Neither predicate actually needs per-candidate scanning: Feature
/// eligibility reduces to two aggregates per (Node, Feature), and priority
/// eligibility to one maximum per Node. One pass answers every candidate in
/// constant time.
pub(super) struct ClaimEligibility {
    feature_eligible: BTreeSet<String>,
    highest_claimable_todo_rank: BTreeMap<String, u8>,
}

impl ClaimEligibility {
    pub(super) fn new(
        snapshot: &ProjectionSnapshot,
        excluded_goal_ids: &BTreeSet<String>,
    ) -> Self {
        // Per (Node, Feature): the lowest order still holding the queue, and how
        // many ordered Goals currently occupy the in-flight slot.
        let mut lowest_holding_order: BTreeMap<(&str, &str), i64> = BTreeMap::new();
        let mut occupying_count: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for projection in snapshot.goals.values() {
            let goal = &projection.goal;
            let Some(feature_id) = goal.feature_id.as_deref() else {
                continue;
            };
            // Only ordered Goals participate: the original scans reached these
            // comparisons through `feature_order`, so an unordered Goal neither
            // holds the queue nor occupies the slot.
            let Some(order) = goal.feature_order else {
                continue;
            };
            let key = (goal.node_id.as_deref().unwrap_or("default"), feature_id);
            if !releases_feature_order(&goal.status) {
                lowest_holding_order
                    .entry(key)
                    .and_modify(|lowest| *lowest = (*lowest).min(order))
                    .or_insert(order);
            }
            if occupies_feature_slot(&goal.status) {
                *occupying_count.entry(key).or_default() += 1;
            }
        }

        let feature_eligible = snapshot
            .goals
            .values()
            .filter(|projection| {
                Self::feature_eligible_from(
                    &projection.goal,
                    &lowest_holding_order,
                    &occupying_count,
                )
            })
            .map(|projection| projection.goal.id.clone())
            .collect::<BTreeSet<_>>();

        // A Goal is priority-eligible when nothing claimable on its Node
        // outranks it. Whether the Goal itself contributes to this maximum does
        // not matter: dropping one entry equal to its own rank cannot change
        // whether a strictly greater entry exists. That is what lets a
        // per-candidate scan collapse into a single maximum.
        let mut highest_claimable_todo_rank: BTreeMap<String, u8> = BTreeMap::new();
        for projection in snapshot.goals.values() {
            let goal = &projection.goal;
            if goal.status != GoalStatus::Todo
                || excluded_goal_ids.contains(&goal.id)
                || !feature_eligible.contains(&goal.id)
            {
                continue;
            }
            let rank = priority_rank(&goal.priority);
            highest_claimable_todo_rank
                .entry(goal.node_id.as_deref().unwrap_or("default").to_string())
                .and_modify(|highest| *highest = (*highest).max(rank))
                .or_insert(rank);
        }

        Self {
            feature_eligible,
            highest_claimable_todo_rank,
        }
    }

    fn feature_eligible_from(
        goal: &GoalIndexProjection,
        lowest_holding_order: &BTreeMap<(&str, &str), i64>,
        occupying_count: &BTreeMap<(&str, &str), usize>,
    ) -> bool {
        let Some(feature_id) = goal.feature_id.as_deref() else {
            return true;
        };
        let Some(order) = goal.feature_order else {
            return true;
        };
        let key = (goal.node_id.as_deref().unwrap_or("default"), feature_id);
        // Nothing earlier in the Feature may still be holding the queue.
        if lowest_holding_order
            .get(&key)
            .is_some_and(|lowest| *lowest < order)
        {
            return false;
        }
        // The Feature admits one ordered Goal in flight at a time, and a Goal
        // already holding that slot does not block itself.
        let occupying = occupying_count.get(&key).copied().unwrap_or_default();
        occupying.saturating_sub(usize::from(occupies_feature_slot(&goal.status))) == 0
    }

    pub(super) fn feature_eligible(&self, goal_id: &str) -> bool {
        self.feature_eligible.contains(goal_id)
    }

    pub(super) fn priority_eligible(&self, goal: &GoalIndexProjection) -> bool {
        self.highest_claimable_todo_rank
            .get(goal.node_id.as_deref().unwrap_or("default"))
            .is_none_or(|highest| *highest <= priority_rank(&goal.priority))
    }
}
