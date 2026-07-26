use super::*;

impl FileProcessControlService {
    pub(super) fn settle_goal_cancellation(
        &self,
        refine_dir: &Path,
        goal_id: &str,
        expectation: &GoalCancellationExpectation,
        ownership: &[WorkflowGoalOwnership],
    ) -> RefineResult<crate::tools::product::project_state::GoalSummaryProjection> {
        let _coordination = acquire_workflow_coordination(refine_dir)?;
        let workflow = WorkflowEngine::new(&self.runtime_root);
        let _workflow_lock = workflow.acquire_state_mutation_lock()?;
        let work_items = FileWorkItemService::new(refine_dir);
        let mut goal_transaction =
            work_items.prepare_goal_cancellation_if_current(goal_id, expectation)?;
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
        let goal_after = goal_transaction.cancelled_value();
        let mut execution_ids = ownership
            .iter()
            .map(|ownership| ownership.execution_id.clone())
            .collect::<Vec<_>>();
        execution_ids.sort();
        execution_ids.dedup();
        let receipt_path = self.cancellation_settlement_receipt_path(goal_id, &claim_ids);
        let mut journal = CancellationSettlementJournal {
            schema_version: 2,
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
            recorded_at: Utc::now().to_rfc3339(),
            goal_cancelled: false,
            claim_cancelled: false,
            capacity_released: false,
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

        goal_transaction.projection()
    }

    pub(super) fn replay_cancellation_settlement(
        &self,
        refine_dir: &Path,
        execution_id: &str,
    ) -> RefineResult<Option<Value>> {
        let Some((receipt_path, mut journal)) =
            self.cancellation_settlement_journal_for_execution(execution_id)?
        else {
            return Ok(None);
        };
        if journal.state == "rolled_back" {
            return Ok(None);
        }

        let _coordination = acquire_workflow_coordination(refine_dir)?;
        let workflow = WorkflowEngine::new(&self.runtime_root);
        let _workflow_lock = workflow.acquire_state_mutation_lock()?;
        let work_items = FileWorkItemService::new(refine_dir);
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
        let exact_replay_after = goal_transaction.cancelled_value();
        if journal.replay_goal_before.as_ref() != Some(&exact_replay_before)
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

        let mut terminations = Vec::new();
        for claim_id in &journal.claim_ids {
            for recovered in
                self.recoverable_workflow_terminations(&journal.goal_id, claim_id, execution_id)?
            {
                self.complete_outcome_receipt(
                    &recovered.ownership.process_id,
                    Some(&journal.goal_id),
                    &recovered.termination,
                    true,
                    true,
                )?;
                terminations.push(recovered.termination);
            }
        }
        let goal = goal_transaction.projection()?.goal;
        Ok(Some(json!({
            "cancelled": true,
            "execution_id": execution_id,
            "claim_id": journal.claim_ids.first(),
            "goal_id": journal.goal_id,
            "processes": terminations,
            "goal": goal,
            "replayed_settlement": true
        })))
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
        let directory = self.runtime_root.join("process-stop-outcomes");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to inspect cancellation settlement journals {}: {error}",
                    directory.display()
                )));
            }
        };
        let mut matching = Vec::new();
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
            if value.get("schema_version").and_then(Value::as_u64) != Some(2) {
                continue;
            }
            let journal: CancellationSettlementJournal =
                serde_json::from_value(value).map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to decode cancellation settlement journal {}: {error}",
                        path.display()
                    ))
                })?;
            if journal
                .execution_ids
                .iter()
                .any(|candidate| candidate == execution_id)
            {
                matching.push((path, journal));
            }
        }
        if matching.len() > 1 {
            return Err(RefineError::Conflict(format!(
                "multiple cancellation settlement journals match workflow execution {execution_id}; recovery requires operator inspection"
            )));
        }
        Ok(matching.pop())
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
        journal.goal_cancelled = state == "committed" || state == "goal_persisted";
        journal.claim_cancelled = matches!(
            state,
            "claim_persisted" | "capacity_released" | "goal_persisted" | "committed"
        );
        journal.capacity_released =
            matches!(state, "capacity_released" | "goal_persisted" | "committed");
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
