use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::process::supervisor::errors::RefineError;
use crate::process::supervisor::errors::RefineResult;

pub const SETTINGS_FILE: &str = "settings.json";
pub const GOVERNANCE_FILE: &str = "governance.json";
pub const GUIDANCE_FILE: &str = "guidance.json";
pub const REPORTERS_FILE: &str = "reporters.json";

pub trait ConfigService {
    fn load(&self) -> RefineResult<JsonObject>;
    fn validate(&self, config: &JsonObject) -> RefineResult<()>;
    fn merge(&self, base: JsonObject, overlay: JsonObject) -> RefineResult<JsonObject>;
}

#[derive(Clone, Debug)]
pub struct FileSettingsService {
    pub refine_dir: PathBuf,
}

impl FileSettingsService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.refine_dir.join(SETTINGS_FILE)
    }

    pub fn list_response(&self) -> RefineResult<serde_json::Value> {
        Ok(serde_json::json!({"settings": self.load()?}))
    }

    pub fn update(&self, body: &serde_json::Value) -> RefineResult<serde_json::Value> {
        let Some(updates) = body.as_object() else {
            return Err(RefineError::InvalidInput(
                "expected an object of {key: value}".to_string(),
            ));
        };
        if updates.is_empty() {
            return Err(RefineError::InvalidInput(
                "expected an object of {key: value}".to_string(),
            ));
        }
        let mut current = self.load()?;
        let allowed = allowed_settings();
        for (key, value) in updates {
            if !allowed.contains(key.as_str()) {
                return Err(RefineError::InvalidInput(format!("unknown setting: {key}")));
            }
            current.insert(key.clone(), Value::String(normalize_setting(key, value)?));
        }
        self.validate(&current)?;
        self.write(&current)?;
        Ok(serde_json::json!({"ok": true, "settings": current}))
    }

    fn write(&self, settings: &JsonObject) -> RefineResult<()> {
        if let Some(parent) = self.path().parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create settings directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let encoded = serde_json::to_string_pretty(settings).map_err(|error| {
            RefineError::Serialization(format!("failed to encode settings: {error}"))
        })?;
        let path = self.path();
        fs::write(&path, format!("{encoded}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to write settings {}: {error}",
                path.display()
            ))
        })
    }
}

impl ConfigService for FileSettingsService {
    fn load(&self) -> RefineResult<JsonObject> {
        let path = self.path();
        if !path.exists() {
            return Ok(default_settings());
        }
        let bytes = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read settings {}: {error}",
                path.display()
            ))
        })?;
        let raw = serde_json::from_str::<serde_json::Value>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse settings {}: {error}",
                path.display()
            ))
        })?;
        let Some(object) = raw.as_object() else {
            return Ok(default_settings());
        };
        let mut settings = default_settings();
        let mut migrated = false;
        for (key, value) in object {
            if allowed_settings().contains(key.as_str()) {
                settings.insert(key.clone(), Value::String(normalize_setting(key, value)?));
            } else if let Some(new_key) = legacy_setting_key(key) {
                settings.insert(
                    new_key.to_string(),
                    Value::String(normalize_setting(new_key, value)?),
                );
                migrated = true;
            }
        }
        if migrated {
            self.write(&settings)?;
        }
        Ok(settings)
    }

    fn validate(&self, config: &JsonObject) -> RefineResult<()> {
        let allowed = allowed_settings();
        for key in config.keys() {
            if !allowed.contains(key.as_str()) {
                return Err(RefineError::InvalidInput(format!("unknown setting: {key}")));
            }
        }
        Ok(())
    }

    fn merge(&self, mut base: JsonObject, overlay: JsonObject) -> RefineResult<JsonObject> {
        for (key, value) in overlay {
            base.insert(key, value);
        }
        self.validate(&base)?;
        Ok(base)
    }
}

#[derive(Clone, Debug)]
pub struct FileGovernanceService {
    pub refine_dir: PathBuf,
}

