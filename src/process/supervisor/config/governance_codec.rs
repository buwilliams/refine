use super::*;

pub(super) fn normalize_governance(value: &mut Value) {
    if !value.is_object() {
        *value = json!({"product": "", "constitution": "", "rules": []});
    }
    let configured = !value
        .get("product")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .is_empty()
        && !value
            .get("constitution")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .is_empty();
    let rules = normalize_rules(value.get("rules").unwrap_or(&Value::Array(Vec::new())));
    value["product"] = Value::String(
        value
            .get("product")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
    );
    value["constitution"] = Value::String(
        value
            .get("constitution")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
    );
    value["rules"] = rules;
    value["configured"] = Value::Bool(configured);
}

pub(super) fn normalize_rules(value: &Value) -> Value {
    let mut rules = Vec::new();
    let mut seen = BTreeSet::new();
    for item in value.as_array().into_iter().flatten() {
        let text = item
            .get("text")
            .and_then(|value| value.as_str())
            .or_else(|| item.as_str())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        let mut id = item
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() || seen.contains(&id) {
            id = format!("rule-{}", rules.len() + 1);
        }
        seen.insert(id.clone());
        let created = item
            .get("created")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(now_timestamp);
        let updated = item
            .get("updated")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(now_timestamp);
        rules.push(json!({
            "id": id,
            "text": text.chars().take(500).collect::<String>(),
            "created": created,
            "updated": updated,
            "source": item.get("source").and_then(|value| value.as_str()).unwrap_or("manual")
        }));
    }
    Value::Array(rules)
}

pub(super) fn governance_rule(text: &str, source: &str) -> Value {
    json!({
        "id": format!("rule-{}", Utc::now().timestamp_millis()),
        "text": text,
        "created": now_timestamp(),
        "updated": now_timestamp(),
        "source": source
    })
}
