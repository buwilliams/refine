use super::*;

unsafe fn restore_planning_env(key: &str, value: Option<std::ffi::OsString>) {
    if let Some(value) = value {
        unsafe { std::env::set_var(key, value) };
    } else {
        unsafe { std::env::remove_var(key) };
    }
}

fn run_planning_failure(provider_body: &str) -> (PathBuf, Value) {
    let temp_root = unique_temp_dir("workflow-planning-failure-evidence");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::write(target_root.join("app.py"), "base\n").unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.py"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "Initialize"]).unwrap();
    fs::write(&smoke_ai, format!("#!/bin/sh\n{provider_body}\n")).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Planning failure evidence", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai", "quality_enabled": "0"}))
        .unwrap();

    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    let previous_planning = std::env::var_os("REFINE_SMOKE_AI_GOVERNED_PLANNING");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        std::env::set_var("REFINE_SMOKE_AI_GOVERNED_PLANNING", "1");
    }
    let error = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap_err();
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    let plan = detail["rounds"][0]["implementation_plan"].clone();
    assert_eq!(plan["state"], "failed", "{error}");

    unsafe {
        restore_planning_env("REFINE_SMOKE_AI_PATH", previous_smoke_ai);
        restore_planning_env("REFINE_SMOKE_AI_GOVERNED_PLANNING", previous_planning);
    }
    (temp_root, plan)
}

#[test]
fn invalid_planning_output_is_durable_before_transient_process_cleanup() {
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (temp_root, plan) = run_planning_failure(
        r#"printf '%s\n' '{"state":"completed","message":"invalid plan","guidance_applied":[],"planning_result":{"summary":"missing checklist"}}' > "$REFINE_AGENT_SIGNAL_PATH""#,
    );

    assert_eq!(plan["failure"]["category"], "invalid_output");
    assert_eq!(
        plan["failure"]["process"]["structured_output"]["summary"],
        "missing checklist"
    );
    assert!(plan["failure"]["process"]["process_id"].as_str().is_some());
    assert_eq!(plan["failure"]["process"]["state"], "exited");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn provider_exit_output_and_identity_are_retained_as_round_failure_evidence() {
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (temp_root, plan) = run_planning_failure("printf 'provider diagnostic output\\n'; exit 19");

    assert_eq!(plan["failure"]["category"], "provider");
    assert_eq!(plan["failure"]["process"]["exit_code"], 19);
    assert_eq!(plan["failure"]["process"]["state"], "failed");
    assert!(
        plan["failure"]["process"]["output"]
            .as_str()
            .is_some_and(|output| output.contains("provider diagnostic output"))
    );
    assert!(plan["failure"]["process"]["process_id"].as_str().is_some());

    fs::remove_dir_all(temp_root).unwrap();
}
