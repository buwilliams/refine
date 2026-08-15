use super::*;

pub(in crate::tools::host::git_sync) fn hydrate_remote_with_recovery_cas(
    original_root: &std::path::Path,
    remote_root: &std::path::Path,
    live_root: &std::path::Path,
) -> RefineResult<()> {
    let original = durable_state_map(original_root)?;
    let remote = durable_state_map(remote_root)?;
    let paths = original
        .keys()
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relative in paths {
        let current = path_fingerprint(&live_root.join(&relative))?;
        let before = original.get(&relative).copied();
        let desired = remote.get(&relative).copied();
        if current == desired {
            // File replacement and the derived scheduler-index append cannot
            // be one filesystem transaction. Reconcile the index even when a
            // retry finds that an interrupted apply already settled the file.
            if desired.is_some() {
                record_synchronized_goal(live_root, &relative, &live_root.join(&relative));
            } else {
                forget_synchronized_goal(live_root, &relative);
            }
            continue;
        }
        if current != before {
            return Err(RefineError::Conflict(format!(
                "Live state changed at {} during remote-authority recovery; no baseline was created.",
                relative.display()
            )));
        }
        let destination = live_root.join(&relative);
        if desired.is_some() {
            copy_state_file(&remote_root.join(&relative), &destination)?;
            record_synchronized_goal(live_root, &relative, &destination);
        } else if destination.exists() {
            fs::remove_file(&destination).map_err(|error| {
                RefineError::Io(format!(
                    "failed to hydrate remote-authority deletion {}: {error}",
                    destination.display()
                ))
            })?;
            forget_synchronized_goal(live_root, &relative);
        }
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
