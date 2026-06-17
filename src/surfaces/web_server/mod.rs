use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::process::supervisor::errors::RefineResult;
use crate::process::supervisor::lifecycle::DaemonStatus;
use crate::tools::product::project_state::ProjectionSnapshot;

pub const API_CONTRACT_VERSION: &str = "1";
pub const IDEMPOTENCY_DIR: &str = "idempotency";
pub const API_EVENTS_FILE: &str = "api-events.jsonl";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiRouteGroup {
    pub prefix: &'static str,
    pub capability: &'static str,
}

pub const API_GROUPS: &[ApiRouteGroup] = &[
    ApiRouteGroup {
        prefix: "/system",
        capability: "install state, daemon status, update, doctor",
    },
    ApiRouteGroup {
        prefix: "/apps",
        capability: "app registry, attach, switch, detach, clone",
    },
    ApiRouteGroup {
        prefix: "/project",
        capability: "active app status, app attach helpers, migration, sync",
    },
    ApiRouteGroup {
        prefix: "/target-app",
        capability: "target app lifecycle, health, generated instructions",
    },
    ApiRouteGroup {
        prefix: "/work",
        capability: "Gaps, Features, imports, state transitions",
    },
    ApiRouteGroup {
        prefix: "/workflow",
        capability: "workflow automation",
    },
    ApiRouteGroup {
        prefix: "/activity",
        capability: "activity log, UI error events, cleanup",
    },
    ApiRouteGroup {
        prefix: "/import",
        capability: "Gap import extraction, CSV parsing, dedup, persist",
    },
    ApiRouteGroup {
        prefix: "/dashboard",
        capability: "dashboard projection and diagnostic summary",
    },
    ApiRouteGroup {
        prefix: "/agents",
        capability: "provider configuration, auth, diagnostics",
    },
    ApiRouteGroup {
        prefix: "/operations",
        capability: "operation status, logs, cancel",
    },
    ApiRouteGroup {
        prefix: "/runner-workers",
        capability: "supervised worker actions and maintenance controls",
    },
    ApiRouteGroup {
        prefix: "/processes",
        capability: "managed process list and controls",
    },
    ApiRouteGroup {
        prefix: "/events",
        capability: "server-sent events for app, process, operation, chat updates",
    },
    ApiRouteGroup {
        prefix: "/quality",
        capability: "checks and screenshots",
    },
    ApiRouteGroup {
        prefix: "/chat",
        capability: "sessions, messages, streaming events",
    },
    ApiRouteGroup {
        prefix: "/settings",
        capability: "project and runtime settings",
    },
    ApiRouteGroup {
        prefix: "/governance",
        capability: "governance rules and generated project rules",
    },
    ApiRouteGroup {
        prefix: "/guidance",
        capability: "operator guidance documents",
    },
    ApiRouteGroup {
        prefix: "/reporters",
        capability: "reporter registry, rename, merge, delete",
    },
    ApiRouteGroup {
        prefix: "/nodes",
        capability: "node registry, activation, ownership transfer",
    },
    ApiRouteGroup {
        prefix: "/cluster",
        capability: "node registry and remote operations",
    },
    ApiRouteGroup {
        prefix: "/changes",
        capability: "git change projection and undo operations",
    },
    ApiRouteGroup {
        prefix: "/cache",
        capability: "projection cache rebuild",
    },
    ApiRouteGroup {
        prefix: "/performance",
        capability: "performance metrics and cleanup",
    },
    ApiRouteGroup {
        prefix: "/files",
        capability: "source tree, file read, source search",
    },
    ApiRouteGroup {
        prefix: "/terminal",
        capability: "interactive terminal sessions",
    },
    ApiRouteGroup {
        prefix: "/diagnostics",
        capability: "system diagnostics",
    },
    ApiRouteGroup {
        prefix: "/upgrade",
        capability: "upgrade availability and health",
    },
];

pub trait LocalDaemonWebServer {
    fn serve(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn route_groups(&self) -> &'static [ApiRouteGroup] {
        API_GROUPS
    }
    fn server_sent_events(&self, stream: &str) -> RefineResult<String>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiRequest {
    pub method: String,
    pub path: String,
    pub body: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiResponse {
    pub status: u16,
    pub content_type: String,
    pub body: serde_json::Value,
}

impl ApiResponse {
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json".to_string(),
            body,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct IdempotencyRecord {
    key: String,
    fingerprint: String,
    response: ApiResponse,
    created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ApiMutationEvent {
    method: String,
    path: String,
    status: u16,
    created_at: String,
}

#[derive(Clone, Debug)]
pub struct InProcessWebServer {
    pub status: DaemonStatus,
    pub projection: ProjectionSnapshot,
    /// Test/bootstrap target app root. The served daemon resolves real app state
    /// from the runtime app registry at request time.
    pub target_root: Option<PathBuf>,
    pub runtime_root: Option<PathBuf>,
}

macro_rules! require_refine_dir {
    ($server:expr, $action:expr) => {{
        match $server.current_refine_dir() {
            Ok(Some(path)) => path,
            Ok(None) => return target_root_unavailable($action),
            Err(error) => return error_response(error),
        }
    }};
}

mod http;
mod operation_routes;
mod project_routes;
mod quality_chat_routes;
mod runtime;
mod server;
mod support;
#[cfg(test)]
mod tests;
mod work_routes;

pub use http::{HttpRequest, LocalHttpDaemon, WireResponse};
