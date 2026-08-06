use super::*;

const MAX_SUMMARY_CHARS: usize = 400;
const MAX_TEST_CHARS: usize = 140;
const MAX_CAUSE_CHARS: usize = 190;
const GENERIC_FAILURE_SUMMARY: &str = "Quality failed: no valid structured failure evidence was recorded; inspect Details and supervised logs.";

pub(crate) fn quality_failure_summary(result: &QualityCheckResult) -> String {
    let failed = result
        .results
        .iter()
        .enumerate()
        .filter(|(_, result)| result.status.trim() != "passed" || result.exit_code != Some(0))
        .collect::<Vec<_>>();
    let additional = failed.len().saturating_sub(1);

    let Some((_, selected)) = failed
        .into_iter()
        .max_by_key(|(index, result)| (failure_signal(result), std::cmp::Reverse(*index)))
    else {
        return diagnostic_fallback(&result.diagnostics)
            .unwrap_or_else(|| GENERIC_FAILURE_SUMMARY.to_string());
    };

    let test = bounded_text(&selected.test, MAX_TEST_CHARS);
    if test.is_empty() {
        return diagnostic_fallback(&result.diagnostics)
            .unwrap_or_else(|| GENERIC_FAILURE_SUMMARY.to_string());
    }
    let cause = result_cause(selected)
        .or_else(|| first_diagnostic(&result.diagnostics))
        .unwrap_or_else(|| "no usable observed cause was recorded".to_string());
    let suffix = match additional {
        0 => String::new(),
        1 => " 1 additional test failed.".to_string(),
        count => format!(" {count} additional tests failed."),
    };
    bounded_text(
        &format!(
            "Quality failed: \u{201c}{test}\u{201d} \u{2014} {}.{suffix}",
            bounded_text(&cause, MAX_CAUSE_CHARS).trim_end_matches('.')
        ),
        MAX_SUMMARY_CHARS,
    )
}

pub(crate) fn quality_error_summary(error: &RefineError) -> String {
    let harness_fault = is_quality_harness_fault(error);
    let error = bounded_text(&error.to_string(), MAX_SUMMARY_CHARS);
    if error.is_empty() {
        return GENERIC_FAILURE_SUMMARY.to_string();
    }
    if harness_fault {
        return bounded_text(&error, MAX_SUMMARY_CHARS);
    }
    bounded_text(&format!("Quality failed: {error}"), MAX_SUMMARY_CHARS)
}

fn failure_signal(result: &QualityTestResult) -> u8 {
    match result.exit_code {
        Some(code) if code != 0 => 5,
        None if result.process_id.is_some() => 4,
        None if result.command.trim().is_empty() => 3,
        Some(0) if result.status.trim() != "passed" => 2,
        _ if !result.evidence.trim().is_empty() => 1,
        _ => 0,
    }
}

fn result_cause(result: &QualityTestResult) -> Option<String> {
    match result.exit_code {
        Some(code) if code != 0 => Some(format!("supervised command exited with code {code}")),
        None if result.process_id.is_some() => {
            Some("supervised command ended without an exit code".to_string())
        }
        None if result.command.trim().is_empty() => normalized_optional(&result.evidence)
            .or_else(|| Some("no supervised command execution was observed".to_string())),
        None => Some("no supervised command execution was observed".to_string()),
        Some(0) if result.status.trim() != "passed" => {
            Some("Quality recorded a failed evaluation despite a zero command exit".to_string())
        }
        _ => normalized_optional(&result.evidence),
    }
}

fn diagnostic_fallback(diagnostics: &[String]) -> Option<String> {
    first_diagnostic(diagnostics)
        .map(|cause| bounded_text(&format!("Quality failed: {cause}"), MAX_SUMMARY_CHARS))
}

fn first_diagnostic(diagnostics: &[String]) -> Option<String> {
    diagnostics
        .iter()
        .find_map(|diagnostic| normalized_optional(diagnostic))
}

fn normalized_optional(value: &str) -> Option<String> {
    let value = bounded_text(value, MAX_CAUSE_CHARS);
    (!value.is_empty()).then_some(value)
}

fn bounded_text(value: &str, limit: usize) -> String {
    let redacted = FileSecurityService::new(PathBuf::new()).redact(value);
    let normalized = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        return normalized;
    }
    if limit <= 1 {
        return "\u{2026}".chars().take(limit).collect();
    }
    normalized
        .chars()
        .take(limit - 1)
        .chain(std::iter::once('\u{2026}'))
        .collect()
}
