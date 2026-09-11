//! Knowledge Hub adapters share the Application service with CLI and MCP.
use super::support::*;
use super::*;
use crate::application::hub::Hub;
use crate::error::{RefineError, RefineResult};
use base64::Engine as _;
use serde_json::{Value, json};
impl InProcessWebServer {
    pub(super) fn hub_service(&self) -> RefineResult<Hub> {
        Ok(Hub::new(
            self.current_refine_dir()?
                .ok_or_else(|| RefineError::InvalidInput("Attach a target app first".into()))?,
            self.runtime_root
                .clone()
                .ok_or_else(|| RefineError::InvalidInput("Runtime unavailable".into()))?,
        ))
    }
    pub(super) fn handle_hub(&self, request: ApiRequest) -> ApiResponse {
        let result = (|| -> RefineResult<Value> {
            let hub = self.hub_service()?;
            let parts: Vec<_> = request.path.trim_start_matches('/').split('/').collect();
            if matches!(request.method.as_str(), "PUT" | "POST" | "DELETE")
                && request.body.is_none()
            {
                return Err(RefineError::InvalidInput(
                    "Hub requests require a JSON object body".into(),
                ));
            }
            let body = request.body.unwrap_or(json!({}));
            if !body.is_object() {
                return Err(RefineError::InvalidInput(
                    "Hub request body must be a JSON object".into(),
                ));
            }
            let rev = body["revision"].as_str().unwrap_or("");
            let method = request.method.as_str();
            match parts.as_slice() {
                ["hub", "hosting"] if method == "GET" => {
                    Ok(json!({"public_prefix":"/hub/sites/", "preview_prefix":"/hub/preview/"}))
                }
                ["hub"] | ["hub", "sites"] if method == "GET" => hub.sites(),
                ["hub", "sites", s] => match method {
                    "GET" => hub.show(s),
                    "PUT" | "POST" => hub.save_site(s, &body),
                    "DELETE" => hub.delete_site(s, rev),
                    _ => Err(RefineError::InvalidInput("Unsupported site method".into())),
                },
                ["hub", "sites", s, "status"] if method == "GET" => hub.status(s),
                ["hub", "sites", s, "publish"] if method == "POST" => hub.publish(
                    s,
                    rev,
                    &serde_json::from_value::<Vec<String>>(
                        body.get("collections").cloned().unwrap_or(json!([])),
                    )
                    .map_err(|e| RefineError::InvalidInput(e.to_string()))?,
                    true,
                ),
                ["hub", "sites", s, "unpublish"] if method == "POST" => {
                    hub.publish(s, rev, &[], false)
                }
                ["hub", "sites", s, "assets"] => match method {
                    "GET" => hub.assets(s),
                    "PUT" | "DELETE" => {
                        let bytes = if let Some(text) = body["text"].as_str() {
                            text.as_bytes().to_vec()
                        } else if let Some(encoded) = body["bytes_base64"].as_str() {
                            if encoded.len() > 22 * 1024 * 1024 {
                                return Err(RefineError::InvalidInput(
                                    "Asset exceeds 16 MiB".into(),
                                ));
                            }
                            base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .map_err(|error| {
                                    RefineError::InvalidInput(format!(
                                        "Invalid asset encoding: {error}"
                                    ))
                                })?
                        } else {
                            serde_json::from_value(body.get("bytes").cloned().unwrap_or(json!([])))
                                .map_err(|e| RefineError::InvalidInput(e.to_string()))?
                        };
                        hub.save_asset(
                            s,
                            body["path"].as_str().unwrap_or(""),
                            &bytes,
                            body["revision"].as_str(),
                            method == "DELETE",
                        )
                    }
                    "POST" => {
                        let (bytes, hash) =
                            hub.asset(s, body["path"].as_str().unwrap_or(""), false)?;
                        Ok(
                            json!({"bytes_base64":base64::engine::general_purpose::STANDARD.encode(&bytes),"hash":hash}),
                        )
                    }
                    _ => Err(RefineError::InvalidInput("Unsupported asset method".into())),
                },
                ["hub", "sites", s, "collections"] if method == "GET" => hub.collections(s),
                ["hub", "sites", s, "collections", c] => match method {
                    "PUT" | "POST" => hub.save_collection(s, c, &body),
                    "DELETE" => hub.delete_collection(s, c, rev),
                    "GET" => Ok(super::hub_routes::collection_response(&hub, s, c)?),
                    _ => Err(RefineError::InvalidInput(
                        "Unsupported collection method".into(),
                    )),
                },
                ["hub", "sites", s, "collections", c, "query"] if method == "POST" => hub.query(
                    s,
                    c,
                    &serde_json::from_value(body)
                        .map_err(|e| RefineError::InvalidInput(e.to_string()))?,
                ),
                ["hub", "sites", s, "collections", c, "index"] if method == "POST" => {
                    hub.rebuild_index(s, c)
                }
                ["hub", "sites", s, "collections", c, "import"] if method == "POST" => hub.import(
                    s,
                    c,
                    body["records"].as_array().ok_or_else(|| {
                        RefineError::InvalidInput("records array required".into())
                    })?,
                ),
                ["hub", "sites", s, "collections", c, "records", r] => match method {
                    "GET" => hub.get(s, c, r),
                    "PUT" | "POST" => hub.put(s, c, r, &body),
                    "DELETE" => hub.delete(s, c, r, rev),
                    _ => Err(RefineError::InvalidInput(
                        "Unsupported record method".into(),
                    )),
                },
                _ => Err(RefineError::NotFound(
                    "Knowledge Hub route not found".into(),
                )),
            }
        })();
        match result {
            Ok(v) => ApiResponse::json(200, v),
            Err(e) => error_response(e),
        }
    }
}
fn collection_response(hub: &Hub, s: &str, c: &str) -> RefineResult<Value> {
    hub.collections(s)?["collections"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["item"]["id"] == c))
        .cloned()
        .ok_or_else(|| RefineError::NotFound("Collection not found".into()))
}
