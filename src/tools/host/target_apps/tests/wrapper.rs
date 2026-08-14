use super::*;

#[test]
fn target_app_service_writes_manage_app_wrapper() {
    let temp_root = unique_temp_dir("target-app-wrapper");
    let target_root = temp_root.join("app");
    let runtime_root = temp_root.join("run/8080");
    let inner_root = target_root.join("client");
    fs::create_dir_all(&inner_root).unwrap();
    git_init(&target_root);
    let refine_dir =
        crate::tools::host::project_layout::refine_dir_for_target_root(&target_root).unwrap();
    fs::create_dir_all(&refine_dir).unwrap();

    let mut config = TargetAppGeneratedConfig {
        start_instructions: String::new(),
        stop_instructions: String::new(),
        build_instructions: String::new(),
        start_command: "printf \"$WRAP_VALUE\" > ../started".to_string(),
        stop_command: String::new(),
        build_command: "printf built > ../built".to_string(),
        test_command: "printf tested > ../tested".to_string(),
        status_command: "printf status-ok".to_string(),
        cwd: "client".to_string(),
        env: serde_json::Map::from_iter([(
            "WRAP_VALUE".to_string(),
            Value::String("wrapped".to_string()),
        )]),
        start_timeout_seconds: 120,
        stop_timeout_seconds: 60,
        build_timeout_seconds: 300,
        test_timeout_seconds: 600,
        status_timeout_seconds: 10,
        log_path: String::new(),
        http_check_url: String::new(),
        tcp_check_host: String::new(),
        tcp_check_port: String::new(),
        process_check_command: String::new(),
        notes: "provider analysis".to_string(),
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
    assert_eq!(config.cwd, ".");
    assert_eq!(config.log_path, MANAGE_APP_LOG_PATH);

    let wrapper_path = refine_dir.join("manage-app.sh");
    let script = fs::read_to_string(&wrapper_path).unwrap();
    assert!(script.contains("APP_CWD='client'"));
    assert!(script.contains("START_COMMAND='printf \"$WRAP_VALUE\" > ../started'"));
    assert!(script.contains("TEST_COMMAND='printf tested > ../tested'"));
    assert!(script.contains("# Analysis notes: provider analysis"));
    assert!(script.contains("export WRAP_VALUE='wrapped'"));

    // Run through `sh` rather than executing the freshly written script:
    // a concurrently spawning test can hold a fork-inherited copy of the
    // write descriptor for a moment, and direct execution then fails with
    // ETXTBSY. The interpreter only opens the script for reading.
    let output = std::process::Command::new("sh")
        .arg(&wrapper_path)
        .arg("start")
        .current_dir(&target_root)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(target_root.join("started")).unwrap(),
        "wrapped"
    );
    let log = fs::read_to_string(refine_dir.join("manage-app.log")).unwrap();
    assert!(log.contains("[start] cwd="));
    assert!(log.contains("[start] exit=0"));
    assert!(!target_root.join(".refine").exists());

    fs::remove_dir_all(temp_root).unwrap();
}
