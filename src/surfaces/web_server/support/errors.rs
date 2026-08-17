use serde_json::json;

use crate::process::supervisor::errors::RefineError;

use super::super::*;

pub(in crate::surfaces::web_server) fn error_response(error: RefineError) -> ApiResponse {
    let (status, code) = match &error {
        RefineError::InvalidInput(_) => (400, "invalid_input"),
        RefineError::NotFound(_) => (404, "not_found"),
        RefineError::Unauthorized(_) => (401, "unauthorized"),
        RefineError::Conflict(_)
        | RefineError::StateSyncMissingBaseline(_)
        | RefineError::StateRecoveryConflict { .. } => (409, "conflict"),
        RefineError::MergeConflict { .. } => (409, "merge_conflict"),
        RefineError::StaleCandidate { .. } => (409, "stale_candidate"),
        RefineError::TargetAdvanced { .. } => (409, "target_advanced"),
        RefineError::QualityCandidateInfrastructure(_) => (409, "quality_candidate_infrastructure"),
        RefineError::Degraded(_) => (503, "degraded"),
        RefineError::Io(_) | RefineError::Serialization(_) | RefineError::StructuredOutput(_) => {
            (500, "storage_error")
        }
        RefineError::NotImplemented(_) => (501, "not_implemented"),
    };
    let reason = match &error {
        RefineError::StateRecoveryConflict { reason, .. } => Some(reason.as_str()),
        _ => None,
    };
    let mut error_body = json!({
        "code": code,
        "message": error.to_string()
    });
    if let (Some(reason), Some(fields)) = (reason, error_body.as_object_mut()) {
        fields.insert("reason".to_string(), json!(reason));
    }
    ApiResponse::json(status, json!({"error": error_body}))
}