impl FileGovernanceService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn load(&self) -> RefineResult<Value> {
        let mut value = read_json_or_default(
            self.refine_dir.join(GOVERNANCE_FILE),
            json!({"product": "", "constitution": "", "rules": []}),
        )?;
        normalize_governance(&mut value);
        Ok(value)
    }

    pub fn save(&self, body: &Value) -> RefineResult<Value> {
        let mut current = self.load()?;
        if let Some(product) = body.get("product").and_then(|value| value.as_str()) {
            current["product"] = Value::String(product.trim().to_string());
        }
        if let Some(constitution) = body.get("constitution").and_then(|value| value.as_str()) {
            current["constitution"] = Value::String(constitution.trim().to_string());
        }
        if let Some(rules) = body.get("rules") {
            if !rules.is_array() {
                return Err(RefineError::InvalidInput(
                    "rules must be a list".to_string(),
                ));
            }
            current["rules"] = normalize_rules(rules);
        }
        normalize_governance(&mut current);
        write_json(self.refine_dir.join(GOVERNANCE_FILE), &current)?;
        Ok(current)
    }

    pub fn generate_rules(&self, body: &Value) -> RefineResult<Value> {
        let product = body
            .get("product")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        let constitution = body
            .get("constitution")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if product.is_empty() || constitution.is_empty() {
            return Err(RefineError::InvalidInput(
                "product and constitution are required".to_string(),
            ));
        }
        Ok(json!({
            "ok": true,
            "rules": [
                governance_rule("Keep implementation aligned with the documented product intent.", "generated"),
                governance_rule("Respect the project constitution before adding new behavior.", "generated")
            ],
            "raw": ""
        }))
    }
}

#[derive(Clone, Debug)]
pub struct FileGuidanceService {
    pub refine_dir: PathBuf,
}

impl FileGuidanceService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn list(&self) -> RefineResult<Value> {
        let value = read_json_or_default(self.refine_dir.join(GUIDANCE_FILE), json!([]))?;
        Ok(json!({"guidance": normalize_guidance_list(&value)}))
    }

    pub fn update(&self, body: &Value) -> RefineResult<Value> {
        let Some(items) = body.get("guidance") else {
            return Err(RefineError::InvalidInput(
                "guidance must be a list".to_string(),
            ));
        };
        if !items.is_array() {
            return Err(RefineError::InvalidInput(
                "guidance must be a list".to_string(),
            ));
        }
        let guidance = normalize_guidance_list(items);
        write_json(self.refine_dir.join(GUIDANCE_FILE), &guidance)?;
        Ok(json!({"guidance": guidance}))
    }
}

#[derive(Clone, Debug)]
pub struct FileReporterService {
    pub refine_dir: PathBuf,
}

impl FileReporterService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
        }
    }

    pub fn list(&self) -> RefineResult<Value> {
        Ok(json!({"reporters": self.load_reporters()?}))
    }

    pub fn create(&self, name: &str) -> RefineResult<Value> {
        let clean = normalize_reporter_name(name)?;
        let mut reporters = self.load_reporters()?;
        if let Some(existing) = reporters.iter().find(|reporter| {
            reporter.get("name").and_then(|value| value.as_str()) == Some(clean.as_str())
        }) {
            return Ok(json!({"reporter": existing}));
        }
        let next_id = reporters
            .iter()
            .filter_map(|reporter| reporter.get("id").and_then(|value| value.as_u64()))
            .max()
            .unwrap_or(0)
            + 1;
        let reporter = json!({"id": next_id, "name": clean, "created": now_timestamp()});
        reporters.push(reporter.clone());
        self.save_reporters(&reporters)?;
        Ok(json!({"reporter": reporter}))
    }

    pub fn rename(&self, id: u64, name: &str) -> RefineResult<Value> {
        let clean = normalize_reporter_name(name)?;
        let mut reporters = self.load_reporters()?;
        let Some(reporter) = reporters
            .iter_mut()
            .find(|reporter| reporter.get("id").and_then(|value| value.as_u64()) == Some(id))
        else {
            return Err(RefineError::NotFound(format!(
                "Reporter {id} was not found"
            )));
        };
        let old = reporter
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        reporter["name"] = Value::String(clean.clone());
        self.save_reporters(&reporters)?;
        if old != clean {
            rewrite_reporter_references(&self.refine_dir, &old, &clean)?;
        }
        Ok(json!({"ok": true, "old": old, "new": clean}))
    }

    pub fn delete(&self, id: u64) -> RefineResult<Value> {
        let mut reporters = self.load_reporters()?;
        let len = reporters.len();
        reporters
            .retain(|reporter| reporter.get("id").and_then(|value| value.as_u64()) != Some(id));
        if reporters.len() == len {
            return Err(RefineError::NotFound(format!(
                "Reporter {id} was not found"
            )));
        }
        self.save_reporters(&reporters)?;
        Ok(json!({"ok": true}))
    }

    pub fn merge(&self, id: u64, target_id: u64) -> RefineResult<Value> {
        if id == target_id {
            return Err(RefineError::InvalidInput(
                "cannot merge a reporter into itself".to_string(),
            ));
        }
        let reporters = self.load_reporters()?;
        let old = reporters
            .iter()
            .find(|reporter| reporter.get("id").and_then(|value| value.as_u64()) == Some(id))
            .and_then(|reporter| reporter.get("name").and_then(|value| value.as_str()))
            .unwrap_or("")
            .to_string();
        let new = reporters
            .iter()
            .find(|reporter| reporter.get("id").and_then(|value| value.as_u64()) == Some(target_id))
            .and_then(|reporter| reporter.get("name").and_then(|value| value.as_str()))
            .unwrap_or("")
            .to_string();
        if old.is_empty() || new.is_empty() {
            return Err(RefineError::NotFound("Reporter was not found".to_string()));
        }
        self.delete(id)?;
        rewrite_reporter_references(&self.refine_dir, &old, &new)?;
        Ok(json!({"ok": true, "old": old, "new": new}))
    }

    fn load_reporters(&self) -> RefineResult<Vec<Value>> {
        let value = read_json_or_default(self.refine_dir.join(REPORTERS_FILE), json!([]))?;
        let reporters = normalize_reporters(&value);
        if reporters.is_empty() {
            let seeded = self.seed_reporters_from_gap_rounds()?;
            if !seeded.is_empty() {
                self.save_reporters(&seeded)?;
                return Ok(seeded);
            }
        }
        Ok(reporters)
    }

    fn save_reporters(&self, reporters: &[Value]) -> RefineResult<()> {
        write_json(
            self.refine_dir.join(REPORTERS_FILE),
            &Value::Array(reporters.to_vec()),
        )
    }

    fn seed_reporters_from_gap_rounds(&self) -> RefineResult<Vec<Value>> {
        let mut names = BTreeSet::new();
        collect_reporter_names(&self.refine_dir.join("gaps"), "gap.json", &mut names)?;
        collect_reporter_names(
            &self.refine_dir.join("features"),
            "feature.json",
            &mut names,
        )?;
        let now = now_timestamp();
        Ok(names
            .into_iter()
            .enumerate()
            .map(|(idx, name)| json!({"id": idx + 1, "name": name, "created": now}))
            .collect())
    }
}

