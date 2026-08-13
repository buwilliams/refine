use super::*;

use crate::process::supervisor::config::{
    FileGovernanceService, FileGuidanceService, FileSettingsService,
};
use crate::tools::host::quality::{FileQualityService, QualitySettingsPatch};

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
        ConfigAction::Quality { action } => dispatch_quality(action),
        ConfigAction::Governance { action } => dispatch_governance(action),
        ConfigAction::Guidance { action } => dispatch_guidance(action),
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

fn dispatch_quality(action: ConfigQualityAction) -> RefineResult<Value> {
    match action {
        ConfigQualityAction::Show { target_root } => {
            read_domain(ConfigDomain::Quality, target_root)
        }
        ConfigQualityAction::Set {
            business_requirements,
            instructions,
            tests,
            payload,
            target_root,
        } => {
            let mut flags = serde_json::Map::new();
            insert_optional(&mut flags, "business_requirements", business_requirements);
            insert_optional(&mut flags, "instructions", instructions);
            if !tests.is_empty() {
                flags.insert("tests".to_string(), json!(tests));
            }
            let body = decode_config_input(payload, flags, "Quality patch")?;
            with_target_or_daemon(
                target_root,
                "PATCH",
                "/quality",
                body,
                |refine_dir, body| {
                    let patch = serde_json::from_value::<QualitySettingsPatch>(body.clone())
                        .map_err(|error| {
                            RefineError::InvalidInput(format!(
                                "invalid Quality settings body: {error}"
                            ))
                        })?;
                    Ok(json!(
                        FileQualityService::new(refine_dir).save_settings(patch)?
                    ))
                },
            )
        }
    }
}

fn dispatch_governance(action: ConfigGovernanceAction) -> RefineResult<Value> {
    match action {
        ConfigGovernanceAction::Show { target_root } => {
            read_domain(ConfigDomain::Governance, target_root)
        }
        ConfigGovernanceAction::Set {
            product,
            constitution,
            max_automatic_round_retries,
            rules,
            payload,
            target_root,
        } => {
            let mut flags = serde_json::Map::new();
            insert_optional(&mut flags, "product", product);
            insert_optional(&mut flags, "constitution", constitution);
            if let Some(limit) = max_automatic_round_retries {
                flags.insert("max_automatic_round_retries".to_string(), json!(limit));
            }
            if !rules.is_empty() {
                flags.insert(
                    "rules".to_string(),
                    Value::Array(
                        rules
                            .into_iter()
                            .map(|text| json!({"text": text}))
                            .collect(),
                    ),
                );
            }
            let mut body = decode_config_input(payload, flags, "Governance patch")?;
            if body.get("rules").is_some() && body.get("rules_revision").is_none() {
                let current = read_domain(ConfigDomain::Governance, target_root.clone())?;
                body["rules_revision"] = json!(current["rules_revision"].as_u64().unwrap_or(0));
            }
            with_target_or_daemon(
                target_root,
                "PATCH",
                "/governance",
                body,
                |refine_dir, body| FileGovernanceService::new(refine_dir).save(body),
            )
        }
        ConfigGovernanceAction::GenerateRules {
            product,
            constitution,
            payload,
            target_root,
        } => {
            let mut flags = serde_json::Map::new();
            insert_optional(&mut flags, "product", product);
            insert_optional(&mut flags, "constitution", constitution);
            let supplied = decode_optional_config_input(payload, flags, "Governance generation")?;
            let current = read_domain(ConfigDomain::Governance, target_root.clone())?;
            let product = supplied
                .get("product")
                .cloned()
                .unwrap_or_else(|| current["product"].clone());
            let constitution = supplied
                .get("constitution")
                .cloned()
                .unwrap_or_else(|| current["constitution"].clone());
            let request = json!({"product": product, "constitution": constitution});
            let generated = with_target_or_daemon(
                target_root.clone(),
                "POST",
                "/governance/generate-rules",
                request,
                |refine_dir, body| FileGovernanceService::new(refine_dir).generate_rules(body),
            )?;
            let saved = with_target_or_daemon(
                target_root,
                "PATCH",
                "/governance",
                json!({
                    "rules": generated["rules"],
                    "rules_revision": current["rules_revision"].as_u64().unwrap_or(0)
                }),
                |refine_dir, body| FileGovernanceService::new(refine_dir).save(body),
            )?;
            Ok(json!({"governance": saved, "generation": generated}))
        }
    }
}

