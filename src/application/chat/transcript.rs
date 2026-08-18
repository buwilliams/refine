use serde_json::{Value, json};

use crate::model::JsonObject;

use super::ChatSessionRecord;
use super::session_foundation::{new_event_id, now_timestamp};

pub(super) fn unread_lines(record: &ChatSessionRecord) -> Vec<String> {
    record
        .transcript_events
        .iter()
        .filter(|event| !event_bool(event, "delivered"))
        .filter(|event| !event_bool(event, "progress"))
        .filter_map(event_text)
        .collect()
}

pub(super) fn unread_progress(record: &ChatSessionRecord) -> Vec<String> {
    record
        .transcript_events
        .iter()
        .filter(|event| !event_bool(event, "delivered"))
        .filter(|event| event_bool(event, "progress"))
        .filter_map(event_text)
        .collect()
}

pub(super) fn chat_event(
    role: &str,
    text: &str,
    progress: bool,
    provider_session_id: Option<String>,
    extra: Option<Value>,
) -> JsonObject {
    let mut value = json!({
        "id": new_event_id(),
        "role": role,
        "text": text,
        "progress": progress,
        "delivered": false,
        "created_at": now_timestamp(),
        "provider_session_id": provider_session_id
    });
    if let Some(extra) = extra {
        value["extra"] = extra;
    }
    value.as_object().cloned().unwrap_or_default()
}

pub(super) fn event_text(event: &JsonObject) -> Option<String> {
    let role = event
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let text = event.get("text").and_then(|value| value.as_str())?;
    match role {
        "user" => Some(format!("> {text}")),
        "assistant" | "system" => Some(text.to_string()),
        _ => Some(text.to_string()),
    }
}

pub(super) fn event_bool(event: &JsonObject, key: &str) -> bool {
    event
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(super) fn importable_artifacts_from_output(output: &str) -> Vec<JsonObject> {
    let mut artifacts = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(output.trim()) {
        collect_importable_artifacts(&value, &mut artifacts);
    }
    for line in output.lines() {
        let Some(raw) = line
            .trim()
            .strip_prefix("REFINE_ARTIFACT:")
            .or_else(|| line.trim().strip_prefix("refine_artifact:"))
        else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(raw.trim()) {
            collect_importable_artifacts(&value, &mut artifacts);
        }
    }
    artifacts
}

fn collect_importable_artifacts(value: &Value, artifacts: &mut Vec<JsonObject>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_importable_artifacts(value, artifacts);
            }
        }
        Value::Object(object) => {
            if let Some(values) = object
                .get("importable_artifacts")
                .and_then(|value| value.as_array())
            {
                for value in values {
                    collect_importable_artifacts(value, artifacts);
                }
            }
            if recognized_artifact(object) {
                artifacts.push(object.clone());
                return;
            }
            for (key, artifact_type) in [
                ("round", "round"),
                ("goal", "goal"),
                ("feature_plan", "feature_plan"),
            ] {
                if let Some(Value::Object(payload)) = object.get(key) {
                    let mut artifact = JsonObject::new();
                    artifact.insert("type".to_string(), Value::String(artifact_type.to_string()));
                    artifact.insert(key.to_string(), Value::Object(payload.clone()));
                    artifacts.push(artifact);
                }
            }
            if let Some(Value::Array(goals)) = object.get("goals") {
                let mut artifact = JsonObject::new();
                artifact.insert("type".to_string(), Value::String("goals".to_string()));
                artifact.insert("goals".to_string(), Value::Array(goals.clone()));
                artifacts.push(artifact);
            }
        }
        _ => {}
    }
}

fn recognized_artifact(object: &JsonObject) -> bool {
    matches!(
        object.get("type").and_then(|value| value.as_str()),
        Some("round" | "goal" | "goals" | "feature_plan")
    )
}