fn default_settings() -> JsonObject {
    let mut settings = JsonObject::new();
    for (key, value) in [
        ("parallel_run_cap", "2"),
        ("parallel_per_node_cap", "2"),
        ("parallel_per_provider_cap", "2"),
        ("parallel_per_target_app_cap", "2"),
        ("branch_name_pattern", "refine/{gap_id}"),
        ("agent_idle_timeout_seconds", "900"),
        ("agent_hard_cap_seconds", "7200"),
        ("agent_limit_pause_seconds", "60"),
        ("worker_memory_limit_mb", ""),
        ("ui_memory_limit_mb", ""),
        ("worker_cpu_priority", "normal"),
        ("resource_isolation_mode", "process_group"),
        ("chat_idle_timeout_seconds", "300"),
        ("backlog_promote_after_seconds", "3600"),
        ("project_update_pulse_interval_seconds", "300"),
        ("file_browser_ignore_patterns", ""),
        ("agent_subpath", ""),
        ("merge_target_branch", "main"),
        ("quality_enabled", "0"),
        ("quality_timing", "pre_merge"),
        ("allowed_commands", ""),
        ("agent_cli", "claude"),
        ("paused", "0"),
        ("target_app_start_instructions", ""),
        ("target_app_stop_instructions", ""),
        ("target_app_health_url", ""),
        ("target_app_url", ""),
        ("target_app_start_command", ""),
        ("target_app_stop_command", ""),
        ("target_app_build_command", ""),
        ("target_app_test_command", ""),
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

fn allowed_settings() -> BTreeSet<&'static str> {
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
        "project_update_pulse_interval_seconds",
        "file_browser_ignore_patterns",
        "agent_subpath",
        "merge_target_branch",
        "quality_enabled",
        "quality_timing",
        "allowed_commands",
        "agent_cli",
        "paused",
        "target_app_start_instructions",
        "target_app_stop_instructions",
        "target_app_health_url",
        "target_app_url",
        "target_app_start_command",
        "target_app_stop_command",
        "target_app_build_command",
        "target_app_test_command",
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

fn legacy_setting_key(key: &str) -> Option<&'static str> {
    match key {
        "target_app_rebuild_command" => Some("target_app_build_command"),
        "target_app_rebuild_timeout_seconds" => Some("target_app_build_timeout_seconds"),
        "target_app_auto_rebuild" => Some("target_app_auto_build"),
        "target_app_auto_rebuild_hour_utc" => Some("target_app_auto_build_hour_utc"),
        _ => None,
    }
}

fn normalize_setting(key: &str, value: &Value) -> RefineResult<String> {
    match key {
        "agent_cli" => {
            let choice = as_string(value).trim().to_ascii_lowercase();
            if matches!(
                choice.as_str(),
                "claude" | "codex" | "gemini" | "copilot" | "smoke-ai"
            ) {
                Ok(choice)
            } else {
                Err(RefineError::InvalidInput(
                    "agent_cli must be one of claude, codex, gemini, copilot, smoke-ai".to_string(),
                ))
            }
        }
        "quality_enabled" | "paused" => {
            Ok(if value_is_truthy(value) { "1" } else { "0" }.to_string())
        }
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
        "parallel_run_cap"
        | "parallel_per_node_cap"
        | "parallel_per_provider_cap"
        | "parallel_per_target_app_cap" => normalize_range(key, value, 1, 100),
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
                    | "project_update_pulse_interval_seconds"
            ) =>
        {
            normalize_integer(key, value)
        }
        _ => Ok(as_string(value).trim().to_string()),
    }
}

