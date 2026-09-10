use super::*;
use crate::application::events::FileEventService;

impl InProcessWebServer {
    pub(crate) fn handle_event_capability(
        &self,
        request: ApiRequest,
        raw_path: &str,
    ) -> ApiResponse {
        let refine_dir = require_refine_dir!(self, "configure Events and Skills");
        let service = match &self.runtime_root {
            Some(runtime) => FileEventService::with_runtime_root(&refine_dir, runtime),
            None => FileEventService::new(&refine_dir),
        };
        let result = (|| -> RefineResult<Value> {
            let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
            let body = request.body.clone().unwrap_or_else(|| json!({}));
            let query: BTreeMap<String, String> = ["offset", "limit", "node_id"]
                .into_iter()
                .filter_map(|key| query_param(raw_path, key).map(|v| (key.into(), v)))
                .collect();
            if parts.first() == Some(&"event-invocations") {
                return match (request.method.as_str(), parts.as_slice()) {
                    ("GET", [_]) => {
                        let offset = query
                            .get("offset")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        let limit = query
                            .get("limit")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(30);
                        if let Some(goal_id) = query_param(raw_path, "goal_id") {
                            service.goal_invocations(&goal_id, offset, limit)
                        } else {
                            service.invocations(offset, limit)
                        }
                    }
                    ("GET", [_, id]) => Ok(json!(service.invocation(id)?)),
                    ("POST", [_, id, "cancel"]) => Ok(json!(service.cancel_invocation(id)?)),
                    _ => Err(RefineError::NotFound("Event invocation route".into())),
                };
            }
            let collection = if parts.first() == Some(&"skills") {
                "skills"
            } else {
                "events"
            };
            match (request.method.as_str(), parts.as_slice()) {
                ("GET", ["event-definitions", "catalog"]) => Ok(service.catalog()),
                ("GET", ["skills", "catalog"]) => Ok(service.skill_catalog()),
                ("GET", ["skills", id]) => service.show_skill(id),
                ("GET", ["skills", id, "inputs"]) => {
                    let target = self
                        .target_root()
                        .ok_or_else(|| RefineError::InvalidInput("select a project".into()))?;
                    service.skill_inputs(id, &target)
                }
                ("POST", ["skills", id, "trigger"]) => {
                    let target = self
                        .target_root()
                        .ok_or_else(|| RefineError::InvalidInput("select a project".into()))?;
                    let invocation = service.trigger_skill(id, &target, &body)?;
                    let _ = service.dispatch_pending(&target);
                    Ok(json!(invocation))
                }
                ("GET", ["event-definitions", id, "inputs"]) => {
                    let target = self
                        .target_root()
                        .ok_or_else(|| RefineError::InvalidInput("select a project".into()))?;
                    let context = service.manual_context(
                        &target,
                        &json!({"goal_id": query_param(raw_path, "goal_id")}),
                    )?;
                    let config = service.config()?;
                    let event = config
                        .events
                        .get(*id)
                        .ok_or_else(|| RefineError::NotFound(format!("Event {id}")))?;
                    Ok(
                        json!({"parameters": service.launch_parameters(&config, event, &context)?, "revision": config.revision}),
                    )
                }
                ("GET", [_]) => service.list(collection, query.get("node_id").map(String::as_str)),
                ("GET", [_, id]) => {
                    let config = service.config()?;
                    let item = if collection == "skills" {
                        config.skills.get(*id).map(|s| json!(s))
                    } else {
                        config.events.get(*id).map(|e| json!(e))
                    }
                    .ok_or_else(|| RefineError::NotFound(format!("{collection} {id}")))?;
                    Ok(json!({"revision": config.revision, "item": item}))
                }
                ("PUT" | "PATCH", [_, id]) => service.save(collection, id, body),
                ("DELETE", [_, id]) => service.remove(
                    collection,
                    id,
                    body.get("revision")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| {
                            RefineError::InvalidInput("observed revision is required".into())
                        })?,
                ),
                ("POST", ["event-definitions", id, "trigger"]) => {
                    let target = self.target_root().ok_or_else(|| {
                        RefineError::InvalidInput(
                            "select a project before triggering an Event".into(),
                        )
                    })?;
                    let invocation = service.trigger(id, &target, &body)?;
                    let _ = service.dispatch_pending(&target);
                    Ok(json!(invocation))
                }
                _ => Err(RefineError::NotFound("Events/Skills route".into())),
            }
        })();
        match result {
            Ok(value) => ApiResponse::json(
                if request.path.ends_with("/trigger") {
                    202
                } else {
                    200
                },
                value,
            ),
            Err(error) => error_response(error),
        }
    }
}
