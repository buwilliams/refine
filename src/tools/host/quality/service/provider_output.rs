use super::*;

#[derive(Clone, Debug)]
pub(super) struct QualityTestDefinition {
    pub(super) test: String,
    pub(super) required_command: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct ObservedExecution {
    pub(super) process_id: String,
    pub(super) exit_code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

impl ObservedExecution {
    pub(super) fn evidence(&self) -> String {
        let mut evidence = format!(
            "Observed supervised process {} exited {}.",
            self.process_id,
            self.exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "without an exit code".to_string())
        );
        if let Some(stdout) = tail_text(&self.stdout, 2000) {
            evidence.push_str(&format!(" stdout: {stdout}"));
        }
        if let Some(stderr) = tail_text(&self.stderr, 2000) {
            evidence.push_str(&format!(" stderr: {stderr}"));
        }
        evidence
    }
}

pub(crate) fn parse_quality_provider_output(
    owner_id: &str,
    configured_tests: &[String],
    output: &str,
) -> RefineResult<QualityCheckResult> {
    let value = parse_json_value(output).ok_or_else(|| {
        RefineError::Serialization(
            "Quality agent did not return the required JSON evaluation".to_string(),
        )
    })?;
    let returned = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RefineError::Serialization(
                "Quality agent response is missing a results array".to_string(),
            )
        })?;
    let mut results = Vec::with_capacity(configured_tests.len());
    let mut diagnostics = Vec::new();
    for test in configured_tests {
        let matches = returned
            .iter()
            .filter(|item| item.get("test").and_then(Value::as_str) == Some(test.as_str()))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            let evidence = if matches.is_empty() {
                "Quality agent omitted this configured test.".to_string()
            } else {
                "Quality agent returned this configured test more than once.".to_string()
            };
            diagnostics.push(evidence.clone());
            results.push(QualityTestResult {
                test: test.clone(),
                status: "failed".to_string(),
                evidence,
                command: String::new(),
                process_id: None,
                exit_code: None,
            });
            continue;
        }
        let item = matches[0];
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let evidence = item
            .get("evidence")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let command = item
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let valid_status = matches!(status.as_str(), "passed" | "failed");
        if !valid_status {
            diagnostics.push(format!(
                "Quality agent returned invalid status {status:?} for {test:?}."
            ));
        }
        if evidence.is_empty() {
            diagnostics.push(format!("Quality agent returned no evidence for {test:?}."));
        }
        results.push(QualityTestResult {
            test: test.clone(),
            status: if valid_status && !evidence.is_empty() {
                status
            } else {
                "failed".to_string()
            },
            evidence,
            command,
            process_id: None,
            exit_code: None,
        });
    }
    if returned.len() != configured_tests.len() {
        let mismatch = format!(
            "Quality agent returned {} result(s) for {} configured test(s).",
            returned.len(),
            configured_tests.len()
        );
        diagnostics.push(mismatch.clone());
        if let Some(result) = results.first_mut() {
            result.status = "failed".to_string();
            result.evidence = if result.evidence.is_empty() {
                mismatch
            } else {
                format!("{} {mismatch}", result.evidence)
            };
        }
    }
    diagnostics.push(output.trim().to_string());
    Ok(QualityCheckResult {
        owner_id: owner_id.to_string(),
        ok: false,
        summary: value
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string(),
        results,
        diagnostics,
        candidate_commit: String::new(),
    })
}

pub(super) fn verify_candidate(
    root: &Path,
    expected_commit: &str,
    phase: &str,
) -> RefineResult<()> {
    let git = FileGitWorktreeService::new(root);
    let head = git.head_ref()?;
    let actual = head.commit.as_deref().unwrap_or("<unborn>");
    if actual != expected_commit {
        return Err(RefineError::Conflict(format!(
            "Quality {phase} check found candidate HEAD {actual}, expected recorded candidate {expected_commit}; user work was preserved"
        )));
    }
    let status = git.inspect(root.to_str().unwrap_or(""))?;
    if status.dirty_user_changes || !status.refine_owned_artifacts.is_empty() {
        return Err(RefineError::Conflict(format!(
            "Quality {phase} check found a dirty candidate index or worktree at {}; user work was preserved",
            root.display()
        )));
    }
    Ok(())
}

pub(super) fn quality_test_definitions(settings: &QualitySettings) -> Vec<QualityTestDefinition> {
    let mut definitions = settings
        .tests
        .iter()
        .map(|test| QualityTestDefinition {
            test: test.clone(),
            required_command: None,
        })
        .collect::<Vec<_>>();
    definitions.extend(
        settings
            .legacy_commands
            .iter()
            .map(|command| QualityTestDefinition {
                test: format!("Migrated Quality command passes: {command}"),
                required_command: Some(command.clone()),
            }),
    );
    definitions
}

pub(super) fn enabled_legacy_commands(settings: &Map<String, Value>) -> Vec<String> {
    let raw = settings
        .get("target_app_test_commands")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut commands = serde_json::from_str::<Value>(raw.trim())
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.get("enabled").and_then(Value::as_bool).unwrap_or(true))
        .filter_map(|item| {
            item.get("command")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    if commands.is_empty()
        && let Some(command) = settings
            .get("target_app_test_command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
    {
        commands.push(command.to_string());
    }
    normalize_commands(commands)
}

pub(super) fn legacy_quality_enabled(settings: &Map<String, Value>) -> bool {
    match settings.get("quality_enabled") {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or_default() != 0,
        Some(Value::String(value)) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => false,
    }
}

fn parse_json_value(raw: &str) -> Option<Value> {
    serde_json::from_str::<Value>(raw.trim()).ok().or_else(|| {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        serde_json::from_str::<Value>(&raw[start..=end]).ok()
    })
}

pub(super) fn normalize_tests(tests: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for test in tests {
        let test = test
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(1000)
            .collect::<String>();
        if !test.is_empty() && !normalized.contains(&test) {
            normalized.push(test);
        }
    }
    normalized
}

pub(super) fn normalize_commands(commands: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for command in commands {
        let command = command.trim().chars().take(4000).collect::<String>();
        if !command.is_empty() && !normalized.contains(&command) {
            normalized.push(command);
        }
    }
    normalized
}

pub(super) fn normalize_timing(value: &str) -> RefineResult<String> {
    match value.trim() {
        PRE_MERGE => Ok(PRE_MERGE.to_string()),
        POST_BUILD | "post_rebuild" => Ok(POST_BUILD.to_string()),
        _ => Err(RefineError::InvalidInput(
            "quality timing must be one of pre_merge, post_build".to_string(),
        )),
    }
}

pub(super) fn normalize_timing_lossy(value: &str) -> String {
    if matches!(value.trim(), POST_BUILD | "post_rebuild") {
        POST_BUILD.to_string()
    } else {
        PRE_MERGE.to_string()
    }
}

pub(super) fn shell_program_args(command: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    } else {
        (
            "sh".to_string(),
            vec!["-lc".to_string(), command.to_string()],
        )
    }
}

fn tail_text(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let chars = trimmed.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(max_chars);
    Some(chars[start..].iter().collect())
}

pub(super) fn quality_operation_log(
    owner_id: &str,
    severity: &str,
    message: &str,
    details: Option<Value>,
) -> LogEntry {
    LogEntry {
        datetime: now_timestamp(),
        severity: severity.to_string(),
        category: "quality".to_string(),
        message: message.to_string(),
        details: details.and_then(|value| value.as_object().cloned()),
        actions: Vec::new(),
        actor: Some("refine".to_string()),
        goal_id: Some(owner_id.to_string()),
    }
}

pub(super) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