fn normalize_range(key: &str, value: &Value, min: i64, max: i64) -> RefineResult<String> {
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

fn normalize_integer(key: &str, value: &Value) -> RefineResult<String> {
    let number = as_string(value)
        .trim()
        .parse::<i64>()
        .map_err(|_| RefineError::InvalidInput(format!("{key} must be an integer")))?;
    Ok(number.to_string())
}

fn as_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn value_is_truthy(value: &Value) -> bool {
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

fn read_json_or_default(path: PathBuf, default: Value) -> RefineResult<Value> {
    if !path.exists() {
        return Ok(default);
    }
    let bytes = fs::read_to_string(&path)
        .map_err(|error| RefineError::Io(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_str(&bytes).map_err(|error| {
        RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
    })
}

fn write_json(path: PathBuf, value: &Value) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let encoded = serde_json::to_string_pretty(value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    fs::write(&path, format!("{encoded}\n"))
        .map_err(|error| RefineError::Io(format!("failed to write {}: {error}", path.display())))
}

fn normalize_governance(value: &mut Value) {
    if !value.is_object() {
        *value = json!({"product": "", "constitution": "", "rules": []});
    }
    let configured = value
        .get("product")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .is_empty()
        == false
        && value
            .get("constitution")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .is_empty()
            == false;
    let rules = normalize_rules(value.get("rules").unwrap_or(&Value::Array(Vec::new())));
    value["product"] = Value::String(
        value
            .get("product")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
    );
    value["constitution"] = Value::String(
        value
            .get("constitution")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
    );
    value["rules"] = rules;
    value["configured"] = Value::Bool(configured);
}

fn normalize_rules(value: &Value) -> Value {
    let mut rules = Vec::new();
    let mut seen = BTreeSet::new();
    for item in value.as_array().into_iter().flatten() {
        let text = item
            .get("text")
            .and_then(|value| value.as_str())
            .or_else(|| item.as_str())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        let mut id = item
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() || seen.contains(&id) {
            id = format!("rule-{}", rules.len() + 1);
        }
        seen.insert(id.clone());
        let created = item
            .get("created")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(now_timestamp);
        let updated = item
            .get("updated")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(now_timestamp);
        rules.push(json!({
            "id": id,
            "text": text.chars().take(500).collect::<String>(),
            "created": created,
            "updated": updated,
            "source": item.get("source").and_then(|value| value.as_str()).unwrap_or("manual")
        }));
    }
    Value::Array(rules)
}

fn governance_rule(text: &str, source: &str) -> Value {
    json!({
        "id": format!("rule-{}", Utc::now().timestamp_millis()),
        "text": text,
        "created": now_timestamp(),
        "updated": now_timestamp(),
        "source": source
    })
}

fn normalize_guidance_list(value: &Value) -> Value {
    let items = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.trim();
            let rule = item.get("rule")?.as_str()?.trim();
            let instructions = item.get("instructions")?.as_str()?.trim();
            if name.is_empty() || rule.is_empty() || instructions.is_empty() {
                return None;
            }
            Some(json!({
                "name": name,
                "rule": rule,
                "instructions": instructions,
                "enabled": item.get("enabled").and_then(|value| value.as_bool()).unwrap_or(true)
            }))
        })
        .collect::<Vec<_>>();
    Value::Array(items)
}

fn normalize_reporters(value: &Value) -> Vec<Value> {
    let mut reporters = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id").and_then(|value| value.as_u64())?;
            let name = item.get("name").and_then(|value| value.as_str())?.trim();
            if name.is_empty() {
                return None;
            }
            Some(json!({
                "id": id,
                "name": name,
                "created": item.get("created").and_then(|value| value.as_str()).unwrap_or("")
            }))
        })
        .collect::<Vec<_>>();
    reporters.sort_by(|a, b| {
        a.get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_lowercase()
            .cmp(
                &b.get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_lowercase(),
            )
    });
    reporters
}

