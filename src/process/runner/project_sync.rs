use super::*;

pub(super) fn run_project_sync_operation(
    runtime_root: &Path,
    target_root: &Path,
    operation_id: &str,
) -> RefineResult<()> {
    let registry = FileOperationRegistry::new(runtime_root);
    let mut attempted = None;
    let result = (|| {
        registry.update_progress(
            operation_id,
            json!({"message": "Synchronizing Refine state"}),
        )?;
        let node_id = state_sync_node_id(runtime_root, target_root)?;
        let source = "project_sync_operation";
        let attempt_id = record_state_sync_attempt(runtime_root, target_root, &node_id, source)?;
        attempted = Some((attempt_id, source));
        let git_sync = match FileGitSyncService::new(target_root, runtime_root)
            .with_agent_resolution()
            .sync_with_attempt(StateSyncAttemptContext::new(attempt_id.to_string(), source))
        {
            Ok(result) => {
                record_state_sync_result(
                    runtime_root,
                    target_root,
                    &node_id,
                    attempt_id,
                    source,
                    &result,
                )?;
                result
            }
            Err(error) => {
                let _ = record_state_sync_failure(
                    runtime_root,
                    target_root,
                    &node_id,
                    attempt_id,
                    source,
                    &error,
                );
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
            let conflict_report = latest_state_sync_conflict_report(runtime_root)
                .ok()
                .flatten()
                .filter(|report| {
                    attempted.is_some_and(|(attempt_id, source)| {
                        report.attempt_id == attempt_id.to_string()
                            && report.attempt_source == source
                    })
                });
            let _ = registry.fail_with_error(
                operation_id,
                json!({
                    "code": "project_sync_failed",
                    "message": error.to_string(),
                    "conflict_report_id": conflict_report.as_ref().map(|report| report.report_id.as_str()),
                    "conflict_report_location": conflict_report.as_ref().map(|report| report.report_location.as_str()),
                    "recovery_guidance": conflict_report.as_ref().map(|report| &report.recovery)
                }),
            );
            Err(error)
        }
    }
}
