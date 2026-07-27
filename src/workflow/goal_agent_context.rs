use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::Value;

use crate::model::JsonObject;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};

pub(super) fn goal_agent_prompt(goal_id: &str, agent_context: &Value) -> RefineResult<String> {
    agent_context.get("goal").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned Goal context"))
    })?;
    agent_context.get("previous_rounds").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned previous Rounds"))
    })?;
    agent_context.get("current_round").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned current Round"))
    })?;
    let context_object = agent_context.as_object().ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} agent context is not an object"))
    })?;
    let agent_context = serde_json::to_string_pretty(&OrderedAgentContext(context_object))
        .map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode Goal {goal_id} agent context: {error}"
            ))
        })?;

    Ok(render(
        PromptTemplate::GoalAgent,
        &[("goal_id", goal_id), ("agent_context", &agent_context)],
    ))
}

/// Keep context encoded once while placing the authoritative Round after every
/// contextual field, including future schema additions.
struct OrderedAgentContext<'a>(&'a serde_json::Map<String, Value>);

impl Serialize for OrderedAgentContext<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self
            .0
            .iter()
            .filter(|(key, _)| key.as_str() != "previous_rounds" && key.as_str() != "current_round")
        {
            map.serialize_entry(key, value)?;
        }
        if let Some(previous) = self.0.get("previous_rounds") {
            map.serialize_entry("previous_rounds", previous)?;
        }
        if let Some(current) = self.0.get("current_round") {
            map.serialize_entry("current_round", current)?;
        }
        map.end()
    }
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
