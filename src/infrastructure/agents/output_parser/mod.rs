pub(crate) fn extract_final_text(stdout: &str, output_format: &str) -> String {
    if output_format == "plain" {
        return stdout.trim().to_string();
    }
    let mut last = String::new();
    let mut deltas = Vec::new();
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(object) = event.as_object() else {
            continue;
        };
        if let Some(item) = object.get("item").and_then(|value| value.as_object()) {
            let item_type = item.get("type").and_then(|value| value.as_str());
            let text = item
                .get("text")
                .or_else(|| item.get("content"))
                .or_else(|| object.get("text"))
                .and_then(|value| value.as_str());
            if matches!(item_type, Some("agent_message" | "assistant_message")) {
                if let Some(text) = text {
                    last = text.to_string();
                }
                continue;
            }
        }
        if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message") {
            if let Some(content) = object
                .get("data")
                .and_then(|value| value.get("content"))
                .and_then(|value| value.as_str())
            {
                last = content.to_string();
            }
            continue;
        }
        if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message_delta") {
            if let Some(delta) = object
                .get("data")
                .and_then(|value| value.get("deltaContent"))
                .and_then(|value| value.as_str())
            {
                deltas.push(delta.to_string());
            }
            continue;
        }
        if object.get("type").and_then(|value| value.as_str()) == Some("assistant")
            && let Some(text) = object
                .get("message")
                .and_then(|value| value.get("content"))
                .and_then(text_from_content)
        {
            last = text;
        }
    }
    if last.is_empty() {
        if deltas.is_empty() {
            stdout.trim().to_string()
        } else {
            deltas.join("")
        }
    } else {
        last
    }
}

pub(crate) fn provider_error_message(stdout: &str, stderr: &str) -> Option<String> {
    for text in [stderr, stdout] {
        for line in text.lines().rev() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(object) = event.as_object() else {
                continue;
            };
            let is_error = object
                .get("is_error")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let has_api_error = object
                .get("api_error_status")
                .map(|value| !value.is_null())
                .unwrap_or(false);
            if !is_error && !has_api_error {
                continue;
            }
            let message = object
                .get("result")
                .or_else(|| object.get("message"))
                .or_else(|| object.get("error"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("provider returned an error");
            let status = object
                .get("api_error_status")
                .and_then(|value| value.as_i64())
                .map(|value| value.to_string());
            return Some(match status {
                Some(status) => format!("{message} ({status})"),
                None => message.to_string(),
            });
        }
    }
    None
}

pub(crate) fn extract_provider_session_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(session_id) = find_session_id_value(&event) {
            return Some(session_id);
        }
    }
    None
}

pub(crate) fn find_session_id_value(value: &serde_json::Value) -> Option<String> {
    const SESSION_KEYS: &[&str] = &[
        "provider_session_id",
        "session_id",
        "sessionId",
        "conversation_id",
        "conversationId",
    ];
    match value {
        serde_json::Value::Object(object) => {
            for key in SESSION_KEYS {
                if let Some(session_id) = object
                    .get(*key)
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return Some(session_id.to_string());
                }
            }
            object.values().find_map(find_session_id_value)
        }
        serde_json::Value::Array(values) => values.iter().find_map(find_session_id_value),
        _ => None,
    }
}

pub(crate) fn text_from_content(content: &serde_json::Value) -> Option<String> {
    let parts = content
        .as_array()?
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|value| value.as_str()) == Some("text") {
                block
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n").trim().to_string())
    }
}

pub(crate) fn last_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(500).collect())
}
