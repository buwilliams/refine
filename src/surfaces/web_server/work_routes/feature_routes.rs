mod authoring;
mod bulk;
mod lifecycle;
mod membership;
mod queries;
use super::{
    ApiRequest, ApiResponse, BulkFeatureSelection, BulkFeatureUpdate, BulkGoalSelection,
    FeatureGoalAuthoringRequest, FeatureProjectionQuery, FileWorkItemService, GoalStatus,
    InProcessWebServer, PageRequest, ProjectionQuery, RefineError, Value, bounded_query_usize,
    error_response, feature_detail_response_from_goals, feature_id_required,
    feature_reorder_order_from_body, goal_id_required, invalid_bulk_body, json, query_param,
    target_root_unavailable,
};

impl InProcessWebServer {}
