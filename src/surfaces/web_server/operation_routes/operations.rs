use super::*;

impl InProcessWebServer {
    pub(in crate::surfaces::web_server) fn handle_operation_status(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        if self.runtime_root.is_none() {
            return runtime_root_unavailable("read background operations");
        }
        let Some(operation_id) = request
            .path
            .strip_prefix("/operations/")
            .filter(|operation_id| !operation_id.is_empty() && !operation_id.contains('/'))
        else {
            return operation_id_required();
        };
        match self.current_projection_with_runtime() {
            Ok(projection) => projection
                .runtime
                .background_operations
                .into_iter()
                .find(|operation| {
                    operation.get("id").and_then(|value| value.as_str()) == Some(operation_id)
                })
                .map(|operation| ApiResponse::json(200, json!({"operation": operation})))
                .unwrap_or_else(|| {
                    error_response(RefineError::NotFound(format!(
                        "Operation {operation_id} was not found"
                    )))
                }),
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_operation_logs(
        &self,
        request: ApiRequest,
        raw_path: &str,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read background operation logs");
        };
        let Some(operation_id) = request
            .path
            .strip_prefix("/operations/")
            .and_then(|path| path.strip_suffix("/logs"))
            .filter(|operation_id| !operation_id.is_empty() && !operation_id.contains('/'))
        else {
            return operation_id_required();
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 200);
        let offset = bounded_query_usize(raw_path, "offset", 0, usize::MAX);
        match FileOperationRegistry::new(runtime_root).page_logs(operation_id, limit, offset) {
            Ok((logs, has_more, total)) => {
                let log_count = logs.len();
                ApiResponse::json(
                    200,
                    json!({
                        "operation_id": operation_id,
                        "logs": logs,
                        "log_count": log_count,
                        "has_more": has_more,
                        "total": total,
                        "page": {
                            "limit": limit,
                            "offset": offset,
                            "has_more": has_more,
                            "total": total
                        }
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(in crate::surfaces::web_server) fn handle_operation_cancel(
        &self,
        request: ApiRequest,
    ) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("cancel background operations");
        };
        let Some(operation_id) = request
            .path
            .strip_prefix("/operations/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .filter(|operation_id| !operation_id.is_empty() && !operation_id.contains('/'))
        else {
            return operation_id_required();
        };
        let registry = FileOperationRegistry::new(runtime_root);
        let cancellation = registry.status(operation_id).and_then(|operation| {
            if operation.owner == "maintenance:source-upgrade" {
                let checkout = discover_refine_checkout()?;
                FileSourcePromotionService::new(checkout, runtime_root, self.status.port)
                    .cancel_operation(operation_id)
            } else {
                registry.cancel_supervised(operation_id, self)
            }
        });
        match cancellation {
            Ok(operation) => {
                ApiResponse::json(200, json!({"operation": operation_response(operation)}))
            }
            Err(error) => error_response(error),
        }
    }
}
