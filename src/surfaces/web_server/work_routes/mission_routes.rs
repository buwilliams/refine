use serde_json::json;

use crate::application::missions::FileMissionService;
use crate::application::missions::runner::MissionWorkflowEngine;
use crate::application::projects::projection::{
    MissionProjectionQuery, PageRequest, ProjectionQuery,
};
use crate::application::work_items::FileWorkItemService;
use crate::model::mission::{GoalContribution, MissionPlan, MissionStatus};

use super::{
    ApiRequest, ApiResponse, InProcessWebServer, Value, bounded_query_usize, error_response,
    query_param, runtime_root_unavailable, target_root_unavailable,
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
        let body = request.body.as_ref();
        let plan_digest = body
            .and_then(|body| body.get("plan_digest"))
            .and_then(Value::as_str)
            .map(str::to_string);
        // An explicit plan body records (or re-records) the draft first, so
        // the approval binds the exact content that was just submitted.
        if let Some(plan_value) = body.and_then(|body| body.get("plan")) {
            let plan = match serde_json::from_value::<MissionPlan>(plan_value.clone()) {
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
            let service = self.mission_service(&refine_dir);
            // The record is a draft submission; the consequential fence is
            // the approval itself, which revalidates the observed revision.
            if let Err(error) = service.record_plan(mission_id, plan, None) {
                return error_response(error);
            }
        }
        let Some(plan_digest) = plan_digest else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_plan_digest",
                        "message": "body.plan_digest is required"
                    }
                }),
            );
        };
        let actor = body
            .and_then(|body| body.get("actor"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let rationale = body
            .and_then(|body| body.get("rationale"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).approve_plan(
            mission_id,
            &plan_digest,
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

    pub(crate) fn handle_mission_advance(&self, request: ApiRequest) -> ApiResponse {
        let Some(target_root) = self.target_root.clone() else {
            return target_root_unavailable("advance Missions");
        };
        let Some(runtime_root) = self.runtime_root.clone() else {
            return runtime_root_unavailable("advance Missions");
        };
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/advance") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission advance route requires a Mission id"
                    }
                }),
            );
        };
        let refine_dir = require_refine_dir!(self, "advance Missions");
        let service = self.mission_service(&refine_dir);
        let engine = MissionWorkflowEngine::new(&runtime_root, &target_root);
        match engine.evaluate_one(&service, mission_id) {
            Ok(Some(detail)) => {
                let mission = match service.show_mission(mission_id) {
                    Ok(mission) => mission,
                    Err(error) => return error_response(error),
                };
                ApiResponse::json(
                    200,
                    json!({"mission": mission, "advanced": true, "detail": detail}),
                )
            }
            Ok(None) => {
                let mission = match service.show_mission(mission_id) {
                    Ok(mission) => mission,
                    Err(error) => return error_response(error),
                };
                ApiResponse::json(200, json!({"mission": mission, "advanced": false}))
            }
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_goal_mission_contribution(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "settle Mission contributions");
        let Some(goal_id) = request
            .path
            .strip_prefix("/work/goals/")
            .and_then(|rest| rest.strip_suffix("/mission-contribution"))
            .filter(|id| !id.is_empty() && !id.contains('/'))
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission contribution route requires a Goal id"
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
                        "message": "a contribution body is required"
                    }
                }),
            );
        };
        let contribution = match serde_json::from_value::<GoalContribution>(
            body.get("contribution").cloned().unwrap_or_default(),
        ) {
            Ok(contribution) => contribution,
            Err(error) => {
                return ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_contribution",
                            "message": format!("body.contribution is invalid: {error}")
                        }
                    }),
                );
            }
        };
        let work_items = FileWorkItemService::new(&refine_dir);
        match work_items.settle_goal_mission_contribution(goal_id, contribution) {
            Ok(goal) => ApiResponse::json(200, json!({"goal": goal})),
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

impl InProcessWebServer {
    /// Parse `/work/missions/<id>/<suffix...>` into (mission_id, rest).
    fn mission_path_segments<'a>(&self, path: &'a str) -> Option<(&'a str, &'a str)> {
        let rest = path.strip_prefix("/work/missions/")?;
        if rest.is_empty() {
            return None;
        }
        let (mission_id, remainder) = match rest.split_once('/') {
            Some((mission_id, remainder)) => (mission_id, remainder),
            None => (rest, ""),
        };
        if mission_id.is_empty() || mission_id.contains('/') {
            return None;
        }
        Some((mission_id, remainder))
    }

    pub(crate) fn handle_mission_decision(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "answer Mission decisions");
        let Some((mission_id, remainder)) = self.mission_path_segments(&request.path) else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission decision route requires a Mission id"
                    }
                }),
            );
        };
        let Some(decision_id) = remainder
            .strip_prefix("decisions/")
            .filter(|id| !id.is_empty() && !id.contains('/'))
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission decision route requires a decision id"
                    }
                }),
            );
        };
        let body = request.body.as_ref();
        let Some(choice) = body
            .and_then(|body| body.get("choice"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|choice| !choice.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_choice",
                        "message": "body.choice is required"
                    }
                }),
            );
        };
        let rationale = body
            .and_then(|body| body.get("rationale"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let actor = body
            .and_then(|body| body.get("actor"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).answer_decision(
            mission_id,
            decision_id,
            &choice,
            rationale,
            actor,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_retry(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "retry Mission stages");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/retry") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission retry route requires a Mission id"
                    }
                }),
            );
        };
        let stage = request
            .body
            .as_ref()
            .and_then(|body| body.get("stage"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if stage.is_empty() {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_stage",
                        "message": "body.stage is required"
                    }
                }),
            );
        }
        let observed_revision = self.observed_revision(&request);
        match self
            .mission_service(refine_dir)
            .retry_stage(mission_id, stage, observed_revision)
        {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_transfer(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "transfer Missions");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/transfer") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission transfer route requires a Mission id"
                    }
                }),
            );
        };
        let Some(node_id) = request
            .body
            .as_ref()
            .and_then(|body| body.get("node_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|node_id| !node_id.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_node",
                        "message": "body.node_id is required"
                    }
                }),
            );
        };
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).transfer_mission(
            mission_id,
            &node_id,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_add_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "adopt Goals into Mission plans");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/goals") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission goal adoption route requires a Mission id"
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
                        "message": "an adoption body is required"
                    }
                }),
            );
        };
        let Some(goal_id) = body
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|goal_id| !goal_id.trim().is_empty())
        else {
            return ApiResponse::json(
                400,
                json!({
                    "error": {
                        "code": "invalid_goal",
                        "message": "body.goal_id is required"
                    }
                }),
            );
        };
        let wave = body.get("wave").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
        let role = body
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|role| !role.trim().is_empty());
        let required = body
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let criterion_ids: Vec<String> = body
            .get("criterion_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).add_plan_goal(
            mission_id,
            &goal_id,
            wave,
            role.as_deref(),
            required,
            &criterion_ids,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_remove_goal(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "remove Goals from Mission plans");
        let Some((mission_id, goal_id)) =
            self.mission_path_segments(&request.path)
                .and_then(|(mission_id, remainder)| {
                    remainder
                        .strip_prefix("goals/")
                        .filter(|goal_id| !goal_id.is_empty() && !goal_id.contains('/'))
                        .map(|goal_id| (mission_id, goal_id))
                })
        else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission goal removal route requires a Mission id and Goal id"
                    }
                }),
            );
        };
        let observed_revision = self.observed_revision(&request);
        match self.mission_service(refine_dir).remove_plan_goal(
            mission_id,
            goal_id,
            observed_revision,
        ) {
            Ok(mission) => ApiResponse::json(200, json!({"mission": mission})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_context(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "read Mission context");
        let Some(mission_id) = self.mission_id_from_path(&request.path, "/context") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission context route requires a Mission id"
                    }
                }),
            );
        };
        match self
            .mission_service(refine_dir)
            .mission_context_summary(mission_id)
        {
            Ok(context) => ApiResponse::json(200, json!({"context": context})),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_mission_distribution(&self, raw_path: &str) -> ApiResponse {
        let path = raw_path.split('?').next().unwrap_or(raw_path);
        let Some(mission_id) = self.mission_id_from_path(path, "/distribution") else {
            return ApiResponse::json(
                404,
                json!({
                    "error": {
                        "code": "not_found",
                        "message": "Mission distribution route requires a Mission id"
                    }
                }),
            );
        };
        let Some(target_root) = self.target_root.clone() else {
            return target_root_unavailable("preview Mission distribution");
        };
        let refine_dir = require_refine_dir!(self, "preview Mission distribution");
        let wave = query_param(raw_path, "wave")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let mission_service = self.mission_service(&refine_dir);
        let work_items = FileWorkItemService::new(&refine_dir);
        match crate::application::missions::phases::distribution::preview_wave_distribution(
            &mission_service,
            &work_items,
            mission_id,
            wave,
        ) {
            Ok(distribution) => {
                let _ = target_root;
                ApiResponse::json(200, json!({"distribution": distribution}))
            }
            Err(error) => error_response(error),
        }
    }
}
