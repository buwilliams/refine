use super::*;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn exclusive_operation_registration_serializes_one_owner() {
    let temp_root = unique_temp_dir("operations-exclusive-owner");
    let registry = FileOperationRegistry::new(&temp_root);
    let first = registry
        .register_exclusive_with_request("quality:GOAL1:abc", json!({"source": "workflow"}))
        .unwrap();
    let conflict = registry
        .register_exclusive_with_request("quality:GOAL1:abc", json!({"source": "manual"}))
        .unwrap_err();
    assert!(conflict.to_string().contains(&first.id));
    registry
        .finish(&first.id, OperationState::Succeeded)
        .unwrap();
    assert!(
        registry
            .register_exclusive_with_request("quality:GOAL1:abc", json!({"source": "manual"}))
            .is_ok()
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn workflow_cancellation_tombstone_blocks_late_operation_registration() {
    let temp_root = unique_temp_dir("operations-workflow-cancellation-tombstone");
    let registry = FileOperationRegistry::new(&temp_root);

    assert!(
        registry
            .cancel_workflow_execution_operations("execution-cancelled-before-registration")
            .unwrap()
            .is_empty()
    );
    let exclusive = registry
        .register_exclusive_with_request(
            "merger:GOAL1:1",
            json!({"execution_id": "execution-cancelled-before-registration"}),
        )
        .unwrap_err();
    assert!(
        exclusive
            .to_string()
            .contains("cancelled before operation registration"),
        "{exclusive}"
    );
    let ordinary = registry
        .register_with_request(
            "workflow:test",
            json!({"execution_id": "execution-cancelled-before-registration"}),
        )
        .unwrap_err();
    assert!(
        ordinary
            .to_string()
            .contains("cancelled before operation registration"),
        "{ordinary}"
    );
    assert!(
        registry
            .register_exclusive_with_request(
                "merger:GOAL1:1",
                json!({"execution_id": "replacement-execution"}),
            )
            .is_ok()
    );

    let cancellations = fs::read_dir(registry.operations_dir().join(".workflow-cancellations"))
        .unwrap()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    assert_eq!(cancellations.len(), 1);
    let evidence: Value =
        serde_json::from_slice(&fs::read(cancellations[0].path()).unwrap()).unwrap();
    assert_eq!(evidence["error"]["code"], "workflow_execution_cancelled");
    assert!(
        evidence["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.trim().is_empty())
    );
    fs::remove_dir_all(temp_root).unwrap();
}

use crate::process::subprocess::{
    ManagedProcessSpec, ProcessOwner, ProcessResourceLimits, ProcessSupervisor,
    managed_pid_is_alive,
};

#[test]
fn file_operation_registry_registers_recovers_and_cancels_operations() {
    let temp_root = unique_temp_dir("operations");
    let registry = FileOperationRegistry::new(temp_root.join("run/8080"));
    let operation = registry.register("bulk_update_goals").unwrap();
    assert_eq!(operation.state, OperationState::Running);
    assert_eq!(
        registry.status(&operation.id).unwrap().owner,
        "bulk_update_goals"
    );
    assert_eq!(registry.recover().unwrap().len(), 1);

    let interrupted = registry.interrupt_active().unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(
        registry.status(&operation.id).unwrap().state,
        OperationState::Interrupted
    );
    assert_eq!(
        registry.status(&operation.id).unwrap().error.unwrap()["code"],
        "operation_interrupted"
    );
    let late_completion = registry
        .finish_with_result(
            &operation.id,
            OperationState::Succeeded,
            json!({"result": "must not replace interruption"}),
        )
        .unwrap();
    assert_eq!(late_completion.state, OperationState::Interrupted);
    let late_failure = registry
        .fail_with_error(
            &operation.id,
            json!({"code": "late_worker_failure", "message": "worker exited late"}),
        )
        .unwrap();
    assert_eq!(late_failure.state, OperationState::Interrupted);
    assert_eq!(late_failure.error.unwrap()["code"], "operation_interrupted");
    let late_progress = registry
        .update_progress(&operation.id, json!({"stage": "complete"}))
        .unwrap();
    assert_eq!(late_progress.state, OperationState::Interrupted);
    assert_eq!(late_progress.progress, json!({}));

    let cancelled = registry.cancel(&operation.id).unwrap();
    assert_eq!(cancelled.state, OperationState::Cancelled);
    assert_eq!(cancelled.state.as_api_status(), "cancelled");

    let recovery_failure = registry.register("import:persist").unwrap();
    registry.cancel(&recovery_failure.id).unwrap();
    let failed = registry
        .fail_with_error(
            &recovery_failure.id,
            json!({
                "code": "projection_refresh_failed",
                "message": "cancel rollback could not refresh the projection"
            }),
        )
        .unwrap();
    assert_eq!(failed.state, OperationState::Cancelled);
    assert_eq!(failed.error.unwrap()["code"], "projection_refresh_failed");
    assert_eq!(
        registry.status(&recovery_failure.id).unwrap().state,
        OperationState::Cancelled
    );
    let (logs, _, _) = registry.page_logs(&recovery_failure.id, 20, 0).unwrap();
    assert!(logs.iter().any(|entry| entry.message == "Operation failed"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn deferred_cancellation_can_settle_as_a_durable_partial_failure() {
    let temp_root = unique_temp_dir("operations-deferred-cancel-partial-failure");
    let registry = FileOperationRegistry::new(temp_root.join("run/8080"));
    let operation = registry
        .register_with_request(
            "import:persist",
            json!({"defer_cancellation_terminal": true}),
        )
        .unwrap();
    let cancelling = registry.cancel(&operation.id).unwrap();
    assert_eq!(cancelling.state, OperationState::Cancelling);

    let error = json!({
        "code": "import_rollback_incomplete",
        "kind": "partial_failure",
        "message": "manual recovery required"
    });
    let result = json!({
        "unrecovered_goal_ids": ["GOAL1"],
        "rollback_failures": ["Goal GOAL1: injected deletion failure"]
    });
    let failed = registry
        .fail_with_partial_result(&operation.id, error.clone(), result.clone())
        .unwrap();
    assert_eq!(failed.state, OperationState::Failed);
    assert_eq!(failed.error, Some(error));
    assert_eq!(failed.result, result);

    let restarted_registry = FileOperationRegistry::new(temp_root.join("run/8080"));
    let durable = restarted_registry.status(&operation.id).unwrap();
    assert_eq!(durable, failed);
    assert_eq!(
        restarted_registry.recover().unwrap(),
        vec![failed],
        "partial-failure evidence must survive registry reconstruction"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_operation_registry_state_replacement_never_exposes_partial_json() {
    let temp_root = unique_temp_dir("operations-atomic-state");
    let registry = FileOperationRegistry::new(temp_root.join("run/8080"));
    let operation = registry.register("goals:jira-export").unwrap();
    let reader_registry = registry.clone();
    let operation_id = operation.id.clone();
    let reader = thread::spawn(move || {
        for _ in 0..500 {
            reader_registry.status(&operation_id).unwrap();
        }
    });
    for completed in 0..500 {
        registry
            .update_progress(&operation.id, json!({"completed": completed}))
            .unwrap();
    }
    reader.join().unwrap();
    assert_eq!(
        registry.status(&operation.id).unwrap().progress["completed"],
        499
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_operation_registry_atomically_finds_or_registers_one_durable_replacement() {
    let temp_root = unique_temp_dir("operations-replacement-idempotency");
    let runtime_root = temp_root.join("run/8080");
    let registry = FileOperationRegistry::new(&runtime_root);
    let source = registry
        .register_with_request(
            "goals:jira-export",
            json!({"selection": {"selected_ids": ["GOAL1"]}}),
        )
        .unwrap();
    registry.interrupt_active().unwrap();

    let retry_identity = format!("goals:jira-export:retry:{}", source.id);
    let barrier = Arc::new(Barrier::new(3));
    let callers = (0..2)
        .map(|_| {
            let registry = registry.clone();
            let source_id = source.id.clone();
            let retry_identity = retry_identity.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                registry
                    .find_or_register_replacement(
                        "goals:jira-export",
                        &source_id,
                        &retry_identity,
                        json!({"selection": {"selected_ids": ["GOAL1"]}}),
                    )
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let registrations = callers
        .into_iter()
        .map(|caller| caller.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(registrations[0].operation.id, registrations[1].operation.id);
    assert_eq!(
        registrations
            .iter()
            .filter(|registration| registration.created)
            .count(),
        1
    );
    let replacement = &registrations[0].operation;
    assert_eq!(replacement.request["recovery_of"], source.id);
    assert_eq!(replacement.request["retry_identity"], retry_identity);
    assert_eq!(
        registry
            .recover()
            .unwrap()
            .iter()
            .filter(|operation| operation.request["recovery_of"] == source.id)
            .count(),
        1
    );

    let reopened = FileOperationRegistry::new(&runtime_root)
        .find_or_register_replacement(
            "goals:jira-export",
            &source.id,
            &retry_identity,
            json!({"selection": {"selected_ids": ["GOAL1"]}}),
        )
        .unwrap();
    assert!(!reopened.created);
    assert_eq!(reopened.operation.id, replacement.id);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_operation_registry_supervised_cancel_terminates_process_and_refreshes_projection() {
    let temp_root = unique_temp_dir("operations-supervised-cancel");
    let runtime_root = temp_root.join("run/8080");
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = registry.register("goals:jira-export").unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let process = supervisor
        .launch(operation_helper_process_spec(&operation.id))
        .unwrap();
    let pid = process.pid.unwrap();
    assert!(managed_pid_is_alive(pid).unwrap());

    let projection_refreshed = AtomicBool::new(false);
    let projection_refresher = || {
        projection_refreshed.store(true, AtomicOrdering::SeqCst);
        Ok(())
    };
    let cancelled = registry
        .cancel_supervised(&operation.id, &projection_refresher)
        .unwrap();

    assert_eq!(cancelled.state, OperationState::Cancelled);
    assert!(projection_refreshed.load(AtomicOrdering::SeqCst));
    wait_for_managed_pid_exit(pid);
    assert!(!managed_pid_is_alive(pid).unwrap());
    assert_eq!(
        registry.status(&operation.id).unwrap().state,
        OperationState::Cancelled
    );
    let late_completion = registry
        .finish_with_result(
            &operation.id,
            OperationState::Succeeded,
            json!({"result": "must not replace cancellation"}),
        )
        .unwrap();
    assert_eq!(late_completion.state, OperationState::Cancelled);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn operation_launch_guard_serializes_cancel_and_rejects_late_launch() {
    let temp_root = unique_temp_dir("operations-launch-cancel-barrier");
    let runtime_root = temp_root.join("run/8080");
    let registry = FileOperationRegistry::new(&runtime_root);
    let operation = registry.register("quality:GOAL1:abc").unwrap();

    let launch_guard = registry.active_launch_guard(&operation.id).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let cancel_registry = registry.clone();
    let cancel_operation_id = operation.id.clone();
    let cancel_barrier = Arc::clone(&barrier);
    let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
    let cancellation = thread::spawn(move || {
        cancel_barrier.wait();
        let cancelled = cancel_registry.cancel(&cancel_operation_id).unwrap();
        cancelled_tx.send(cancelled).unwrap();
    });
    barrier.wait();
    assert!(
        cancelled_rx
            .recv_timeout(Duration::from_millis(25))
            .is_err(),
        "cancellation must wait until process registration releases the launch barrier"
    );
    drop(launch_guard);
    assert_eq!(
        cancelled_rx.recv().unwrap().state,
        OperationState::Cancelled
    );
    cancellation.join().unwrap();

    let late_launch = FileProcessSupervisor::new(&runtime_root)
        .launch(operation_helper_process_spec(&operation.id))
        .unwrap_err();
    assert!(
        late_launch
            .to_string()
            .contains("no later supervised process may start")
    );
    assert!(
        FileProcessSupervisor::new(&runtime_root)
            .list()
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_operation_registry_supervised_cancel_persists_capability_failures() {
    let temp_root = unique_temp_dir("operations-supervised-cancel-failures");

    let termination_runtime_root = temp_root.join("run/termination");
    let termination_registry = FileOperationRegistry::new(&termination_runtime_root);
    let termination_operation = termination_registry.register("goals:jira-export").unwrap();
    fs::write(
        termination_runtime_root.join("processes"),
        b"not a directory",
    )
    .unwrap();
    let projection_refreshed = AtomicBool::new(false);
    let projection_refresher = || {
        projection_refreshed.store(true, AtomicOrdering::SeqCst);
        Ok(())
    };
    let termination_error = termination_registry
        .cancel_supervised(&termination_operation.id, &projection_refresher)
        .unwrap_err();
    assert!(termination_error.to_string().contains("process registry"));
    assert!(!projection_refreshed.load(AtomicOrdering::SeqCst));
    let termination_operation = termination_registry
        .status(&termination_operation.id)
        .unwrap();
    assert_eq!(termination_operation.state, OperationState::Cancelled);
    assert_eq!(
        termination_operation.error.unwrap()["code"],
        "operation_process_termination_failed"
    );

    let projection_runtime_root = temp_root.join("run/projection");
    let projection_registry = FileOperationRegistry::new(&projection_runtime_root);
    let projection_operation = projection_registry.register("goals:jira-export").unwrap();
    let projection_refresher = || Err(RefineError::Io("projection refresh failed".to_string()));
    let projection_error = projection_registry
        .cancel_supervised(&projection_operation.id, &projection_refresher)
        .unwrap_err();
    assert_eq!(projection_error.to_string(), "projection refresh failed");
    let projection_operation = projection_registry
        .status(&projection_operation.id)
        .unwrap();
    assert_eq!(projection_operation.state, OperationState::Cancelled);
    assert_eq!(
        projection_operation.error.unwrap()["code"],
        "operation_cancel_projection_refresh_failed"
    );
    let (logs, _, _) = projection_registry
        .page_logs(&projection_operation.id, 20, 0)
        .unwrap();
    assert!(logs.iter().any(|entry| entry.message == "Operation failed"));

    fs::remove_dir_all(temp_root).unwrap();
}

fn operation_helper_process_spec(operation_id: &str) -> ManagedProcessSpec {
    #[cfg(windows)]
    let (command, args) = (
        "cmd".to_string(),
        vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()],
    );
    #[cfg(not(windows))]
    let (command, args) = (
        "sh".to_string(),
        vec!["-c".to_string(), "while :; do sleep 1; done".to_string()],
    );
    ManagedProcessSpec {
        owner: ProcessOwner::Runner,
        command,
        args,
        cwd: None,
        env: Vec::new(),
        stdin: None,
        limits: Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        }),
        authorization_command: Some("refine test operation helper".to_string()),
        sensitive: false,
        metadata: serde_json::from_value(json!({
            "kind": "runner",
            "worker_kind": "operation-capability-test-helper",
            "operation_id": operation_id
        }))
        .unwrap(),
    }
}

fn wait_for_managed_pid_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while managed_pid_is_alive(pid).unwrap_or(false) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}
