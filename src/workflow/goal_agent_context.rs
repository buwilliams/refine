use serde_json::Value;

use crate::model::JsonObject;

pub(super) fn round_agent_context(round: &Value, round_idx: usize) -> Value {
    let mut context = selected_agent_context(
        round,
        &[
            "reporter",
            "assignee",
            "prompt",
            "guidance_decision",
            "implementation_report",
            "implementation_reported_at",
            "rule_state",
            "meta_rule_state",
            "product_state",
            "constitution_state",
            "governance_message",
            "governance_details",
            "governance_checked_at",
            "governance_rule_actions",
            "quality_state",
            "quality_message",
            "quality_details",
            "quality_checked_at",
        ],
    );
    if let Some(context) = context.as_object_mut() {
        context.insert("round".to_string(), Value::from(round_idx + 1));
    }
    context
}

pub(super) fn selected_agent_context(value: &Value, keys: &[&str]) -> Value {
    let mut context = JsonObject::new();
    let Some(source) = value.as_object() else {
        return Value::Object(context);
    };
    for key in keys {
        let Some(value) = source.get(*key) else {
            continue;
        };
        if agent_context_value_is_meaningful(value) {
            context.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(context)
}

fn agent_context_value_is_meaningful(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty() && value != "unclassified",
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}
