use super::*;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct CancellationSettlementJournal {
    pub(super) schema_version: u32,
    pub(super) state: String,
    pub(super) goal_id: String,
    pub(super) claim_ids: Vec<String>,
    pub(super) execution_ids: Vec<String>,
    pub(super) workflow_before: WorkflowAutomationState,
    pub(super) workflow_after: WorkflowAutomationState,
    pub(super) capacity_before: AgentCapacityState,
    pub(super) capacity_after: AgentCapacityState,
    pub(super) goal_before: Value,
    pub(super) goal_after: Value,
    #[serde(default = "default_goal_stop_disposition")]
    pub(super) goal_disposition: GoalStopDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) termination_intent: Option<TerminationIntent>,
    #[serde(default)]
    pub(super) worktrees: Vec<WorkflowWorktree>,
    pub(super) recorded_at: String,
    pub(super) goal_cancelled: bool,
    #[serde(default)]
    pub(super) goal_requeued: bool,
    pub(super) claim_cancelled: bool,
    pub(super) capacity_released: bool,
    #[serde(default)]
    pub(super) worktrees_retained: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rollback_failure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rollback_goal_restored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rollback_capacity_restored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rollback_claim_restored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) rollback_goal_state: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) replay_goal_before: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) replay_goal_after: Option<Value>,
    pub(super) recovery: String,
}

