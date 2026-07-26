use super::*;

#[test]
fn file_process_supervisor_output_observation_skips_a_process_reaped_after_listing() {
    let temp_root = unique_temp_dir("process-output-reaping-race");
    let runtime_root = temp_root.join("run/8080");
    let release_path = temp_root.join("release");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command: shell_binary().to_string(),
            args: shell_args(&format!(
                "while [ ! -f '{}' ]; do sleep 0.01; done",
                release_path.display()
            )),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    let listed = supervisor
        .list()
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == process.id)
        .unwrap();

    fs::write(&release_path, "").unwrap();
    let process_path = supervisor
        .processes_dir()
        .join(format!("{}.json", process.id));
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!process_path.exists(), "managed process was not reaped");

    assert_eq!(
        supervisor.observe_output(&listed).unwrap(),
        ProcessOutputObservation::Terminal {
            process_id: listed.id
        }
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_output_observation_skips_a_reaped_output_artifact() {
    let temp_root = unique_temp_dir("process-output-artifact-race");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let stdout_path = runtime_root.join("observed.stdout.log");
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(&stdout_path, "before reaping\n").unwrap();
    supervisor
        .register(ManagedProcess {
            id: "artifact-race".to_string(),
            owner: ProcessOwner::Runner,
            pid: None,
            state: "running".to_string(),
            label: Some("artifact race".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "observed-start".to_string(),
            exit_code: None,
        })
        .unwrap();
    let listed = supervisor.list().unwrap().pop().unwrap();

    fs::remove_file(&stdout_path).unwrap();

    assert_eq!(
        supervisor.observe_output(&listed).unwrap(),
        ProcessOutputObservation::Terminal {
            process_id: listed.id
        }
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_output_observation_preserves_actionable_errors() {
    let temp_root = unique_temp_dir("process-output-errors");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let output_dir = runtime_root.join("not-a-log");
    fs::create_dir_all(&output_dir).unwrap();
    let process = supervisor
        .register(ManagedProcess {
            id: "output-errors".to_string(),
            owner: ProcessOwner::Runner,
            pid: None,
            state: "running".to_string(),
            label: Some("output errors".to_string()),
            details: None,
            stdout_path: Some(output_dir.display().to_string()),
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "original-start".to_string(),
            exit_code: None,
        })
        .unwrap();
    let listed = supervisor.list().unwrap().pop().unwrap();
    let io_error = supervisor.observe_output(&listed).unwrap_err();
    assert!(matches!(io_error, RefineError::Io(_)), "{io_error}");

    let mut changed = process.clone();
    changed.started_at = "replacement-start".to_string();
    supervisor.write_process(&changed).unwrap();
    let conflict = supervisor.observe_output(&listed).unwrap_err();
    assert!(matches!(conflict, RefineError::Conflict(_)), "{conflict}");

    fs::write(
        supervisor
            .processes_dir()
            .join(format!("{}.json", process.id)),
        "{not-json",
    )
    .unwrap();
    let serialization = supervisor.observe_output(&listed).unwrap_err();
    assert!(
        matches!(serialization, RefineError::Serialization(_)),
        "{serialization}"
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_streams_registered_output_files() {
    let temp_root = unique_temp_dir("process-stream");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let stdout_path = runtime_root.join("stdout.log");
    let stderr_path = runtime_root.join("stderr.log");
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(&stdout_path, "hello stdout\n").unwrap();
    fs::write(&stderr_path, "warn stderr\n").unwrap();
    supervisor
        .register(ManagedProcess {
            id: "stream-test".to_string(),
            owner: ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("stream".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    let stream = supervisor.stream("stream-test").unwrap();
    assert!(stream.contains("hello stdout"));
    assert!(stream.contains("warn stderr"));

    fs::remove_dir_all(temp_root).unwrap();
}
