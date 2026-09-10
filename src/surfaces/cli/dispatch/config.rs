use super::config_input::*;
use super::*;

use crate::infrastructure::process::supervisor::config::FileSettingsService;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    let Commands::Config { action } = command else {
        unreachable!("command family was routed incorrectly")
    };
    match dispatch_config(action) {
        Ok(value) => {
            print_json(&value);
            Ok(())
        }
        Err(error) => Err(structured_config_error(error)),
    }
}

pub(crate) fn dispatch_config(action: ConfigAction) -> RefineResult<Value> {
    match action {
        ConfigAction::Show {
            domain,
            target_root,
        } => match domain {
            Some(domain) => read_domain(domain, target_root),
            None => read_all(target_root),
        },
        ConfigAction::Settings { action } => dispatch_settings(action),
    }
}

fn dispatch_settings(action: ConfigSettingsAction) -> RefineResult<Value> {
    match action {
        ConfigSettingsAction::Show { target_root } => {
            read_domain(ConfigDomain::Settings, target_root)
        }
        ConfigSettingsAction::Set {
            values,
            payload,
            target_root,
        } => {
            let mut flags = serde_json::Map::new();
            for assignment in values {
                let (key, value) = assignment.split_once('=').ok_or_else(|| {
                    RefineError::InvalidInput(format!(
                        "invalid --set {assignment:?}; expected KEY=VALUE"
                    ))
                })?;
                let key = key.trim();
                if key.is_empty() {
                    return Err(RefineError::InvalidInput(
                        "settings key must not be empty".to_string(),
                    ));
                }
                let raw = value.trim();
                let value =
                    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
                flags.insert(key.to_string(), value);
            }
            let body = decode_config_input(payload, flags, "settings patch")?;
            FileSettingsService::validate_update(&body)?;
            with_target_or_daemon(
                target_root,
                "PATCH",
                "/settings",
                body,
                |refine_dir, body| FileSettingsService::new(refine_dir).update(body),
            )
        }
    }
}

fn read_all(target_root: Option<PathBuf>) -> RefineResult<Value> {
    let settings = read_domain(ConfigDomain::Settings, target_root.clone())?;
    let skills = read_domain(ConfigDomain::Skills, target_root)?;
    Ok(json!({"settings": settings.get("settings").unwrap_or(&settings), "skills": skills}))
}

fn read_domain(domain: ConfigDomain, target_root: Option<PathBuf>) -> RefineResult<Value> {
    let path = match domain {
        ConfigDomain::Settings => "/settings",
        ConfigDomain::Skills => "/skills",
    };
    match target_root {
        None => daemon_json("GET", path, None),
        Some(target_root) => {
            let refine_dir = refine_dir_for_target_root(&target_root)?;
            match domain {
                ConfigDomain::Settings => FileSettingsService::new(refine_dir).list_response(),
                ConfigDomain::Skills => {
                    crate::application::events::FileEventService::new(refine_dir)
                        .list("skills", None)
                }
            }
        }
    }
}

fn with_target_or_daemon(
    target_root: Option<PathBuf>,
    method: &str,
    path: &str,
    body: Value,
    local: impl FnOnce(PathBuf, &Value) -> RefineResult<Value>,
) -> RefineResult<Value> {
    match target_root {
        Some(target_root) => local(refine_dir_for_target_root(&target_root)?, &body),
        None => daemon_json(method, path, Some(body)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_errors_distinguish_detached_unreachable_and_conflict_failures() {
        let detached_body = serde_json::to_vec(&json!({
            "error": {
                "code": "target_root_unavailable",
                "message": "No active app is attached"
            }
        }))
        .unwrap();
        let detached_response = format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\n\r\n",
            detached_body.len()
        );
        let mut detached_response = detached_response.into_bytes();
        detached_response.extend(detached_body);
        let detached = parse_daemon_response(&detached_response).unwrap_err();

        for (error, code) in [
            (detached, "missing_active_app"),
            (
                RefineError::Degraded("daemon connection refused".to_string()),
                "daemon_unavailable",
            ),
            (
                RefineError::Conflict("stale configuration revision".to_string()),
                "conflict",
            ),
        ] {
            let structured = structured_config_error(error);
            let value: Value = serde_json::from_str(&structured.to_string()).unwrap();
            assert_eq!(value["error"]["code"], code);
            assert!(value["error"]["message"].is_string());
        }
    }
}

pub(super) fn structured_config_error(error: RefineError) -> RefineError {
    let message = error.to_string();
    let code = if message.starts_with("missing active app:") {
        "missing_active_app"
    } else {
        match error.category() {
            crate::error::ErrorCategory::InvalidInput => "invalid_input",
            crate::error::ErrorCategory::NotFound => "not_found",
            crate::error::ErrorCategory::Unauthorized => "unauthorized",
            crate::error::ErrorCategory::Conflict => "conflict",
            crate::error::ErrorCategory::Degraded => "daemon_unavailable",
            crate::error::ErrorCategory::Io => "io_error",
            crate::error::ErrorCategory::Serialization => "serialization_error",
            crate::error::ErrorCategory::NotImplemented => "not_implemented",
        }
    };
    let encoded = serde_json::to_string_pretty(&json!({
        "error": {"code": code, "message": message}
    }))
    .unwrap_or_else(|_| error.to_string());
    match error.category() {
        crate::error::ErrorCategory::InvalidInput => RefineError::InvalidInput(encoded),
        crate::error::ErrorCategory::NotFound => RefineError::NotFound(encoded),
        crate::error::ErrorCategory::Unauthorized => RefineError::Unauthorized(encoded),
        crate::error::ErrorCategory::Conflict => RefineError::Conflict(encoded),
        crate::error::ErrorCategory::Degraded => RefineError::Degraded(encoded),
        crate::error::ErrorCategory::Io => RefineError::Io(encoded),
        crate::error::ErrorCategory::Serialization => RefineError::Serialization(encoded),
        crate::error::ErrorCategory::NotImplemented => RefineError::NotImplemented(encoded),
    }
}