impl FileProcessControlService {
    pub(super) fn settle_goal_cancellation(
        &self,
        refine_dir: &Path,
        goal_id: &str,
        expectation: &GoalCancellationExpectation,
        ownership: &[WorkflowGoalOwnership],
        intent: TerminationIntent,
        worktrees: &[WorkflowWorktree],
    ) -> RefineResult<GoalStopSettlement> {
        let disposition = intent.disposition();
        let _coordination = acquire_workflow_coordination(refine_dir)?;
        let workflow = WorkflowEngine::new(&self.runtime_root);
        let _workflow_lock = workflow.acquire_state_mutation_lock()?;
        let work_items = FileWorkItemService::for_node(refine_dir, &expectation.node_id);
        let mut goal_transaction = work_items.prepare_goal_cancellation_if_current(
            goal_id,
            expectation,
            disposition.goal_status(),
        )?;
        let state = workflow.load_state()?;
        let original_state = state.clone();
        let mut claim_ids = Vec::new();
        if ownership.is_empty() {
            ensure_goal_has_no_active_workflow_claim_in_state(
                &state,
                goal_id,
                "stopped process",
                WorkflowOwnershipPhase::BeforeCancellation,
            )?;
        } else {
            for ownership in ownership {
                validate_workflow_goal_ownership_in_state(
                    &state,
                    goal_id,
                    ownership,
                    WorkflowOwnershipPhase::BeforeCancellation,
                )?;
                validate_expected_goal_round(
                    expectation,
                    goal_id,
                    ownership,
                    WorkflowOwnershipPhase::BeforeCancellation,
                )?;
                claim_ids.push(ownership.claim_id.clone());
            }
        }
        #[cfg(test)]
        if let Some(hook) = &self.settlement_hook {
            hook();
        }
        claim_ids.sort();
        claim_ids.dedup();
        let mut capacity = workflow
            .capacity_service_for_settlement()
            .begin_cancellation_settlement()?;
        let workflow_after = workflow.claims_cancelled_state(&state, &claim_ids)?;
        let capacity_before = capacity.original_state();
        let capacity_after = capacity.state_after_releasing_claims(&claim_ids);
        let goal_before = goal_transaction.original_value();
        let goal_after = goal_transaction.settled_value();
        let mut execution_ids = ownership
            .iter()
            .filter_map(|ownership| ownership.execution_id.clone())
            .collect::<Vec<_>>();
        execution_ids.sort();
        execution_ids.dedup();
        let worktree_retention = WorkflowWorktreeRetention::from_targets(worktrees);
        let receipt_path = self.cancellation_settlement_receipt_path(goal_id, &claim_ids);
        let mut journal = CancellationSettlementJournal {
            schema_version: 5,
            state: "prepared".to_string(),
            goal_id: goal_id.to_string(),
            claim_ids: claim_ids.clone(),
            execution_ids,
            workflow_before: original_state.clone(),
            workflow_after: workflow_after.clone(),
            capacity_before,
            capacity_after,
            goal_before,
            goal_after,
            goal_disposition: disposition,
            termination_intent: Some(intent),
            worktrees: worktree_retention.worktrees.clone(),
            recorded_at: Utc::now().to_rfc3339(),
            goal_cancelled: false,
            goal_requeued: false,
            claim_cancelled: false,
            capacity_released: false,
            worktrees_retained: worktree_retention.retained,
            cause: None,
            rollback_failure: None,
            rollback_goal_restored: None,
            rollback_capacity_restored: None,
            rollback_claim_restored: None,
            rollback_goal_state: None,
            replay_goal_before: None,
            replay_goal_after: None,
            recovery: cancellation_settlement_recovery("prepared").to_string(),
        };
        self.write_cancellation_settlement_journal(&receipt_path, &journal)?;

        let settlement = (|| -> RefineResult<()> {
            workflow.persist_state_preserving_policy_locked(&workflow_after)?;
            self.update_cancellation_settlement_journal(
                &receipt_path,
                &mut journal,
                "claim_persisted",
                None,
                None,
            )?;
            self.inject_settlement_failure(
                CancellationSettlementFailureStage::AfterClaimPersistence,
            )?;

            capacity.release_claims(&claim_ids)?;
            self.update_cancellation_settlement_journal(
                &receipt_path,
                &mut journal,
                "capacity_released",
                None,
                None,
            )?;
            self.inject_settlement_failure(
                CancellationSettlementFailureStage::AfterCapacityRelease,
            )?;

            goal_transaction.commit()?;
            self.update_cancellation_settlement_journal(
                &receipt_path,
                &mut journal,
                "goal_persisted",
                None,
                None,
            )?;
            self.inject_settlement_failure(
                CancellationSettlementFailureStage::AfterGoalPersistence,
            )?;
            self.update_cancellation_settlement_journal(
                &receipt_path,
                &mut journal,
                "committed",
                None,
                None,
            )?;
            Ok(())
        })();

        if let Err(cause) = settlement {
            let cause_message = cause.to_string();
            let mut rollback_failures = Vec::new();
            match goal_transaction.restore() {
                Ok(restored) => {
                    journal.rollback_goal_restored = Some(true);
                    journal.rollback_goal_state = Some(restored);
                }
                Err(error) => {
                    journal.rollback_goal_restored = Some(false);
                    rollback_failures.push(format!("Goal restore failed: {error}"));
                }
            }
            let capacity_restore = self
                .inject_rollback_failure(CancellationRollbackFailureStage::CapacityRestore)
                .and_then(|()| capacity.restore());
            match capacity_restore {
                Ok(()) => journal.rollback_capacity_restored = Some(true),
                Err(error) => {
                    journal.rollback_capacity_restored = Some(false);
                    rollback_failures.push(format!("capacity restore failed: {error}"));
                }
            }
            let claim_restore = self
                .inject_rollback_failure(CancellationRollbackFailureStage::ClaimRestore)
                .and_then(|()| workflow.restore_state_locked(&original_state));
            match claim_restore {
                Ok(()) => journal.rollback_claim_restored = Some(true),
                Err(error) => {
                    journal.rollback_claim_restored = Some(false);
                    rollback_failures.push(format!("claim restore failed: {error}"));
                }
            }
            let rollback_state = if rollback_failures.is_empty() {
                "rolled_back"
            } else {
                "rollback_failed"
            };
            let rollback_detail =
                (!rollback_failures.is_empty()).then(|| rollback_failures.join("; "));
            let _ = self.update_cancellation_settlement_journal(
                &receipt_path,
                &mut journal,
                rollback_state,
                Some(&cause_message),
                rollback_detail.as_deref(),
            );
            return Err(error_with_message(
                cause,
                format!(
                    "linked cancellation settlement failed after {cause_message} and {}: claim, capacity, and Goal writes {}; durable recovery evidence is at {}{}",
                    if rollback_failures.is_empty() {
                        "was restored to its pre-settlement state"
                    } else {
                        "could not be fully restored"
                    },
                    if rollback_failures.is_empty() {
                        "were rolled back"
                    } else {
                        "require recovery"
                    },
                    receipt_path.display(),
                    rollback_detail
                        .map(|detail| format!("; {detail}"))
                        .unwrap_or_default()
                ),
            ));
        }

        Ok(GoalStopSettlement {
            goal: goal_transaction.projection()?,
            worktree_retention,
        })
    }

