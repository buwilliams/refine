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
    if round
        .get("failure_message")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        let evidence = serde_json::json!({"failure_category": round.get("failure_category"), "failure_message": round.get("failure_message"), "failure_at": round.get("failure_at"), "occurrence": round.get("workflow_failure_occurrence")});
        if let Some(history) = round
            .entry("failure_history")
            .or_insert(serde_json::json!([]))
            .as_array_mut()
        {
            history.push(evidence);
        }
    }
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
    round.insert("workflow_reconciliation".to_string(), Value::Null);
    round.insert("workflow_recovery".to_string(), Value::Null);
    round.insert("workflow_attempt_authority".to_string(), Value::Null);
    round.insert(
        "rule_state".to_string(),
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
        "governance_candidate_commit".to_string(),
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

/// Retry selects fresh gate evidence while retaining the complete previous evaluation.
/// Claims remain provenance and are never cleared as a lifecycle side effect.
pub(super) fn archive_round_for_retry(round: &mut Value, target: &GoalStatus) -> RefineResult<()> {
    let mut prior = round.clone();
    let prior_object = prior
        .as_object_mut()
        .ok_or_else(|| RefineError::Serialization("Invalid Round".into()))?;
    prior_object.remove("prior_attempts");
    let object = round
        .as_object_mut()
        .ok_or_else(|| RefineError::Serialization("Invalid Round".into()))?;
    object
        .entry("prior_attempts")
        .or_insert(serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| RefineError::Serialization("Invalid prior attempts".into()))?
        .push(prior);
    if *target != GoalStatus::Governance {
        for key in [
            "quality_state",
            "quality_message",
            "quality_details",
            "quality_checked_at",
            "quality_candidate_commit",
        ] {
            object.remove(key);
        }
    }
    for key in [
        "rule_state",
        "governance_message",
        "governance_details",
        "governance_checked_at",
        "governance_candidate_commit",
        "failure_category",
        "failure_message",
        "failure_at",
        "event_results",
        "gate_configurations",
        "event_configuration",
    ] {
        object.remove(key);
    }
    for key in ["failure_category", "failure_message", "failure_at"] {
        object.insert(key.into(), Value::String(String::new()));
    }
    Ok(())
}
