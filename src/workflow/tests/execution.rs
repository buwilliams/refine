use super::*;

#[test]
fn goal_agent_prompt_renders_one_ordered_flat_markdown_specification() {
    let sentinels = [
        "PRODUCT_SENTINEL",
        "CONSTITUTION_SENTINEL",
        "RULE_ONE_SENTINEL",
        "RULE_TWO_SENTINEL",
        "GOAL_NAME_SENTINEL",
        "GOAL_NOTE_SENTINEL",
        "GUIDANCE_ZERO_SENTINEL",
        "GUIDANCE_TWO_SENTINEL",
        "PREVIOUS_ONE_REQUEST_SENTINEL",
        "PREVIOUS_ONE_REPORT_SENTINEL",
        "PREVIOUS_TWO_REQUEST_SENTINEL",
        "PREVIOUS_TWO_GOVERNANCE_SENTINEL",
        "NESTED_QUALITY_SENTINEL",
        "LATEST_REQUEST_SENTINEL",
    ];
    let prompt = goal_agent_prompt(
        "GOAL1",
        &json!({
            "version": 1,
            "assembled_at": "TIMESTAMP_MUST_NOT_ENTER_THE_PROMPT",
            "governance": {
                "product": "PRODUCT_SENTINEL",
                "constitution": "CONSTITUTION_SENTINEL",
                "configured": true,
                "rules": [
                    {"id": "rule-1", "text": "RULE_ONE_SENTINEL", "created": "RULE_TIMESTAMP_MUST_NOT_ENTER"},
                    {"id": "rule-2", "text": "RULE_TWO_SENTINEL"}
                ]
            },
            "workflow_summary": "WORKFLOW_SENTINEL: isolated worktree through human Review.",
            "guidance_candidates": [
                {
                    "enabled": true,
                    "name": "GUIDANCE_ZERO_SENTINEL",
                    "rule": "GUIDANCE_ZERO_RULE_SENTINEL",
                    "instructions": "GUIDANCE_ZERO_INSTRUCTIONS_SENTINEL"
                },
                {
                    "enabled": false,
                    "name": "DISABLED_GUIDANCE_MUST_NOT_ENTER",
                    "instructions": "DISABLED_INSTRUCTIONS_MUST_NOT_ENTER"
                },
                {
                    "enabled": true,
                    "name": "GUIDANCE_TWO_SENTINEL",
                    "rule": "GUIDANCE_TWO_RULE_SENTINEL",
                    "instructions": "GUIDANCE_TWO_INSTRUCTIONS_SENTINEL"
                }
            ],
            "goal": {
                "id": "GOAL1",
                "name": "GOAL_NAME_SENTINEL",
                "priority": "high",
                "reporter": "GOAL_REPORTER_SENTINEL",
                "assignee": "GOAL_ASSIGNEE_SENTINEL",
                "node_id": "NODE_SENTINEL",
                "notes": [{
                    "author": "NOTE_AUTHOR_SENTINEL",
                    "body": "GOAL_NOTE_SENTINEL\n\n- preserved Markdown"
                }],
                "empty": "",
                "classification": "unclassified"
            },
            "previous_rounds": [
                {
                    "round": 1,
                    "reporter": "ROUND_ONE_REPORTER_SENTINEL",
                    "prompt": "PREVIOUS_ONE_REQUEST_SENTINEL",
                    "implementation_report": "PREVIOUS_ONE_REPORT_SENTINEL",
                    "quality_details": {
                        "checks": [{
                            "name": "NESTED_CHECK_NAME_SENTINEL",
                            "outcome": "NESTED_QUALITY_SENTINEL"
                        }]
                    },
                    "logs": [{"message": "RAW_PREVIOUS_LOG_MUST_NOT_ENTER"}]
                },
                {
                    "round": 2,
                    "prompt": "PREVIOUS_TWO_REQUEST_SENTINEL",
                    "governance_message": "PREVIOUS_TWO_GOVERNANCE_SENTINEL"
                }
            ],
            "current_round": {
                "round": 3,
                "reporter": "LATEST_REPORTER_SENTINEL",
                "prompt": "LATEST_REQUEST_SENTINEL\n\n- Markdown with \"quotes\", {braces}, and Unicode café λ.",
                "logs": [{"message": "RAW_CURRENT_LOG_MUST_NOT_ENTER"}]
            }
        }),
    )
    .unwrap();

    for sentinel in sentinels {
        assert_eq!(
            prompt.matches(sentinel).count(),
            1,
            "{sentinel} must appear exactly once"
        );
    }
    let headings = [
        "## Refine Context",
        "## What",
        "## Why",
        "## Rules",
        "## Previous Rounds",
        "## Latest Round",
    ];
    for pair in headings.windows(2) {
        assert!(
            prompt.find(pair[0]).unwrap() < prompt.find(pair[1]).unwrap(),
            "{} must precede {}",
            pair[0],
            pair[1]
        );
    }
    assert!(prompt.find("### Round 1").unwrap() < prompt.find("### Round 2").unwrap());
    assert!(prompt.contains("#### 0. GUIDANCE_ZERO_SENTINEL"));
    assert!(prompt.contains("#### 2. GUIDANCE_TWO_SENTINEL"));
    assert!(prompt.contains("\"quotes\", {braces}, and Unicode café λ."));
    assert!(prompt.trim_end().ends_with(
        "LATEST_REQUEST_SENTINEL\n\n- Markdown with \"quotes\", {braces}, and Unicode café λ."
    ));
    for absent in [
        "DISABLED_GUIDANCE_MUST_NOT_ENTER",
        "DISABLED_INSTRUCTIONS_MUST_NOT_ENTER",
        "RAW_PREVIOUS_LOG_MUST_NOT_ENTER",
        "RAW_CURRENT_LOG_MUST_NOT_ENTER",
        "TIMESTAMP_MUST_NOT_ENTER_THE_PROMPT",
        "RULE_TIMESTAMP_MUST_NOT_ENTER",
        "Pinned Goal Agent Context",
        "\"agent_context\"",
        "\"current_round\"",
        "\"previous_rounds\"",
        "\"goal\":",
        "\\n",
        "```json",
        "{{",
    ] {
        assert!(!prompt.contains(absent), "{absent} must remain absent");
    }
}

