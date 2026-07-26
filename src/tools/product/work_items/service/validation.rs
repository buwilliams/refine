pub(super) fn validate_goal_operation(
    status: &GoalStatus,
    operation: &GoalOperation,
) -> RefineResult<()> {
    let decision = goal_operation_allowed(status, operation);
    if decision.allowed {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(
            decision
                .reason
                .unwrap_or_else(|| "operation is not allowed".to_string()),
        ))
    }
}

pub(super) fn validate_feature_operation(
    statuses: &[GoalStatus],
    operation: &FeatureOperation,
) -> RefineResult<()> {
    let decision = feature_operation_allowed(statuses, operation);
    if decision.allowed {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(decision.reason.unwrap_or_else(
            || "feature operation is not allowed".to_string(),
        )))
    }
}

pub(super) fn goal_json_path(refine_dir: &std::path::Path, goal_id: &str) -> PathBuf {
    let goal_id = goal_id.to_uppercase();
    refine_dir
        .join("goals")
        .join(&goal_id[..2])
        .join(&goal_id[2..])
        .join("goal.json")
}

pub(super) fn feature_json_path(refine_dir: &std::path::Path, feature_id: &str) -> PathBuf {
    let feature_id = feature_id.to_uppercase();
    refine_dir
        .join("features")
        .join(&feature_id[..2])
        .join(&feature_id[2..])
        .join("feature.json")
}
use super::*;
