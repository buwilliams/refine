use super::*;

pub(super) fn state_tree_digest(
    root: &std::path::Path,
    state: &DurableStateMap,
) -> RefineResult<String> {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    for path in state.keys() {
        digest.update(path.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        let bytes = fs::read(root.join(path)).map_err(|error| {
            RefineError::Io(format!(
                "failed to hash state recovery evidence {}: {error}",
                root.join(path).display()
            ))
        })?;
        digest.update(Sha256::digest(bytes));
        digest.update([0xff]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(super) fn preview_evidence_id(preview: &StateRecoveryPreview) -> RefineResult<String> {
    use sha2::{Digest, Sha256};

    let mut unsigned = preview.clone();
    unsigned.evidence_id.clear();
    let bytes = serde_json::to_vec(&unsigned).map_err(|error| {
        RefineError::Serialization(format!("failed to encode state recovery preview: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn write_atomic_durable(path: &std::path::Path, bytes: &[u8]) -> RefineResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RefineError::Io(format!(
            "state recovery path {} has no parent",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        RefineError::Io(format!(
            "failed to create state recovery directory {}: {error}",
            parent.display()
        ))
    })?;
    let temp = parent.join(format!(
        ".refine-state-recovery-{}-{}.tmp",
        std::process::id(),
        STATE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = File::create(&temp).map_err(|error| {
            RefineError::Io(format!(
                "failed to create state recovery evidence {}: {error}",
                temp.display()
            ))
        })?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to persist state recovery evidence {}: {error}",
                    temp.display()
                ))
            })?;
        fs::rename(&temp, path).map_err(|error| {
            RefineError::Io(format!(
                "failed to commit state recovery evidence {}: {error}",
                path.display()
            ))
        })?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to durably commit state recovery directory {}: {error}",
                    parent.display()
                ))
            })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub(super) fn write_manifest(
    path: &std::path::Path,
    manifest: &StateRecoveryManifest,
) -> RefineResult<()> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
        RefineError::Serialization(format!("failed to encode state recovery manifest: {error}"))
    })?;
    write_atomic_durable(path, &bytes)
}

pub(super) fn load_manifest(path: &std::path::Path) -> RefineResult<Option<StateRecoveryManifest>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read state recovery manifest {}: {error}",
                path.display()
            )));
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse state recovery manifest {}: {error}",
            path.display()
        ))
    })
}

pub(super) fn recovery_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(super) fn bounded_message(message: &str) -> String {
    const LIMIT: usize = 2_000;
    message.chars().take(LIMIT).collect()
}