#[test]
fn goal_agent_prompt_renders_empty_optional_context_cleanly() {
    let latest = "LATEST_ONLY_SENTINEL";
    let prompt = goal_agent_prompt(
        "GOAL2",
        &json!({
            "version": 1,
            "governance": {
                "product": "",
                "constitution": null,
                "configured": false,
                "rules": []
            },
            "workflow_summary": "",
            "guidance_candidates": [],
            "goal": {"id": "GOAL2", "notes": [], "priority": "unclassified"},
            "previous_rounds": [],
            "current_round": {"round": 1, "prompt": latest}
        }),
    )
    .unwrap();

    assert_eq!(
        prompt,
        r#"# Goal Agent Specification

## Refine Context

You are the workflow-owned Goal Agent for this implementation attempt. Work autonomously, implement and verify the ready Goal, leave the result reviewable, and ask nothing about routine decisions.

### Goal Identity

- **Pinned Context Version:** 1
- **Goal ID:** GOAL2

## What

No additional product intent or Goal metadata was pinned.

## Why

- **Governance Configured:** No

## Rules

No governance rules or enabled Guidance candidates were pinned.

## Previous Rounds

No previous Rounds were pinned for this implementation attempt.

## Latest Round

This is the authoritative instruction to implement. It wins over conflicting Goal context or earlier Rounds.

- **Round:** 1

### Request

LATEST_ONLY_SENTINEL"#
    );
    assert!(prompt.contains("No previous Rounds were pinned"));
    assert!(prompt.contains("No governance rules or enabled Guidance candidates were pinned."));
    assert!(!prompt.contains("unclassified"));
    assert!(!prompt.contains("null"));
    assert_eq!(prompt.matches("# Goal Agent Specification").count(), 1);
    assert!(prompt.trim_end().ends_with(latest));
}

