use super::*;

impl LocalHttpDaemon {
    pub(super) fn serve_listener(
        &self,
        listener: TcpListener,
        shutdown: Option<AxumShutdown>,
    ) -> RefineResult<()> {
        let daemon = self.clone();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| RefineError::Io(format!("failed to start Tokio runtime: {error}")))?;
        runtime.block_on(async move {
            listener.set_nonblocking(true).map_err(|error| {
                RefineError::Io(format!("failed to configure daemon listener: {error}"))
            })?;
            let listener = tokio::net::TcpListener::from_std(listener).map_err(|error| {
                RefineError::Io(format!("failed to hand daemon listener to Tokio: {error}"))
            })?;
            let app = daemon.axum_router(
                shutdown
                    .as_ref()
                    .and_then(|shutdown| shutdown.complete_after_request.as_ref().map(Arc::clone)),
            );
            let server = axum::serve(listener, app);
            match shutdown {
                Some(shutdown) => server
                    .with_graceful_shutdown(shutdown.wait())
                    .await
                    .map_err(|error| {
                        RefineError::Io(format!("Axum daemon server failed: {error}"))
                    }),
                None => server.await.map_err(|error| {
                    RefineError::Io(format!("Axum daemon server failed: {error}"))
                }),
            }
        })
    }

    pub(super) fn axum_router(&self, complete_after_request: Option<Arc<Notify>>) -> Router {
        Router::new()
            .fallback(any(axum_entrypoint))
            .with_state(AxumDaemonState {
                daemon: self.clone(),
                complete_after_request,
            })
    }

    pub(super) fn handle_axum_request(&self, request: HttpRequest) -> Response {
        if request.method == "GET" && (request.path == "/events" || request.path == "/api/sse") {
            return self.sse_response("events");
        }
        if request.method == "GET"
            && let Some(session_id) = terminal_events_route(&request.path)
            && !terminal_events_snapshot_requested(&request.path)
        {
            let after = query_param(&request.path, "after")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            return self.terminal_sse_response(session_id, after);
        }
        self.handle_wire_request(request).into_response()
    }

    pub fn handle_wire_request(&self, request: HttpRequest) -> WireResponse {
        let started = Instant::now();
        if request.path == "/events" || request.path == "/api/sse" {
            return match self.server_sent_events("events") {
                Ok(events) => WireResponse::sse(events),
                Err(error) => WireResponse::json(error_response(error)),
            };
        }
        if request.method == "GET" && terminal_events_route(&request.path).is_some() {
            return WireResponse::json(self.server.handle_terminal_events_snapshot(&request.path));
        }

        if request.method != "GET" {
            if !mutation_origin_allowed(&request) {
                return WireResponse::json(ApiResponse::json(
                    403,
                    json!({
                        "error": {
                            "code": "forbidden_origin",
                            "message": "mutation request origin must match the request host"
                        }
                    }),
                ));
            }
            if let Some(version) = request.headers.get("x-refine-api-version")
                && version != API_CONTRACT_VERSION
            {
                return WireResponse::json(ApiResponse::json(
                    426,
                    json!({
                        "error": {
                            "code": "api_version_mismatch",
                            "message": "unsupported Refine API contract version"
                        },
                        "api_contract_version": API_CONTRACT_VERSION,
                        "supported_api_contract_versions": [API_CONTRACT_VERSION]
                    }),
                ));
            }
            if let Some(key) = request.headers.get("idempotency-key")
                && !valid_idempotency_key(key)
            {
                return WireResponse::json(ApiResponse::json(
                    400,
                    json!({
                        "error": {
                            "code": "invalid_idempotency_key",
                            "message": "idempotency key must be 1-128 ASCII letters, digits, '.', '_', '-', or ':'"
                        }
                    }),
                ));
            }
        }

        if request.method == "GET"
            && let Some(response) = self.try_static_response(&request.path)
        {
            return self.with_request_metric(&request, "static", started, response);
        }

        let idempotency_key = request.headers.get("idempotency-key").cloned();
        let fingerprint = idempotency_key.as_ref().map(|_| {
            idempotency_fingerprint(
                &request.method,
                &normalize_api_path(&request.path),
                request.body.as_deref().unwrap_or(&[]),
            )
        });
        if let (Some(runtime_root), Some(key), Some(fingerprint)) = (
            self.server.runtime_root.as_ref(),
            idempotency_key.as_deref(),
            fingerprint.as_deref(),
        ) {
            match load_idempotency_record(runtime_root, key) {
                Ok(Some(record)) if record.fingerprint == fingerprint => {
                    return self.with_request_metric(
                        &request,
                        "idempotency",
                        started,
                        WireResponse::json(record.response),
                    );
                }
                Ok(Some(_)) => {
                    return WireResponse::json(ApiResponse::json(
                        409,
                        json!({
                            "error": {
                                "code": "idempotency_conflict",
                                "message": "idempotency key was already used for a different request"
                            }
                        }),
                    ));
                }
                Ok(None) => {}
                Err(error) => return WireResponse::json(error_response(error)),
            }
        }

        let method = request.method.clone();
        let path = request.path.clone();
        let response = self.server.handle(ApiRequest {
            method: request.method,
            path: request.path,
            body: request
                .body
                .and_then(|body| serde_json::from_slice(&body).ok()),
        });
        if method != "GET"
            && response.status < 400
            && let Some(runtime_root) = self.server.runtime_root.as_ref()
            && let Err(error) =
                append_api_mutation_event(runtime_root, &method, &path, response.status)
        {
            return WireResponse::json(error_response(error));
        }
        if let (Some(runtime_root), Some(key), Some(fingerprint)) = (
            self.server.runtime_root.as_ref(),
            idempotency_key.as_deref(),
            fingerprint.as_deref(),
        ) && let Err(error) = save_idempotency_record(runtime_root, key, fingerprint, &response)
        {
            return WireResponse::json(error_response(error));
        }
        self.with_request_metric(
            &HttpRequest {
                method,
                path,
                headers: BTreeMap::new(),
                body: None,
            },
            "hot",
            started,
            WireResponse::json(response),
        )
    }

    pub(super) fn try_static_response(&self, path: &str) -> Option<WireResponse> {
        let static_root = self.static_root.as_ref()?;
        let path = path.split('?').next().unwrap_or(path);
        let website_mode = website_index_path(static_root).is_file();

        if website_mode && matches!(path, "/docs" | "/docs/") {
            return Some(render_docs_landing_page());
        }

        if website_mode && (path == "/read" || path.starts_with("/read/")) {
            return Some(render_markdown_document(static_root, path));
        }

        let relative = match path {
            "/" if website_mode => PathBuf::from("src/surfaces/website/index.html"),
            "/" => PathBuf::from("index.html"),
            path => {
                let trimmed = path.trim_start_matches('/');
                let trimmed = if website_mode {
                    trimmed
                } else {
                    trimmed.strip_prefix("static/").unwrap_or(trimmed)
                };
                if trimmed.contains("..") || trimmed.starts_with('/') {
                    return Some(WireResponse::json(ApiResponse::json(
                        400,
                        json!({
                            "error": {
                                "code": "invalid_path",
                                "message": "static asset path is invalid"
                            }
                        }),
                    )));
                }
                if website_mode && !website_public_path_allowed(trimmed) {
                    return None;
                }
                PathBuf::from(trimmed)
            }
        };
        let full_path = static_root.join(relative);
        if !is_within(static_root, &full_path) || !full_path.is_file() {
            return None;
        }
        let entry = match cached_static_asset(static_root, &full_path) {
            Ok(entry) => entry,
            Err(error) => {
                return Some(WireResponse::json(ApiResponse::json(
                    500,
                    json!({
                        "error": {
                            "code": "static_read_failed",
                            "message": format!("failed to read static asset: {error}")
                        }
                    }),
                )));
            }
        };
        Some(WireResponse::bytes(200, entry.content_type, entry.bytes))
    }

    pub(super) fn with_request_metric(
        &self,
        request: &HttpRequest,
        cache_mode: &'static str,
        started: Instant,
        response: WireResponse,
    ) -> WireResponse {
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        if !record_http_request_metric(&self.server, request, &response, cache_mode, elapsed_ms) {
            return response;
        }
        response
    }
}