fn collect_reporter_names(
    path: &Path,
    file_name: &str,
    names: &mut BTreeSet<String>,
) -> RefineResult<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read reporter directory {}: {error}",
            path.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read Gap directory entry {}: {error}",
                path.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_reporter_names(&path, file_name, names)?;
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) != Some(file_name) {
            continue;
        }
        let value = read_json_or_default(path.clone(), json!({}))?;
        collect_reporter_name(value.get("reporter"), names);
        collect_reporter_name(value.get("assignee"), names);
        for round in value
            .get("rounds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect_reporter_name(round.get("reporter"), names);
            collect_reporter_name(round.get("assignee"), names);
        }
    }
    Ok(())
}

fn collect_reporter_name(value: Option<&Value>, names: &mut BTreeSet<String>) {
    if let Some(name) = value.and_then(Value::as_str) {
        let clean = name.trim();
        if !clean.is_empty() {
            names.insert(clean.to_string());
        }
    }
}

fn rewrite_reporter_references(refine_dir: &Path, old: &str, new: &str) -> RefineResult<()> {
    if old.trim().is_empty() || old == new {
        return Ok(());
    }
    rewrite_reporter_references_in_tree(&refine_dir.join("gaps"), "gap.json", old, new)?;
    rewrite_reporter_references_in_tree(&refine_dir.join("features"), "feature.json", old, new)
}

fn rewrite_reporter_references_in_tree(
    path: &Path,
    file_name: &str,
    old: &str,
    new: &str,
) -> RefineResult<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read reporter directory {}: {error}",
            path.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read reporter directory entry {}: {error}",
                path.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            rewrite_reporter_references_in_tree(&path, file_name, old, new)?;
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) != Some(file_name) {
            continue;
        }
        let mut value = read_json_or_default(path.clone(), json!({}))?;
        if rewrite_reporter_reference_value(&mut value, old, new) {
            write_json(path, &value)?;
        }
    }
    Ok(())
}

fn rewrite_reporter_reference_value(value: &mut Value, old: &str, new: &str) -> bool {
    let mut changed = false;
    if let Some(object) = value.as_object_mut() {
        changed |= rewrite_reporter_field(object.get_mut("reporter"), old, new);
        changed |= rewrite_reporter_field(object.get_mut("assignee"), old, new);
        if let Some(rounds) = object.get_mut("rounds").and_then(Value::as_array_mut) {
            for round in rounds {
                if let Some(round_object) = round.as_object_mut() {
                    changed |= rewrite_reporter_field(round_object.get_mut("reporter"), old, new);
                    changed |= rewrite_reporter_field(round_object.get_mut("assignee"), old, new);
                }
            }
        }
    }
    changed
}

fn rewrite_reporter_field(value: Option<&mut Value>, old: &str, new: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    if value.as_str() == Some(old) {
        *value = Value::String(new.to_string());
        return true;
    }
    false
}