fn dispatch_guidance(action: ConfigGuidanceAction) -> RefineResult<Value> {
    match action {
        ConfigGuidanceAction::List { target_root } => {
            read_domain(ConfigDomain::Guidance, target_root)
        }
        ConfigGuidanceAction::Add {
            name,
            rule,
            instructions,
            enabled,
            payload,
            target_root,
        } => {
            let mut flags = serde_json::Map::new();
            insert_optional(&mut flags, "name", name);
            insert_optional(&mut flags, "rule", rule);
            insert_optional(&mut flags, "instructions", instructions);
            if let Some(enabled) = enabled {
                flags.insert("enabled".to_string(), json!(enabled));
            }
            let mut body = decode_config_input(payload, flags, "Guidance entry")?;
            if body.get("enabled").is_none() {
                body["enabled"] = json!(true);
            }
            guidance_item_mutation(target_root, "POST", None, body)
        }
        ConfigGuidanceAction::Edit {
            id,
            name,
            rule,
            instructions,
            enabled,
            payload,
            target_root,
        } => {
            let mut flags = serde_json::Map::new();
            insert_optional(&mut flags, "name", name);
            insert_optional(&mut flags, "rule", rule);
            insert_optional(&mut flags, "instructions", instructions);
            if let Some(enabled) = enabled {
                flags.insert("enabled".to_string(), json!(enabled));
            }
            guidance_item_mutation(
                target_root,
                "PATCH",
                Some(id),
                decode_config_input(payload, flags, "Guidance patch")?,
            )
        }
        ConfigGuidanceAction::Enable { id, target_root } => {
            guidance_item_mutation(target_root, "PATCH", Some(id), json!({"enabled": true}))
        }
        ConfigGuidanceAction::Disable { id, target_root } => {
            guidance_item_mutation(target_root, "PATCH", Some(id), json!({"enabled": false}))
        }
        ConfigGuidanceAction::Remove { id, target_root } => {
            guidance_item_mutation(target_root, "DELETE", Some(id), json!({}))
        }
    }
}

fn guidance_item_mutation(
    target_root: Option<PathBuf>,
    method: &str,
    id: Option<String>,
    mut body: Value,
) -> RefineResult<Value> {
    let current = read_domain(ConfigDomain::Guidance, target_root.clone())?;
    body["revision"] = json!(current["revision"].as_u64().unwrap_or(0));
    let path = id
        .as_deref()
        .map(|id| format!("/guidance/{}", path_segment(id)))
        .unwrap_or_else(|| "/guidance".to_string());
    match target_root {
        Some(target_root) => {
            let service = FileGuidanceService::new(refine_dir_for_target_root(&target_root)?);
            match (method, id.as_deref()) {
                ("POST", None) => service.add(&body),
                ("PATCH", Some(id)) => service.edit(id, &body),
                ("DELETE", Some(id)) => service.remove(id, &body),
                _ => unreachable!("unsupported Guidance mutation"),
            }
        }
        None => daemon_json(method, &path, Some(body)),
    }
}

fn read_all(target_root: Option<PathBuf>) -> RefineResult<Value> {
    let settings = read_domain(ConfigDomain::Settings, target_root.clone())?;
    let quality = read_domain(ConfigDomain::Quality, target_root.clone())?;
    let governance = read_domain(ConfigDomain::Governance, target_root.clone())?;
    let guidance = read_domain(ConfigDomain::Guidance, target_root)?;
    Ok(json!({
        "settings": settings.get("settings").unwrap_or(&settings),
        "quality": quality,
        "governance": governance,
        "guidance": guidance
    }))
}