#[test]
fn file_automation_uses_global_cap_for_single_node_defaults() {
    let temp_root = unique_temp_dir("automation-global-cap");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"parallel_run_cap": 3}))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    for id in ["GOAL1", "GOAL2", "GOAL3", "GOAL4"] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items
            .update_goal_metadata_summary(id, None, Some("high"), None, None)
            .unwrap();
        work_items
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
    }

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 3);
    let state = automation.load_state().unwrap();
    assert_eq!(state.policy.global_limit, 3);
    assert_eq!(state.policy.per_node_limit, 3);
    assert_eq!(state.policy.per_provider_limit, 3);
    assert_eq!(state.policy.per_target_app_limit, 3);
    assert_eq!(state.claims.len(), 3);
    assert_eq!(
        state
            .claims
            .iter()
            .map(|claim| claim.goal_id.as_str())
            .collect::<Vec<_>>(),
        vec!["GOAL1", "GOAL2", "GOAL3"]
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_applies_runtime_settings_without_waiting_for_automation() {
    let temp_root = unique_temp_dir("automation-apply-runtime-settings");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Instant Backlog", Some("GOAL1"))
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "parallel_run_cap": 7,
            "parallel_per_node_cap": 7,
            "backlog_promote_after_seconds": "0",
            "agent_cli": "smoke-ai"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.apply_runtime_settings().unwrap(), 1);
    let state = automation.load_state().unwrap();
    assert_eq!(state.policy.global_limit, 7);
    assert_eq!(state.policy.per_node_limit, 7);
    assert_eq!(state.policy.provider, "smoke-ai");
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].goal_id, "GOAL1");
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_applies_runtime_settings_with_legacy_gap_claims() {
    let temp_root = unique_temp_dir("automation-legacy-gap-claims");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&runtime_root).unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();
    fs::write(
        runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE),
        serde_json::to_vec_pretty(&json!({
            "claims": [{
                "claim_id": "res-legacy",
                "gap_id": "GOAL1",
                "state": "completed",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z"
            }],
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.apply_runtime_settings().unwrap(), 0);
    let state = automation.load_state().unwrap();
    assert_eq!(state.policy.provider, "smoke-ai");
    assert_eq!(state.claims[0].goal_id, "GOAL1");
    let persisted: Value = serde_json::from_slice(
        &fs::read(runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["claims"][0]["goal_id"], "GOAL1");
    assert!(persisted["claims"][0].get("gap_id").is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_runtime_settings_skip_off_node_backlog_promotions() {
    let temp_root = unique_temp_dir("automation-runtime-settings-off-node");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    FileNodeRegistryService::new(&refine_dir)
        .create("remote-node")
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Local backlog", Some("LOCAL"))
        .unwrap();
    work_items
        .create_goal_summary("Remote backlog", Some("REMOTE"))
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
    assert_eq!(automation.apply_runtime_settings().unwrap(), 1);
    assert_eq!(
        work_items.show_goal_summary("LOCAL").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert_eq!(
        work_items.show_goal_summary("REMOTE").unwrap().goal.status,
        GoalStatus::Backlog
    );
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].goal_id, "LOCAL");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_accepts_agent_precommitted_implementation_branch() {
    let temp_root = unique_temp_dir("automation-agent-precommit");
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\n\
             printf '%s\\n' 'agent precommitted implementation' > agent.txt\n\
             git add agent.txt\n\
             git commit -q -m 'agent precommit'\n\
             printf '%s\\n' 'smoke-ai committed before Refine commit step'\n",
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
        .create_goal_summary("Precommitted implementation", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert!(!target_root.join(".git/refine-worktrees").exists());
    let result = automation.evaluate_workflow().unwrap();
    let worktree_path = target_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.steps[0].commit.len(), 40);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );
    assert_eq!(
        fs::read_to_string(worktree_path.join("agent.txt")).unwrap(),
        "agent precommitted implementation\n"
    );
    assert_eq!(
        fs::read_to_string(target_root.join("agent.txt")).unwrap(),
        "agent precommitted implementation\n"
    );
    assert!(result.steps[0].merge.as_ref().is_some_and(|merge| merge.ok));
    assert_eq!(
        git_stdout(&worktree_path, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        result.steps[0].commit
    );
    assert_eq!(
        git_stdout(&worktree_path, &["log", "--pretty=%s", "-1"])
            .unwrap()
            .trim(),
        "agent precommit"
    );
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    let round = &goal["rounds"][0];
    assert_eq!(round["workflow_quality_timing"], "pre_merge");
    assert_eq!(
        round["workflow_integration"]["candidate_commit"],
        result.steps[0].commit
    );
    assert_eq!(
        round["quality_details"]["evaluation_scope"],
        "isolated_candidate"
    );
    assert_eq!(
        round["quality_details"]["source_candidate_commit"],
        result.steps[0].commit
    );
    let state_messages = round["logs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|log| log["category"] == "state")
        .filter_map(|log| log["message"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        state_messages,
        vec![
            "Workflow status changed: todo -> in-progress",
            "Workflow status changed: in-progress -> qa",
            "Workflow status changed: qa -> ready-merge",
            "Workflow status changed: ready-merge -> build",
            "Workflow status changed: build -> review",
        ]
    );
    assert!(round["logs"].as_array().unwrap().iter().any(|log| {
        log["message"].as_str() == Some("Target app rebuild skipped")
            && log["details"]["skipped"] == true
    }));

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_treats_clean_noop_implementation_as_reviewable() {
    let temp_root = unique_temp_dir("automation-agent-noop");
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    let initial_head = git_stdout(&target_root, &["rev-parse", "HEAD"]).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\n\
             printf '%s\\n' 'smoke-ai verified clean no-op implementation'\n",
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
        .create_goal_summary("No-op implementation", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let result = automation.evaluate_workflow().unwrap();
    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.steps[0].commit, initial_head.trim());
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );
    assert_eq!(
        git_stdout(&target_root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        initial_head.trim()
    );
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    let round_logs = goal["rounds"][0]["logs"].as_array().unwrap();
    assert!(
        round_logs
            .iter()
            .any(|log| { log["message"].as_str() == Some("No implementation changes to commit") })
    );
    assert!(!round_logs.iter().any(|log| {
        log["message"]
            .as_str()
            .unwrap_or("")
            .starts_with("Workflow failed")
    }));

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
fn file_automation_reuses_existing_round_worktree_on_retry() {
    let temp_root = unique_temp_dir("automation-existing-worktree-retry");
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();

    let branch = "refine/GOAL1/round-1";
    let worktree_path = temp_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    fs::create_dir_all(worktree_path.parent().unwrap()).unwrap();
    git(
        &temp_root,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            worktree_path.to_str().unwrap(),
        ],
    )
    .unwrap();
    fs::write(
        worktree_path.join("agent.txt"),
        "existing retry implementation\n",
    )
    .unwrap();
    git(&worktree_path, &["add", "agent.txt"]).unwrap();
    git(&worktree_path, &["commit", "-q", "-m", "agent precommit"]).unwrap();
    let precommitted = git_stdout(&worktree_path, &["rev-parse", "HEAD"]).unwrap();
    fs::write(
        &smoke_ai,
        "#!/bin/sh\n\
             printf '%s\\n' 'smoke-ai reused existing worktree'\n",
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
        .create_goal_summary("Retry existing worktree", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .update_goal_branch_name("GOAL1", Some(branch))
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "0"
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let result = automation.evaluate_workflow().unwrap();
    assert_eq!(result.steps.len(), 1);
    assert_eq!(result.steps[0].commit, precommitted.trim());
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );
    assert_eq!(
        fs::read_to_string(worktree_path.join("agent.txt")).unwrap(),
        "existing retry implementation\n"
    );
    assert_eq!(
        fs::read_to_string(target_root.join("agent.txt")).unwrap(),
        "existing retry implementation\n"
    );
    assert!(result.steps[0].merge.as_ref().is_some_and(|merge| merge.ok));

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_fails_goal_and_preserves_candidate_on_qa_failure() {
    let temp_root = unique_temp_dir("automation-qa-candidate");
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"Post-implementation Quality evaluation\"*)\n\
               printf '%s\\n' '{\"ok\":false,\"summary\":\"Candidate marker check failed.\",\"results\":[{\"test\":\"The candidate has no fail-qa marker.\",\"status\":\"failed\",\"evidence\":\"fail-qa exists\",\"command\":\"test ! -f fail-qa\"}]}'\n\
               ;;\n\
             *\"Post-implementation governance evaluation\"*)\n\
               printf '%s\\n' '{\"status\":\"passed\",\"message\":\"Governance passed.\",\"violations\":[]}'\n\
               ;;\n\
             *)\n\
               printf 'qa should fail\\n' > fail-qa\n\
               printf '%s\\n' 'smoke-ai goal-agent response'\n\
               ;;\n\
             esac\n",
        )
        .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }

    let branch = "refine/GOAL1/round-1";
    let worktree_path = target_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    let initial_head = git_stdout(&target_root, &["rev-parse", "HEAD"]).unwrap();

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Implementation with failing QA", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "0",
            "target_app_build_command": "printf build-ran > build-ran",
            "target_app_test_command": "test ! -f fail-qa",
            "allowed_commands": "printf, test"
        }))
        .unwrap();
    FileQualityService::new(&refine_dir)
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["The candidate has no fail-qa marker.".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let error = automation.evaluate_workflow().unwrap_err();
    assert!(error.to_string().contains("quality checks failed"));
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Failed
    );
    assert!(!target_root.join("fail-qa").exists());
    assert!(worktree_path.exists());
    assert!(worktree_path.join("fail-qa").exists());
    assert!(
        !worktree_path.join("build-ran").exists(),
        "pre-merge Quality must fail before the target-app build"
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(detail["rounds"][0]["quality_state"], "failed");
    assert_eq!(
        detail["rounds"][0]["quality_details"]["results"][0]["test"],
        "The candidate has no fail-qa marker."
    );
    // A failed Goal has to carry its own reason. Reading `failed` with the
    // gates it reached recorded as passed otherwise explains nothing, and
    // the operator has no reason to go opening operations files.
    assert_eq!(detail["rounds"][0]["failure_category"], "quality");
    assert!(
        detail["rounds"][0]["failure_message"]
            .as_str()
            .unwrap_or_default()
            .contains("quality checks failed"),
        "the Goal must record why it failed, got {:?}",
        detail["rounds"][0]["failure_message"]
    );
    assert!(
        !detail["rounds"][0]["failure_at"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    assert_eq!(
        git_stdout(&target_root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        initial_head.trim()
    );
    let worktrees = git_stdout(&target_root, &["worktree", "list", "--porcelain"]).unwrap();
    assert!(worktrees.contains(&format!("branch refs/heads/{branch}")));

    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(&worktree_path).ok();
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn workflow_evaluation_does_not_hold_the_repository_lock_between_git_steps() {
    let temp_root = unique_temp_dir("automation-narrow-git-lock");
    let target_root = temp_root.join("target");
    let _refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");

    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let lock_root = target_root.clone();
    let lock_thread = std::thread::spawn(move || {
        with_repository_git_lock(&lock_root, || {
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        })
        .unwrap();
    });
    locked_rx.recv_timeout(Duration::from_secs(2)).unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let evaluation_thread = std::thread::spawn(move || {
        finished_tx.send(automation.evaluate_workflow()).unwrap();
    });
    #[cfg(not(target_os = "macos"))]
    let lock_independence_timeout = Duration::from_millis(250);
    // APFS-backed debug test runs can exceed the Linux budget without waiting
    // on the held repository lock. A real lock dependency still times out
    // because the lock is not released until after this receive completes.
    #[cfg(target_os = "macos")]
    let lock_independence_timeout = Duration::from_secs(2);
    let evaluation = finished_rx.recv_timeout(lock_independence_timeout);

    release_tx.send(()).unwrap();
    lock_thread.join().unwrap();
    evaluation_thread.join().unwrap();
    evaluation
        .expect("workflow evaluation waited on the repository lock outside a Git step")
        .unwrap();

    fs::remove_dir_all(temp_root).unwrap();
}
