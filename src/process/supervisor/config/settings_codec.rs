use super::*;

pub(super) fn default_settings() -> JsonObject {
    // The concurrency caps are deliberately absent.
    //
    // Seeding them meant `load` always produced a value, so the scheduler never
    // saw the key as unset and the host-capacity governor it falls back to could
    // not be reached through any supported configuration — a fixed 2 applied to
    // a two-core node and a thirty-two-core one alike, which is exactly what the
    // governor exists to stop. Absent means "let this host decide"; an explicit
    // value still wins.
    let mut settings = JsonObject::new();
    for (key, value) in [
        ("branch_name_pattern", "refine/{goal_id}"),
        ("agent_idle_timeout_seconds", "900"),
        ("agent_hard_cap_seconds", "7200"),
        ("agent_limit_pause_seconds", "60"),
        ("worker_memory_limit_mb", ""),
        ("ui_memory_limit_mb", ""),
        ("worker_cpu_priority", "normal"),
        ("resource_isolation_mode", "process_group"),
        ("chat_idle_timeout_seconds", "300"),
        ("backlog_promote_after_seconds", "3600"),
        ("worktree_cleanup_after_seconds", "0"),
        ("worktree_cleanup_generated_paths", ""),
        ("state_sync_debounce_seconds", "5"),
        ("project_update_pulse_interval_seconds", "300"),
        ("file_browser_ignore_patterns", ""),
        ("agent_subpath", ""),
        ("git_remote", "origin"),
        ("merge_target_branch", "main"),
        ("quality_enabled", "0"),
        ("allowed_commands", ""),
        ("agent_cli", "claude"),
        ("target_app_start_instructions", ""),
        ("target_app_stop_instructions", ""),
        ("target_app_build_instructions", ""),
        ("target_app_health_url", ""),
        ("target_app_url", ""),
        ("target_app_start_command", ""),
        ("target_app_stop_command", ""),
        ("target_app_build_command", ""),
        ("target_app_test_command", ""),
        ("target_app_test_commands", ""),
        ("target_app_status_command", ""),
        ("target_app_cwd", ""),
        ("target_app_env_json", "{}"),
        ("target_app_start_timeout_seconds", "60"),
        ("target_app_stop_timeout_seconds", "30"),
        ("target_app_build_timeout_seconds", "600"),
        ("target_app_test_timeout_seconds", "600"),
        ("target_app_status_timeout_seconds", "30"),
        ("target_app_log_path", ""),
        ("target_app_http_check_url", ""),
        ("target_app_tcp_check_host", ""),
        ("target_app_tcp_check_port", ""),
        ("target_app_process_check_command", ""),
        ("target_app_auto_build", "never"),
        ("target_app_auto_build_hour_utc", "3"),
    ] {
        settings.insert(key.to_string(), Value::String(value.to_string()));
    }
    settings
}

pub(super) fn allowed_settings() -> BTreeSet<&'static str> {
    [
        "parallel_run_cap",
        "parallel_per_node_cap",
        "parallel_per_provider_cap",
        "parallel_per_target_app_cap",
        "branch_name_pattern",
        "agent_idle_timeout_seconds",
        "agent_hard_cap_seconds",
        "agent_limit_pause_seconds",
        "worker_memory_limit_mb",
        "ui_memory_limit_mb",
        "worker_cpu_priority",
        "resource_isolation_mode",
        "chat_idle_timeout_seconds",
        "backlog_promote_after_seconds",
        "worktree_cleanup_after_seconds",
        "worktree_cleanup_generated_paths",
        "state_sync_debounce_seconds",
        "project_update_pulse_interval_seconds",
        "file_browser_ignore_patterns",
        "agent_subpath",
        "git_remote",
        "merge_target_branch",
        "quality_enabled",
        "quality_timing",
        "allowed_commands",
        "agent_cli",
        "target_app_start_instructions",
        "target_app_stop_instructions",
        "target_app_build_instructions",
        "target_app_health_url",
        "target_app_url",
        "target_app_start_command",
        "target_app_stop_command",
        "target_app_build_command",
        "target_app_test_command",
        "target_app_test_commands",
        "target_app_status_command",
        "target_app_cwd",
        "target_app_env_json",
        "target_app_start_timeout_seconds",
        "target_app_stop_timeout_seconds",
        "target_app_build_timeout_seconds",
        "target_app_test_timeout_seconds",
        "target_app_status_timeout_seconds",
        "target_app_log_path",
        "target_app_http_check_url",
        "target_app_tcp_check_host",
        "target_app_tcp_check_port",
        "target_app_process_check_command",
        "target_app_auto_build",
        "target_app_auto_build_hour_utc",
    ]
    .into_iter()
    .collect()
}