fn read_domain(domain: ConfigDomain, target_root: Option<PathBuf>) -> RefineResult<Value> {
    let path = match domain {
        ConfigDomain::Settings => "/settings",
        ConfigDomain::Quality => "/quality",
        ConfigDomain::Governance => "/governance",
        ConfigDomain::Guidance => "/guidance",
    };
    match target_root {
        None => daemon_json("GET", path, None),
        Some(target_root) => {
            let refine_dir = refine_dir_for_target_root(&target_root)?;
            match domain {
                ConfigDomain::Settings => FileSettingsService::new(refine_dir).list_response(),
                ConfigDomain::Quality => {
                    Ok(json!(FileQualityService::new(refine_dir).load_settings()?))
                }
                ConfigDomain::Governance => FileGovernanceService::new(refine_dir).load(),
                ConfigDomain::Guidance => FileGuidanceService::new(refine_dir).list(),
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

fn decode_optional_config_input(
    payload: ConfigPayload,
    flags: serde_json::Map<String, Value>,
    label: &str,
) -> RefineResult<Value> {
    if !flags.is_empty() || payload.json.is_some() || payload.file.is_some() || payload.stdin {
        decode_config_input(payload, flags, label)
    } else {
        Ok(json!({}))
    }
}

fn decode_config_input(
    payload: ConfigPayload,
    flags: serde_json::Map<String, Value>,
    label: &str,
) -> RefineResult<Value> {
    decode_config_input_with_reader(payload, flags, label, &mut std::io::stdin().lock())
}

fn decode_config_input_with_reader(
    payload: ConfigPayload,
    flags: serde_json::Map<String, Value>,
    label: &str,
    stdin: &mut impl Read,
) -> RefineResult<Value> {
    let source_count = usize::from(payload.json.is_some())
        + usize::from(payload.file.is_some())
        + usize::from(payload.stdin);
    if source_count > 1 {
        return Err(RefineError::InvalidInput(
            "use exactly one of --json, --file, or --stdin".to_string(),
        ));
    }
    if source_count > 0 && !flags.is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "{label} flags cannot be combined with --json, --file, or --stdin"
        )));
    }
    let value = if let Some(text) = payload.json {
        parse_object(&text, "inline JSON")?
    } else if let Some(path) = payload.file {
        let text = fs::read_to_string(&path).map_err(|error| {
            RefineError::InvalidInput(format!(
                "failed to read configuration file {}: {error}",
                path.display()
            ))
        })?;
        parse_object(&text, &format!("configuration file {}", path.display()))?
    } else if payload.stdin {
        let mut text = String::new();
        stdin.read_to_string(&mut text).map_err(|error| {
            RefineError::InvalidInput(format!("failed to read configuration stdin: {error}"))
        })?;
        parse_object(&text, "configuration stdin")?
    } else {
        Value::Object(flags)
    };
    if value.as_object().is_none_or(serde_json::Map::is_empty) {
        return Err(RefineError::InvalidInput(format!(
            "{label} requires at least one value"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn input_decoder_handles_file_and_stdin_and_reports_malformed_sources() {
        let path =
            std::env::temp_dir().join(format!("refine-config-input-{}.json", std::process::id()));
        fs::write(&path, "{not-json").unwrap();
        let malformed_file = decode_config_input_with_reader(
            ConfigPayload {
                json: None,
                file: Some(path.clone()),
                stdin: false,
            },
            serde_json::Map::new(),
            "Quality patch",
            &mut Cursor::new(Vec::<u8>::new()),
        );
        assert!(matches!(malformed_file, Err(RefineError::InvalidInput(_))));
        fs::remove_file(path).unwrap();

        let malformed_stdin = decode_config_input_with_reader(
            ConfigPayload {
                json: None,
                file: None,
                stdin: true,
            },
            serde_json::Map::new(),
            "Quality patch",
            &mut Cursor::new(b"[not-an-object]"),
        );
        assert!(matches!(malformed_stdin, Err(RefineError::InvalidInput(_))));

        let decoded = decode_config_input_with_reader(
            ConfigPayload {
                json: None,
                file: None,
                stdin: true,
            },
            serde_json::Map::new(),
            "Quality patch",
            &mut Cursor::new(br#"{"instructions":"line one\nline two"}"#),
        )
        .unwrap();
        assert_eq!(decoded["instructions"], "line one\nline two");
    }
}

fn parse_object(text: &str, source: &str) -> RefineResult<Value> {
    let value = serde_json::from_str::<Value>(text)
        .map_err(|error| RefineError::InvalidInput(format!("malformed {source}: {error}")))?;
    if !value.is_object() {
        return Err(RefineError::InvalidInput(format!(
            "{source} must contain a JSON object"
        )));
    }
    Ok(value)
}

fn insert_optional(values: &mut serde_json::Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        values.insert(key.to_string(), Value::String(value));
    }
}

pub(super) fn structured_config_error(error: RefineError) -> RefineError {
    let message = error.to_string();
    let code = if message.starts_with("missing active app:") {
        "missing_active_app"
    } else {
        match error.category() {
            crate::process::supervisor::errors::ErrorCategory::InvalidInput => "invalid_input",
            crate::process::supervisor::errors::ErrorCategory::NotFound => "not_found",
            crate::process::supervisor::errors::ErrorCategory::Unauthorized => "unauthorized",
            crate::process::supervisor::errors::ErrorCategory::Conflict => "conflict",
            crate::process::supervisor::errors::ErrorCategory::Degraded => "daemon_unavailable",
            crate::process::supervisor::errors::ErrorCategory::Io => "io_error",
            crate::process::supervisor::errors::ErrorCategory::Serialization => {
                "serialization_error"
            }
            crate::process::supervisor::errors::ErrorCategory::NotImplemented => "not_implemented",
        }
    };
    let encoded = serde_json::to_string_pretty(&json!({
        "error": {"code": code, "message": message}
    }))
    .unwrap_or_else(|_| error.to_string());
    match error.category() {
        crate::process::supervisor::errors::ErrorCategory::InvalidInput => {
            RefineError::InvalidInput(encoded)
        }
        crate::process::supervisor::errors::ErrorCategory::NotFound => {
            RefineError::NotFound(encoded)
        }
        crate::process::supervisor::errors::ErrorCategory::Unauthorized => {
            RefineError::Unauthorized(encoded)
        }
        crate::process::supervisor::errors::ErrorCategory::Conflict => {
            RefineError::Conflict(encoded)
        }
        crate::process::supervisor::errors::ErrorCategory::Degraded => {
            RefineError::Degraded(encoded)
        }
        crate::process::supervisor::errors::ErrorCategory::Io => RefineError::Io(encoded),
        crate::process::supervisor::errors::ErrorCategory::Serialization => {
            RefineError::Serialization(encoded)
        }
        crate::process::supervisor::errors::ErrorCategory::NotImplemented => {
            RefineError::NotImplemented(encoded)
        }
    }
}
