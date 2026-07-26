use serde_json::Value;

use crate::model::JsonObject;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};

pub(super) fn goal_agent_prompt(goal_id: &str, agent_context: &Value) -> RefineResult<String> {
    let goal_context = agent_context.get("goal").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned Goal context"))
    })?;
    let previous_rounds = agent_context.get("previous_rounds").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned previous Rounds"))
    })?;
    let current_round = agent_context.get("current_round").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned current Round"))
    })?;
    let goal_context = serde_json::to_string_pretty(&goal_context).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Goal {goal_id} context: {error}"))
    })?;
    let previous_rounds = serde_json::to_string_pretty(&previous_rounds).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode Goal {goal_id} previous-round context: {error}"
        ))
    })?;
    let current_round = serde_json::to_string_pretty(&current_round).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode Goal {goal_id} current-round context: {error}"
        ))
    })?;
    let agent_context = serde_json::to_string_pretty(agent_context).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode Goal {goal_id} agent context: {error}"
        ))
    })?;

    Ok(render(
        PromptTemplate::GoalAgent,
        &[
            ("goal_id", goal_id),
            ("agent_context", &agent_context),
            ("goal_context", &goal_context),
            ("previous_rounds", &previous_rounds),
            ("latest_round", &current_round),
        ],
    ))
}

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
