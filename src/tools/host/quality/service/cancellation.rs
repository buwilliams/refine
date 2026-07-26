use super::*;

pub(super) fn required_operation_request_string(
    operation: &OperationHandle,
    field: &str,
) -> RefineResult<String> {
    operation
        .request
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            RefineError::Serialization(format!(
                "Quality operation {} has no valid {field} for cancellation recovery",
                operation.id
            ))
        })
}

pub(super) fn cancellation_requested(operation: &OperationHandle) -> bool {
    operation
        .request
        .get("cancellation_requested")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(super) fn recoverable_quality_cancellation(operation: &OperationHandle) -> bool {
    matches!(operation.state, OperationState::Cancelling)
        && operation
            .request
            .get("defer_cancellation_terminal")
            .and_then(Value::as_bool)
            == Some(true)
        && cancellation_requested(operation)
        && operation.owner.starts_with("quality:")
}
