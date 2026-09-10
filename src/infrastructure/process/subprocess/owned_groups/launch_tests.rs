use super::*;
fn spec(script: &str) -> ManagedProcessSpec {
    ManagedProcessSpec {
        owner: ProcessOwner::Agent,
        command: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: None,
        env: vec![("REFINE_LAUNCH_VALUE".into(), "prepared value".into())],
        stdin: None,
        limits: None,
        authorization_command: None,
        sensitive: false,
        metadata: serde_json::from_value(
            json!({"workflow_incarnation":"launch-contract", "isolated_process_group":true}),
        )
        .unwrap(),
    }
}
fn root() -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("refine-launch-contract-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}
#[test]
fn registration_gate_abandonment_prevents_workload_side_effects_and_reaps_bootstrap() {
    let root = root();
    let supervisor = FileProcessSupervisor::new(&root);
    let mut spec = spec("touch ran");
    spec.cwd = Some(root.display().to_string());
    let mut command = process_command(&spec).unwrap();
    let mut scope = launch_scope::ScopeLaunch::prepare(&supervisor, "gated", &spec, &mut command)
        .unwrap()
        .unwrap();
    let mut child = command.spawn().unwrap();
    let pid = scope.attach(&child, &mut process_details(&spec)).unwrap();
    assert!(os_process_identity(pid).unwrap().is_some());
    assert!(!root.join("ran").exists());
    drop(scope);
    assert!(!child.wait().unwrap().success());
    assert!(!root.join("ran").exists());
    assert!(os_process_identity(pid).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn missing_helper_receipt_fails_within_handshake_bound() {
    let root = root();
    let supervisor = FileProcessSupervisor::new(&root);
    let spec = spec("exit 0");
    let mut command = process_command(&spec).unwrap();
    let mut scope = launch_scope::ScopeLaunch::prepare(&supervisor, "missing", &spec, &mut command)
        .unwrap()
        .unwrap();
    let mut child = Command::new("/bin/true").spawn().unwrap();
    child.wait().unwrap();
    let start = Instant::now();
    assert!(scope.attach(&child, &mut process_details(&spec)).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn prepared_environment_stdin_cwd_and_signal_status_survive_helper_launch() {
    let root = root();
    let supervisor = FileProcessSupervisor::new(&root);
    let mut spec = spec(
        "read value; printf '%s|%s|%s' \"$REFINE_LAUNCH_VALUE\" \"$value\" \"$PWD\"; kill -TERM $$",
    );
    spec.cwd = Some(root.display().to_string());
    spec.stdin = Some("input value\n".into());
    let output = supervisor.run_to_completion(spec).unwrap();
    assert_eq!(
        output.stdout,
        format!("prepared value|input value|{}", root.display())
    );
    assert_eq!(output.process.exit_code, None);
    assert_eq!(output.process.state, "failed");
    assert!(output.stderr.is_empty());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn workload_completion_retains_capacity_until_detached_scope_exits() {
    let root = root();
    let supervisor = FileProcessSupervisor::new(&root);
    let start = Instant::now();
    let output = supervisor
        .run_to_completion(spec(
            "setsid sleep 30 </dev/null >/dev/null 2>&1 & printf completed; exit 9",
        ))
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(output.stdout, "completed");
    assert_eq!(output.process.exit_code, Some(9));
    assert!(supervisor.group_pending(&output.process).unwrap());
    let group = supervisor.owned_groups().unwrap().remove(0);
    assert!(
        supervisor
            .stop_owned_group(&group, Duration::from_secs(2))
            .unwrap()
            .confirmed_exit
    );
    assert!(!supervisor.group_pending(&output.process).unwrap());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn launcher_exit_preserves_parent_death_limits_and_complete_scope_proof() {
    const ENV: &str = "REFINE_TEST_SCOPE_PARENT_ROOT";
    if let Ok(path) = std::env::var(ENV) {
        let supervisor = FileProcessSupervisor::new(path);
        let mut spec = spec("exec sleep 60");
        spec.limits = Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        });
        supervisor.launch(spec).unwrap();
        return;
    }
    let root = root();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "infrastructure::process::subprocess::owned_groups::launch_tests::launcher_exit_preserves_parent_death_limits_and_complete_scope_proof", "--nocapture"])
        .env(ENV, &root).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
    assert!(status.success());
    let supervisor = FileProcessSupervisor::new(&root);
    let group = supervisor.owned_groups().unwrap().remove(0);
    let deadline = Instant::now() + Duration::from_secs(3);
    while supervisor.assess_owned_group(&group).unwrap().pending() {
        assert!(
            Instant::now() < deadline,
            "scope did not complete after launcher exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = group
        .launch_scope
        .as_ref()
        .unwrap()
        .workload_status(&group.process, &root)
        .unwrap()
        .unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    fs::remove_dir_all(root).unwrap();
}
