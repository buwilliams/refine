use super::config_input::*;
use super::*;

use crate::application::workflow::phases::quality::{FileQualityService, QualitySettingsPatch};
use crate::infrastructure::process::supervisor::config::{
    FileGovernanceService, FileGuidanceService, FileSettingsService,
};

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
            serde_json::from_value::<QualitySettingsPatch>(body.clone()).map_err(|error| {
                RefineError::InvalidInput(format!("invalid Quality settings body: {error}"))
            })?;
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
            FileGovernanceService::validate_patch(&body)?;
            if body.get("rules").is_some() && body.get("rules_revision").is_none() {
                let current = read_domain(ConfigDomain::Governance, target_root.clone())?;
                body["rules_revision"] = json!(current["rules_revision"].as_u64().unwrap_or(0));
            }
            FileGovernanceService::validate_patch(&body)?;
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
            validate_governance_generation(&supplied)?;
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
    match (method, id.as_deref()) {
        ("POST", None) => FileGuidanceService::validate_add(&body)?,
        ("PATCH", Some(_)) => FileGuidanceService::validate_edit(&body)?,
        ("DELETE", Some(_)) => FileGuidanceService::validate_remove(&body)?,
        _ => unreachable!("unsupported Guidance mutation"),
    }
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
