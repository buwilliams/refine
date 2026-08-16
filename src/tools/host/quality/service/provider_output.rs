use super::*;

#[derive(Clone, Debug)]
pub(super) struct QualityTestDefinition {
    pub(super) test: String,
    pub(super) required_command: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct ObservedExecution {
    pub(super) process_id: String,
    pub(super) shell: String,
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

    pub(super) fn shell_parser_aborted(&self) -> bool {
        if self.exit_code != Some(2) {
            return false;
        }
        let stderr = self.stderr.to_ascii_lowercase();
        let shell = self.shell.to_ascii_lowercase();
        let shell_name = Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&shell);
        let emitted_by_shell = stderr.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with(&format!("{shell}:")) || line.starts_with(&format!("{shell_name}:"))
        });
        emitted_by_shell
            && [
                "syntax error",
                "redirection unexpected",
                "unexpected token",
                "unexpected end of file",
                "bad substitution",
            ]
            .iter()
            .any(|marker| stderr.contains(marker))
    }
}

const QUALITY_COMMAND_HARNESS_FAULT_PREFIX: &str = "Quality command harness fault:";

pub(super) fn quality_command_harness_fault(
    command: &str,
    observed: &ObservedExecution,
) -> RefineError {
    RefineError::Degraded(format!(
        "{QUALITY_COMMAND_HARNESS_FAULT_PREFIX} supervised shell {} could not parse {command:?}. {}",
        observed.shell,
        observed.evidence()
    ))
}

pub(crate) fn is_quality_harness_fault(error: &RefineError) -> bool {
    matches!(
        error,
        RefineError::Degraded(message)
            if message.starts_with(QUALITY_COMMAND_HARNESS_FAULT_PREFIX)
    )
}

pub(crate) fn is_quality_output_contract_fault(error: &RefineError) -> bool {
    matches!(
        error,
        RefineError::StructuredOutput(inner)
            if inner.label() == QualityEvaluationWire::LABEL
    )
}

pub(super) fn record_quality_provider_attempt(
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
    configured_tests: &[String],
    output: &str,
) -> RefineResult<QualityCheckResult> {
    // Transport and schema faults are repairable contract errors; everything
    // past this decode is fail-safe coercion that never rejects the response.
    let evaluation = QualityEvaluationWire::decode(output)?;
    let returned = &evaluation.results;
    let mut results = Vec::with_capacity(configured_tests.len());
    let mut diagnostics = Vec::new();
    for test in configured_tests {
        let matches = returned
            .iter()
            .filter(|item| item.test == *test)
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
        let status = item.status.trim().to_ascii_lowercase();
        let evidence = item.evidence.trim().to_string();
        let command = item.command.trim().to_string();
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
        summary: evaluation.summary.trim().to_string(),
        results,
        diagnostics,
        candidate_commit: String::new(),
        checked_at: None,
        provider_attempts: Vec::new(),
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
    if !status.is_pristine() {
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

pub(super) fn shell_program_args(command: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (
            find_bash().unwrap_or_else(|| "sh".to_string()),
            vec!["-c".to_string(), command.to_string()],
        )
    }
}

#[cfg(not(windows))]
fn find_bash() -> Option<String> {
    let path_candidates = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join("bash"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    path_candidates
        .into_iter()
        .chain([PathBuf::from("/bin/bash"), PathBuf::from("/usr/bin/bash")])
        .find(|path| executable_file(path))
        .map(|path| path.display().to_string())
}

#[cfg(unix)]
fn executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(all(not(windows), not(unix)))]
fn executable_file(path: &Path) -> bool {
    path.is_file()
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
