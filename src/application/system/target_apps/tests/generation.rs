use super::*;

#[test]
fn target_app_service_generates_package_json_config() {
    let temp_root = unique_temp_dir("target-app-generate");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(
        target_root.join("package.json"),
        r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest run"}}"#,
    )
    .unwrap();
    fs::write(target_root.join("pnpm-lock.yaml"), "").unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_url": "http://127.0.0.1:5173",
            "target_app_cwd": target_root.to_str().unwrap()
        }))
        .unwrap();

    let generated = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root)
        .generate_config()
        .unwrap();
    assert_eq!(generated.start_command, "");
    assert_eq!(generated.build_command, "");
    assert!(generated.start_instructions.contains("pnpm run dev"));
    assert!(generated.build_instructions.contains("pnpm run build"));
    assert_eq!(generated.test_command, "pnpm test");
    assert_eq!(generated.tcp_check_port, "5173");
    assert!(generated.notes.contains("package.json"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn target_app_service_generates_static_web_server_for_package_without_start_script() {
    let temp_root = unique_temp_dir("target-app-static-package");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    let port = free_test_port();
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(
        target_root.join("package.json"),
        r#"{"scripts":{"test":"node --test"}}"#,
    )
    .unwrap();
    fs::write(target_root.join("index.html"), "<h1>Static app</h1>").unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_url": format!("http://127.0.0.1:{port}/"),
            "target_app_cwd": target_root.to_str().unwrap()
        }))
        .unwrap();

    let generated = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root)
        .generate_config()
        .unwrap();
    assert!(generated.start_command.is_empty());
    assert!(generated.stop_command.is_empty());
    assert!(generated.build_command.is_empty());
    assert!(
        generated
            .start_instructions
            .contains("python3 -m http.server")
    );
    assert!(generated.stop_instructions.contains("target-app.pid"));
    assert!(
        generated
            .build_instructions
            .contains("No build step configured")
    );
    assert_eq!(
        generated.status_command,
        format!("curl -fsS 'http://127.0.0.1:{port}/' >/dev/null")
    );
    assert_eq!(generated.tcp_check_host, "127.0.0.1");
    assert_eq!(generated.tcp_check_port, port.to_string());
    assert!(generated.notes.contains("static web content"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn target_app_generation_does_not_embed_existing_manage_app_entrypoints() {
    let temp_root = unique_temp_dir("target-app-wrapper-regeneration");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let target_root = temp_root.join("app");
    let port = free_test_port();
    fs::create_dir_all(&refine_dir).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(
        target_root.join("package.json"),
        r#"{"scripts":{"test":"node --test"}}"#,
    )
    .unwrap();
    fs::write(target_root.join("index.html"), "<h1>Static app</h1>").unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "target_app_url": format!("http://127.0.0.1:{port}/"),
            "target_app_cwd": target_root.to_str().unwrap(),
            "target_app_start_command": "./.refine/manage-app.sh start",
            "target_app_stop_command": "./.refine/manage-app.sh stop",
            "target_app_build_command": "./.refine/manage-app.sh build",
            "target_app_test_command": "./.refine/manage-app.sh test",
            "target_app_status_command": "./.refine/manage-app.sh status"
        }))
        .unwrap();

    let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);
    let generated = service.generate_config().unwrap();

    assert!(generated.start_command.is_empty());
    assert!(generated.stop_command.is_empty());
    assert!(generated.build_command.is_empty());
    assert_eq!(generated.test_command, "npm test");
    assert_eq!(
        generated.status_command,
        format!("curl -fsS 'http://127.0.0.1:{port}/' >/dev/null")
    );
    assert!(
        generated
            .start_instructions
            .contains("python3 -m http.server")
    );
    assert!(generated.stop_instructions.contains("target-app.pid"));
    assert!(
        generated
            .notes
            .contains("Ignored existing manage-app wrapper entrypoints")
    );
    assert!(!target_root.join(".refine/manage-app.sh").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn target_app_wrapper_turns_partial_ai_web_config_into_managed_server() {
    let temp_root = unique_temp_dir("target-app-wrapper-static");
    let target_root = temp_root.join("app");
    let runtime_root = temp_root.join("run/8080");
    let port = free_test_port();
    fs::create_dir_all(&target_root).unwrap();
    git_init(&target_root);
    let refine_dir =
        crate::infrastructure::storage::project_layout::refine_dir_for_target_root(&target_root)
            .unwrap();
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(target_root.join("index.html"), "<h1>AI static app</h1>").unwrap();

    let mut config = TargetAppGeneratedConfig {
        start_instructions: String::new(),
        stop_instructions: String::new(),
        build_instructions: String::new(),
        start_command: String::new(),
        stop_command: String::new(),
        build_command: String::new(),
        test_command: "npm test".to_string(),
        status_command: "npm test -- --help >/dev/null 2>&1 || true".to_string(),
        cwd: ".".to_string(),
        env: serde_json::Map::new(),
        start_timeout_seconds: 120,
        stop_timeout_seconds: 60,
        build_timeout_seconds: 300,
        test_timeout_seconds: 600,
        status_timeout_seconds: 10,
        log_path: String::new(),
        http_check_url: format!("http://127.0.0.1:{port}/"),
        tcp_check_host: String::new(),
        tcp_check_port: String::new(),
        process_check_command: String::new(),
        notes: "provider returned only a test status command".to_string(),
    };
    let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

    service.write_manage_app_wrapper(&mut config).unwrap();

    assert_eq!(config.start_command, manage_app_wrapper_entrypoint("start"));
    assert_eq!(config.stop_command, manage_app_wrapper_entrypoint("stop"));
    assert_eq!(config.build_command, manage_app_wrapper_entrypoint("build"));
    assert_eq!(config.test_command, manage_app_wrapper_entrypoint("test"));
    assert_eq!(
        config.status_command,
        manage_app_wrapper_entrypoint("status")
    );
    assert!(config.notes.contains("static web content"));

    let wrapper_path = refine_dir.join("manage-app.sh");
    let script = fs::read_to_string(&wrapper_path).unwrap();
    assert!(!script.contains("START_COMMAND=''"));
    assert!(!script.contains("STOP_COMMAND=''"));
    assert!(script.contains(&format!("PORT={port};")));
    assert!(script.contains(&format!("http://127.0.0.1:{port}/")));
    assert!(script.contains("python3 -m http.server"));
    assert!(script.contains("STATUS_COMMAND='curl -fsS"));

    // Run through `sh` rather than executing the freshly written script:
    // a concurrently spawning test can hold a fork-inherited copy of the
    // write descriptor for a moment, and direct execution then fails with
    // ETXTBSY. The interpreter only opens the script for reading.
    let start = std::process::Command::new("sh")
        .arg(&wrapper_path)
        .arg("start")
        .current_dir(&target_root)
        .output()
        .unwrap();
    assert!(
        start.status.success(),
        "start failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&start.stdout),
        String::from_utf8_lossy(&start.stderr)
    );
    assert!(!target_root.join(".refine").exists());
    let status = std::process::Command::new("sh")
        .arg(&wrapper_path)
        .arg("status")
        .current_dir(&target_root)
        .output()
        .unwrap();
    let stop = std::process::Command::new("sh")
        .arg(&wrapper_path)
        .arg("stop")
        .current_dir(&target_root)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "status failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(
        stop.status.success(),
        "stop failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&stop.stdout),
        String::from_utf8_lossy(&stop.stderr)
    );

    fs::remove_dir_all(temp_root).unwrap();
}
