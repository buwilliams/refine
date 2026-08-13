use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::prompts::{PromptTemplate, render};

use super::json_object;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct GovernanceEvaluation {
    pub(super) failed: bool,
    pub(super) message: Option<String>,
    pub(super) recovery_analysis: Option<String>,
    pub(super) recovery_round_prompt: Option<String>,
    pub(super) details: JsonObject,
}

pub(super) fn post_implementation_governance_prompt(
    governance: &Value,
    rules: &[Value],
    guidance: &[Value],
    worktree_path: &str,
    provider_cwd: &Path,
    goal_id: &str,
    round_idx: usize,
) -> String {
    let product = governance
        .get("product")
        .and_then(Value::as_str)
        .unwrap_or("");
    let constitution = governance
        .get("constitution")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rules_json = serde_json::to_string_pretty(rules).unwrap_or_else(|_| "[]".to_string());
    let guidance_json = serde_json::to_string_pretty(guidance).unwrap_or_else(|_| "[]".to_string());
    let round_number = (round_idx + 1).to_string();
    let provider_cwd = provider_cwd.display().to_string();
    render(
        PromptTemplate::PostImplementationGovernance,
        &[
            ("goal_id", goal_id),
            ("round_number", &round_number),
            ("worktree_path", worktree_path),
            ("provider_cwd", &provider_cwd),
            ("product", product),
            ("constitution", constitution),
            ("rules_json", &rules_json),
            ("guidance_json", &guidance_json),
        ],
    )
}

/// Keys that only a governance verdict object carries. A JSON object quoted in
/// the review prose is very unlikely to hold one.
const GOVERNANCE_VERDICT_KEYS: [&str; 5] = [
    "status",
    "verdict",
    "violations",
    "rule_violations",
    "failed_actions",
];

/// Weaker verdict signals, consulted only when no object carries a strong key.
const GOVERNANCE_VERDICT_FALLBACK_KEYS: [&str; 5] =
    ["ok", "result", "failed", "violates", "violation"];

pub(super) const GOVERNANCE_VERDICT_UNPARSABLE: &str = "Governance verdict could not be parsed: the review did not return the required JSON verdict object.";

pub(super) fn parse_governance_provider_output(
    output: &str,
    rules_checked: usize,
) -> GovernanceEvaluation {
    let trimmed = output.trim();
    match parse_governance_verdict(trimmed) {
        Some(value) => governance_evaluation_from_json(value, trimmed, rules_checked),
        // No verdict means the review could not be read, which is a parsing
        // failure and not a rule violation. Guessing pass/fail from the prose
        // scores explanatory text ("no rule violation here") as a violation, so
        // fail closed and say plainly that the verdict was unreadable.
        None => unparsable_governance_evaluation(trimmed, rules_checked),
    }
}

fn unparsable_governance_evaluation(
    raw_output: &str,
    rules_checked: usize,
) -> GovernanceEvaluation {
    GovernanceEvaluation {
        failed: true,
        message: Some(GOVERNANCE_VERDICT_UNPARSABLE.to_string()),
        recovery_analysis: None,
        recovery_round_prompt: None,
        details: json_object(json!({
            "phase": "post_implementation",
            "configured": true,
            "rules_checked": rules_checked,
            "verdict_parse_error": true,
            "failed_actions": [{
                "action": "verdict_parse_error",
                "message": GOVERNANCE_VERDICT_UNPARSABLE
            }],
            "raw_output": raw_output
        })),
    }
}

/// Find the review's verdict, not merely the first brace in its prose. Reviews
/// routinely quote code and JSON while reasoning about rules, so scan every
/// balanced object and take the last one that actually looks like a verdict.
fn parse_governance_verdict(raw: &str) -> Option<Value> {
    let candidates = json_object_candidates(raw);
    candidates
        .iter()
        .rev()
        .find(|value| has_any_key(value, &GOVERNANCE_VERDICT_KEYS))
        .or_else(|| {
            candidates
                .iter()
                .rev()
                .find(|value| has_any_key(value, &GOVERNANCE_VERDICT_FALLBACK_KEYS))
        })
        .cloned()
}

fn has_any_key(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|object| keys.iter().any(|key| object.contains_key(*key)))
}

fn governance_evaluation_from_json(
    value: Value,
    raw_output: &str,
    rules_checked: usize,
) -> GovernanceEvaluation {
    let violations = value
        .get("violations")
        .or_else(|| value.get("rule_violations"))
        .or_else(|| value.get("failed_actions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let status = value
        .get("status")
        .or_else(|| value.get("verdict"))
        .or_else(|| value.get("result"))
        .and_then(Value::as_str)
        .map(|status| status.trim().to_ascii_lowercase());
    let ok = value.get("ok").and_then(Value::as_bool);
    let explicit_failed = value
        .get("failed")
        .or_else(|| value.get("violates"))
        .or_else(|| value.get("violation"))
        .and_then(Value::as_bool);
    let failed = explicit_failed
        .or_else(|| ok.map(|ok| !ok))
        .or_else(|| {
            status.as_ref().map(|status| {
                matches!(
                    status.as_str(),
                    "failed" | "fail" | "blocked" | "violated" | "violation"
                )
            })
        })
        .unwrap_or(!violations.is_empty());
    let provider_message = value
        .get("message")
        .or_else(|| value.get("reason"))
        .or_else(|| value.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToString::to_string);
    let message = if failed {
        Some(provider_message.unwrap_or_else(|| violation_message_from_actions(&violations)))
    } else {
        provider_message
    };
    GovernanceEvaluation {
        failed,
        message,
        recovery_analysis: value
            .get("recovery_analysis")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        recovery_round_prompt: value
            .get("recovery_round_prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        details: json_object(json!({
            "phase": "post_implementation",
            "configured": true,
            "rules_checked": rules_checked,
            "failed_actions": violations,
            "raw_output": raw_output,
            "verdict": value
        })),
    }
}

/// Every top-level balanced object in `raw` that parses as JSON, in the order
/// they appear. An opening brace that never balances (unclosed code in prose)
/// only costs the scan that brace: it resumes right after it instead of
/// swallowing the rest of the text.
fn json_object_candidates(raw: &str) -> Vec<Value> {
    let mut candidates = Vec::new();
    let mut search_from = 0usize;
    while let Some(relative_start) = raw[search_from..].find('{') {
        let start = search_from + relative_start;
        if let Some(end) = balanced_object_end(raw, start)
            && let Ok(value) = serde_json::from_str::<Value>(&raw[start..=end])
        {
            candidates.push(value);
            search_from = end + 1;
            continue;
        }
        search_from = start + 1;
    }
    candidates
}

/// Byte index of the `}` closing the object that opens at `start`.
fn balanced_object_end(raw: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in raw[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn violation_message_from_actions(actions: &[Value]) -> String {
    actions
        .iter()
        .find_map(|action| {
            action
                .get("message")
                .or_else(|| action.get("reason"))
                .or_else(|| action.get("text"))
                .or_else(|| action.get("rule"))
                .and_then(Value::as_str)
                .map(governance_violation_message)
        })
        .unwrap_or_else(|| "Governance rule violation detected.".to_string())
}

fn governance_violation_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "Governance rule violation detected.".to_string()
    } else if message
        .to_ascii_lowercase()
        .contains("governance rule violation")
    {
        message.to_string()
    } else {
        format!("Governance rule violation: {message}")
    }
}
