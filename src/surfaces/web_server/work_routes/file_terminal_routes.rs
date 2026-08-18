use super::{
    ApiRequest, ApiResponse, ConfigService, HostAgentProviderService, InProcessWebServer, PathBuf,
    RefineError, TerminalLaunchSpec, TerminalSessionLaunchSurface, Value,
    attach_toolbar_goal_agent_session, cleanup_failed_terminal_worktree,
    create_terminal_standalone_worktree, default_interactive_shell, error_response,
    files_read_response, files_search_response, files_tree_response, find_goal_agent_session, json,
    query_param, resume_terminal_standalone_worktree, runtime_root_unavailable,
    target_root_unavailable, terminal_events_range, terminal_input_response,
    terminal_profile_prompt, terminal_resize_response, terminal_session_start_response,
    terminal_status_response, terminal_stop_response,
};
use crate::application::work_items::FileWorkItemService;

impl InProcessWebServer {
    pub(crate) fn handle_files_tree(&self, raw_path: &str) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("read source files");
        };
        let path = query_param(raw_path, "path").unwrap_or_default();
        let recursive = query_param(raw_path, "recursive")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let max_depth = query_param(raw_path, "max_depth")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1)
            .min(8);
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(200)
            .clamp(1, 1000);
        match files_tree_response(&target_root, &path, recursive, max_depth, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_files_read(&self, raw_path: &str) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("read source file");
        };
        let path = query_param(raw_path, "path").unwrap_or_default();
        let offset = query_param(raw_path, "offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param(raw_path, "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(128_000)
            .clamp(1, 512_000);
        match files_read_response(&target_root, &path, offset, limit) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_files_search(&self, raw_path: &str) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("search source files");
        };
        let query = query_param(raw_path, "q").unwrap_or_default();
        let max_entries = query_param(raw_path, "max_entries")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20)
            .clamp(1, 200);
        match files_search_response(&target_root, &query, max_entries) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_terminal_session_start(&self, request: ApiRequest) -> ApiResponse {
        let Some(target_root) = self.target_root() else {
            return target_root_unavailable("start terminal session");
        };
        let Some(runtime_root) = self.runtime_root.clone() else {
            return runtime_root_unavailable("start managed terminal sessions");
        };
        let body = request.body.unwrap_or_else(|| json!({}));
        let cols = body
            .get("cols")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        let rows = body
            .get("rows")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        let profile = body
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or("terminal")
            .trim()
            .to_lowercase();
        if !matches!(
            profile.as_str(),
            "terminal" | "agent" | "plan" | "goal" | "standalone"
        ) {
            return error_response(RefineError::InvalidInput(format!(
                "unknown terminal profile {profile}"
            )));
        }
        let surface = match TerminalSessionLaunchSurface::from_request(body.get("surface")) {
            Ok(surface) => surface,
            Err(error) => return error_response(error),
        };
        let goal_id = body
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if profile == "goal" {
            let Some(goal_id) = goal_id.as_deref() else {
                return error_response(RefineError::InvalidInput(
                    "goal_id is required to open a Goal Agent".to_string(),
                ));
            };
            match find_goal_agent_session(&runtime_root, goal_id) {
                Ok(snapshot) => {
                    let snapshot = if surface == TerminalSessionLaunchSurface::Toolbar {
                        match attach_toolbar_goal_agent_session(&runtime_root, &snapshot.id) {
                            Ok(snapshot) => snapshot,
                            Err(error) => return error_response(error),
                        }
                    } else {
                        snapshot
                    };
                    return match serde_json::to_value(snapshot) {
                        Ok(value) => ApiResponse::json(200, value),
                        Err(error) => error_response(RefineError::Serialization(format!(
                            "failed to encode Goal Agent session: {error}"
                        ))),
                    };
                }
                Err(RefineError::NotFound(message)) => {
                    let planning_between_processes = self
                        .current_refine_dir()
                        .ok()
                        .flatten()
                        .and_then(|refine_dir| {
                            FileWorkItemService::new(refine_dir)
                                .show_goal_detail(goal_id)
                                .ok()
                        })
                        .is_some_and(|goal| {
                            matches!(
                                goal.get("status").and_then(Value::as_str),
                                Some("plan" | "implement")
                            ) && goal
                                .get("rounds")
                                .and_then(Value::as_array)
                                .and_then(|rounds| rounds.last())
                                .and_then(|round| round.get("implementation_plan"))
                                .is_some_and(|plan| {
                                    plan.get("state").and_then(Value::as_str) == Some("in_progress")
                                })
                        });
                    if planning_between_processes {
                        return error_response(RefineError::Conflict(format!(
                            "{message}; the workflow is between supervised implementation-planning phase processes, so Refine did not create a duplicate agent"
                        )));
                    }
                }
                Err(error) => return error_response(error),
            }
        }

        let refine_dir = match self.current_refine_dir() {
            Ok(Some(path)) => path,
            Ok(None) => return target_root_unavailable("start managed terminal sessions"),
            Err(error) => return error_response(error),
        };
        let feature_id = body
            .get("feature_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let supplemental_prompt = body
            .get("initial_prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let mut cwd = target_root.clone();
        let mut metadata = serde_json::Map::new();
        metadata.insert("profile".to_string(), json!(&profile));
        metadata.insert("surface".to_string(), json!(surface.as_str()));
        if let Some(goal_id) = &goal_id {
            let key = if profile == "goal" {
                // This association is context only. Workflow process control
                // must not treat stopping a diagnostic session as Goal
                // cancellation or requeue authority.
                "attached_goal_id"
            } else {
                "goal_id"
            };
            metadata.insert(key.to_string(), json!(goal_id));
        }
        if let Some(feature_id) = &feature_id {
            metadata.insert("feature_id".to_string(), json!(feature_id));
        }

        let (worktree, worktree_created) = if profile == "standalone" {
            let requested_worktree = body.get("worktree").filter(|value| value.is_object());
            let result = match requested_worktree {
                Some(worktree) => {
                    resume_terminal_standalone_worktree(&target_root, &runtime_root, worktree)
                }
                None => create_terminal_standalone_worktree(&target_root, &runtime_root),
            };
            match result {
                Ok(worktree) => {
                    cwd = PathBuf::from(&worktree["path"].as_str().unwrap_or_default());
                    metadata.insert("worktree".to_string(), worktree.clone());
                    (Some(worktree), requested_worktree.is_none())
                }
                Err(error) => return error_response(error),
            }
        } else {
            (None, false)
        };

        let launch = if profile == "terminal" {
            TerminalLaunchSpec {
                runtime_root: runtime_root.clone(),
                cwd,
                profile: profile.clone(),
                provider: None,
                command: default_interactive_shell(),
                args: vec!["-i".to_string()],
                metadata,
                prompt_transport: None,
                prompt_artifact: None,
                authorization_command: None,
                launch_environment: None,
            }
        } else {
            let provider = self
                .settings_service(&refine_dir)
                .load()
                .ok()
                .and_then(|settings| {
                    settings
                        .get("agent_cli")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "claude".to_string());
            let prompt = match terminal_profile_prompt(
                self,
                &profile,
                surface,
                goal_id.as_deref(),
                feature_id.as_deref(),
                supplemental_prompt,
            ) {
                Ok(prompt) => prompt,
                Err(error) => {
                    if worktree_created && let Some(worktree) = worktree.as_ref() {
                        cleanup_failed_terminal_worktree(&target_root, worktree);
                    }
                    return error_response(error);
                }
            };
            let environment_overrides = vec![
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string()),
                ("REFINE_TERMINAL".to_string(), "1".to_string()),
                ("REFINE_SESSION_ROLE".to_string(), profile.clone()),
            ];
            let provider_service = HostAgentProviderService::with_runtime_root(&runtime_root);
            let command = match provider_service.interactive_command_with_environment(
                &provider,
                &prompt,
                &environment_overrides,
            ) {
                Ok(command) => command,
                Err(error) => {
                    if worktree_created && let Some(worktree) = worktree.as_ref() {
                        cleanup_failed_terminal_worktree(&target_root, worktree);
                    }
                    return error_response(error);
                }
            };
            TerminalLaunchSpec {
                runtime_root: runtime_root.clone(),
                cwd,
                profile: profile.clone(),
                provider: Some(provider),
                command: command.binary,
                args: command.args,
                metadata,
                prompt_transport: Some(command.prompt_transport),
                prompt_artifact: command.prompt_artifact,
                authorization_command: Some(command.authorization_command),
                launch_environment: Some(command.launch_environment),
            }
        };

        match terminal_session_start_response(launch, cols, rows) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => {
                if worktree_created && let Some(worktree) = worktree.as_ref() {
                    cleanup_failed_terminal_worktree(&target_root, worktree);
                }
                error_response(error)
            }
        }
    }

    pub(crate) fn handle_terminal_input(
        &self,
        request: ApiRequest,
        session_id: &str,
    ) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let data = body.get("data").and_then(Value::as_str).unwrap_or("");
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("write terminal input");
        };
        match terminal_input_response(runtime_root, session_id, data) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_terminal_resize(
        &self,
        request: ApiRequest,
        session_id: &str,
    ) -> ApiResponse {
        let body = request.body.unwrap_or_else(|| json!({}));
        let cols = body
            .get("cols")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        let rows = body
            .get("rows")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("resize terminal session");
        };
        match terminal_resize_response(runtime_root, session_id, cols, rows) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_terminal_stop(&self, session_id: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("stop terminal session");
        };
        let refine_dir = match self.current_refine_dir() {
            Ok(refine_dir) => refine_dir,
            Err(error) => return error_response(error),
        };
        match terminal_stop_response(runtime_root, refine_dir.as_deref(), session_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_terminal_status(&self, session_id: &str) -> ApiResponse {
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("read terminal session");
        };
        match terminal_status_response(runtime_root, session_id) {
            Ok(value) => ApiResponse::json(200, value),
            Err(error) => error_response(error),
        }
    }

    pub(crate) fn handle_terminal_events_snapshot(&self, raw_path: &str) -> ApiResponse {
        let Some(session_id) = raw_path
            .split('?')
            .next()
            .and_then(|path| path.strip_prefix("/api/terminal/"))
            .and_then(|rest| rest.strip_suffix("/events"))
            .or_else(|| {
                raw_path
                    .split('?')
                    .next()
                    .and_then(|path| path.strip_prefix("/terminal/"))
                    .and_then(|rest| rest.strip_suffix("/events"))
            })
        else {
            return error_response(RefineError::InvalidInput(
                "terminal session id is required".to_string(),
            ));
        };
        let after = query_param(raw_path, "after")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let before = query_param(raw_path, "before").and_then(|value| value.parse::<u64>().ok());
        let Some(runtime_root) = &self.runtime_root else {
            return runtime_root_unavailable("stream terminal session");
        };
        match terminal_events_range(runtime_root, session_id, after, before) {
            Ok(events) => ApiResponse::json(200, json!({ "events": events })),
            Err(error) => error_response(error),
        }
    }
}
