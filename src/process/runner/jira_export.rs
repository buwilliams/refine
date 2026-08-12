use super::*;

pub(super) fn run_jira_export_operation(
    runtime_root: &Path,
    operation_id: &str,
) -> RefineResult<()> {
    let registry = FileOperationRegistry::new(runtime_root);
    let operation = registry.status(operation_id)?;
    if operation.owner != "goals:jira-export" {
        return Err(RefineError::InvalidInput(format!(
            "Operation {operation_id} is not a Jira export"
        )));
    }
    if matches!(
        operation.state,
        OperationState::Cancelled | OperationState::Interrupted
    ) {
        return Ok(());
    }
    let request = match serde_json::from_value::<JiraExportOperationRequest>(operation.request) {
        Ok(request) => request,
        Err(error) => {
            let error = RefineError::Serialization(format!(
                "failed to decode Jira export request: {error}"
            ));
            let _ = registry.fail_with_error(
                operation_id,
                json!({
                    "code": "jira_export_request_invalid",
                    "message": error.to_string()
                }),
            );
            return Err(error);
        }
    };
    let mut last_stage = String::new();
    let result = FileGoalExportService::with_runtime_root(
        request.refine_dir,
        request.target_root,
        runtime_root,
    )
    .with_operation_id(operation_id)
    .export_bulk_jira_csv_with_progress(&request.selection, |message, completed, total| {
        ensure_jira_export_active(&registry, operation_id)?;
        let stage = match message {
            "Loading selected Goal evidence" => "load-goals",
            "Looking up commit evidence" => "commit-evidence",
            "Building Jira CSV" => "build-csv",
            _ => "export",
        };
        registry.update_progress(
            operation_id,
            json!({
                "stage": stage,
                "message": message,
                "completed": completed,
                "total": total
            }),
        )?;
        if last_stage != stage {
            last_stage = stage.to_string();
            append_jira_export_log(
                &registry,
                operation_id,
                "info",
                message,
                json!({"stage": stage, "completed": completed, "total": total}),
            )?;
        }
        Ok(())
    });

    match result {
        Ok(export) => {
            let completion = registry.succeed_with_result_and_progress(
                operation_id,
                json!({
                    "stage": "complete",
                    "message": "Jira CSV ready",
                    "completed": export.goal_count,
                    "total": export.goal_count
                }),
                json!({"http_status": 200, "export": export}),
            )?;
            if !matches!(completion.state, OperationState::Succeeded) {
                return Ok(());
            }
            append_jira_export_log(
                &registry,
                operation_id,
                "info",
                "Jira CSV export completed",
                json!({
                    "goal_count": export.goal_count,
                    "commit_count": export.commit_count,
                    "filename": export.filename
                }),
            )?;
            Ok(())
        }
        Err(_error) if jira_export_stopped(&registry, operation_id) => Ok(()),
        Err(error) => {
            let _ = append_jira_export_log(
                &registry,
                operation_id,
                "error",
                "Jira CSV export failed",
                json!({"message": error.to_string()}),
            );
            let _ = registry.fail_with_error(
                operation_id,
                json!({
                    "code": "jira_export_failed",
                    "message": error.to_string()
                }),
            );
            Err(error)
        }
    }
}

pub(super) fn ensure_jira_export_active(
    registry: &FileOperationRegistry,
    operation_id: &str,
) -> RefineResult<()> {
    if jira_export_stopped(registry, operation_id) {
        return Err(RefineError::Conflict(
            "Jira export operation is no longer active".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn jira_export_stopped(registry: &FileOperationRegistry, operation_id: &str) -> bool {
    registry
        .status(operation_id)
        .map(|operation| {
            matches!(
                operation.state,
                OperationState::Cancelled | OperationState::Interrupted | OperationState::Failed
            )
        })
        .unwrap_or(false)
}

pub(super) fn append_jira_export_log(
    registry: &FileOperationRegistry,
    operation_id: &str,
    severity: &str,
    message: &str,
    details: Value,
) -> RefineResult<()> {
    registry.append_log(
        operation_id,
        LogEntry {
            datetime: String::new(),
            severity: severity.to_string(),
            category: "jira-export".to_string(),
            message: message.to_string(),
            details: details.as_object().cloned(),
            actions: Vec::new(),
            actor: Some("jira-export-worker".to_string()),
            goal_id: None,
        },
    )?;
    Ok(())
}

pub(super) fn refresh_projection(
    runtime_root: &Path,
    target_root: &Path,
) -> RefineResult<crate::tools::product::project_projection::ProjectionSnapshot> {
    let refine_dir = prepare_refine_dir(target_root)?;
    let store = FileProjectProjectionStore::with_runtime_root(&refine_dir, runtime_root);
    let projection = store.rebuild_projection()?;
    store.persist_projection_snapshot(&runtime_root.join("cache"), &projection)?;
    Ok(projection)
}
