use super::*;

pub fn run_worker(
    worker_kind: &str,
    runtime_root: PathBuf,
    target_root: Option<PathBuf>,
    operation_id: Option<String>,
) -> RefineResult<()> {
    validate_worker_kind(worker_kind, true)?;
    match worker_kind {
        WORKFLOW_RUNNER => run_workflow_worker(&runtime_root),
        GIT_SYNC_RUNNER => run_git_sync_worker(&runtime_root),
        PROJECT_SYNC_RUNNER => {
            let target_root = target_root.ok_or_else(|| {
                RefineError::InvalidInput("project-sync worker requires --target-root".to_string())
            })?;
            let operation_id = operation_id.ok_or_else(|| {
                RefineError::InvalidInput("project-sync worker requires --operation-id".to_string())
            })?;
            run_project_sync_operation(&runtime_root, &target_root, &operation_id)
        }
        JIRA_EXPORT_RUNNER => {
            let operation_id = operation_id.ok_or_else(|| {
                RefineError::InvalidInput("jira-export worker requires --operation-id".to_string())
            })?;
            run_jira_export_operation(&runtime_root, &operation_id)
        }
        _ => unreachable!(),
    }
}
