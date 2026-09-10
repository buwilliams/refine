//! Startup recovery uses the same lifetime proof as deadline maintenance.
use super::*;
use crate::infrastructure::process::subprocess::owned_groups::test_fixture::UnobservedChild;
#[test]
fn recovery_preserves_missing_primary_and_unobserved_descendant_evidence() {
    for agents in [false, true] {
        for tracked in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "refine-operation-ownership-{}",
                uuid::Uuid::new_v4()
            ));
            let registry = FileOperationRegistry::new(&root);
            let operation = registry.register("test:ownership").unwrap();
            let owner = FileProcessSupervisor::new(if agents {
                root.join("agents")
            } else {
                root.clone()
            });
            let fixture = UnobservedChild::launch(
                &owner,
                tracked,
                json!({"operation_id":operation.id,"workflow_incarnation":"ownership-recovery"}),
            );
            fs::remove_file(
                owner
                    .runtime_root
                    .join("processes")
                    .join(format!("{}.json", fixture.group.process.id)),
            )
            .unwrap();
            assert!(fixture.child_alive());
            registry.recover_active_supervised().unwrap();
            let result = registry.status(&operation.id).unwrap();
            if tracked {
                assert_eq!(result.state, OperationState::Interrupted);
                assert!(!owner.group_pending(&fixture.group.process).unwrap());
                assert!(!fixture.child_alive());
            } else {
                assert_eq!(result.state, OperationState::Failed);
                assert_eq!(
                    result.error.unwrap()["code"],
                    "operation_recovery_process_termination_failed"
                );
                assert!(owner.group_pending(&fixture.group.process).unwrap());
                assert!(!owner.capacity_processes().unwrap().is_empty());
            }
            drop(fixture);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn cancellation_retains_unobserved_descendants_until_scope_exit_is_proved() {
    let mut results = Vec::new();
    for agents in [false, true] {
        for tracked in [false, true] {
            let root = std::env::temp_dir()
                .join(format!("refine-cancel-ownership-{}", uuid::Uuid::new_v4()));
            let registry = FileOperationRegistry::new(&root);
            let operation = registry
                .register_with_request(
                    "test:cancel-ownership",
                    json!({"defer_cancellation_terminal":true}),
                )
                .unwrap();
            let owner = FileProcessSupervisor::new(if agents {
                root.join("agents")
            } else {
                root.clone()
            });
            let fixture = UnobservedChild::launch(
                &owner,
                tracked,
                json!({"event_operation_id":operation.id,"workflow_incarnation":"cancel-ownership"}),
            );
            fs::remove_file(
                owner
                    .runtime_root
                    .join("processes")
                    .join(format!("{}.json", fixture.group.process.id)),
            )
            .unwrap();
            registry.cancel(&operation.id).unwrap();
            let verify_deferred = registry
                .ensure_cancellation_processes_exited(&operation.id)
                .is_err();
            let settle_deferred = registry.settle_cancellation(&operation.id).is_err();
            let state = registry.status(&operation.id).unwrap().state;
            let stopped = registry
                .terminate_associated_processes(&operation.id)
                .is_ok();
            let alive = fixture.child_alive();
            let settled_after_stop = registry.settle_cancellation(&operation.id).is_ok();
            drop(fixture);
            fs::remove_dir_all(root).unwrap();
            results.push((
                agents,
                tracked,
                verify_deferred,
                settle_deferred,
                state,
                stopped,
                alive,
                settled_after_stop,
            ));
        }
    }
    for (agents, tracked, verify, settle, state, stopped, alive, final_settlement) in results {
        assert!(
            verify && settle && state == OperationState::Cancelling,
            "agents={agents}, tracked={tracked}: verification deferred={verify}, settlement deferred={settle}, state={state:?}"
        );
        assert_eq!(stopped, tracked);
        assert_eq!(alive, !tracked);
        assert_eq!(final_settlement, tracked);
    }
}
