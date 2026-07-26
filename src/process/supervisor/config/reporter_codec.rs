use super::*;

pub(super) fn normalize_reporters(value: &Value) -> Vec<Value> {
    let mut reporters = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id").and_then(|value| value.as_u64())?;
            let name = item.get("name").and_then(|value| value.as_str())?.trim();
            if name.is_empty() {
                return None;
            }
            Some(json!({
                "id": id,
                "name": name,
                "created": item.get("created").and_then(|value| value.as_str()).unwrap_or("")
            }))
        })
        .collect::<Vec<_>>();
    reporters.sort_by(|a, b| {
        a.get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_lowercase()
            .cmp(
                &b.get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_lowercase(),
            )
    });
    reporters
}

pub(super) fn collect_reporter_names(
    path: &Path,
    file_name: &str,
    names: &mut BTreeSet<String>,
) -> RefineResult<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read reporter directory {}: {error}",
            path.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read Goal directory entry {}: {error}",
                path.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_reporter_names(&path, file_name, names)?;
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) != Some(file_name) {
            continue;
        }
        let value = read_json_or_default(path.clone(), json!({}))?;
        collect_reporter_name(value.get("reporter"), names);
        collect_reporter_name(value.get("assignee"), names);
        for round in value
            .get("rounds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect_reporter_name(round.get("reporter"), names);
            collect_reporter_name(round.get("assignee"), names);
        }
    }
    Ok(())
}

pub(super) fn collect_reporter_name(value: Option<&Value>, names: &mut BTreeSet<String>) {
    if let Some(name) = value.and_then(Value::as_str) {
        let clean = name.trim();
        if !clean.is_empty() {
            names.insert(clean.to_string());
        }
    }
}

pub(super) fn rewrite_reporter_references(
    refine_dir: &Path,
    old: &str,
    new: &str,
) -> RefineResult<()> {
    if old.trim().is_empty() || old == new {
        return Ok(());
    }
    rewrite_reporter_references_in_tree(&refine_dir.join("goals"), "goal.json", old, new)?;
    rewrite_reporter_references_in_tree(&refine_dir.join("features"), "feature.json", old, new)?;
    FileTodoService::new(refine_dir).reassign_reporter(old, new)
}

pub(super) fn rewrite_reporter_references_in_tree(
    path: &Path,
    file_name: &str,
    old: &str,
    new: &str,
) -> RefineResult<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read reporter directory {}: {error}",
            path.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to read reporter directory entry {}: {error}",
                path.display()
            ))
        })?;
        let path = entry.path();
        if path.is_dir() {
            rewrite_reporter_references_in_tree(&path, file_name, old, new)?;
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) != Some(file_name) {
            continue;
        }
        let mut value = read_json_or_default(path.clone(), json!({}))?;
        if rewrite_reporter_reference_value(&mut value, old, new) {
            write_json(path, &value)?;
        }
    }
    Ok(())
}

pub(super) fn rewrite_reporter_reference_value(value: &mut Value, old: &str, new: &str) -> bool {
    let mut changed = false;
    if let Some(object) = value.as_object_mut() {
        changed |= rewrite_reporter_field(object.get_mut("reporter"), old, new);
        changed |= rewrite_reporter_field(object.get_mut("assignee"), old, new);
        if let Some(rounds) = object.get_mut("rounds").and_then(Value::as_array_mut) {
            for round in rounds {
                if let Some(round_object) = round.as_object_mut() {
                    changed |= rewrite_reporter_field(round_object.get_mut("reporter"), old, new);
                    changed |= rewrite_reporter_field(round_object.get_mut("assignee"), old, new);
                }
            }
        }
    }
    changed
}

pub(super) fn rewrite_reporter_field(value: Option<&mut Value>, old: &str, new: &str) -> bool {
    let Some(value) = value else {
        return false;
    };
    if value.as_str() == Some(old) {
        *value = Value::String(new.to_string());
        return true;
    }
    false
}
