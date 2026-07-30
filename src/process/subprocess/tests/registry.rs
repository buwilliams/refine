use super::*;

#[test]
fn file_process_supervisor_tracks_running_processes_and_pause_state() {
    let temp_root = unique_temp_dir("processes");
    let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: shell_binary().to_string(),
            args: long_running_shell_args().to_vec(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: Some(ProcessResourceLimits {
                max_memory_bytes: Some(512 * 1024 * 1024),
                cpu_priority: Some("normal".to_string()),
                kill_on_parent_exit: false,
                stall_timeout_seconds: None,
            }),
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    assert_eq!(supervisor.list().unwrap().len(), 1);
    assert_eq!(process.api_json()["kind"], "agent");
    assert_eq!(process.state, "running");

    let paused = supervisor.set_workflow_paused(true).unwrap();
    assert!(paused.workflow_paused);
    assert!(supervisor.pause_state_path().exists());
    let stored: Value =
        serde_json::from_slice(&fs::read(supervisor.pause_state_path()).unwrap()).unwrap();
    assert_eq!(stored, json!({"workflow_paused": true}));

    fs::write(
        supervisor.pause_state_path(),
        serde_json::to_vec_pretty(&json!({
            "background_processes_stopped": true,
            "agents_paused": false
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        supervisor.pause_state().unwrap().workflow_paused,
        "legacy split pause state must migrate to the single workflow gate"
    );

    let stopped = supervisor.signal(&process.id, "stop").unwrap();
    assert_eq!(stopped.state, "stopped");
    assert!(supervisor.inspect(&process.id).is_err());
    assert_eq!(supervisor.list().unwrap().len(), 0);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_archives_launched_process_exit_and_output() {
    let temp_root = unique_temp_dir("process-reaper");
    let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command: shell_binary().to_string(),
            args: shell_args("printf 'retained failure\\n' >&2; exit 7"),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    while supervisor
        .processes_dir()
        .join(format!("{}.json", process.id))
        .exists()
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(supervisor.list().unwrap().is_empty());
    let terminal = supervisor.wait(&process.id).unwrap();
    assert_eq!(terminal.state, "failed");
    assert_eq!(terminal.exit_code, Some(7));
    assert!(
        supervisor
            .stream(&process.id)
            .unwrap()
            .contains("retained failure")
    );
    assert!(supervisor.process_history_path(&process.id).exists());

    supervisor.cleanup(&process.id).unwrap();
    assert!(!supervisor.process_history_path(&process.id).exists());
    assert!(supervisor.stream(&process.id).is_err());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_recovery_does_not_steal_completion_status() {
    let temp_root = unique_temp_dir("process-completion-recovery");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let output = supervisor
        .run_to_completion_with_output(
            ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command: shell_binary().to_string(),
                args: shell_args("printf recovered-output").to_vec(),
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: None,
                sensitive: false,
                metadata: Default::default(),
            },
            |_, _| {
                let _ = FileProcessSupervisor::new(&runtime_root).recover();
            },
        )
        .unwrap();

    assert!(output.success());
    assert_eq!(output.stdout, "recovered-output");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_keeps_process_launch_separate_from_workflow_pause_and_recovers_stale_processes()
 {
    let temp_root = unique_temp_dir("process-recover");
    let supervisor = FileProcessSupervisor::new(temp_root.join("run/8080"));
    supervisor.set_workflow_paused(true).unwrap();
    let launched = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: shell_binary().to_string(),
            args: long_running_shell_args().to_vec(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    assert_eq!(launched.state, "running");
    supervisor.set_workflow_paused(false).unwrap();
    supervisor.signal(&launched.id, "terminate").unwrap();

    supervisor
        .register(ManagedProcess {
            id: "stale".to_string(),
            owner: ProcessOwner::Maintenance,
            pid: None,
            state: "running".to_string(),
            label: Some("stale".to_string()),
            details: None,
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let recovered = supervisor.recover().unwrap();
    assert!(recovered.is_empty());
    assert!(supervisor.inspect("stale").is_err());
    let stale = supervisor.wait("stale").unwrap();
    assert_eq!(stale.state, "interrupted");
    assert!(
        stale
            .details
            .as_deref()
            .unwrap()
            .contains("running process had no pid during recovery")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_cleans_deferred_artifacts_after_handoff_release() {
    let temp_root = unique_temp_dir("process-artifact-handoff");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process_id = "workflow-owned-transcript";
    let stdout_path = supervisor
        .processes_dir()
        .join(format!("{process_id}.stdout.log"));
    fs::create_dir_all(supervisor.processes_dir()).unwrap();
    fs::write(&stdout_path, "workflow result").unwrap();
    let handoff = supervisor.begin_artifact_handoff(process_id).unwrap();
    supervisor
        .register(ManagedProcess {
            id: process_id.to_string(),
            owner: ProcessOwner::Agent,
            pid: None,
            state: "running".to_string(),
            label: Some("Goal Agent".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();

    assert!(supervisor.recover().unwrap().is_empty());
    assert!(stdout_path.is_file());
    let process_path = supervisor.process_history_path(process_id);
    let reconciled: ManagedProcess =
        serde_json::from_slice(&fs::read(&process_path).unwrap()).unwrap();
    assert_eq!(reconciled.state, "interrupted");

    supervisor.finish_artifact_handoff(handoff).unwrap();
    supervisor.cleanup(process_id).unwrap();
    assert!(!stdout_path.exists());
    assert!(!process_path.exists());
    assert!(!supervisor.artifact_handoff_path(process_id).exists());

    fs::remove_dir_all(temp_root).unwrap();
}
