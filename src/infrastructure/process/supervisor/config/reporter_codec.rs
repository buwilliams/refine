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

pub(super) enum ReporterCascadeStage<'a> {
    BeforeRecord(&'a Path),
    RecordLocked(&'a Path),
    AfterRecord(&'a Path),
}

pub(super) fn rewrite_reporter_references(
    refine_dir: &Path,
    old: &str,
    new: &str,
    mut observe: impl FnMut(ReporterCascadeStage<'_>),
) -> RefineResult<()> {
    if old.trim().is_empty() || old == new {
        return Ok(());
    }
    let mut paths = Vec::new();
    collect_reporter_reference_paths(&refine_dir.join("goals"), "goal.json", &mut paths)?;
    collect_reporter_reference_paths(&refine_dir.join("features"), "feature.json", &mut paths)?;
    paths.sort();
    for path in paths {
        observe(ReporterCascadeStage::BeforeRecord(&path));
        let record_key = record_lock_key(&path);
        with_record_lock(refine_dir, &record_key, || {
            observe(ReporterCascadeStage::RecordLocked(&path));
            if rewrite_reporter_references_in_record(refine_dir, &path, old, new)? {
                observe(ReporterCascadeStage::AfterRecord(&path));
            }
            Ok(())
        })?;
    }
    FileTodoService::new(refine_dir).reassign_reporter(old, new)
}

fn collect_reporter_reference_paths(
    path: &Path,
    file_name: &str,
    paths: &mut Vec<PathBuf>,
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
            collect_reporter_reference_paths(&path, file_name, paths)?;
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) != Some(file_name) {
            continue;
        }
        paths.push(path);
    }
    Ok(())
}

fn rewrite_reporter_references_in_record(
    refine_dir: &Path,
    path: &Path,
    old: &str,
    new: &str,
) -> RefineResult<bool> {
    let mut value = read_json_or_default(path.to_path_buf(), json!({}))?;
    if !rewrite_reporter_reference_value(&mut value, old, new) {
        return Ok(false);
    }
    let revision = value
        .get("workflow_revision")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1);
    value
        .as_object_mut()
        .ok_or_else(|| {
            RefineError::Serialization(format!(
                "workflow record {} is not a JSON object",
                path.display()
            ))
        })?
        .insert("workflow_revision".to_string(), Value::from(revision));
    let encoded = serde_json::to_vec_pretty(&value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    replace_file_durably(path, &encoded)?;
    if path.file_name().and_then(|name| name.to_str()) == Some("goal.json")
        && let Err(error) = ActiveGoalIndex::record_goal(refine_dir, path)
    {
        eprintln!(
            "refine: active Goal index was not updated for {}: {error}",
            path.display()
        );
    }
    Ok(true)
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
