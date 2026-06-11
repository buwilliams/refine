use serde_json::json;

use crate::process::supervisor::errors::RefineError;

use super::super::*;

pub(in crate::surfaces::web_server) fn error_response(error: RefineError) -> ApiResponse {
    let (status, code) = match &error {
        RefineError::InvalidInput(_) => (400, "invalid_input"),
        RefineError::NotFound(_) => (404, "not_found"),
        RefineError::Unauthorized(_) => (401, "unauthorized"),
        RefineError::Conflict(_) => (409, "conflict"),
        RefineError::Degraded(_) => (503, "degraded"),
        RefineError::Io(_) | RefineError::Serialization(_) => (500, "storage_error"),
        RefineError::NotImplemented(_) => (501, "not_implemented"),
    };
    ApiResponse::json(
        status,
        json!({
            "error": {
                "code": code,
                "message": error.to_string()
            }
        }),
    )
}
