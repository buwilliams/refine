use super::*;

#[test]
fn cancellation_settlement_failures_restore_exact_durable_state_and_are_recoverable() {
    for (suffix, stage, expected_cause) in [
        (
            "claim",
            CancellationSettlementFailureStage::AfterClaimPersistence,
            "after claim persistence",
        ),
        (
            "capacity",
            CancellationSettlementFailureStage::AfterCapacityRelease,
            "after capacity release",
        ),
        (
            "goal",
            CancellationSettlementFailureStage::AfterGoalPersistence,
            "after Goal persistence",
        ),
    ] {
        let temp_root = unique_temp_dir(&format!("process-control-settlement-{suffix}"));
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        let goal_id = format!("GOAL-SETTLEMENT-{}", suffix.to_uppercase());
        let claim_id = format!("claim-settlement-{suffix}");
        let execution_id = format!("exec-settlement-{suffix}");
        create_in_progress_goal_with_rounds(&refine_dir, &goal_id, 1);
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_workflow_agent(&supervisor, &goal_id, &claim_id, &execution_id, 0);
        write_workflow_state(
            &runtime_root,
            json!([{
                "claim_id": claim_id,
                "goal_id": goal_id,
                "execution_id": execution_id,
                "state": "running",
                "created_at": "2026-07-23T00:00:00Z",
                "updated_at": "2026-07-23T00:00:00Z"
            }]),
        );
        reserve_workflow_capacity(&runtime_root, &claim_id);

        let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .with_settlement_failure(stage)
            .stop(&process.id, "terminate")
            .unwrap_err();
        assert!(error.to_string().contains(expected_cause), "{error}");
        assert!(
            error
                .to_string()
                .contains("restored to its pre-settlement state"),
            "{error}"
        );
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary(&goal_id)
                .unwrap()
                .goal
                .status,
            GoalStatus::InProgress
        );
        let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
        let claim = state
            .claims
            .iter()
            .find(|claim| claim.claim_id == claim_id)
            .unwrap();
        assert_eq!(claim.state, WorkflowClaimState::Running);
        let capacity = AgentCapacityService::new(&runtime_root).snapshot().unwrap();
        assert_eq!(capacity.leases.len(), 1);
        assert_eq!(capacity.leases[0].owner_id, format!("workflow:{claim_id}"));
        let transaction_receipt: Value = serde_json::from_slice(
            &fs::read(
                runtime_root
                    .join("process-stop-outcomes")
                    .join(format!("workflow-cancellation-{goal_id}-{claim_id}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(transaction_receipt["state"], "rolled_back");
        assert!(
            transaction_receipt["recovery"]
                .as_str()
                .unwrap()
                .contains("retry cancellation through the shared capability")
        );
        let process_receipt: Value = serde_json::from_slice(
            &fs::read(
                runtime_root
                    .join("process-stop-outcomes")
                    .join(format!("{}.json", process.id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(process_receipt["state"], "partial_failure");
        assert_eq!(process_receipt["confirmed_exit"], true);
        assert_eq!(process_receipt["workflow"]["execution_id"], execution_id);

        let recovered = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .cancel_workflow_execution(&execution_id)
            .unwrap();
        assert_eq!(recovered["cancelled"], true);
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary(&goal_id)
                .unwrap()
                .goal
                .status,
            GoalStatus::Cancelled
        );
        assert_eq!(
            WorkflowEngine::new(&runtime_root)
                .load_state()
                .unwrap()
                .claims[0]
                .state,
            WorkflowClaimState::Cancelled
        );
        assert!(
            AgentCapacityService::new(&runtime_root)
                .snapshot()
                .unwrap()
                .leases
                .is_empty()
        );

        remove_temp_dir(&temp_root);
    }
}

#[test]
fn rollback_failed_after_goal_restore_replays_from_exact_restored_revision() {
    let temp_root = unique_temp_dir("process-control-rollback-failed-replay");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-ROLLBACK-REPLAY";
    let claim_id = "claim-rollback-replay";
    let execution_id = "exec-rollback-replay";
    let policy = non_default_workflow_policy();
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    write_workflow_state_with_policy(
        &runtime_root,
        json!([{
            "claim_id": claim_id,
            "goal_id": goal_id,
            "node_id": policy.active_node_id,
            "provider": policy.provider,
            "target_app_id": policy.target_app_id,
            "execution_id": execution_id,
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
        &policy,
    );
    reserve_workflow_capacity_with_policy(&runtime_root, claim_id, &policy);
    let policy_bytes = serde_json::to_vec(&policy).unwrap();

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_settlement_failure(CancellationSettlementFailureStage::AfterGoalPersistence)
        .with_rollback_failure(CancellationRollbackFailureStage::CapacityRestore)
        .stop(&process.id, "terminate")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected cancellation rollback failure during capacity restore"),
        "{error}"
    );
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );

    let journal_path = runtime_root
        .join("process-stop-outcomes")
        .join(format!("workflow-cancellation-{goal_id}-{claim_id}.json"));
    let failed: CancellationSettlementJournal =
        serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(failed.state, "rollback_failed");
    assert_eq!(failed.rollback_goal_restored, Some(true));
    assert_eq!(failed.rollback_capacity_restored, Some(false));
    assert_eq!(failed.rollback_claim_restored, Some(true));
    let restored_goal = failed.rollback_goal_state.as_ref().unwrap();
    assert_ne!(
        workflow_revision(restored_goal),
        workflow_revision(&failed.goal_before)
    );
    assert_eq!(
        restored_goal.get("status"),
        failed.goal_before.get("status")
    );

    let replayed = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_workflow_execution(execution_id)
        .unwrap();
    assert_eq!(replayed["cancelled"], true);
    assert_eq!(replayed["replayed_settlement"], true);
    assert_eq!(replayed["goal"]["status"], "cancelled");
    let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
    assert_eq!(serde_json::to_vec(&state.policy).unwrap(), policy_bytes);
    assert_eq!(state.claims[0].state, WorkflowClaimState::Cancelled);
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );
    let committed: CancellationSettlementJournal =
        serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(committed.state, "committed");
    assert_eq!(committed.rollback_goal_restored, Some(true));
    assert_eq!(committed.rollback_capacity_restored, Some(false));
    assert_eq!(committed.rollback_claim_restored, Some(true));
    assert_eq!(
        workflow_revision(committed.replay_goal_before.as_ref().unwrap()),
        workflow_revision(restored_goal)
    );
    assert_eq!(
        workflow_revision(committed.replay_goal_after.as_ref().unwrap()),
        workflow_revision(restored_goal).saturating_add(1)
    );
    assert!(
        committed
            .rollback_failure
            .as_deref()
            .is_some_and(|failure| failure.contains("capacity restore"))
    );
    let process_receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(process_receipt["state"], "completed");
    assert_eq!(process_receipt["goal_cancelled"], true);
    assert_eq!(process_receipt["claim_cancelled"], true);
    let repeated = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_workflow_execution(execution_id)
        .unwrap();
    assert_eq!(repeated["cancelled"], true);
    assert_eq!(repeated["replayed_settlement"], true);

    remove_temp_dir(&temp_root);
}

#[test]
fn cancellation_journal_writer_exposes_synced_atomic_boundaries() {
    let temp_root = unique_temp_dir("process-control-journal-durability");
    let journal_path = temp_root
        .join("process-stop-outcomes")
        .join("workflow-cancellation-GOAL-claim.json");
    write_json_receipt(&journal_path, &json!({"state": "prepared"})).unwrap();

    let before_rename = write_json_receipt_with_boundary(
        &journal_path,
        &json!({"state": "claim_persisted"}),
        |boundary| {
            if boundary == DurableReceiptBoundary::FileSyncedBeforeRename {
                Err(RefineError::Io(
                    "injected crash after journal file sync".to_string(),
                ))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert!(
        before_rename
            .to_string()
            .contains("injected crash after journal file sync")
    );
    let retained: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(retained["state"], "prepared");

    let after_rename = write_json_receipt_with_boundary(
        &journal_path,
        &json!({"state": "capacity_released"}),
        |boundary| {
            if boundary == DurableReceiptBoundary::RenamedBeforeDirectorySync {
                Err(RefineError::Io(
                    "injected crash before journal directory sync".to_string(),
                ))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert!(
        after_rename
            .to_string()
            .contains("injected crash before journal directory sync")
    );
    let replaced: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(replaced["state"], "capacity_released");

    let mut boundaries = Vec::new();
    write_json_receipt_with_boundary(&journal_path, &json!({"state": "committed"}), |boundary| {
        boundaries.push(boundary);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        boundaries,
        vec![
            DurableReceiptBoundary::FileSyncedBeforeRename,
            DurableReceiptBoundary::RenamedBeforeDirectorySync,
            DurableReceiptBoundary::DirectorySynced,
        ]
    );
    let committed: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(committed["state"], "committed");
    assert!(
        fs::read_dir(journal_path.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn interrupted_settlement_replays_after_restart_before_cancelled_short_circuit() {
    for (suffix, stage, interrupted_state) in [
        (
            "claim",
            CancellationSettlementFailureStage::AfterClaimPersistence,
            "claim_persisted",
        ),
        (
            "capacity",
            CancellationSettlementFailureStage::AfterCapacityRelease,
            "capacity_released",
        ),
        (
            "goal",
            CancellationSettlementFailureStage::AfterGoalPersistence,
            "goal_persisted",
        ),
    ] {
        let temp_root = unique_temp_dir(&format!("process-control-restart-{suffix}"));
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        let goal_id = format!("GOAL-RESTART-{}", suffix.to_uppercase());
        let claim_id = format!("claim-restart-{suffix}");
        let execution_id = format!("exec-restart-{suffix}");
        let policy = non_default_workflow_policy();
        create_in_progress_goal_with_rounds(&refine_dir, &goal_id, 1);
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_workflow_agent(&supervisor, &goal_id, &claim_id, &execution_id, 0);
        write_workflow_state_with_policy(
            &runtime_root,
            json!([{
                "claim_id": claim_id,
                "goal_id": goal_id,
                "node_id": policy.active_node_id,
                "provider": policy.provider,
                "target_app_id": policy.target_app_id,
                "execution_id": execution_id,
                "state": "running",
                "created_at": "2026-07-23T00:00:00Z",
                "updated_at": "2026-07-23T00:00:00Z"
            }]),
            &policy,
        );
        reserve_workflow_capacity_with_policy(&runtime_root, &claim_id, &policy);
        let policy_bytes = serde_json::to_vec(&policy).unwrap();

        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
                .with_settlement_interruption(stage)
                .stop(&process.id, "terminate")
                .unwrap();
        }));
        assert!(interrupted.is_err());
        assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());

        let mut concurrent_workflow = WorkflowEngine::new(&runtime_root).load_state().unwrap();
        concurrent_workflow.claims.push(WorkflowClaim {
            claim_id: format!("claim-unrelated-{suffix}"),
            goal_id: format!("GOAL-UNRELATED-{}", suffix.to_uppercase()),
            node_id: policy.active_node_id.clone(),
            provider: policy.provider.clone(),
            target_app_id: policy.target_app_id.clone(),
            execution_id: None,
            round_idx: None,
            goal_revision: None,
            decision_version: 1,
            state: WorkflowClaimState::Claimed,
            created_at: "2026-07-23T00:03:00Z".to_string(),
            updated_at: "2026-07-23T00:03:00Z".to_string(),
        });
        concurrent_workflow.updated_at = Some("2026-07-23T00:03:00Z".to_string());
        concurrent_workflow.version = concurrent_workflow.version.saturating_add(1);
        WorkflowEngine::new(&runtime_root)
            .persist_state_preserving_policy_locked(&concurrent_workflow)
            .unwrap();
        assert!(
            AgentCapacityService::new(&runtime_root)
                .try_acquire(
                    &policy,
                    AgentCapacityRequest {
                        owner_id: format!("supervisor:unrelated-{suffix}"),
                        role: "supervisor".to_string(),
                        node_id: policy.active_node_id.clone(),
                        provider: policy.provider.clone(),
                        target_app_id: policy.target_app_id.clone(),
                    },
                )
                .unwrap()
        );

        let journal_path = runtime_root
            .join("process-stop-outcomes")
            .join(format!("workflow-cancellation-{goal_id}-{claim_id}.json"));
        let interrupted_journal: CancellationSettlementJournal =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert_eq!(interrupted_journal.state, interrupted_state);
        assert_eq!(
            serde_json::to_vec(&interrupted_journal.workflow_before.policy).unwrap(),
            policy_bytes
        );
        assert_eq!(
            serde_json::to_vec(&interrupted_journal.workflow_after.policy).unwrap(),
            policy_bytes
        );
        assert_eq!(interrupted_journal.workflow_before.version, 0);
        assert_eq!(interrupted_journal.workflow_after.version, 1);

        let replayed = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .cancel_workflow_execution(&execution_id)
            .unwrap();
        assert_eq!(replayed["cancelled"], true);
        assert_eq!(replayed["replayed_settlement"], true);
        assert_eq!(replayed["goal"]["status"], "cancelled");

        let committed: CancellationSettlementJournal =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert_eq!(committed.state, "committed");
        let state = WorkflowEngine::new(&runtime_root).load_state().unwrap();
        assert_eq!(serde_json::to_vec(&state.policy).unwrap(), policy_bytes);
        assert_eq!(state.version, 2);
        assert_eq!(
            state
                .claims
                .iter()
                .find(|claim| claim.claim_id == claim_id)
                .unwrap()
                .state,
            WorkflowClaimState::Cancelled
        );
        assert_eq!(
            state
                .claims
                .iter()
                .find(|claim| claim.claim_id == claim_id)
                .unwrap()
                .decision_version,
            1
        );
        assert_eq!(
            state
                .claims
                .iter()
                .find(|claim| claim.claim_id == format!("claim-unrelated-{suffix}"))
                .unwrap()
                .state,
            WorkflowClaimState::Claimed
        );
        let capacity = AgentCapacityService::new(&runtime_root).snapshot().unwrap();
        assert_eq!(capacity.leases.len(), 1);
        assert_eq!(
            capacity.leases[0].owner_id,
            format!("supervisor:unrelated-{suffix}")
        );
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary(&goal_id)
                .unwrap()
                .goal
                .status,
            GoalStatus::Cancelled
        );
        let process_receipt: Value = serde_json::from_slice(
            &fs::read(
                runtime_root
                    .join("process-stop-outcomes")
                    .join(format!("{}.json", process.id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(process_receipt["state"], "completed");
        assert_eq!(process_receipt["goal_cancelled"], true);
        assert_eq!(process_receipt["claim_cancelled"], true);

        remove_temp_dir(&temp_root);
    }
}

#[test]
fn registry_cleanup_failure_retains_confirmed_exit_before_goal_settlement() {
    let temp_root = unique_temp_dir("process-control-registry-cleanup-failure");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    create_in_progress_goal_with_rounds(&refine_dir, "GOAL-REGISTRY-CLEANUP", 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = register_workflow_agent(
        &supervisor,
        "GOAL-REGISTRY-CLEANUP",
        "claim-current",
        "exec-current",
        0,
    );
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": "claim-current",
            "goal_id": "GOAL-REGISTRY-CLEANUP",
            "execution_id": "exec-current",
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_cleanup_failure(ProcessCleanupStage::Registry)
        .stop(&process.id, "terminate")
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("confirmed_exit=true"), "{message}");
    assert!(
        message.contains("registry_cleanup_completed=false"),
        "{message}"
    );
    assert!(
        message.contains("identity_cleanup_completed=false"),
        "{message}"
    );
    assert!(message.contains("goal_cancelled=false"), "{message}");
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert!(supervisor.inspect(&process.id).is_ok());
    assert!(
        runtime_root
            .join("agents/process-identities")
            .join(format!("{}.json", process.id))
            .exists()
    );
    assert_partial_cleanup_receipt(
        &runtime_root,
        &process.id,
        false,
        false,
        "injected registry cleanup failure",
    );
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL-REGISTRY-CLEANUP")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    assert_eq!(
        WorkflowEngine::new(&runtime_root)
            .load_state()
            .unwrap()
            .claims[0]
            .state,
        WorkflowClaimState::Running
    );

    remove_temp_dir(&temp_root);
}