pub(super) fn legacy_setting_key(key: &str) -> Option<&'static str> {
    match key {
        "target_app_rebuild_command" => Some("target_app_build_command"),
        "target_app_rebuild_instructions" => Some("target_app_build_instructions"),
        "target_app_rebuild_timeout_seconds" => Some("target_app_build_timeout_seconds"),
        "target_app_auto_rebuild" => Some("target_app_auto_build"),
        "target_app_auto_rebuild_hour_utc" => Some("target_app_auto_build_hour_utc"),
        _ => None,
    }
}

pub(super) fn normalize_setting(key: &str, value: &Value) -> RefineResult<String> {
    match key {
        "agent_cli" => {
            let raw = as_string(value);
            let choice = raw.trim();
            let known = choice.to_ascii_lowercase();
            if matches!(
                known.as_str(),
                "claude" | "codex" | "gemini" | "copilot" | "smoke-ai"
            ) {
                Ok(known)
            } else if !choice.is_empty()
                && !choice.as_bytes().contains(&0)
                && !choice.chars().any(char::is_whitespace)
            {
                // A configured generic provider is one executable path/name.
                // Provider-specific prompt semantics still remain centralized
                // in HostAgentProviderService's generic capability contract.
                Ok(choice.to_string())
            } else {
                Err(RefineError::InvalidInput(
                    "agent_cli must be a known provider or one generic CLI executable without whitespace"
                        .to_string(),
                ))
            }
        }
        "quality_enabled" => Ok(if value_is_truthy(value) { "1" } else { "0" }.to_string()),
        "quality_timing" => {
            let value = as_string(value);
            if matches!(value.trim(), "post_build" | "post_rebuild") {
                Ok("post_build".to_string())
            } else if value.trim() == "pre_merge" {
                Ok("pre_merge".to_string())
            } else {
                Err(RefineError::InvalidInput(
                    "quality_timing must be one of pre_merge, post_build".to_string(),
                ))
            }
        }
        "target_app_env_json" => {
            let raw = as_string(value);
            let parsed = serde_json::from_str::<Value>(raw.trim()).map_err(|_| {
                RefineError::InvalidInput("target_app_env_json must be a JSON object".to_string())
            })?;
            if !parsed.is_object() {
                return Err(RefineError::InvalidInput(
                    "target_app_env_json must be a JSON object".to_string(),
                ));
            }
            Ok(parsed.to_string())
        }
        "target_app_test_commands" => normalize_target_app_test_commands(value),
        "worktree_cleanup_generated_paths" => normalize_worktree_cleanup_generated_paths(value),
        // Empty hands the decision back to the host-capacity governor. Without
        // it the only way to unpin a cap was to hand-edit `nodes.json`, since
        // the range rejects every value the scheduler treats as unset.
        "parallel_run_cap"
        | "parallel_per_node_cap"
        | "parallel_per_provider_cap"
        | "parallel_per_target_app_cap" => {
            if as_string(value).trim().is_empty() {
                Ok(String::new())
            } else {
                normalize_range(key, value, 1, 100)
            }
        }
        "target_app_tcp_check_port" => {
            let text = as_string(value);
            if text.trim().is_empty() {
                Ok(String::new())
            } else {
                normalize_range(key, value, 1, 65535)
            }
        }
        "target_app_auto_build_hour_utc" => normalize_range(key, value, 0, 23),
        key if key.ends_with("_timeout_seconds")
            || matches!(
                key,
                "agent_idle_timeout_seconds"
                    | "agent_hard_cap_seconds"
                    | "agent_limit_pause_seconds"
                    | "backlog_promote_after_seconds"
                    | "worktree_cleanup_after_seconds"
                    | "state_sync_debounce_seconds"
                    | "project_update_pulse_interval_seconds"
            ) =>
        {
            normalize_integer(key, value)
        }
        _ => Ok(as_string(value).trim().to_string()),
    }
}

