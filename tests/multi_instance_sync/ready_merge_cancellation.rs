use super::*;

#[test]
#[ignore = "multi-process Ready Merge cancellation/replacement gate; run through xtask"]
fn ready_merge_multi_process_cancellation_replacement_retry_is_exactly_once() {
    let fixture = ready_merge_fixture("ready-merge-process-cancel");
    install_slow_main_hook(&fixture.remote);
    let first_output = fixture.root.join("first.json");
    let mut first = spawn_ready_merge_child(&fixture, &fixture.execution_id, &first_output);
    wait_for_workflow_git_process(&fixture.runtime_root, &fixture.execution_id, "push");

    let concurrent_work_items = FileWorkItemService::new(&fixture.refine_dir);
    let (revision_tx, revision_rx) = std::sync::mpsc::channel();
    let revision_writer = thread::spawn(move || {
        let result = concurrent_work_items.update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({"quality_message": "concurrent daemon revision"}),
        );
        revision_tx.send(result.map(|_| ())).unwrap();
    });
    assert!(
        matches!(
            revision_rx.recv_timeout(Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "the Ready Merge workflow lease must exclude a concurrent Goal revision"
    );
    fixture.automation().cancel(&fixture.execution_id).unwrap();
    assert!(first.wait().unwrap().success());
    revision_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    revision_writer.join().unwrap();
    let cancelled = read_json(&first_output);
    assert_eq!(cancelled["ok"], false, "{cancelled:#}");
    let operations = fs::read_dir(fixture.runtime_root.join("operations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .map(|entry| read_json(&entry.path()))
        .collect::<Vec<_>>();
    assert!(
        operations.iter().any(|operation| {
            operation["owner"] == "merger:GOAL1:1"
                && operation["state"] == "Cancelled"
                && operation["error"]["code"] == "workflow_execution_cancelled"
        }),
        "{operations:#?}"
    );
    assert!(
        AgentCapacityService::new(&fixture.runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty(),
        "workflow cancellation must release capacity even after signalling a child"
    );
    assert!(!git_succeeds(
        &fixture.repo,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.candidate,
            "origin/main",
        ],
    ));

    fs::remove_file(fixture.remote.join("hooks/pre-receive")).unwrap();
    let replacement_execution = fixture.automation().retry(&fixture.execution_id).unwrap();
    let stale_output = fixture.root.join("stale.json");
    assert!(
        spawn_ready_merge_child(&fixture, &fixture.execution_id, &stale_output)
            .wait()
            .unwrap()
            .success()
    );
    let stale = read_json(&stale_output);
    assert_eq!(stale["ok"], false, "{stale:#}");
    assert!(
        stale["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no longer owns"),
        "{stale:#}"
    );

    let retry_output = fixture.root.join("retry.json");
    assert!(
        spawn_ready_merge_child(&fixture, &replacement_execution, &retry_output)
            .wait()
            .unwrap()
            .success()
    );
    assert_eq!(read_json(&retry_output)["ok"], true);
    let repeat_output = fixture.root.join("repeat.json");
    assert!(
        spawn_ready_merge_child(&fixture, &replacement_execution, &repeat_output)
            .wait()
            .unwrap()
            .success()
    );
    assert_eq!(read_json(&repeat_output)["ok"], true);
    assert!(git_succeeds(
        &fixture.repo,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.candidate,
            "origin/main",
        ],
    ));
    let audit = fs::read_to_string(fixture.repo.join(".git/refine-audit.jsonl")).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"action\":\"merge_commit_no_ff\""))
            .count(),
        1
    );
    let _ = fs::remove_dir_all(&fixture.root);
}

