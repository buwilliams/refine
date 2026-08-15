use super::*;

pub(super) fn run_project_sync_operation(
    runtime_root: &Path,
    target_root: &Path,
    operation_id: &str,
) -> RefineResult<()> {
    let registry = FileOperationRegistry::new(runtime_root);
    let result = (|| {
        registry.update_progress(
            operation_id,
            json!({"message": "Synchronizing Refine state"}),
        )?;
        let node_id = state_sync_node_id(runtime_root, target_root)?;
        record_state_sync_attempt(runtime_root, target_root, &node_id)?;
        let git_sync = match FileGitSyncService::new(target_root, runtime_root).sync() {
            Ok(result) => {
                record_state_sync_result(runtime_root, target_root, &node_id, &result)?;
                result
            }
            Err(error) => {
                let _ = record_state_sync_failure(runtime_root, target_root, &node_id, &error);
                return Err(error);
            }
        };
        registry.update_progress(operation_id, json!({"message": "Rebuilding projection"}))?;
        let projection = refresh_projection(runtime_root, target_root)?;
        Ok::<Value, RefineError>(project_sync_result(&git_sync, &projection))
    })();
    match result {
        Ok(result) => {
            registry.finish_with_result(operation_id, OperationState::Succeeded, result)?;
            Ok(())
        }
        Err(error) => {
            let _ = registry.fail_with_error(
                operation_id,
                json!({
                    "code": "project_sync_failed",
                    "message": error.to_string()
                }),
            );
            Err(error)
        }
    }
}
