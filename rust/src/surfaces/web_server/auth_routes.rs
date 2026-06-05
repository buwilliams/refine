use serde_json::json;

use crate::core::product::scheduling::{FileSchedulingService, SchedulingService};
use crate::core::supervisor::security::{AuthToken, FileSecurityService, SecurityService};
use crate::core::supervisor::sessions::{FileSessionService, SessionService, SurfaceKind};

use super::support::*;
use super::*;

impl InProcessWebServer {
    pub(super) fn authorized(&self, request: &ApiRequest) -> bool {
        if let (Some(runtime_root), Some(token)) = (&self.runtime_root, &request.auth_token) {
            let command = format!("{} {}", request.method, request.path);
            if FileSecurityService::new(runtime_root)
                .authorize_mutation(
                    &AuthToken {
                        token: token.clone(),
                    },
                    &command,
                )
                .is_ok()
            {
                return true;
            }
        }
        match &self.auth_token {
            Some(expected) => request.auth_token.as_deref() == Some(expected.as_str()),
            None => self.runtime_root.is_none(),
        }
    }

    pub(super) fn handle_session_create(&self, request: ApiRequest) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("create surface sessions");
        };
        let surface = request
            .body
            .as_ref()
            .and_then(|body| body.get("surface"))
            .and_then(|surface| surface.as_str())
            .and_then(parse_surface_kind)
            .unwrap_or(SurfaceKind::Browser);
        match FileSessionService::new(
            runtime_root,
            format!("http://127.0.0.1:{}", self.status.port),
        )
        .authenticate_local_surface(surface)
        {
            Ok(session) => ApiResponse::json(
                201,
                json!({
                    "session": session,
                    "local_url": format!("http://127.0.0.1:{}", self.status.port),
                    "api_contract_version": API_CONTRACT_VERSION
                }),
            ),
            Err(error) => error_response(error),
        }
    }

    pub(super) fn handle_workflow_schedule(&self) -> ApiResponse {
        let durable_root = require_durable_root!(self, "schedule work items");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("persist scheduler state");
        };
        let scheduler = FileSchedulingService::with_durable_root(runtime_root, durable_root);
        match scheduler.promote().and_then(|promoted| {
            scheduler
                .load_state()
                .map(|state| json!({"promoted": promoted, "reservations": state.reservations}))
        }) {
            Ok(body) => ApiResponse::json(200, body),
            Err(error) => error_response(error),
        }
    }
}
