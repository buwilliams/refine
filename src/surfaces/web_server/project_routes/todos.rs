use super::*;

impl InProcessWebServer {
    pub(crate) fn handle_todos_list(&self, raw_path: &str) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "list todos");
        let reporter = query_param(raw_path, "reporter").unwrap_or_default();
        match FileTodoService::new(refine_dir).list(&reporter) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_todo_list_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "create todo lists");
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileTodoService::new(refine_dir)
            .create_list(body_string(&body, "reporter"), body_string(&body, "name"))
        {
            Ok(value) => ApiResponse::json(201, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_todo_list_rename(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "rename todo lists");
        let Some(list_id) = todo_list_id_from_path(&request.path) else {
            return todo_route_not_found();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileTodoService::new(refine_dir).rename_list(
            body_string(&body, "reporter"),
            list_id,
            body_string(&body, "name"),
        ) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_todo_list_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete todo lists");
        let Some(list_id) = todo_list_id_from_path(&request.path) else {
            return todo_route_not_found();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileTodoService::new(refine_dir).delete_list(body_string(&body, "reporter"), list_id)
        {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_todo_item_create(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "add todo items");
        let Some(list_id) = todo_item_collection_list_id(&request.path) else {
            return todo_route_not_found();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileTodoService::new(refine_dir).add_item(
            body_string(&body, "reporter"),
            list_id,
            body_string(&body, "text"),
        ) {
            Ok(value) => ApiResponse::json(201, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_todo_item_update(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "update todo items");
        let Some((list_id, item_id)) = todo_item_ids_from_path(&request.path) else {
            return todo_route_not_found();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let text = body.get("text").and_then(Value::as_str);
        let done = body.get("done").and_then(Value::as_bool);
        match FileTodoService::new(refine_dir).update_item(
            body_string(&body, "reporter"),
            list_id,
            item_id,
            text,
            done,
        ) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_todo_item_delete(&self, request: ApiRequest) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "delete todo items");
        let Some((list_id, item_id)) = todo_item_ids_from_path(&request.path) else {
            return todo_route_not_found();
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        match FileTodoService::new(refine_dir).delete_item(
            body_string(&body, "reporter"),
            list_id,
            item_id,
        ) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }
}
