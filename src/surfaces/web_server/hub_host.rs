//! Static Hub sites are served by the existing Refine HTTP server under /hub/.
use super::*;
use crate::error::{RefineError, RefineResult};
impl LocalHttpDaemon {
    pub(super) fn hub_wire(&self, request: &HttpRequest) -> WireResponse {
        let result = (|| -> RefineResult<WireResponse> {
            let path = request.path.split('?').next().unwrap_or("");
            let (relative, public) = if let Some(relative) = path.strip_prefix("/hub/preview/") {
                (relative, false)
            } else if let Some(relative) = path.strip_prefix("/hub/sites/") {
                (relative, true)
            } else {
                return Err(RefineError::NotFound(
                    "Knowledge Hub site route not found".into(),
                ));
            };
            if !path.ends_with('/')
                && !relative.contains('/')
                && matches!(request.method.as_str(), "GET" | "HEAD")
            {
                let mut response = WireResponse::bytes(308, "text/plain", vec![]);
                response
                    .extra_headers
                    .push(("Location".into(), format!("{path}/")));
                return Ok(response);
            }
            let parts: Vec<_> = relative.split('/').collect();
            let hub = self.server.hub_service()?;
            if let [site, "api", collection, "query"] = parts.as_slice() {
                if request.method != "POST" {
                    return Err(RefineError::InvalidInput("Queries require POST".into()));
                }
                let query = serde_json::from_slice(request.body.as_deref().unwrap_or(b"{}"))
                    .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
                return Ok(WireResponse::json(ApiResponse::json(
                    200,
                    if public {
                        hub.public_query(site, collection, &query)?
                    } else {
                        hub.query(site, collection, &query)?
                    },
                )));
            }
            if request.method != "GET" && request.method != "HEAD" {
                return Err(RefineError::InvalidInput(
                    "Hosted sites are read-only".into(),
                ));
            }
            let site = parts.first().copied().unwrap_or("");
            let name = super::support::percent_decode(
                &parts.get(1..).unwrap_or(&[]).join("/").replace("+", "%2B"),
            );
            let name = if name.is_empty() || name.ends_with('/') {
                format!("{name}index.html")
            } else {
                name
            };
            let (bytes, hash) = hub.asset(site, &name, public)?;
            let mime = match name.rsplit('.').next().unwrap_or("") {
                "html" => "text/html; charset=utf-8",
                "css" => "text/css; charset=utf-8",
                "js" | "mjs" => "text/javascript; charset=utf-8",
                "json" => "application/json",
                "svg" => "image/svg+xml",
                "png" => "image/png",
                "webp" => "image/webp",
                "gif" => "image/gif",
                "ico" => "image/x-icon",
                "jpg" | "jpeg" => "image/jpeg",
                "pdf" => "application/pdf",
                "txt" => "text/plain; charset=utf-8",
                "csv" => "text/csv; charset=utf-8",
                "wasm" => "application/wasm",
                "woff" => "font/woff",
                "woff2" => "font/woff2",
                "ttf" => "font/ttf",
                _ => "application/octet-stream",
            };
            let etag = format!("\"{hash}\"");
            let unchanged = request.headers.get("if-none-match") == Some(&etag);
            let mut response = WireResponse::bytes(
                if unchanged { 304 } else { 200 },
                mime,
                if unchanged || request.method == "HEAD" {
                    vec![]
                } else {
                    bytes
                },
            );
            response.extra_headers = vec![
                ("ETag".into(), etag),
                ("Cache-Control".into(), "no-cache".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
                ("Referrer-Policy".into(), "no-referrer".into()),
            ];
            Ok(response)
        })();
        match result {
            Ok(r) => r,
            Err(e) => WireResponse::json(super::support::error_response(e)),
        }
    }
}
