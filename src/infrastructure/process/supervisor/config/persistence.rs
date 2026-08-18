use super::*;

pub(super) fn read_json_or_default(path: PathBuf, default: Value) -> RefineResult<Value> {
    if !path.exists() {
        return Ok(default);
    }
    let bytes = fs::read_to_string(&path)
        .map_err(|error| RefineError::Io(format!("failed to read {}: {error}", path.display())))?;
    serde_json::from_str(&bytes).map_err(|error| {
        RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
    })
}

pub(super) fn write_json(path: PathBuf, value: &Value) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let encoded = serde_json::to_string_pretty(value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    write_json_atomically(&path, format!("{encoded}\n").as_bytes(), "JSON")
}
