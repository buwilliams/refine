use super::*;

pub(super) fn normalize_guidance_list(value: &Value) -> Value {
    let items = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.trim();
            let rule = item.get("rule")?.as_str()?.trim();
            let instructions = item.get("instructions")?.as_str()?.trim();
            if name.is_empty() || rule.is_empty() || instructions.is_empty() {
                return None;
            }
            Some(json!({
                "name": name,
                "rule": rule,
                "instructions": instructions,
                "enabled": item.get("enabled").and_then(|value| value.as_bool()).unwrap_or(true)
            }))
        })
        .collect::<Vec<_>>();
    Value::Array(items)
}
