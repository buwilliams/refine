use super::*;
use crate::infrastructure::process::subprocess::scheduler_observation::workflow_incarnation;
fn fixture(metadata: Value) -> (PathBuf, FileProcessSupervisor, ManagedProcess) {
    let root = std::env::temp_dir().join(format!("refine-maintenance-{}", uuid::Uuid::new_v4()));
    let supervisor = FileProcessSupervisor::new(&root);
    let mut values = json!({"isolated_process_group": true, "agent_hard_cap_millis": 1000});
    values
        .as_object_mut()
        .unwrap()
        .extend(metadata.as_object().unwrap().clone());
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: "/bin/sleep".into(),
            args: vec!["60".into()],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(values).unwrap(),
        })
        .unwrap();
    (root, supervisor, process)
}
fn stop(supervisor: &FileProcessSupervisor) {
    for group in supervisor.owned_groups().unwrap() {
        supervisor
            .stop_owned_group(&group, Duration::from_secs(2))
            .unwrap();
    }
}
#[test]
fn original_deadline_survives_metadata_refresh_and_preserves_transcripts() {
    let (root, supervisor, mut process) = fixture(json!({}));
    let group = supervisor.owned_groups().unwrap().remove(0);
    let transcript = root.join("retained-agent.txt");
    std::fs::write(&transcript, "original evidence").unwrap();
    process.stdout_path = Some(transcript.display().to_string());
    let mut details: Value = serde_json::from_str(process.details.as_deref().unwrap()).unwrap();
    details["agent_hard_cap_millis"] = json!(600_000);
    process.details = Some(details.to_string());
    supervisor.register(process.clone()).unwrap();
    let now = process.started_at.parse::<i64>().unwrap() + 1001;
    assert!(maintain_group(&root, &supervisor, &group, now).unwrap());
    assert!(!FileProcessSupervisor::process_is_alive(&process).unwrap());
    assert_eq!(
        std::fs::read_to_string(transcript).unwrap(),
        "original evidence"
    );
    let evidence: Value = serde_json::from_slice(
        &std::fs::read(
            root.join("deadline-reconciliation")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(evidence["confirmed_exit"], true);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn toolbar_and_human_input_exceptions_do_not_create_false_timeouts() {
    for metadata in [
        json!({"toolbar_timeout_protected": true}),
        json!({"agent_hard_cap_millis":600_000,"agent_idle_timeout_millis":1,"attention_state":"needs_input"}),
    ] {
        let (root, supervisor, process) = fixture(metadata);
        let group = supervisor.owned_groups().unwrap().remove(0);
        assert!(
            !maintain_group(
                &root,
                &supervisor,
                &group,
                process.started_at.parse::<i64>().unwrap() + 2000
            )
            .unwrap()
        );
        assert!(FileProcessSupervisor::process_is_alive(&process).unwrap());
        stop(&supervisor);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn attached_input_and_signal_writes_refresh_the_independent_idle_deadline() {
    for key in ["command_path", "signal_path"] {
        let (root, supervisor, mut process) = fixture(json!({
            "agent_hard_cap_millis": 600_000, "agent_idle_timeout_millis": 1000,
            "kind": "interactive_session", "attention_state": "working"
        }));
        let start = process.started_at.parse::<i64>().unwrap();
        let group = supervisor.owned_groups().unwrap().remove(0);
        let activity_path = root.join(format!("{key}.jsonl"));
        std::fs::write(&activity_path, "activity").unwrap();
        let set_time = |path: &str, at: i64| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + Duration::from_millis(at as u64)),
                )
                .unwrap();
        };
        for path in [&process.stdout_path, &process.stderr_path]
            .into_iter()
            .flatten()
        {
            set_time(path, start);
        }
        set_time(activity_path.to_str().unwrap(), start + 1500);
        let mut details: Value = serde_json::from_str(process.details.as_deref().unwrap()).unwrap();
        details[key] = json!(activity_path);
        process.details = Some(details.to_string());
        supervisor.register(process.clone()).unwrap();
        let prematurely_expired = maintain_group(&root, &supervisor, &group, start + 2000).unwrap();
        let alive = FileProcessSupervisor::process_is_alive(&process).unwrap();
        let expired_after_silence = if !prematurely_expired {
            maintain_group(&root, &supervisor, &group, start + 2501).unwrap()
        } else {
            false
        };
        stop(&supervisor);
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            !prematurely_expired && alive,
            "recent {key} activity was ignored"
        );
        assert!(
            expired_after_silence,
            "idle enforcement was disabled after {key} activity"
        );
    }
}

#[test]
fn a_silent_process_uses_its_original_start_before_any_output_file_exists() {
    let (root, supervisor, process) = fixture(json!({
        "agent_hard_cap_millis": 600_000, "agent_idle_timeout_millis": 1000
    }));
    for path in [&process.stdout_path, &process.stderr_path]
        .into_iter()
        .flatten()
    {
        std::fs::remove_file(path).unwrap();
    }
    let group = supervisor.owned_groups().unwrap().remove(0);
    let expired = maintain_group(
        &root,
        &supervisor,
        &group,
        process.started_at.parse::<i64>().unwrap() + 1001,
    )
    .unwrap();
    stop(&supervisor);
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        expired,
        "a silent process escaped its recorded idle deadline"
    );
}
#[test]
fn corrupt_group_and_stalled_workflow_do_not_block_other_deadlines() {
    let (root, supervisor, process) = fixture(json!({"agent_hard_cap_millis":1}));
    let bad = root.join("owned-groups/corrupt.json");
    std::fs::write(&bad, "{bad").unwrap();
    // A non-ticking worker is alive during deadline maintenance; no scheduler participation.
    let mut spec = background_worker_spec(Path::new("/bin/sleep"), &root, None, WORKFLOW_RUNNER);
    spec.command = "/bin/sleep".into();
    spec.args = vec!["60".into()];
    let worker = supervisor.launch(spec).unwrap();
    assert!(workflow_incarnation(&worker).is_some());
    thread::sleep(Duration::from_millis(5));
    let health = maintain_daemon(&root);
    assert_eq!(health.expired_groups, 1);
    assert!(health.failures.iter().any(|e| e.contains("corrupt.json")));
    assert!(!FileProcessSupervisor::process_is_alive(&process).unwrap());
    assert!(FileProcessSupervisor::process_is_alive(&worker).unwrap());
    std::fs::remove_file(bad).unwrap();
    stop(&supervisor);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn deadline_reconciliation_defers_live_siblings_and_preserves_newer_operation_revision() {
    let (root, supervisor, mut process) = fixture(json!({"agent_hard_cap_millis":1}));
    let operations = FileOperationRegistry::new(&root);
    let operation = operations.register("deadline:test").unwrap();
    let mut metadata: Value = serde_json::from_str(process.details.as_deref().unwrap()).unwrap();
    metadata["operation_id"] = json!(operation.id);
    process.details = Some(metadata.to_string());
    supervisor.register(process.clone()).unwrap();
    let sibling = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: "/bin/sleep".into(),
            args: vec!["60".into()],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(json!({"operation_id": operation.id,
            "agent_hard_cap_millis":600_000}))
            .unwrap(),
        })
        .unwrap();
    let groups = supervisor.owned_groups().unwrap();
    let group = groups.iter().find(|g| g.process.id == process.id).unwrap();
    let sibling_group = groups.iter().find(|g| g.process.id == sibling.id).unwrap();
    let now = process.started_at.parse::<i64>().unwrap() + 1001;
    assert!(
        maintain_group(&root, &supervisor, group, now)
            .unwrap_err()
            .to_string()
            .contains("live process group")
    );
    assert_eq!(
        operations.status(&operation.id).unwrap().state,
        OperationState::Running
    );
    assert!(supervisor.group_pending(&sibling).unwrap());
    let newer = operations
        .compare_and_set(&operation.id, operation.revision, |current| {
            current.progress = json!({"newer_owner": true});
            Ok(())
        })
        .unwrap();
    supervisor
        .stop_owned_group(sibling_group, Duration::from_secs(2))
        .unwrap();
    assert!(maintain_group(&root, &supervisor, group, now).unwrap());
    let retained = operations.status(&operation.id).unwrap();
    assert_eq!(retained.revision, newer.revision);
    assert_eq!(retained.state, OperationState::Running);
    assert_eq!(retained.progress, newer.progress);
    // The same API settles an authoritative operation only after the shared exit proof.
    operations
        .interrupt_after_process_exit(&operation.id, newer.revision, || Ok(true))
        .unwrap();
    assert_eq!(
        operations.status(&operation.id).unwrap().state,
        OperationState::Interrupted
    );
    std::fs::remove_dir_all(root).unwrap();
}
