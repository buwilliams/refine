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
        let git_sync = FileGitSyncService::new(target_root, runtime_root).sync()?;
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
