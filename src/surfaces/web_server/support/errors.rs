use serde_json::json;

use crate::error::RefineError;

use super::super::*;

pub(in crate::surfaces::web_server) fn error_response(error: RefineError) -> ApiResponse {
    let (status, code) = match &error {
        RefineError::InvalidInput(_) => (400, "invalid_input"),
        RefineError::NotFound(_) => (404, "not_found"),
        RefineError::Unauthorized(_) => (401, "unauthorized"),
        RefineError::Conflict(_) | RefineError::StateRecoveryConflict { .. } => (409, "conflict"),
        RefineError::MergeConflict { .. } => (409, "merge_conflict"),
        RefineError::StaleCandidate { .. } => (409, "stale_candidate"),
        RefineError::TargetAdvanced { .. } => (409, "target_advanced"),
        RefineError::QualityCandidateInfrastructure(_) => (409, "quality_candidate_infrastructure"),
        RefineError::Degraded(_) => (503, "degraded"),
        RefineError::UnsupportedGitVersion { .. } => (503, "unsupported_git_version"),
        RefineError::Io(_) | RefineError::Serialization(_) | RefineError::StructuredOutput(_) => {
            (500, "storage_error")
        }
        RefineError::NotImplemented(_) => (501, "not_implemented"),
    };
    // A stable `error.reason` is what lets a caller act on the condition
    // rather than parse prose — the fleet fan-out reads this one to report a
    // peer's Git as that peer's own status.
    let reason = match &error {
        RefineError::StateRecoveryConflict { reason, .. } => Some(reason.as_str()),
        RefineError::UnsupportedGitVersion { .. } => Some("unsupported_git_version"),
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