    pub(super) fn replay_cancellation_settlement(
        &self,
        refine_dir: &Path,
        execution_id: &str,
        requested_intent: TerminationIntent,
    ) -> RefineResult<Option<Value>> {
        let Some((receipt_path, journal)) =
            self.cancellation_settlement_journal_for_execution(execution_id)?
        else {
            return Ok(None);
        };
        self.replay_cancellation_settlement_journal(
            refine_dir,
            receipt_path,
            journal,
            Some(execution_id),
            requested_intent,
        )
    }

    pub(super) fn replay_cancellation_settlement_for_goal(
        &self,
        refine_dir: &Path,
        goal_id: &str,
        requested_intent: TerminationIntent,
    ) -> RefineResult<Option<Value>> {
        let Some((receipt_path, journal)) =
            self.cancellation_settlement_journal_for_goal(goal_id)?
        else {
            return Ok(None);
        };
        let execution_id = journal.execution_ids.first().cloned();
        self.replay_cancellation_settlement_journal(
            refine_dir,
            receipt_path,
            journal,
            execution_id.as_deref(),
            requested_intent,
        )
    }

    fn replay_cancellation_settlement_journal(
        &self,
        refine_dir: &Path,
        receipt_path: PathBuf,
        mut journal: CancellationSettlementJournal,
        execution_id: Option<&str>,
        requested_intent: TerminationIntent,
    ) -> RefineResult<Option<Value>> {
        if journal.state == "rolled_back" {
            return Ok(None);
        }
        let journal_intent = journal.termination_intent.unwrap_or_else(|| {
            TerminationIntent::from_legacy_disposition(journal.goal_disposition)
        });
        if journal.state == "committed" && journal_intent != requested_intent {
            return Ok(None);
        }

        let _coordination = acquire_workflow_coordination(refine_dir)?;
        let workflow = WorkflowEngine::new(&self.runtime_root);
        let _workflow_lock = workflow.acquire_state_mutation_lock()?;
        let node_id = journal
            .goal_before
            .get("node_id")
            .and_then(Value::as_str)
            .filter(|node_id| !node_id.is_empty())
            .unwrap_or("default");
        let work_items = FileWorkItemService::for_node(refine_dir, node_id);
        let replay_goal_before = journal
            .replay_goal_before
            .as_ref()
            .unwrap_or(&journal.goal_before)
            .clone();
        let replay_goal_after = journal
            .replay_goal_after
            .as_ref()
            .unwrap_or(&journal.goal_after)
            .clone();
        let mut goal_transaction = work_items.prepare_goal_cancellation_replay(
            &journal.goal_id,
            &replay_goal_before,
            &replay_goal_after,
            journal.rollback_goal_state.as_ref(),
        )?;
        let exact_replay_before = goal_transaction.original_value();
        let exact_replay_after = goal_transaction.settled_value();
        let schema_upgrade_required = journal.schema_version < 5;
        journal.schema_version = 5;
        journal.termination_intent = Some(journal_intent);
        if schema_upgrade_required
            || journal.replay_goal_before.as_ref() != Some(&exact_replay_before)
            || journal.replay_goal_after.as_ref() != Some(&exact_replay_after)
        {
            journal.replay_goal_before = Some(exact_replay_before);
            journal.replay_goal_after = Some(exact_replay_after);
            self.write_cancellation_settlement_journal(&receipt_path, &journal)?;
        }
        let mut capacity = workflow
            .capacity_service_for_settlement()
            .begin_cancellation_settlement()?;

        let mut current_workflow = workflow.load_state()?;
        let mut workflow_changed = false;
        for claim_id in &journal.claim_ids {
            let before = journal
                .workflow_before
                .claims
                .iter()
                .find(|claim| claim.claim_id == *claim_id)
                .ok_or_else(|| {
                    RefineError::Serialization(format!(
                        "cancellation settlement journal {} has no before-state for claim {claim_id}",
                        receipt_path.display()
                    ))
                })?;
            let after = journal
                .workflow_after
                .claims
                .iter()
                .find(|claim| claim.claim_id == *claim_id)
                .ok_or_else(|| {
                    RefineError::Serialization(format!(
                        "cancellation settlement journal {} has no after-state for claim {claim_id}",
                        receipt_path.display()
                    ))
                })?;
            let current = current_workflow
                .claims
                .iter_mut()
                .find(|claim| claim.claim_id == *claim_id)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "workflow claim {claim_id} disappeared outside interrupted cancellation settlement {}; replay did not overwrite newer workflow state",
                        receipt_path.display()
                    ))
                })?;
            if current == after {
                continue;
            }
            if current != before {
                return Err(RefineError::Conflict(format!(
                    "workflow claim {claim_id} changed outside interrupted cancellation settlement {}; replay did not overwrite newer claim state",
                    receipt_path.display(),
                )));
            }
            *current = after.clone();
            workflow_changed = true;
        }
        if workflow_changed {
            if current_workflow.updated_at == journal.workflow_before.updated_at {
                current_workflow.updated_at = journal.workflow_after.updated_at.clone();
            }
            current_workflow.version = current_workflow.version.saturating_add(1);
            workflow.persist_state_preserving_policy_locked(&current_workflow)?;
        }
        self.update_cancellation_settlement_journal(
            &receipt_path,
            &mut journal,
            "claim_persisted",
            None,
            None,
        )?;

        capacity.replay_exact(&journal.capacity_before, &journal.claim_ids)?;
        self.update_cancellation_settlement_journal(
            &receipt_path,
            &mut journal,
            "capacity_released",
            None,
            None,
        )?;

        goal_transaction.commit()?;
        self.update_cancellation_settlement_journal(
            &receipt_path,
            &mut journal,
            "goal_persisted",
            None,
            None,
        )?;
        self.update_cancellation_settlement_journal(
            &receipt_path,
            &mut journal,
            "committed",
            None,
            None,
        )?;
        let worktree_retention = WorkflowWorktreeRetention::from_targets(&journal.worktrees);

        let mut terminations = Vec::new();
        let goal = goal_transaction.projection()?.goal;
        if let Some(execution_id) = execution_id {
            for claim_id in &journal.claim_ids {
                for recovered in self.recoverable_workflow_terminations(
                    &journal.goal_id,
                    claim_id,
                    execution_id,
                )? {
                    self.complete_outcome_receipt(
                        &recovered.ownership.process_id,
                        Some(&journal.goal_id),
                        &recovered.termination,
                        Some(&goal.status),
                        true,
                        Some(&worktree_retention),
                    )?;
                    terminations.push(recovered.termination);
                }
            }
        }
        if journal_intent != requested_intent {
            if requested_intent == TerminationIntent::ExplicitCancellation
                && journal_intent == TerminationIntent::InteractiveStop
            {
                return Ok(None);
            }
            return Err(RefineError::Conflict(format!(
                "settlement journal {} completed {:?}, not requested {:?}; durable Goal status is {}",
                receipt_path.display(),
                journal_intent,
                requested_intent,
                goal.status.as_str()
            )));
        }
        let mut result = json!({
            "execution_id": execution_id,
            "claim_id": journal.claim_ids.first(),
            "goal_id": journal.goal_id,
            "processes": terminations,
            "goal": goal,
            "goal_requeued": goal.status == GoalStatus::Todo,
            "worktree_retention": worktree_retention,
            "replayed_settlement": true,
            "termination_intent": journal_intent
        });
        if let Some(object) = result.as_object_mut() {
            match journal_intent {
                TerminationIntent::ExplicitCancellation => {
                    object.insert(
                        "cancelled".to_string(),
                        json!(goal.status == GoalStatus::Cancelled),
                    );
                }
                TerminationIntent::InteractiveStop => {
                    object.insert("stopped".to_string(), json!(true));
                }
            }
        }
        Ok(Some(result))
    }

    pub(super) fn settle_claim_cancellation_only(
        &self,
        goal_id: &str,
        ownership: &[WorkflowGoalOwnership],
    ) -> RefineResult<()> {
        let _coordination = acquire_workflow_coordination(&self.runtime_root)?;
        let workflow = WorkflowEngine::new(&self.runtime_root);
        let _workflow_lock = workflow.acquire_state_mutation_lock()?;
        let mut state = workflow.load_state()?;
        let mut claim_ids = Vec::new();
        for ownership in ownership {
            validate_workflow_goal_ownership_in_state(
                &state,
                goal_id,
                ownership,
                WorkflowOwnershipPhase::BeforeCancellation,
            )?;
            claim_ids.push(ownership.claim_id.clone());
        }
        claim_ids.sort();
        claim_ids.dedup();
        let original_state = state.clone();
        let mut capacity = workflow
            .capacity_service_for_settlement()
            .begin_cancellation_settlement()?;
        if let Err(cause) = workflow.persist_claims_cancelled_locked(&mut state, &claim_ids) {
            return Err(cause);
        }
        if let Err(cause) = capacity.release_claims(&claim_ids) {
            let _ = capacity.restore();
            let _ = workflow.restore_state_locked(&original_state);
            return Err(cause);
        }
        Ok(())
    }

    pub(super) fn cancellation_settlement_receipt_path(
        &self,
        goal_id: &str,
        claim_ids: &[String],
    ) -> PathBuf {
        let owner = claim_ids.first().map(String::as_str).unwrap_or("no-claim");
        self.runtime_root
            .join("process-stop-outcomes")
            .join(format!("workflow-cancellation-{goal_id}-{owner}.json"))
    }

    pub(super) fn cancellation_settlement_journal_for_execution(
        &self,
        execution_id: &str,
    ) -> RefineResult<Option<(PathBuf, CancellationSettlementJournal)>> {
        let mut matching = self
            .cancellation_settlement_journals()?
            .into_iter()
            .filter(|(_, journal)| {
                journal
                    .execution_ids
                    .iter()
                    .any(|candidate| candidate == execution_id)
            })
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(RefineError::Conflict(format!(
                "multiple cancellation settlement journals match workflow execution {execution_id}; recovery requires operator inspection"
            )));
        }
        Ok(matching.pop())
    }

    pub(super) fn cancellation_settlement_journal_for_goal(
        &self,
        goal_id: &str,
    ) -> RefineResult<Option<(PathBuf, CancellationSettlementJournal)>> {
        let mut matching = self
            .cancellation_settlement_journals()?
            .into_iter()
            .filter(|(_, journal)| {
                journal.goal_id == goal_id
                    && !matches!(journal.state.as_str(), "committed" | "rolled_back")
            })
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(RefineError::Conflict(format!(
                "multiple unfinished cancellation settlement journals match Goal {goal_id}; recovery requires operator inspection"
            )));
        }
        Ok(matching.pop())
    }

    fn cancellation_settlement_journals(
        &self,
    ) -> RefineResult<Vec<(PathBuf, CancellationSettlementJournal)>> {
        let directory = self.runtime_root.join("process-stop-outcomes");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to inspect cancellation settlement journals {}: {error}",
                    directory.display()
                )));
            }
        };
        let mut journals = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect cancellation settlement journal entry: {error}"
                ))
            })?;
            let path = entry.path();
            let is_journal = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("workflow-cancellation-") && name.ends_with(".json")
                });
            if !is_journal {
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read cancellation settlement journal {}: {error}",
                    path.display()
                ))
            })?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse cancellation settlement journal {}: {error}",
                    path.display()
                ))
            })?;
            if !matches!(
                value.get("schema_version").and_then(Value::as_u64),
                Some(2 | 3 | 4 | 5)
            ) {
                continue;
            }
            let journal: CancellationSettlementJournal =
                serde_json::from_value(value).map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to decode cancellation settlement journal {}: {error}",
                        path.display()
                    ))
                })?;
            journals.push((path, journal));
        }
        Ok(journals)
    }

    pub(super) fn write_cancellation_settlement_journal(
        &self,
        path: &Path,
        journal: &CancellationSettlementJournal,
    ) -> RefineResult<()> {
        let value = serde_json::to_value(journal).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode cancellation settlement journal: {error}"
            ))
        })?;
        write_json_receipt(path, &value)
    }

    pub(super) fn update_cancellation_settlement_journal(
        &self,
        path: &Path,
        journal: &mut CancellationSettlementJournal,
        state: &str,
        cause: Option<&str>,
        rollback_failure: Option<&str>,
    ) -> RefineResult<()> {
        journal.state = state.to_string();
        journal.recorded_at = Utc::now().to_rfc3339();
        let goal_persisted = matches!(state, "goal_persisted" | "committed");
        journal.goal_cancelled =
            goal_persisted && journal.goal_disposition == GoalStopDisposition::Cancel;
        journal.goal_requeued =
            goal_persisted && journal.goal_disposition == GoalStopDisposition::Requeue;
        journal.claim_cancelled = matches!(
            state,
            "claim_persisted" | "capacity_released" | "goal_persisted" | "committed"
        );
        journal.capacity_released =
            matches!(state, "capacity_released" | "goal_persisted" | "committed");
        journal.worktrees_retained = !journal.worktrees.is_empty();
        if let Some(cause) = cause {
            journal.cause = Some(cause.to_string());
        }
        if let Some(rollback_failure) = rollback_failure {
            journal.rollback_failure = Some(rollback_failure.to_string());
        }
        journal.recovery = cancellation_settlement_recovery(state).to_string();
        self.write_cancellation_settlement_journal(path, journal)
    }

    pub(super) fn inject_settlement_failure(
        &self,
        stage: CancellationSettlementFailureStage,
    ) -> RefineResult<()> {
        #[cfg(test)]
        if self.settlement_interruption == Some(stage) {
            panic!(
                "injected cancellation settlement interruption after {}",
                cancellation_settlement_stage_label(stage)
            );
        }
        #[cfg(test)]
        if self.settlement_failure == Some(stage) {
            return Err(RefineError::Io(format!(
                "injected cancellation settlement failure after {}",
                cancellation_settlement_stage_label(stage)
            )));
        }
        #[cfg(not(test))]
        let _ = stage;
        Ok(())
    }

    pub(super) fn inject_rollback_failure(
        &self,
        stage: CancellationRollbackFailureStage,
    ) -> RefineResult<()> {
        #[cfg(test)]
        if self.rollback_failure == Some(stage) {
            return Err(RefineError::Io(format!(
                "injected cancellation rollback failure during {}",
                match stage {
                    CancellationRollbackFailureStage::CapacityRestore => "capacity restore",
                    CancellationRollbackFailureStage::ClaimRestore => "claim restore",
                }
            )));
        }
        #[cfg(not(test))]
        let _ = stage;
        Ok(())
    }
}
