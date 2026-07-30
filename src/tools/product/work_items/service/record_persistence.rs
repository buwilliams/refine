use super::*;

pub(super) fn write_json_atomically(path: &std::path::Path, value: &Value) -> RefineResult<()> {
    let coordination_root = workflow_record_root(path);
    with_workflow_coordination(&coordination_root, || {
        let expected_revision = workflow_revision(value);
        let current = match fs::read(path) {
            Ok(bytes) => Some(serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse current workflow record {}: {error}",
                    path.display()
                ))
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read current workflow record {}: {error}",
                    path.display()
                )));
            }
        };
        match current.as_ref() {
            Some(current) if workflow_revision(current) != expected_revision => {
                return Err(RefineError::Conflict(format!(
                    "workflow record {} changed after it was read (expected revision {}, current revision {})",
                    path.display(),
                    expected_revision,
                    workflow_revision(current)
                )));
            }
            Some(_) => {}
            None if expected_revision != 0 => {
                return Err(RefineError::Conflict(format!(
                    "workflow record {} was removed after it was read",
                    path.display()
                )));
            }
            None => {}
        }

        let mut next = value.clone();
        let object = next.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "workflow record {} is not a JSON object",
                path.display()
            ))
        })?;
        object.insert(
            "workflow_revision".to_string(),
            Value::from(expected_revision.saturating_add(1)),
        );
        let encoded = serde_json::to_vec_pretty(&next).map_err(|error| {
            RefineError::Serialization(format!("failed to encode workflow JSON: {error}"))
        })?;
        replace_file_durably(path, &encoded)?;
        // Follow the record with the scheduler's view of it. Write-through is
        // what keeps that view affordable: the alternative is comparing every
        // Goal file against the index to discover which moved, which costs a
        // stat per Goal on every scheduling pass and is the scan the index
        // exists to avoid. The record is already durable, so a failure here
        // leaves a stale index rather than a lost mutation — recoverable by
        // reconstruction, which is why it does not fail the write.
        if is_goal_record(path) {
            let refine_dir = workflow_record_root(path);
            if let Err(error) = ActiveGoalIndex::record_goal(&refine_dir, path) {
                eprintln!(
                    "refine: active Goal index was not updated for {}: {error}",
                    path.display()
                );
            }
        }
        Ok(())
    })
}

fn is_goal_record(path: &std::path::Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("goal.json")
}

pub(crate) fn workflow_revision(value: &Value) -> u64 {
    value
        .get("workflow_revision")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub(super) fn set_workflow_revision(value: &mut Value, revision: u64) -> RefineResult<()> {
    let object = value.as_object_mut().ok_or_else(|| {
        RefineError::Serialization("workflow record is not a JSON object".to_string())
    })?;
    object.insert("workflow_revision".to_string(), Value::from(revision));
    Ok(())
}

pub(super) fn workflow_record_root(path: &std::path::Path) -> PathBuf {
    for ancestor in path.ancestors() {
        if matches!(
            ancestor.file_name().and_then(|name| name.to_str()),
            Some("goals" | "features")
        ) {
            return ancestor
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| path.parent().unwrap_or(path).to_path_buf());
        }
    }
    path.parent().unwrap_or(path).to_path_buf()
}

pub(super) fn new_ulid_like() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut value = (now.as_millis() << 64)
        ^ ((now.subsec_nanos() as u128) << 32)
        ^ ((std::process::id() as u128) << 16)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let mut chars = [b'0'; 26];
    for idx in (0..26).rev() {
        chars[idx] = ALPHABET[(value & 0x1f) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).unwrap()
}
