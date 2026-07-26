use super::*;

#[test]
fn target_app_service_runs_status_and_build_commands() {
    let temp_root = unique_temp_dir("target-app-service");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_status_command": "test -f status-ok",
            "target_app_build_command": "touch built && echo built",
            "target_app_test_commands": [
                {"command": "printf skipped > disabled-test", "enabled": false},
                {"command": "printf first-ok > tested && echo first-ok", "enabled": true},
                {"command": "printf second-ok > tested-two && echo second-ok", "enabled": true}
            ],
            "target_app_cwd": target_root.to_str().unwrap(),
            "allowed_commands": "test, touch, printf"
        }))
        .unwrap();
    fs::write(target_root.join("status-ok"), "").unwrap();
    let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

    let status = service.status().unwrap();
    assert_eq!(status.state, "running");
    assert!(status.last_check_ok);

    let built = service.build().unwrap();
    assert!(built.ok);
    assert!(target_root.join("built").exists());

    let tested = service.test().unwrap();
    assert!(tested.ok);
    assert_eq!(tested.last_operation.as_ref().unwrap().kind, "test");
    assert_eq!(tested.last_operation.as_ref().unwrap().stdout, "second-ok");
    assert!(target_root.join("tested").exists());
    assert!(target_root.join("tested-two").exists());
    assert!(!target_root.join("disabled-test").exists());
    let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
    assert!(audit.contains("\"actor\":\"quality\""));
    assert!(runtime_root.join(TARGET_APP_STATE_FILE).exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn target_app_service_treats_missing_build_command_as_success() {
    let temp_root = unique_temp_dir("target-app-no-build");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"target_app_cwd": target_root.to_str().unwrap()}))
        .unwrap();
    let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

    let built = service.build().unwrap();
    assert!(built.ok);
    assert_eq!(built.state, "stopped");
    assert_eq!(
        built.message,
        "No target-app build instructions are configured."
    );
    assert!(built.last_check_ok);
    assert!(built.last_health_ok);
    assert_eq!(built.last_error, "");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn target_app_service_spawns_start_command_and_registers_process() {
    let temp_root = unique_temp_dir("target-app-start");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_start_command": "printf target-started; sleep 2",
            "target_app_cwd": target_root.to_str().unwrap()
        }))
        .unwrap();
    let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

    let started = service.start().unwrap();
    assert_eq!(started.state, "running");
    assert!(started.pid.is_some());
    assert_eq!(
        FileProcessSupervisor::new(&runtime_root)
            .list()
            .unwrap()
            .len(),
        1
    );
    std::thread::sleep(Duration::from_millis(50));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process_id = started.process_id.as_deref().unwrap();
    assert!(
        supervisor
            .stream(process_id)
            .unwrap()
            .contains("target-started")
    );
    service.stop().unwrap();
    match supervisor.wait(process_id) {
        Ok(_) | Err(RefineError::NotFound(_)) => {}
        Err(error) => panic!("target app process did not settle after stop: {error}"),
    }
    supervisor.cleanup(process_id).unwrap();

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn target_app_service_runs_lifecycle_instructions_with_agent_provider() {
    let temp_root = unique_temp_dir("target-app-agent-lifecycle");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    let smoke_ai = temp_root.join("smoke-ai");
    fs::write(&smoke_ai, "#!/bin/sh\nprintf 'agent lifecycle ok\\n'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "agent_cli": "smoke-ai",
            "target_app_start_instructions": "Start the target app and verify it.",
            "target_app_stop_instructions": "Stop the target app and verify it.",
            "target_app_build_instructions": "Build the target app and report evidence.",
            "target_app_cwd": target_root.to_str().unwrap()
        }))
        .unwrap();
    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

    let started = service.start().unwrap();
    assert_eq!(started.state, "running");
    assert_eq!(
        started.last_operation.as_ref().unwrap().stdout,
        "agent lifecycle ok"
    );
    assert!(started.process_id.is_none());

    let built = service.build().unwrap();
    assert!(built.ok);
    assert_eq!(built.last_operation.as_ref().unwrap().kind, "build");
    assert_eq!(
        built.last_operation.as_ref().unwrap().stdout,
        "agent lifecycle ok"
    );

    let stopped = service.stop().unwrap();
    assert_eq!(stopped.state, "stopped");
    assert_eq!(
        stopped.last_operation.as_ref().unwrap().stdout,
        "agent lifecycle ok"
    );

    unsafe {
        match previous {
            Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
            None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
        }
    }
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn target_app_snapshot_write_replaces_longer_state() {
    let temp_root = unique_temp_dir("target-app-state");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);
    let mut long_snapshot = TargetAppSnapshot {
        state: "running".to_string(),
        message: "Target application started.".to_string(),
        last_operation_id: "target-start-1".to_string(),
        last_operation: Some(TargetAppOperation {
            id: "target-start-1".to_string(),
            kind: "start".to_string(),
            state: "running".to_string(),
            started_at: now_timestamp(),
            finished_at: String::new(),
            exit_code: None,
            stdout: "long target app stdout".repeat(8),
            stderr: String::new(),
        }),
        process_id: Some("proc-target-app-state".to_string()),
        pid: Some(12345),
        ..TargetAppSnapshot::default()
    };
    service.save_snapshot(&long_snapshot).unwrap();
    long_snapshot.last_operation = None;
    long_snapshot.last_operation_id = String::new();
    long_snapshot.process_id = None;
    long_snapshot.pid = None;
    long_snapshot.message = "short".to_string();
    service.save_snapshot(&long_snapshot).unwrap();

    let raw = fs::read_to_string(service.state_path()).unwrap();
    assert!(!raw.contains("long target app stdout"));
    assert!(!raw.contains("proc-target-app-state"));
    let loaded = service.load_snapshot().unwrap();
    assert_eq!(loaded.message, "short");
    assert!(loaded.last_operation.is_none());

    fs::remove_dir_all(temp_root).unwrap();
}
