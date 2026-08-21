use serde_json::json;

use crate::application::missions::FileMissionService;
use crate::application::projects::projection::{
    MissionProjectionQuery, PageRequest, ProjectionQuery,
};
use crate::model::mission::{MissionPlan, MissionStatus};

use super::{
    ApiRequest, ApiResponse, InProcessWebServer, Value, bounded_query_usize, error_response,
    query_param, target_root_unavailable,
};

impl InProcessWebServer {
    fn mission_service(&self, refine_dir: impl Into<std::path::PathBuf>) -> FileMissionService {
        FileMissionService::new(refine_dir)
    }

    fn mission_id_from_path<'a>(&self, path: &'a str, suffix: &str) -> Option<&'a str> {
        path.strip_prefix("/work/missions/")
            .and_then(|rest| rest.strip_suffix(suffix))
            .filter(|id| !id.is_empty() && !id.contains('/'))
    }

    fn observed_revision(&self, request: &ApiRequest) -> Option<u64> {
        request
            .body
            .as_ref()
            .and_then(|body| body.get("observed_revision"))
            .and_then(Value::as_u64)
    }

    pub(crate) fn handle_missions_list(&self, raw_path: &str) -> ApiResponse {
        let projection = match self.current_projection_shared() {
            Ok(projection) => projection,
            Err(error) => return error_response(error),
        };
        let limit = bounded_query_usize(raw_path, "limit", 50, 1000);
        let page = bounded_query_usize(raw_path, "page", 1, usize::MAX).max(1);
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| (page - 1).saturating_mul(limit));
        let query = MissionProjectionQuery {
            page: PageRequest {
                limit,
                offset,
                sort: query_param(raw_path, "sort").unwrap_or_else(|| "updated".to_string()),
                dir: query_param(raw_path, "dir").unwrap_or_else(|| "desc".to_string()),
            },
            q: query_param(raw_path, "q"),
            status: query_param(raw_path, "status")
                .and_then(|value| MissionStatus::parse_wire(&value)),
            reporter: query_param(raw_path, "reporter"),
            assignee: query_param(raw_path, "assignee"),
            coordinator: query_param(raw_path, "coordinator"),
            outcome: query_param(raw_path, "outcome").and_then(|value| match value.as_str() {
                "published" | "true" | "1" => Some(true),
                "unpublished" | "false" | "0" => Some(false),
                _ => None,
            }),
        };
        let result = projection.list_missions(query);
        let body = json!({
            "missions": result.missions,
            "matching_ids": result.matching_ids,
            "projection_version": projection.version,
            "page": {
                "limit": limit,
                "offset": offset,
                "page": page,
                "total": result.total,
                "has_more": offset + limit < result.total
            }
        });
        ApiResponse::json(200, body)
    }

    pub(crate) fn handle_mission_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create Missions");
        let body = request.body.as_ref();
        let field = |name| {
            body.and_then(|body| body.get(name))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let Some(name) = field("name") else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_name",
                        "message": "body.name is required"
                    }
                }),
            );
        };
        let Some(intent) = field("intent") else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_intent",
                        "message": "body.intent is required"
                    }
                }),
            );
        };
        let service = self.mission_service(refine_dir);
        match service.create_mission(
            &name,
            &intent,
            field("reporter").as_deref(),
            field("coordinator_node_id").as_deref(),
            field("id").as_deref(),
        ) {
            Ok(mission) => ApiResponse::json(201, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_show(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read Mission detail");
        let Some(mission_id) = request
            .path
            .strip_prefix("/work/missions/")
            .filter(|id| !id.is_empty() && !id.contains('/'))
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission route requires a Mission id"
                    }
                }),
            );
        };
        let service = self.mission_service(refine_dir);
        match service.show_mission(mission_id) {
            Ok(mission) => {
                let projection = match self.current_projection_shared() {
                    Ok(projection) => projection,
                    Err(error) => return error_response(error),
                };
                let goals: Vec<_> = projection
                    .goals
                    .values()
                    .filter(|goal| {
                        goal.goal
                            .mission
                            .as_ref()
                            .is_some_and(|binding| binding.mission_id == mission_id)
                    })
                    .map(|goal| goal.goal.clone())
                    .collect();
                let rollup = service.mission_rollup(mission_id, &goals);
                ApiResponse::json(
                    200,
                    json!({
                        "mission": mission,
                        "goals": goals,
                        "rollup": rollup
                    }),
                )
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update Missions");
        let Some(mission_id) = request
            .path
            .strip_prefix("/work/missions/")
            .filter(|id| !id.is_empty() && !id.contains('/'))
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission route requires a Mission id"
                    }
                }),
            );
        };
        let body = request.body.as_ref();
        let name = body
            .and_then(|body| body.get("name"))
            .and_then(Value::as_str);
        let intent = body
            .and_then(|body| body.get("intent"))
            .and_then(Value::as_str);
        let success_criteria = body.and_then(|body| body.get("success_criteria"));
        let artifact_contract = body.and_then(|body| body.get("artifact_contract"));
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).edit_mission_frame(
            mission_id,
            name,
            intent,
            success_criteria,
            artifact_contract,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_round(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "append Mission Rounds");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/rounds") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission round route requires a Mission id"
                    }
                }),
            );
        };
        let body = request.body.as_ref();
        let reporter = body
            .and_then(|body| body.get("reporter"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let prompt = body
            .and_then(|body| body.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if prompt.is_empty() {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_prompt",
                        "message": "body.prompt is required"
                    }
                }),
            );
        }
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).append_round(
            mission_id,
            reporter,
            prompt,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_start(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "start Missions");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/start") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission start route requires a Mission id"
                    }
                }),
            );
        };
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).transition_mission(
            mission_id,
            MissionStatus::Investigate,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_approve_plan(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "approve Mission plans");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/approve-plan") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission approve-plan route requires a Mission id"
                    }
                }),
            );
        };
        let Some(body) = request.body.as_ref() else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_body",
                        "message": "a plan body is required"
                    }
                }),
            );
        };
        let plan = match serde_json::from_value::<MissionPlan>(
            body.get("plan").cloned().unwrap_or_default(),
        ) {
            Ok(plan) => plan,
            Err(error) => {
                return ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_plan",
                            "message": format!("body.plan is invalid: {error}")
                        }
                    }),
                );
            }
        };
        let actor = body.get("actor").and_then(Value::as_str).unwrap_or("");
        let rationale = body.get("rationale").and_then(Value::as_str).unwrap_or("");
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).approve_plan(
            mission_id,
            plan,
            actor,
            rationale,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_approve_outcome(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "approve Mission outcomes");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/approve-outcome") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission approve-outcome route requires a Mission id"
                    }
                }),
            );
        };
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).transition_mission(
            mission_id,
            MissionStatus::Consolidate,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_cancel(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "cancel Missions");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/cancel") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission cancel route requires a Mission id"
                    }
                }),
            );
        };
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).transition_mission(
            mission_id,
            MissionStatus::Cancelled,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_outcome(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read Mission outcome");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/outcome") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission outcome route requires a Mission id"
                    }
                }),
            );
        };
        match self.mission_service(refine_dir).show_mission(mission_id) {
            Ok(mission) => {
                let outcome = mission
                    .rounds
                    .iter()
                    .rev()
                    .find_map(|round| round.outcome.clone());
                match outcome {
                    Some(outcome) => ApiResponse::json(200, json!({"outcome": outcome})),
                    None => ApiResponse::json(
                        404,
                        json!({
                            "error": {
                                "code": "not_found",
                                "message": format!("Mission {mission_id} has no published Outcome")
                            }
                        }),
                    ),
                }
            }
            Err(error) => error_response(error),
        }
    }
}
