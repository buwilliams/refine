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
    guidance_revision(value)?;
    Ok(())
}

pub(super) fn validate_guidance_document_patch(value: &Value) -> RefineResult<()> {
    let object = value.as_object().ok_or_else(|| {
        RefineError::InvalidInput("Guidance update must be a JSON object".to_string())
    })?;
    for key in object.keys() {
        if !matches!(key.as_str(), "guidance" | "revision") {
            return Err(RefineError::InvalidInput(format!(
                "unknown Guidance field: {key}"
            )));
        }
    }
    let items = object
        .get("guidance")
        .ok_or_else(|| RefineError::InvalidInput("guidance must be a list".to_string()))?;
    normalize_guidance_input(items)?;
    guidance_revision(value)?;
    Ok(())
}

pub(super) fn validate_guidance_remove(value: &Value) -> RefineResult<()> {
    let object = value.as_object().ok_or_else(|| {
        RefineError::InvalidInput("Guidance removal must be a JSON object".to_string())
    })?;
    for key in object.keys() {
        if key != "revision" {
            return Err(RefineError::InvalidInput(format!(
                "unknown Guidance removal field: {key}"
            )));
        }
    }
    guidance_revision(value)?;
    Ok(())
}

pub(super) fn guidance_revision(value: &Value) -> RefineResult<Option<u64>> {
    match value.get("revision") {
        Some(revision) => revision.as_u64().map(Some).ok_or_else(|| {
            RefineError::InvalidInput("Guidance revision must be a nonnegative integer".to_string())
        }),
        None => Ok(None),
    }
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
    if !valid_guidance_id(&id) || seen.contains(&id) {
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

fn valid_guidance_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            matches!(
                byte,
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
            )
        })
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
