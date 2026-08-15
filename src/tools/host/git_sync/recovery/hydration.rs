use super::*;

/// Hydrate an owned recovery target one canonical record lease at a time.
///
/// An unchanged preimage may advance to the target. A path already at the
/// target is an idempotently resumed partial apply. Any other value is a later
/// live write and remains untouched as the next local delta.
pub(in crate::tools::host::git_sync) fn hydrate_recovery_target_with_cas(
    original_root: &std::path::Path,
    target_root: &std::path::Path,
    live_root: &std::path::Path,
) -> RefineResult<bool> {
    let original = durable_state_map(original_root)?;
    hydrate_recovery_target_from_map(&original, target_root, live_root)
}

pub(in crate::tools::host::git_sync) fn hydrate_recovery_target_from_map(
    original: &DurableStateMap,
    target_root: &std::path::Path,
    live_root: &std::path::Path,
) -> RefineResult<bool> {
    let target = durable_state_map(target_root)?;
    let paths = original
        .keys()
        .chain(target.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut concurrent_change = false;
    for relative in paths {
        let destination = live_root.join(&relative);
        let key = record_lock_key(&destination);
        with_record_lock(live_root, &key, || {
            let current = path_fingerprint(&destination)?;
            let before = original.get(&relative).copied();
            let desired = target.get(&relative).copied();
            if current == desired {
                reconcile_hydrated_index(live_root, &relative, &destination, desired);
                return Ok(());
            }
            if current != before {
                concurrent_change = true;
                return Ok(());
            }
            if desired.is_some() {
                let bytes = fs::read(target_root.join(&relative)).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to read owned recovery target {}: {error}",
                        target_root.join(&relative).display()
                    ))
                })?;
                replace_file_durably(&destination, &bytes)?;
            } else if destination.exists() {
                fs::remove_file(&destination).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to hydrate recovery deletion {}: {error}",
                        destination.display()
                    ))
                })?;
                if let Some(parent) = destination.parent() {
                    File::open(parent)
                        .and_then(|directory| directory.sync_all())
                        .map_err(|error| {
                            RefineError::Io(format!(
                                "failed to durably commit recovery deletion {}: {error}",
                                destination.display()
                            ))
                        })?;
                }
            }
            reconcile_hydrated_index(live_root, &relative, &destination, desired);
            Ok(())
        })?;
    }
    Ok(concurrent_change)
}

fn reconcile_hydrated_index(
    live_root: &std::path::Path,
    relative: &std::path::Path,
    destination: &std::path::Path,
    desired: Option<u64>,
) {
    if desired.is_some() {
        record_synchronized_goal(live_root, relative, destination);
    } else {
        forget_synchronized_goal(live_root, relative);
    }
}

pub(in crate::tools::host::git_sync) fn hydrate_remote_with_recovery_cas(
    original_root: &std::path::Path,
    remote_root: &std::path::Path,
    live_root: &std::path::Path,
) -> RefineResult<()> {
    if hydrate_recovery_target_with_cas(original_root, remote_root, live_root)? {
        let original = durable_state_map(original_root)?;
        let desired = durable_state_map(remote_root)?;
        let current = durable_state_map(live_root)?;
        let path = original
            .keys()
            .chain(desired.keys())
            .chain(current.keys())
            .find(|path| {
                current.get(*path) != original.get(*path)
                    && current.get(*path) != desired.get(*path)
            })
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unknown path".to_string());
        return Err(RefineError::Conflict(format!(
            "Live state changed at {path} during remote-authority recovery; no baseline was created."
        )));
    }
    Ok(())
}

fn path_fingerprint(path: &std::path::Path) -> RefineResult<Option<u64>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(state_content_fingerprint(&bytes))),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RefineError::Io(format!(
            "failed to inspect recovery state {}: {error}",
            path.display()
        ))),
    }
}