fn normalize_reporter_name(name: &str) -> RefineResult<String> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err(RefineError::InvalidInput("name is required".to_string()));
    }
    if clean.chars().any(|ch| ch.is_control()) || clean.len() > 120 {
        return Err(RefineError::InvalidInput(
            "invalid reporter name".to_string(),
        ));
    }
    Ok(clean.to_string())
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_settings_service_lists_defaults_and_persists_updates() {
        let temp_root = unique_temp_dir("settings");
        let refine_dir = temp_root.join(".refine");
        let service = FileSettingsService::new(&refine_dir);

        assert_eq!(service.load().unwrap()["agent_cli"], "claude");
        let updated = service
            .update(&serde_json::json!({
                "agent_cli": "smoke-ai",
                "parallel_run_cap": 4,
                "paused": true,
                "target_app_env_json": {"PORT": 3000}
            }))
            .unwrap();
        assert_eq!(updated["settings"]["agent_cli"], "smoke-ai");
        assert_eq!(updated["settings"]["parallel_run_cap"], "4");
        assert_eq!(updated["settings"]["paused"], "1");
        assert!(service.path().exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_settings_service_migrates_target_app_build_settings() {
        let temp_root = unique_temp_dir("settings-build-migration");
        let refine_dir = temp_root.join(".refine");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::write(
            refine_dir.join(SETTINGS_FILE),
            serde_json::to_string_pretty(&json!({
                "target_app_rebuild_command": "npm run build",
                "target_app_rebuild_timeout_seconds": "45",
                "target_app_auto_rebuild": "daily",
                "target_app_auto_rebuild_hour_utc": "4",
                "quality_timing": "post_rebuild"
            }))
            .unwrap(),
        )
        .unwrap();

        let service = FileSettingsService::new(&refine_dir);
        let settings = service.load().unwrap();
        assert_eq!(settings["target_app_build_command"], "npm run build");
        assert_eq!(settings["target_app_build_timeout_seconds"], "45");
        assert_eq!(settings["target_app_auto_build"], "daily");
        assert_eq!(settings["target_app_auto_build_hour_utc"], "4");
        assert_eq!(settings["quality_timing"], "post_build");
        let written = fs::read_to_string(service.path()).unwrap();
        assert!(written.contains("target_app_build_command"));
        assert!(!written.contains("target_app_rebuild_command"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_project_config_services_persist_governance_guidance_and_reporters() {
        let temp_root = unique_temp_dir("project-config");
        let refine_dir = temp_root.join(".refine");

        let governance = FileGovernanceService::new(&refine_dir);
        let saved = governance
            .save(&json!({
                "product": "Refine",
                "constitution": "Be useful",
                "rules": [{"text": "No regressions"}]
            }))
            .unwrap();
        assert_eq!(saved["configured"], true);
        assert_eq!(saved["rules"].as_array().unwrap().len(), 1);
        assert_eq!(
            governance
                .generate_rules(&json!({"product": "Refine", "constitution": "Be useful"}))
                .unwrap()["ok"],
            true
        );

        let guidance = FileGuidanceService::new(&refine_dir);
        let guidance_payload = guidance
            .update(&json!({"guidance": [{
                "name": "Accessibility",
                "rule": "When UI changes",
                "instructions": "Check keyboard behavior",
                "enabled": true
            }]}))
            .unwrap();
        assert_eq!(guidance_payload["guidance"].as_array().unwrap().len(), 1);

        let gap_dir = refine_dir.join("gaps/GA/P1");
        let feature_dir = refine_dir.join("features/FE/A1");
        fs::create_dir_all(&gap_dir).unwrap();
        fs::create_dir_all(&feature_dir).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            serde_json::to_string_pretty(&json!({
                "id": "GAP1",
                "reporter": "Buddy",
                "rounds": [
                    {"reporter": "Alex", "assignee": "Buddy", "actual": "A", "target": "B"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            feature_dir.join("feature.json"),
            serde_json::to_string_pretty(&json!({
                "id": "FEA1",
                "reporter": "Buddy",
                "assignee": "Buddy"
            }))
            .unwrap(),
        )
        .unwrap();

        let reporters = FileReporterService::new(&refine_dir);
        let buddy = reporters.create("Buddy").unwrap()["reporter"].clone();
        let alex = reporters.create("Alex").unwrap()["reporter"].clone();
        reporters
            .rename(buddy["id"].as_u64().unwrap(), "Buddy Williams")
            .unwrap();
        let merged = reporters
            .merge(buddy["id"].as_u64().unwrap(), alex["id"].as_u64().unwrap())
            .unwrap();
        assert_eq!(merged["ok"], true);
        assert_eq!(
            reporters.list().unwrap()["reporters"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let gap: Value =
            serde_json::from_str(&fs::read_to_string(gap_dir.join("gap.json")).unwrap()).unwrap();
        assert_eq!(gap["reporter"], "Alex");
        assert_eq!(gap["rounds"][0]["reporter"], "Alex");
        assert_eq!(gap["rounds"][0]["assignee"], "Alex");
        let feature: Value =
            serde_json::from_str(&fs::read_to_string(feature_dir.join("feature.json")).unwrap())
                .unwrap();
        assert_eq!(feature["reporter"], "Alex");
        assert_eq!(feature["assignee"], "Alex");

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}
