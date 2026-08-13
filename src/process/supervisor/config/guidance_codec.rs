use super::*;

pub(super) fn normalize_guidance_document(value: &Value) -> Value {
    let (revision, items) = match value {
        Value::Array(items) => (0, items.as_slice()),
        Value::Object(object) => (
            object.get("revision").and_then(Value::as_u64).unwrap_or(0),
            object
                .get("guidance")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default(),
        ),
        _ => (0, &[][..]),
    };
    let mut seen = BTreeSet::new();
    let guidance = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| normalize_guidance_item(item, index, &mut seen))
        .collect::<Vec<_>>();
    json!({"revision": revision, "guidance": guidance})
}

pub(super) fn normalize_guidance_input(value: &Value) -> RefineResult<Value> {
    let items = value
        .as_array()
        .ok_or_else(|| RefineError::InvalidInput("guidance must be a list".to_string()))?;
    for item in items {
        validate_guidance_fields(item, true)?;
    }
    Ok(normalize_guidance_document(&json!({"guidance": items}))["guidance"].clone())
}

pub(super) fn validate_guidance_fields(value: &Value, require_all: bool) -> RefineResult<()> {
    let object = value.as_object().ok_or_else(|| {
        RefineError::InvalidInput("Guidance entry must be a JSON object".to_string())
    })?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "id" | "name" | "rule" | "instructions" | "enabled" | "revision"
        ) {
            return Err(RefineError::InvalidInput(format!(
                "unknown Guidance field: {key}"
            )));
        }
    }
    for key in ["name", "rule", "instructions"] {
        match object.get(key) {
            Some(Value::String(value)) if !value.trim().is_empty() => {}
            Some(_) => {
                return Err(RefineError::InvalidInput(format!(
                    "Guidance {key} must be a non-empty string"
                )));
            }
            None if require_all => {
                return Err(RefineError::InvalidInput(format!(
                    "Guidance {key} is required"
                )));
            }
            None => {}
        }
    }
    if object
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(RefineError::InvalidInput(
            "Guidance enabled must be a boolean".to_string(),
        ));
    }
    Ok(())
}

fn normalize_guidance_item(
    item: &Value,
    index: usize,
    seen: &mut BTreeSet<String>,
) -> Option<Value> {
    let name = item.get("name")?.as_str()?.trim();
    let rule = item.get("rule")?.as_str()?.trim();
    let instructions = item.get("instructions")?.as_str()?.trim();
    if name.is_empty() || rule.is_empty() || instructions.is_empty() {
        return None;
    }
    let mut id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() || seen.contains(&id) {
        id = format!("guidance-{}", index + 1);
        while seen.contains(&id) {
            id.push('x');
        }
    }
    seen.insert(id.clone());
    Some(json!({
        "id": id,
        "name": name,
        "rule": rule,
        "instructions": instructions,
        "enabled": item.get("enabled").and_then(Value::as_bool).unwrap_or(true)
    }))
}

pub(super) fn new_guidance_id(items: &[Value]) -> String {
    let mut candidate = new_config_item_id("guidance");
    let ids = items
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    while ids.contains(candidate.as_str()) {
        candidate.push('x');
    }
    candidate
}
