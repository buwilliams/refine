use super::*;

#[test]
fn file_automation_enforces_configured_concurrency_limits() {
    let temp_root = unique_temp_dir("automation-limits");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "parallel_per_provider_cap": 1,
            "parallel_per_target_app_cap": 2,
            "agent_cli": "smoke-ai"
        }))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    for id in ["GOAL1", "GOAL2", "GOAL3"] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
    }

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);
    assert_eq!(automation.promote().unwrap(), 0);
    let state = automation.load_state().unwrap();
    assert_eq!(state.policy.provider, "smoke-ai");
    assert_eq!(state.policy.per_provider_limit, 1);
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].provider, "smoke-ai");
    assert_eq!(state.claims[0].node_id, "default");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_enforces_active_node_ownership() {
    let temp_root = unique_temp_dir("automation-node-ownership");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Local", Some("LOCAL"))
        .unwrap();
    work_items
        .transition_goal_status("LOCAL", GoalStatus::Todo)
        .unwrap();
    work_items
        .create_goal_summary("Remote", Some("REMOTE"))
        .unwrap();
    work_items
        .transition_goal_status("REMOTE", GoalStatus::Todo)
        .unwrap();
    FileNodeRegistryService::new(&refine_dir)
        .create("remote-node")
        .unwrap();
    work_items
        .bulk_transfer_goals_to_node(
            "remote-node",
            BulkGoalSelection {
                selected_ids: Some(vec!["REMOTE".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);
    assert!(automation.claim("REMOTE").is_err());

    FileNodeRegistryService::with_active_root(&refine_dir, &runtime_root)
        .activate("remote-node")
        .unwrap();
    let remote_automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let remote_claim = remote_automation.claim("REMOTE").unwrap();
    let state = remote_automation.load_state().unwrap();
    assert!(
        state
            .claims
            .iter()
            .any(|claim| { claim.claim_id == remote_claim && claim.node_id == "remote-node" })
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn a_running_claim_orphaned_by_a_dead_daemon_releases_its_concurrency_slot() {
    let temp_root = unique_temp_dir("automation-orphaned-running-claim");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    for goal_id in ["GOAL1", "GOAL2"] {
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
    let settings = FileSettingsService::new(&refine_dir);
    // Promotion is capped too, so both goals are claimed at a cap of 2 and the
    // cap is then lowered to the single slot the orphan will occupy.
    settings
        .update(&json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "parallel_per_provider_cap": 2,
            "parallel_per_target_app_cap": 2,
            "quality_enabled": "0"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 2);
    settings
        .update(&json!({
            "parallel_run_cap": 1,
            "parallel_per_node_cap": 1,
            "parallel_per_provider_cap": 1,
            "parallel_per_target_app_cap": 1
        }))
        .unwrap();
    let claim_id = automation.load_state().unwrap().claims[0].claim_id.clone();
    automation.start_claim(&claim_id).unwrap();
    assert_eq!(
        automation.load_state().unwrap().claims[0].state,
        WorkflowClaimState::Running
    );

    // The daemon that took the lease is gone. Rewriting the holder pid to one
    // that cannot be alive reproduces what the next capacity read does on its
    // own: the lease is pruned while the claim record survives.
    let capacity_path = runtime_root.join(crate::workflow::capacity::AGENT_CAPACITY_STATE_FILE);
    let mut capacity: Value = serde_json::from_slice(&fs::read(&capacity_path).unwrap()).unwrap();
    assert_eq!(capacity["leases"].as_array().unwrap().len(), 1);
    capacity["leases"][0]["holder_pid"] = json!(u32::MAX);
    fs::write(
        &capacity_path,
        serde_json::to_vec_pretty(&capacity).unwrap(),
    )
    .unwrap();

    // The orphan is indistinguishable from live work to the admission gate: it
    // is still `Running`, so the single slot stays occupied.
    assert_eq!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .len(),
        0
    );
    assert_eq!(
        automation.load_state().unwrap().claims[0].state,
        WorkflowClaimState::Running
    );

    assert_eq!(automation.reconcile_orphaned_running_claims().unwrap(), 1);
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims[0].state, WorkflowClaimState::Interrupted);
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );
    // Idempotent: a settled claim is not swept again.
    assert_eq!(automation.reconcile_orphaned_running_claims().unwrap(), 0);

    // The freed slot is now available to the goal that was starved.
    let second_claim = state.claims[1].claim_id.clone();
    automation.start_claim(&second_claim).unwrap();
    assert_eq!(
        automation.load_state().unwrap().claims[1].state,
        WorkflowClaimState::Running
    );

    automation.release_claim_capacity(&second_claim).unwrap();
    fs::remove_dir_all(temp_root).unwrap_or(());
}

#[test]
fn file_automation_reapplies_lowered_concurrency_limits_before_launch() {
    let temp_root = unique_temp_dir("automation-lowered-launch-limits");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
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
        "#!/bin/sh\n\
             printf '%s\\n' 'lowered-cap agent completed' > agent.txt\n\
             printf '%s\\n' 'smoke-ai lowered-cap goal-agent response'\n",
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
    for goal_id in ["GOAL1", "GOAL2"] {
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
    let settings = FileSettingsService::new(&refine_dir);
    settings
        .update(&json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "parallel_per_provider_cap": 2,
            "parallel_per_target_app_cap": 2,
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 2);
    settings
        .update(&json!({
            "parallel_run_cap": 1,
            "parallel_per_node_cap": 1,
            "parallel_per_provider_cap": 1,
            "parallel_per_target_app_cap": 1
        }))
        .unwrap();

    let steps = automation.execute_claimed_work().unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].goal_id, "GOAL1");
    let state = automation.load_state().unwrap();
    assert_eq!(state.policy.global_limit, 1);
    assert_eq!(state.claims[0].state, WorkflowClaimState::Completed);
    assert_eq!(state.claims[1].state, WorkflowClaimState::Claimed);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Todo
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
fn file_automation_replenishes_running_agents_when_concurrency_cap_increases() {
    let temp_root = unique_temp_dir("automation-live-concurrency-increase");
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
    for goal_id in ["GOAL1", "GOAL2", "GOAL3", "GOAL4"] {
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
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2,
            "parallel_per_provider_cap": 2,
            "parallel_per_target_app_cap": 2,
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 2);
    let evaluation_automation = automation.clone();
    let evaluation_thread =
        std::thread::spawn(move || evaluation_automation.execute_claimed_work());

    let initial_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while fs::read_dir(&marker_root).unwrap().count() < 2
        && std::time::Instant::now() < initial_deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_dir(&marker_root).unwrap().count(), 2);

    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "parallel_run_cap": 4,
            "parallel_per_node_cap": 4,
            "parallel_per_provider_cap": 4,
            "parallel_per_target_app_cap": 4
        }))
        .unwrap();
    assert_eq!(automation.apply_runtime_settings().unwrap(), 2);

    let expansion_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while fs::read_dir(&marker_root).unwrap().count() < 4
        && std::time::Instant::now() < expansion_deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let expanded_before_release = fs::read_dir(&marker_root).unwrap().count() == 4;
    fs::write(marker_root.join("release"), "release\n").unwrap();

    let result = evaluation_thread.join().unwrap().unwrap();
    assert!(
        expanded_before_release,
        "raising the cap did not start the additional agents while the first batch was running"
    );
    assert_eq!(
        result
            .iter()
            .map(|step| step.goal_id.as_str())
            .collect::<Vec<_>>(),
        vec!["GOAL1", "GOAL2", "GOAL3", "GOAL4"]
    );
    for goal_id in ["GOAL1", "GOAL2", "GOAL3", "GOAL4"] {
        assert_eq!(
            work_items.show_goal_summary(goal_id).unwrap().goal.status,
            GoalStatus::Review
        );
    }
    assert!(
        automation
            .load_state()
            .unwrap()
            .claims
            .iter()
            .all(|claim| claim.state == WorkflowClaimState::Completed)
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
