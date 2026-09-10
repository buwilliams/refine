use super::*;

#[test]
fn file_process_supervisor_signals_registered_os_process() {
    let temp_root = unique_temp_dir("process-signal");
    let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let process = supervisor
        .register(ManagedProcess {
            id: "sleep-test".to_string(),
            owner: ProcessOwner::TargetApp,
            pid: Some(child.id()),
            state: "running".to_string(),
            label: Some("sleep".to_string()),
            details: None,
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    let stopped = supervisor.signal(&process.id, "kill").unwrap();
    assert_eq!(stopped.state, "stopped");
    assert!(supervisor.inspect(&process.id).is_err());
    for _ in 0..20 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(child.try_wait().unwrap().is_some());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn requested_termination_keeps_registry_truth_until_process_exit() {
    let temp_root = unique_temp_dir("process-request-termination");
    let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080/agents"));
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let process = supervisor
        .register(ManagedProcess {
            id: "managed-agent-stop".to_string(),
            owner: ProcessOwner::Agent,
            pid: Some(child.id()),
            state: "running".to_string(),
            label: Some("sleep".to_string()),
            details: Some(json!({"session_id": "CHAT1"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    let stopping = supervisor
        .request_termination(&process.id, "terminate")
        .unwrap();
    assert_eq!(stopping.state, "running");
    assert!(supervisor.inspect(&process.id).is_ok());
    for _ in 0..40 {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(child.try_wait().unwrap().is_some());
    assert!(!FileProcessSupervisor::process_is_alive(&stopping).unwrap());
    assert!(supervisor.recover().unwrap().is_empty());
    assert!(supervisor.inspect(&process.id).is_err());

    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn managed_completion_timeout_bounds_a_process_that_keeps_producing_output() {
    let root = unique_temp_dir("event-completion-timeout");
    let supervisor = FileProcessSupervisor::new(&root);
    let started = Instant::now();
    let result = supervisor.run_to_completion(ManagedProcessSpec {
        owner: ProcessOwner::Agent,
        command: "sh".into(),
        args: shell_args("while true; do printf progress; sleep 0.05; done"),
        cwd: None,
        env: vec![],
        stdin: None,
        limits: None,
        authorization_command: None,
        sensitive: false,
        metadata: Map::from_iter([("completion_timeout_seconds".into(), json!(1))]),
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("completion timeout")
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        supervisor
            .list()
            .unwrap()
            .iter()
            .all(|p| !FileProcessSupervisor::process_is_alive(p).unwrap())
    );
    fs::remove_dir_all(root).unwrap();
}
