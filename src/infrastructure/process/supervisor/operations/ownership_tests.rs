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
