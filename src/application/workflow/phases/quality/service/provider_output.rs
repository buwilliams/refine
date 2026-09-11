use super::*;

const QUALITY_COMMAND_HARNESS_FAULT_PREFIX: &str = "Quality command harness fault:";

// Historical errors remain classifiable; Refine no longer launches commands
// supplied in Quality reports.
pub(crate) fn is_quality_harness_fault(error: &RefineError) -> bool {
    matches!(
        error,
        RefineError::Degraded(message)
            if message.starts_with(QUALITY_COMMAND_HARNESS_FAULT_PREFIX)
    )
}

pub(crate) fn is_quality_output_contract_fault(error: &RefineError) -> bool {
    matches!(error, RefineError::Serialization(message) if message.starts_with("Skill output contract failed"))
        || matches!(
            error,
            RefineError::StructuredOutput(inner)
                if inner.label() == QualityEvaluationWire::LABEL
        )
}

pub(crate) fn record_quality_provider_attempt(
    request: &QualityCheckRequest,
    attempt: &QualityProviderAttempt,
) -> RefineResult<()> {
    let Some(operation_id) = request
        .process_metadata
        .get("operation_id")
        .and_then(Value::as_str)
    else {
        // Direct trait callers have no durable operation. Their returned result still carries
        // the full attempt history; workflow calls always take the durable path below.
        return Ok(());
    };
    let Some(runtime_root) = request
        .process_metadata
        .get("runtime_root")
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    FileOperationRegistry::new(runtime_root).append_log(
        operation_id,
        quality_operation_log(
            &request.owner_id,
            if attempt.accepted { "info" } else { "warning" },
            if attempt.accepted {
                "Quality provider response satisfied the structured output contract"
            } else {
                "Quality provider response required structured output repair"
            },
            Some(json!({"provider_attempt": attempt})),
        ),
    )?;
    Ok(())
}

pub(crate) fn parse_quality_provider_output(
    owner_id: &str,
    _configured_tests: &[String],
    output: &str,
) -> RefineResult<QualityCheckResult> {
    let evaluation = QualityEvaluationWire::decode(output)?;
    Ok(QualityCheckResult {
        owner_id: owner_id.into(),
        ok: evaluation.ok,
        summary: evaluation.summary,
        results: evaluation
            .results
            .into_iter()
            .map(|item| QualityTestResult {
                test: item.test,
                status: item.status,
                evidence: item.evidence,
                command: item.command,
                process_id: None,
                exit_code: None,
            })
            .collect(),
        diagnostics: vec![output.to_string()],
        candidate_commit: String::new(),
        checked_at: None,
        provider_attempts: Vec::new(),
        skill_evidence: None,
    })
}

pub(crate) fn verify_candidate(
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
    if !status.is_pristine() {
        return Err(RefineError::Conflict(format!(
            "Quality {phase} check found a dirty candidate index or worktree at {}; user work was preserved",
            root.display()
        )));
    }
    Ok(())
}

pub(crate) fn enabled_legacy_commands(settings: &Map<String, Value>) -> Vec<String> {
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

pub(crate) fn legacy_quality_enabled(settings: &Map<String, Value>) -> bool {
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

pub(crate) fn normalize_tests(tests: Vec<String>) -> Vec<String> {
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

pub(crate) fn normalize_commands(commands: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for command in commands {
        let command = command.trim().chars().take(4000).collect::<String>();
        if !command.is_empty() && !normalized.contains(&command) {
            normalized.push(command);
        }
    }
    normalized
}

pub(crate) fn quality_operation_log(
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

pub(crate) fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
