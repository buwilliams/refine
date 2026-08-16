use super::*;

pub(super) fn collect_durable_state_files(
    root: &std::path::Path,
    current: &std::path::Path,
    files: &mut Vec<PathBuf>,
) -> RefineResult<()> {
    for entry in fs::read_dir(current).map_err(|error| {
        RefineError::Io(format!(
            "failed to inspect durable Refine state {}: {error}",
            current.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect durable Refine state entry: {error}"
            ))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if is_excluded_from_durable_state(relative) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect durable Refine state {}: {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            collect_durable_state_files(root, &path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn remove_excluded_state_files(root: &std::path::Path) -> RefineResult<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut removed = Vec::new();
    remove_excluded_state_files_from(root, root, &mut removed)?;
    Ok(removed)
}

pub(super) fn remove_excluded_state_files_from(
    root: &std::path::Path,
    current: &std::path::Path,
    removed: &mut Vec<PathBuf>,
) -> RefineResult<()> {
    for entry in fs::read_dir(current).map_err(|error| {
        RefineError::Io(format!(
            "failed to inspect synchronized Refine state {}: {error}",
            current.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect synchronized Refine state entry: {error}"
            ))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        // Resolve the entry type before applying the exclusion. Runtime-only
        // rules match on a leading component, so a directory such as `runtime/`
        // satisfies the predicate; attempting to unlink it as a file would fail
        // the whole sync. Directories are descended into instead, which removes
        // the excluded files they contain.
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to inspect synchronized Refine state {}: {error}",
                    path.display()
                )));
            }
        };
        if file_type.is_dir() {
            remove_excluded_state_files_from(root, &path, removed)?;
            continue;
        }
        if is_excluded_from_durable_state(relative) {
            match fs::remove_file(&path) {
                Ok(()) => removed.push(relative.to_path_buf()),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove excluded Refine state {}: {error}",
                        path.display()
                    )));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn is_transient_refine_path(path: &std::path::Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    file_name.ends_with(".lock")
        || file_name.ends_with(".tmp")
        || file_name.starts_with(".refine-sync-")
}

pub(super) fn is_runtime_only_refine_path(path: &std::path::Path) -> bool {
    if matches!(
        path.components()
            .next()
            .and_then(|component| component.as_os_str().to_str()),
        Some("run" | "runtime" | "logs" | "support-bundles" | "provider-bin")
    ) || path == std::path::Path::new("manage-app.log")
    {
        return true;
    }
    // The active-node selection is machine identity and must never replicate:
    // one synced copy pins the whole fleet to a single node id, and the
    // ownership guards then refuse every other node's goals at Quality. The
    // canonical selection lives under `runtime/` (excluded above); this rule
    // covers the legacy root-level location so an already-replicated copy is
    // purged from live state on the next sync instead of poisoning identity.
    if path == std::path::Path::new(crate::tools::product::nodes::ACTIVE_NODE_FILE) {
        return true;
    }
    // Chat sessions are node-local conversation evidence: the record embeds
    // its full transcript and is rewritten on every provider output line, so
    // synchronizing it committed a fresh full blob of each active session per
    // sync tick — the largest growth source on `refine/state` and a tax on
    // every later sync.
    let mut components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str());
    if components.next() == Some("chat") && components.next() == Some("sessions") {
        return true;
    }
    // Per-Goal log sidecars sit beside the Goal record at
    // `goals/<shard>/<id>/logs.jsonl`, so the leading-component rule above never
    // matched them and every node published its agent logs to `refine/state`.
    // Logs are node-local evidence; only the Goal record and its implementation
    // report belong in synchronized state.
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    file_name == "logs.jsonl" || file_name.ends_with(".logs.jsonl")
}

/// Paths that must never appear in synchronized durable state, whether because
/// they are node-local runtime evidence or in-flight temporary files.
pub(super) fn is_excluded_from_durable_state(path: &std::path::Path) -> bool {
    is_runtime_only_refine_path(path) || is_transient_refine_path(path)
}

pub(super) fn append_output_detail(details: &mut Vec<String>, output: &GitCommandOutput) {
    for text in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(text).trim().to_string();
        if !text.is_empty() {
            details.push(text);
        }
    }
}

pub(super) fn nonempty_detail(details: Vec<String>) -> Option<String> {
    let detail = details.join("\n");
    (!detail.is_empty()).then_some(detail)
}

pub(super) fn push_rejected_by_race(output: &GitCommandOutput) -> bool {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    text.contains("rejected") || text.contains("non-fast-forward") || text.contains("fetch first")
}

pub(super) fn command_failed(command: &str, output: &GitCommandOutput) -> RefineError {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    RefineError::Conflict(format!(
        "{command} failed{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    ))
}
