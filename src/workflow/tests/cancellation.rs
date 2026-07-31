use super::*;

#[test]
fn file_automation_draining_pause_preserves_active_work_and_replenishes_after_resume() {
    let temp_root = unique_temp_dir("automation-live-queue-replenish");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let marker_root = temp_root.join("parallel-markers");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&marker_root).unwrap();
    fs::write(
        target_root.join("app.py"),
        "def health():\n    return 'ok'\n",
    )
    .unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.py"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
        &smoke_ai,
        format!(
            "#!/bin/sh\n\
                 marker_root='{}'\n\
                 touch \"$marker_root/$(basename \"$PWD\")\"\n\
                 attempt=0\n\
                 while [ ! -f \"$marker_root/release\" ]; do\n\
                   attempt=$((attempt + 1))\n\
                   [ \"$attempt\" -ge 1500 ] && exit 42\n\
                   sleep 0.01\n\
                 done\n\
                 printf '%s\\n' 'parallel agent completed' > agent.txt\n\
                 printf '%s\\n' 'smoke-ai parallel goal-agent response'\n",
            marker_root.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_feature_summary("Ordered Feature", Some("FEAT1"), None, None, None)
        .unwrap();
    work_items
        .create_goal_summary("First Feature Goal", Some("FEATURE_FIRST"))
        .unwrap();
    work_items
        .append_goal_round_summary("FEATURE_FIRST", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("FEATURE_FIRST", GoalStatus::Todo)
        .unwrap();
    work_items
        .assign_goal_to_feature("FEAT1", "FEATURE_FIRST")
        .unwrap();
    work_items
        .order_goal_in_feature("FEAT1", "FEATURE_FIRST")
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "parallel_run_cap": 4,
            "parallel_per_node_cap": 4,
            "parallel_per_provider_cap": 4,
            "parallel_per_target_app_cap": 4,
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);
    let evaluation_automation = automation.clone();
    let evaluation_thread =
        std::thread::spawn(move || evaluation_automation.execute_claimed_work());

    let initial_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while fs::read_dir(&marker_root).unwrap().count() < 1
        && std::time::Instant::now() < initial_deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_dir(&marker_root).unwrap().count(), 1);
    automation.set_workflow_paused(true).unwrap();
    assert_eq!(
        work_items
            .show_goal_summary("FEATURE_FIRST")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    assert!(automation.load_state().unwrap().claims.iter().any(|claim| {
        claim.goal_id == "FEATURE_FIRST" && claim.state == WorkflowClaimState::Running
    }));

    for goal_id in ["STANDALONE_ONE", "STANDALONE_TWO", "FEATURE_SECOND"] {
        work_items
            .create_goal_summary(goal_id, Some(goal_id))
            .unwrap();
        work_items
            .append_goal_round_summary(goal_id, "Reporter", "Prompt")
            .unwrap();
        work_items
            .transition_goal_status(goal_id, GoalStatus::Todo)
            .unwrap();
    }
    work_items
        .assign_goal_to_feature("FEAT1", "FEATURE_SECOND")
        .unwrap();
    work_items
        .order_goal_in_feature("FEAT1", "FEATURE_SECOND")
        .unwrap();

    std::thread::sleep(ACTIVE_WORK_REPLENISH_INTERVAL + Duration::from_millis(250));
    assert_eq!(
        fs::read_dir(&marker_root).unwrap().count(),
        1,
        "paused workflow launched newly queued Goals"
    );
    for goal_id in ["STANDALONE_ONE", "STANDALONE_TWO", "FEATURE_SECOND"] {
        assert_eq!(
            work_items.show_goal_summary(goal_id).unwrap().goal.status,
            GoalStatus::Todo
        );
    }

    automation.set_workflow_paused(false).unwrap();
    let replenish_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while fs::read_dir(&marker_root).unwrap().count() < 3
        && std::time::Instant::now() < replenish_deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let markers = fs::read_dir(&marker_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(markers.len(), 3);
    assert!(
        markers
            .iter()
            .any(|marker| marker.contains("STANDALONE_ONE"))
    );
    assert!(
        markers
            .iter()
            .any(|marker| marker.contains("STANDALONE_TWO"))
    );
    assert!(
        !markers
            .iter()
            .any(|marker| marker.contains("FEATURE_SECOND"))
    );
    assert_eq!(
        work_items
            .show_goal_summary("FEATURE_SECOND")
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );

    fs::write(marker_root.join("release"), "release\n").unwrap();
    let result = evaluation_thread.join().unwrap().unwrap();
    for goal_id in ["FEATURE_FIRST", "STANDALONE_ONE", "STANDALONE_TWO"] {
        assert!(result.iter().any(|step| step.goal_id == goal_id));
    }

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_pause_gate_blocks_new_claims_and_keeps_explicit_cancellation_separate() {
    let temp_root = unique_temp_dir("automation-controls");
    let automation = WorkflowEngine::new(temp_root.join("run/8080"));

    automation.set_workflow_paused(true).unwrap();
    assert!(automation.claim("GOAL1").is_err());
    automation.set_workflow_paused(false).unwrap();

    let claim_id = automation.claim("GOAL1").unwrap();
    assert_eq!(automation.claim("GOAL1").unwrap(), claim_id);
    let execution_id = automation.start_claim(&claim_id).unwrap();
    automation.cancel(&execution_id).unwrap();
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims[0].state, WorkflowClaimState::Cancelled);

    let retried_execution_id = automation.retry(&execution_id).unwrap();
    assert_ne!(retried_execution_id, execution_id);
    assert!(retried_execution_id.starts_with("exec-"));
    let state = automation.load_state().unwrap();
    assert_eq!(
        state.claims[0].execution_id.as_deref(),
        Some(retried_execution_id.as_str())
    );
    assert_eq!(state.claims[0].state, WorkflowClaimState::Running);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn workflow_cancel_reaches_managed_agent_registry_before_retry() {
    let temp_root = unique_temp_dir("automation-managed-cancel");
    let runtime_root = temp_root.join("run/8080");
    let automation = WorkflowEngine::new(&runtime_root);
    let claim_id = automation.claim("GOAL1").unwrap();
    let execution_id = automation.start_claim(&claim_id).unwrap();
    let agent_supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    agent_supervisor
        .register(ManagedProcess {
            id: "workflow-provider".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: Some(child.id()),
            state: "running".to_string(),
            label: Some("sleep".to_string()),
            details: Some(
                json!({
                    "kind": "workflow",
                    "execution_id": execution_id,
                    "goal_id": "GOAL1"
                })
                .to_string(),
            ),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    automation.cancel(&execution_id).unwrap();
    assert_eq!(
        automation.load_state().unwrap().claims[0].state,
        WorkflowClaimState::Cancelled
    );
    for _ in 0..40 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(child.try_wait().unwrap().is_some());
    assert!(
        agent_supervisor
            .recover_owner(crate::process::subprocess::ProcessOwner::Agent)
            .unwrap()
            .is_empty()
    );

    let retried_execution_id = automation.retry(&execution_id).unwrap();
    assert_ne!(retried_execution_id, execution_id);
    assert_eq!(
        automation.load_state().unwrap().claims[0].state,
        WorkflowClaimState::Running
    );
    automation.release_claim_capacity(&claim_id).unwrap();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn real_workflow_cancellation_before_registration_fails_closed_then_stops_registered_worker() {
    let temp_root = unique_temp_dir("workflow-cancel-before-registration");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::write(target_root.join("app.txt"), "initial\n").unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.txt"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\n\
             trap 'exit 143' TERM\n\
             while :; do sleep 0.05; done\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Cancel before registration", Some("GOAL-PRE-REGISTER"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL-PRE-REGISTER", "Reporter", "Wait until cancelled")
        .unwrap();
    work_items
        .transition_goal_status("GOAL-PRE-REGISTER", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
    let hook_release = std::sync::Arc::clone(&release_rx);
    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .with_before_worker_prepare_hook(move |claim_id, execution_id| {
            started_tx
                .send((claim_id.to_string(), execution_id.to_string()))
                .unwrap();
            hook_release.lock().unwrap().recv().unwrap();
        });
    automation.claim("GOAL-PRE-REGISTER").unwrap();
    let worker_automation = automation.clone();
    let worker = std::thread::spawn(move || worker_automation.execute_claimed_work());
    let (claim_id, execution_id) = started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("real workflow worker did not reach the pre-registration boundary");

    let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_workflow_execution(&execution_id)
        .unwrap_err();
    assert!(matches!(error, RefineError::Conflict(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("empty lookup is not confirmed process exit"),
        "{error}"
    );
    assert!(
        FileProcessSupervisor::new(&runtime_root)
            .list()
            .unwrap()
            .is_empty()
    );
    assert!(
        FileProcessSupervisor::new(runtime_root.join("agents"))
            .list()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-PRE-REGISTER")
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );
    let state = automation.load_state().unwrap();
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.claim_id == claim_id)
        .unwrap();
    assert_eq!(claim.state, WorkflowClaimState::Running);
    assert_eq!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .len(),
        1
    );

    release_tx.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let registered = [&runtime_root, &runtime_root.join("agents")]
            .into_iter()
            .any(|root| {
                FileProcessSupervisor::new(root)
                    .list()
                    .unwrap()
                    .iter()
                    .any(|process| {
                        process
                            .details
                            .as_deref()
                            .and_then(|details| serde_json::from_str::<Value>(details).ok())
                            .and_then(|details| {
                                details
                                    .get("execution_id")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                            .as_deref()
                            == Some(execution_id.as_str())
                    })
            });
        if registered {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real workflow worker did not register after cancellation failed closed"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let cancelled = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_workflow_execution(&execution_id)
        .unwrap();
    assert_eq!(cancelled["cancelled"], true);
    assert!(worker.join().unwrap().is_err());
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-PRE-REGISTER")
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );
    assert_eq!(
        automation
            .load_state()
            .unwrap()
            .claims
            .iter()
            .find(|claim| claim.claim_id == claim_id)
            .unwrap()
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

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_cancel_stops_real_workflow_worker_and_worker_failure_cannot_overwrite_settlement() {
    let temp_root = unique_temp_dir("workflow-real-worker-cancel");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::write(target_root.join("app.txt"), "initial\n").unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.txt"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\n\
             trap 'exit 143' TERM\n\
             while :; do sleep 0.05; done\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Cancel real worker", Some("GOAL-REAL-CANCEL"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL-REAL-CANCEL", "Reporter", "Wait until cancelled")
        .unwrap();
    work_items
        .transition_goal_status("GOAL-REAL-CANCEL", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();
    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    automation.claim("GOAL-REAL-CANCEL").unwrap();
    let worker_runtime = runtime_root.clone();
    let worker_target = target_root.clone();
    let worker = std::thread::spawn(move || {
        WorkflowEngine::with_target_root(worker_runtime, worker_target).execute_claimed_work()
    });

    let (execution_id, process) = {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let execution_id = automation
                .load_state()
                .unwrap()
                .claims
                .iter()
                .find(|claim| claim.goal_id == "GOAL-REAL-CANCEL")
                .and_then(|claim| claim.execution_id.clone());
            if let Some(execution_id) = execution_id
                && let Some(process) = [&runtime_root, &runtime_root.join("agents")]
                    .into_iter()
                    .find_map(|root| {
                        FileProcessSupervisor::new(root)
                            .list()
                            .unwrap()
                            .into_iter()
                            .find(|process| {
                                process
                                    .details
                                    .as_deref()
                                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                                    .and_then(|details| {
                                        details
                                            .get("execution_id")
                                            .and_then(Value::as_str)
                                            .map(|candidate| candidate == execution_id)
                                    })
                                    .unwrap_or(false)
                            })
                    })
            {
                break (execution_id, process);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "real workflow worker did not register a managed process; state={:?}; goal={:?}; worker_finished={}",
                automation.load_state().unwrap(),
                work_items
                    .show_goal_summary("GOAL-REAL-CANCEL")
                    .unwrap()
                    .goal,
                worker.is_finished()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    let (at_settlement_tx, at_settlement_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Arc::new(std::sync::Mutex::new(release_rx));
    let hook_release = std::sync::Arc::clone(&release_rx);
    let control = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .with_settlement_hook(move || {
            at_settlement_tx.send(()).unwrap();
            hook_release.lock().unwrap().recv().unwrap();
        });
    let cancel_execution = execution_id.clone();
    let cancel_thread =
        std::thread::spawn(move || control.cancel_workflow_execution(&cancel_execution));
    at_settlement_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("shared cancellation did not reach atomic settlement");
    release_tx.send(()).unwrap();

    let cancelled = cancel_thread.join().unwrap().unwrap();
    assert_eq!(cancelled["cancelled"], true);
    let worker_result = worker.join().unwrap();
    assert!(
        worker_result.is_err(),
        "the killed workflow worker must report its causal failure"
    );
    let goal = work_items.show_goal_summary("GOAL-REAL-CANCEL").unwrap();
    assert_eq!(goal.goal.status, GoalStatus::Cancelled);
    let state = automation.load_state().unwrap();
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.execution_id.as_deref() == Some(&execution_id))
        .unwrap();
    assert_eq!(claim.state, WorkflowClaimState::Cancelled);
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );
    assert!(!FileProcessSupervisor::process_is_alive(&process).unwrap());
    let receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["state"], "completed");
    assert_eq!(receipt["confirmed_exit"], true);
    assert_eq!(receipt["goal_cancelled"], true);
    assert_eq!(receipt["claim_cancelled"], true);

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_pause_preserves_in_progress_goal_and_claim() {
    let temp_root = unique_temp_dir("automation-pause-drain");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Running work", Some("GOAL1"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let claim_id = automation.claim("GOAL1").unwrap();
    automation.start_claim(&claim_id).unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();

    let pause_state = automation.set_workflow_paused(true).unwrap();
    assert!(pause_state.workflow_paused);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::InProgress
    );
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims[0].state, WorkflowClaimState::Running);

    automation.set_workflow_paused(false).unwrap();
    assert!(!automation.workflow_paused().unwrap());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_recovery_fails_interrupted_goals_for_restart() {
    let temp_root = unique_temp_dir("automation-interrupted-recovery");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Interrupted work", Some("GOAL1"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let claim_id = automation.claim("GOAL1").unwrap();
    automation.start_claim(&claim_id).unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();

    assert_eq!(
        automation
            .recover_interrupted_goals("runner terminated")
            .unwrap(),
        1
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Failed
    );
    assert_eq!(
        automation.load_state().unwrap().claims[0].state,
        WorkflowClaimState::Interrupted
    );
    let logs = FileLogService::new(&refine_dir)
        .all_round_logs("GOAL1")
        .unwrap();
    assert!(logs.iter().any(|entry| {
        entry.entry.severity == "error"
            && entry
                .entry
                .message
                .contains("interrupted during in-progress work")
            && entry.entry.message.contains("runner terminated")
    }));

    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn restart_retains_ready_merge_checkpoint_for_automatic_resume() {
    let temp_root = unique_temp_dir("automation-checkpoint-recovery");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Checkpointed work", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let claim_id = automation.claim("GOAL1").unwrap();
    automation.start_claim(&claim_id).unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
        .unwrap();

    assert_eq!(
        automation
            .recover_interrupted_goals("runner terminated")
            .unwrap(),
        1
    );

    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::ReadyMerge
    );
    assert_eq!(
        automation.load_state().unwrap().claims[0].state,
        WorkflowClaimState::Interrupted
    );
    assert_eq!(automation.promote().unwrap(), 1);
    let resumed = automation.load_state().unwrap();
    assert!(
        resumed.claims.iter().any(|claim| {
            claim.goal_id == "GOAL1" && claim.state == WorkflowClaimState::Claimed
        })
    );
    let logs = FileLogService::new(&refine_dir)
        .all_round_logs("GOAL1")
        .unwrap();
    assert!(logs.iter().any(|entry| {
        entry.entry.severity == "warning"
            && entry
                .entry
                .message
                .contains("durable ready-merge checkpoint")
            && entry.entry.message.contains("automatic resume retained")
    }));

    fs::remove_dir_all(temp_root).unwrap();
}
