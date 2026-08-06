use super::*;

pub(in crate::surfaces::web_server) fn workflow_retry_response(
    automation: &WorkflowEngine,
    execution_id: &str,
) -> ApiResponse {
    match automation.retry(execution_id) {
        Ok(retried_execution_id) => {
            match workflow_execution_json(automation, &retried_execution_id) {
                Ok(execution) => ApiResponse::json(
                    200,
                    json!({
                        "retried_from": execution_id,
                        "execution": execution
                    }),
                ),
                Err(error) => error_response(error),
            }
        }
        Err(error) => error_response(error),
    }
}

pub(in crate::surfaces::web_server) fn workflow_execution_json(
    automation: &WorkflowEngine,
    execution_id: &str,
) -> RefineResult<Value> {
    let state = automation.load_state()?;
    let claim = state.claim_by_execution(execution_id).ok_or_else(|| {
        RefineError::NotFound(format!("Workflow execution {execution_id} was not found"))
    })?;
    Ok(json!({
        "id": execution_id,
        "claim_id": claim.claim_id,
        "goal_id": claim.goal_id,
        "status": claim.state,
        "node_id": claim.node_id,
        "provider": claim.provider,
        "target_app_id": claim.target_app_id,
        "created_at": claim.created_at,
        "updated_at": claim.updated_at
    }))
}

pub(in crate::surfaces::web_server) fn diagnostics_cache_key(
    runtime_root: &std::path::Path,
    refine_dir: Option<&PathBuf>,
    repo_root: &std::path::Path,
) -> String {
    format!(
        "{}|{}|{}",
        runtime_root.display(),
        refine_dir
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string()),
        repo_root.display()
    )
}

pub(in crate::surfaces::web_server) fn live_process_summary(
    runtime_root: &std::path::Path,
    refine_dir: Option<&std::path::Path>,
) -> RefineResult<Value> {
    match refine_dir {
        Some(refine_dir) => {
            FileProcessStatusService::with_refine_dir(runtime_root, refine_dir).summary()
        }
        None => FileProcessStatusService::new(runtime_root).summary(),
    }
}

pub(in crate::surfaces::web_server) fn secret_scope_name_from_path(
    path: &str,
) -> Option<(String, String)> {
    let rest = path.strip_prefix("/agents/secrets/")?;
    let mut parts = rest.split('/');
    let scope = parts.next()?.trim();
    let name = parts.next()?.trim();
    if scope.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((scope.to_string(), name.to_string()))
}
