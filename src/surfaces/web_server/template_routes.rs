use super::*;
use crate::application::templates::{TemplateScope, TemplateStore, TemplateValue, Variables};
use crate::error::{RefineError, RefineResult};
use serde_json::json;

impl InProcessWebServer {
    pub(crate) fn handle_templates(&self, request: ApiRequest) -> ApiResponse {
        let result = (|| -> RefineResult<serde_json::Value> {
            let root = self.current_refine_dir()?;
            let store = TemplateStore::new(root.as_deref());
            let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
            let body = request.body.unwrap_or_else(|| json!({}));
            match (request.method.as_str(), parts.as_slice()) {
                ("GET", ["templates"]) => store.list(),
                ("POST", ["templates", "reset"]) => {
                    let revisions = serde_json::from_value::<std::collections::BTreeMap<String, u64>>(body["revisions"].clone())
                        .map_err(|_| RefineError::InvalidInput("Template revisions are required".into()))?;
                    store.reset(&revisions)
                }
                ("GET", ["templates", "variables"]) => Ok(json!({"items":crate::application::templates::variables()})),
                ("GET", ["templates", id]) => store.show(id),
                ("PUT", ["templates", id]) => {
                    let revision = body["revision"].as_u64().ok_or_else(|| RefineError::InvalidInput("revision is required".into()))?;
                    let prompt = body["prompt"].as_str().ok_or_else(|| RefineError::InvalidInput("prompt is required".into()))?;
                    store.save(id, revision, prompt)
                }
                ("POST", ["templates", id, "preview"]) => {
                    let mut snapshot = store.snapshot()?;
                    let item = snapshot.records.get_mut(*id).ok_or_else(|| RefineError::NotFound(format!("Template {id}")))?;
                    if let Some(prompt) = body.get("prompt") {
                        item.prompt = prompt.as_str().ok_or_else(|| RefineError::InvalidInput("prompt must be text".into()))?.into();
                    }
                    crate::application::templates::validate_prompt(&item.prompt)?;
                    snapshot.validate_references()?;
                    let mut values = Variables::new();
                    if let Some(input) = body.get("values") {
                        for (key,value) in input.as_object().ok_or_else(|| RefineError::InvalidInput("values must be an object".into()))? {
                            let value = if let Some(template) = value.get("template").and_then(|v| v.as_str()) {
                                TemplateValue::Template(template.into())
                            } else if let Some(literal) = value.as_str() { TemplateValue::Literal(literal.into()) }
                            else { TemplateValue::Literal(serde_json::to_string_pretty(value).unwrap()) };
                            values.insert(key.clone(), value);
                        }
                    }
                    let scope = TemplateScope::enter(snapshot);
                    let prompt = TemplateScope::render(id, values)?;
                    drop(scope);
                    Ok(json!({"prompt":prompt,"bytes":prompt.len()}))
                }
                _ => Err(RefineError::InvalidInput("Templates are fixed system entries: only reading, editing, and previewing are supported".into())),
            }
        })();
        match result {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => super::support::error_response(error),
        }
    }
}
