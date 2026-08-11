use super::*;

pub(super) fn attach_latest_log_fields(
    round: &mut Map<String, Value>,
    logs: &[crate::model::log::RoundLogEntry],
) -> RefineResult<()> {
    let latest_log = logs.last();
    let latest_error_log = logs
        .iter()
        .rev()
        .find(|log| log.entry.severity == "error" || log.entry.severity == "warn");
    let latest_state_log = logs.iter().rev().find(|log| log.entry.category == "state");
    let latest_workflow_log = logs
        .iter()
        .rev()
        .find(|log| log.entry.message.contains("Workflow status changed:"));
    for (key, value) in [
        ("latest_log", latest_log),
        ("latest_error_log", latest_error_log),
        ("latest_state_log", latest_state_log),
        ("latest_workflow_log", latest_workflow_log),
    ] {
        if let Some(log) = value {
            let value = serde_json::to_value(log).map_err(|error| {
                RefineError::Serialization(format!("failed to encode latest Goal log: {error}"))
            })?;
            round.insert(key.to_string(), value);
        }
    }
    Ok(())
}

/// A recorded failure describes the round that failed. Retrying a Goal reuses
/// that same round, so leaving the reason behind would show a live failure on
/// work that has since moved on.
pub(super) fn clear_latest_round_failure(object: &mut Map<String, Value>) {
    let Some(round) = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .and_then(|rounds| rounds.last_mut())
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for key in ["failure_category", "failure_message", "failure_at"] {
        if round.contains_key(key) {
            round.insert(key.to_string(), Value::String(String::new()));
        }
    }
}

pub(super) fn new_round_value(reporter: &str, assignee: &str, prompt: &str) -> Value {
    let now = now_timestamp();
    let mut round = Map::new();
    round.insert("reporter".to_string(), Value::String(reporter.to_string()));
    round.insert("assignee".to_string(), Value::String(assignee.to_string()));
    round.insert("prompt".to_string(), Value::String(prompt.to_string()));
    round.insert("created".to_string(), Value::String(now.clone()));
    round.insert("updated".to_string(), Value::String(now));
    round.insert("logs".to_string(), Value::Array(Vec::new()));
    round.insert("implementation_report".to_string(), Value::Null);
    round.insert("implementation_reported_at".to_string(), Value::Null);
    round.insert("agent_context".to_string(), Value::Null);
    round.insert("implementation_plan".to_string(), Value::Null);
    round.insert("guidance_decision".to_string(), Value::Null);
    round.insert("workflow_quality_timing".to_string(), Value::Null);
    round.insert("workflow_reconciliation".to_string(), Value::Null);
    round.insert("workflow_recovery".to_string(), Value::Null);
    round.insert(
        "rule_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "meta_rule_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "product_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "constitution_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "governance_message".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_details".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_checked_at".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_rule_actions".to_string(),
        Value::Array(Vec::new()),
    );
    round.insert(
        "quality_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert("quality_message".to_string(), Value::String(String::new()));
    round.insert("quality_details".to_string(), Value::String(String::new()));
    round.insert(
        "quality_checked_at".to_string(),
        Value::String(String::new()),
    );
    round.insert("failure_category".to_string(), Value::String(String::new()));
    round.insert("failure_message".to_string(), Value::String(String::new()));
    round.insert("failure_at".to_string(), Value::String(String::new()));
    Value::Object(round)
}