#[test]
#[ignore = "multi-process Ready Merge pre-registration cancellation gate; run through xtask"]
fn ready_merge_multi_process_cancellation_before_operation_registration_is_atomic() {
    let fixture = ready_merge_fixture("ready-merge-process-launch-cancel");
    let operations_dir = fixture.runtime_root.join("operations");
    fs::create_dir_all(&operations_dir).unwrap();
    let operation_mutation_path = operations_dir.join(".mutations.lock");
    let operation_mutation = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&operation_mutation_path)
        .unwrap();
    operation_mutation.lock_exclusive().unwrap();
    let coordination_path = fixture
        .refine_dir
        .parent()
        .unwrap()
        .join(WORKFLOW_COORDINATION_LOCK);
    let coordination = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&coordination_path)
        .unwrap();
    coordination.lock_exclusive().unwrap();

    // Queue cancellation at the operation-registration barrier first. Ready Merge then acquires
    // workflow authority and blocks before registering its operation. Releasing the mutation
    // barrier reproduces the exact pre-registration window: cancellation must tombstone the
    // execution before the already-authoritative worker can register or launch Git.
    let automation = fixture.automation();
    let execution_id = fixture.execution_id.clone();
    let (cancel_started_tx, cancel_started_rx) = std::sync::mpsc::channel();
    let cancellation = thread::spawn(move || {
        cancel_started_tx.send(()).unwrap();
        automation.cancel(&execution_id)
    });
    cancel_started_rx.recv().unwrap();
    thread::sleep(Duration::from_millis(100));

    let output = fixture.root.join("launch-cancel.json");
    let mut child = spawn_ready_merge_child(&fixture, &fixture.execution_id, &output);
    FileExt::unlock(&coordination).unwrap();
    wait_for_exclusive_lock_holder(&coordination_path, "Ready Merge workflow lease");
    FileExt::unlock(&operation_mutation).unwrap();

    cancellation.join().unwrap().unwrap();
    assert!(child.wait().unwrap().success());
    let cancelled = read_json(&output);
    assert_eq!(cancelled["ok"], false, "{cancelled:#}");
    assert!(
        cancelled["error"]
            .as_str()
            .unwrap_or_default()
            .contains("cancelled before operation registration"),
        "{cancelled:#}"
    );
    assert!(!git_succeeds(
        &fixture.repo,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.candidate,
            "origin/main",
        ],
    ));
    let operations = merger_operations(&fixture.runtime_root);
    assert!(operations.is_empty(), "{operations:#?}");
    let cancellation_evidence = fs::read_dir(operations_dir.join(".workflow-cancellations"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| read_json(&entry.path()))
        .collect::<Vec<_>>();
    assert_eq!(cancellation_evidence.len(), 1);
    assert_eq!(
        cancellation_evidence[0]["error"]["code"],
        "workflow_execution_cancelled"
    );
    assert!(
        cancellation_evidence[0]["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.trim().is_empty())
    );
    assert!(
        AgentCapacityService::new(&fixture.runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );
    let _ = fs::remove_dir_all(&fixture.root);
}

#[test]
#[ignore = "multi-process Ready Merge settlement/cancellation gate; run through xtask"]
fn ready_merge_multi_process_cancellation_before_settlement_rejects_transition() {
    let root = temp_root("ready-merge-process-settlement-cancel");
    let runtime_root = root.join("run/8080");
    let ready = root.join("settlement-ready");
    let proceed = root.join("settlement-proceed");
    let transitioned = root.join("transitioned");
    let output = root.join("settlement.json");
    fs::create_dir_all(&runtime_root).unwrap();
    let execution_id = "exec-settlement-race";
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = registry
        .register_with_request(
            "merger:GOAL1:1",
            json!({"execution_id": execution_id, "goal_id": "GOAL1"}),
        )
        .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ready_merge_children::ready_merge_settlement_child_process",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("REFINE_SETTLEMENT_CHILD", "1")
        .env("REFINE_CHILD_RUNTIME", &runtime_root)
        .env("REFINE_CHILD_OPERATION", &operation.id)
        .env("REFINE_CHILD_READY", &ready)
        .env("REFINE_CHILD_PROCEED", &proceed)
        .env("REFINE_CHILD_TRANSITIONED", &transitioned)
        .env("REFINE_CHILD_OUTPUT", &output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_path(&ready, "post-fence pre-settlement child");

    registry
        .cancel_workflow_execution_operations(execution_id)
        .unwrap();
    fs::write(&proceed, b"go").unwrap();
    assert!(child.wait().unwrap().success());

    let result = read_json(&output);
    assert_eq!(result["ok"], false, "{result:#}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no longer owns"),
        "{result:#}"
    );
    assert!(!transitioned.exists());
    let cancelled = registry.status(&operation.id).unwrap();
    assert_eq!(format!("{:?}", cancelled.state).to_lowercase(), "cancelled");
    assert_eq!(
        cancelled.error.unwrap()["code"],
        "workflow_execution_cancelled"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "multi-process Ready Merge restart recovery gate; run through xtask"]
fn ready_merge_multi_process_restart_recovery_preserves_exactly_once() {
    let fixture = ready_merge_fixture("ready-merge-process-restart");
    install_slow_main_hook(&fixture.remote);
    let interrupted_output = fixture.root.join("interrupted.json");
    let mut interrupted =
        spawn_ready_merge_child(&fixture, &fixture.execution_id, &interrupted_output);
    wait_for_workflow_git_process(&fixture.runtime_root, &fixture.execution_id, "push");
    interrupted.kill().unwrap();
    let _ = interrupted.wait();

    let recovered = FileOperationRegistry::new(&fixture.runtime_root)
        .recover_active_supervised()
        .unwrap();
    assert!(
        recovered.iter().any(|operation| {
            operation.owner == "merger:GOAL1:1"
                && format!("{:?}", operation.state).to_lowercase() == "interrupted"
        }),
        "{recovered:#?}"
    );
    FileProcessSupervisor::new(&fixture.runtime_root)
        .recover()
        .unwrap();
    fs::remove_file(fixture.remote.join("hooks/pre-receive")).unwrap();
    let replacement_execution = fixture.automation().retry(&fixture.execution_id).unwrap();
    let retry_output = fixture.root.join("restart-retry.json");
    assert!(
        spawn_ready_merge_child(&fixture, &replacement_execution, &retry_output)
            .wait()
            .unwrap()
            .success()
    );
    assert_eq!(read_json(&retry_output)["ok"], true);
    assert!(git_succeeds(
        &fixture.repo,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.candidate,
            "origin/main",
        ],
    ));
    let audit = fs::read_to_string(fixture.repo.join(".git/refine-audit.jsonl")).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"action\":\"merge_commit_no_ff\""))
            .count(),
        1
    );
    let _ = fs::remove_dir_all(&fixture.root);
}