fn normalize_worktree_cleanup_generated_paths(value: &Value) -> RefineResult<String> {
    let mut paths = BTreeSet::new();
    for raw in as_string(value).split([',', '\n']) {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let path = Path::new(raw);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(RefineError::InvalidInput(
                "worktree_cleanup_generated_paths must contain only relative paths without . or .."
                    .to_string(),
            ));
        }
        paths.insert(raw.replace('\\', "/"));
    }
    Ok(paths.into_iter().collect::<Vec<_>>().join(", "))
}

pub(super) fn normalize_quality_timing_lossy(value: &str) -> String {
    if matches!(value.trim(), "post_build" | "post_rebuild") {
        "post_build".to_string()
    } else {
        DEFAULT_QUALITY_TIMING.to_string()
    }
}

pub(super) fn sync_target_app_test_settings(
    settings: &mut JsonObject,
    prefer_command_list: bool,
) -> RefineResult<bool> {
    let command_text = settings
        .get("target_app_test_command")
        .map(as_string)
        .unwrap_or_default();
    let commands_text = settings
        .get("target_app_test_commands")
        .map(as_string)
        .unwrap_or_default();

    if prefer_command_list {
        let enabled = enabled_target_app_test_command(&commands_text);
        if enabled != command_text {
            settings.insert(
                "target_app_test_command".to_string(),
                Value::String(enabled),
            );
            return Ok(true);
        }
        return Ok(false);
    }

    if commands_text.trim().is_empty() && !command_text.trim().is_empty() {
        settings.insert(
            "target_app_test_commands".to_string(),
            Value::String(normalize_target_app_test_commands(&Value::Array(vec![
                json!({
                    "command": command_text,
                    "enabled": true
                }),
            ]))?),
        );
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn enabled_target_app_test_command(commands_text: &str) -> String {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(commands_text.trim()) else {
        return String::new();
    };
    items
        .iter()
        .find(|item| item.get("enabled").and_then(Value::as_bool).unwrap_or(true))
        .and_then(|item| item.get("command").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string()
}

pub(super) fn normalize_target_app_test_commands(value: &Value) -> RefineResult<String> {
    let raw_items = match value {
        Value::Array(items) => items.clone(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(String::new());
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(Value::Array(items)) => items,
                Ok(_) => {
                    return Err(RefineError::InvalidInput(
                        "target_app_test_commands must be a JSON array".to_string(),
                    ));
                }
                Err(_) => vec![Value::String(trimmed.to_string())],
            }
        }
        Value::Null => return Ok(String::new()),
        _ => {
            return Err(RefineError::InvalidInput(
                "target_app_test_commands must be a JSON array".to_string(),
            ));
        }
    };

    let mut commands = Vec::new();
    let mut seen = BTreeSet::new();
    for item in raw_items {
        let command = item
            .get("command")
            .and_then(Value::as_str)
            .or_else(|| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if command.is_empty() || seen.contains(&command) {
            continue;
        }
        seen.insert(command.clone());
        commands.push(json!({
            "command": command,
            "enabled": item.get("enabled").and_then(Value::as_bool).unwrap_or(true)
        }));
    }
    if commands.is_empty() {
        return Ok(String::new());
    }
    serde_json::to_string(&commands).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode target_app_test_commands: {error}"
        ))
    })
}

pub(super) fn normalize_range(
    key: &str,
    value: &Value,
    min: i64,
    max: i64,
) -> RefineResult<String> {
    let number = as_string(value)
        .trim()
        .parse::<i64>()
        .map_err(|_| RefineError::InvalidInput(format!("{key} must be an integer")))?;
    if number < min || number > max {
        return Err(RefineError::InvalidInput(format!(
            "{key} must be between {min} and {max}"
        )));
    }
    Ok(number.to_string())
}

pub(super) fn normalize_integer(key: &str, value: &Value) -> RefineResult<String> {
    let number = as_string(value)
        .trim()
        .parse::<i64>()
        .map_err(|_| RefineError::InvalidInput(format!("{key} must be an integer")))?;
    Ok(number.to_string())
}

pub(super) fn as_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(super) fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64().unwrap_or_default() != 0,
        Value::String(value) => {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }
        _ => false,
    }
}
