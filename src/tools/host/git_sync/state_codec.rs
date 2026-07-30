use super::*;

#[derive(Debug)]
pub(super) struct StateWorktreeSetup {
    pub(super) path: PathBuf,
    pub(super) pulled: bool,
    pub(super) local_ahead: bool,
    pub(super) created: bool,
}

pub(super) type DurableStateMap = BTreeMap<PathBuf, u64>;

pub(super) fn bootstrap_only_state(state: &DurableStateMap) -> bool {
    state.keys().all(|path| {
        matches!(
            path.to_string_lossy().replace('\\', "/").as_str(),
            "refine.json"
                | "nodes.json"
                | "logs/activity.jsonl"
                | "quality/settings.json"
                | "quality/legacy-command-transition.json"
        )
    })
}

pub(super) fn remote_first_bootstrap_baseline(
    local: &DurableStateMap,
    remote: &DurableStateMap,
) -> DurableStateMap {
    local
        .iter()
        .filter(|(path, _)| remote.contains_key(*path))
        .map(|(path, fingerprint)| (path.clone(), *fingerprint))
        .collect()
}

/// Identity of a file as it was when its contents were last hashed.
///
/// Size alone is far too weak, and mtime can be preserved across a rewrite, so
/// the inode change time is included: it advances on any content or metadata
/// change and cannot be set backwards by a writer. A stale entry here would
/// make synchronization believe a changed record was untouched, so the guard is
/// deliberately stricter than a cache would otherwise need.
#[derive(Clone, Copy, PartialEq, Eq)]
struct HashedFileIdentity {
    size: u64,
    modified_unix_ns: Option<i128>,
    change_unix_ns: Option<i64>,
}

static STATE_CONTENT_HASHES: OnceLock<Mutex<BTreeMap<PathBuf, (HashedFileIdentity, u64)>>> =
    OnceLock::new();
/// Entries retained before the memo is dropped wholesale. It is a pure cache, so
/// discarding it costs only the next scan and keeps memory bounded on a project
/// far larger than the working set.
const STATE_CONTENT_HASH_CAPACITY: usize = 50_000;

fn hashed_file_identity(metadata: &fs::Metadata) -> HashedFileIdentity {
    #[cfg(unix)]
    let change_unix_ns = {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.ctime().saturating_mul(1_000_000_000) + i64::from(metadata.ctime_nsec() as i32))
    };
    #[cfg(not(unix))]
    let change_unix_ns = None;
    HashedFileIdentity {
        size: metadata.len(),
        modified_unix_ns: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as i128),
        change_unix_ns,
    }
}

/// Content hashes for every durable record under `root`.
///
/// Synchronization compares these across worktrees, where modification times
/// are unrelated, so content is what identity has to be built from — metadata
/// cannot stand in for it here as it does for projection staleness.
///
/// What can be avoided is re-reading files that have not moved. A full scan
/// otherwise costs the whole project on every sync, and syncs are frequent, so
/// a node paid to re-read its entire history each time one record changed.
/// Files whose identity is unchanged reuse their previous hash, leaving the
/// steady-state cost at one stat per record.
pub(super) fn durable_state_map(root: &std::path::Path) -> RefineResult<DurableStateMap> {
    if !root.exists() {
        return Ok(BTreeMap::new());
    }
    let mut files = Vec::new();
    collect_durable_state_files(root, root, &mut files)?;
    let memo = STATE_CONTENT_HASHES.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut memo = memo.lock().ok();
    if let Some(memo) = memo.as_deref_mut()
        && memo.len() > STATE_CONTENT_HASH_CAPACITY
    {
        memo.clear();
    }
    let mut state = BTreeMap::new();
    for path in files {
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let identity = fs::metadata(&path).ok().as_ref().map(hashed_file_identity);
        if let Some(identity) = identity
            && let Some(memo) = memo.as_deref_mut()
            && let Some((cached, hash)) = memo.get(&path)
            && *cached == identity
        {
            state.insert(relative, *hash);
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Refine state {}: {error}",
                path.display()
            ))
        })?;
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let hash = hasher.finish();
        // Re-stat after reading: a write that landed between the first stat and
        // the read would otherwise be memoized under the earlier identity and
        // never observed again.
        if let Some(memo) = memo.as_deref_mut()
            && let Some(settled) = fs::metadata(&path).ok().as_ref().map(hashed_file_identity)
            && Some(settled) == identity
        {
            memo.insert(path.clone(), (settled, hash));
        }
        state.insert(relative, hash);
    }
    Ok(state)
}

pub(super) fn state_conflicts(
    base: &DurableStateMap,
    local: &DurableStateMap,
    remote: &DurableStateMap,
    resolved: &BTreeSet<PathBuf>,
) -> Vec<String> {
    let paths = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    paths
        .into_iter()
        .filter(|path| {
            if resolved.contains(path) {
                return false;
            }
            let base_value = base.get(path);
            let local_value = local.get(path);
            let remote_value = remote.get(path);
            local_value != base_value && remote_value != base_value && local_value != remote_value
        })
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

pub(super) fn state_change_status(
    before: &DurableStateMap,
    after: &DurableStateMap,
) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|path| {
            let status = match (before.get(&path), after.get(&path)) {
                (None, Some(_)) => "A",
                (Some(_), None) => "D",
                (Some(left), Some(right)) if left != right => "M",
                _ => return None,
            };
            Some(format!(
                "{status}  .refine/{}",
                path.to_string_lossy().replace('\\', "/")
            ))
        })
        .collect()
}

pub(super) fn apply_local_state_delta(
    live_root: &std::path::Path,
    state_root: &std::path::Path,
    base: &DurableStateMap,
    local: &DurableStateMap,
    resolved: &BTreeSet<PathBuf>,
) -> RefineResult<()> {
    let paths = base
        .keys()
        .chain(local.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relative in paths {
        if resolved.contains(&relative) {
            continue;
        }
        if local.get(&relative) == base.get(&relative) {
            continue;
        }
        let destination = state_root.join(&relative);
        if local.contains_key(&relative) {
            copy_state_file(&live_root.join(&relative), &destination)?;
        } else if destination.exists() {
            fs::remove_file(&destination).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove synchronized Refine state {}: {error}",
                    destination.display()
                ))
            })?;
        }
    }
    Ok(())
}

pub(super) fn replace_live_durable_state(
    source_root: &std::path::Path,
    destination_root: &std::path::Path,
) -> RefineResult<()> {
    let existing = durable_state_map(destination_root)?;
    for relative in existing.keys() {
        let path = destination_root.join(relative);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to replace Refine state {}: {error}",
                    path.display()
                ))
            })?;
        }
    }
    let source = durable_state_map(source_root)?;
    for relative in source.keys() {
        copy_state_file(
            &source_root.join(relative),
            &destination_root.join(relative),
        )?;
    }
    Ok(())
}

pub(super) fn merge_state_into_live(
    source_root: &std::path::Path,
    live_root: &std::path::Path,
    original_local: &DurableStateMap,
) -> RefineResult<bool> {
    let source = durable_state_map(source_root)?;
    let current = durable_state_map(live_root)?;
    let concurrent_change = current != *original_local;
    let paths = source
        .keys()
        .chain(current.keys())
        .chain(original_local.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relative in paths {
        // A mutation that completed after this sync captured its local snapshot
        // wins this copy-back. The daemon will publish it in the next batch.
        if current.get(&relative) != original_local.get(&relative) {
            continue;
        }
        let destination = live_root.join(&relative);
        if source.contains_key(&relative) {
            copy_state_file(&source_root.join(&relative), &destination)?;
            record_synchronized_goal(live_root, &relative, &destination);
        } else if destination.exists() {
            fs::remove_file(&destination).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove synchronized Refine state {}: {error}",
                    destination.display()
                ))
            })?;
            forget_synchronized_goal(live_root, &relative);
        }
    }
    Ok(concurrent_change)
}

/// Keep the scheduler's view current for Goals arriving from another Node.
///
/// Local mutations update that view as they write, but synchronization copies
/// records straight into the live store without going through the write path.
/// Without this, work another Node published would be invisible to scheduling
/// until something unrelated forced a reconstruction.
fn record_synchronized_goal(
    live_root: &std::path::Path,
    relative: &std::path::Path,
    destination: &std::path::Path,
) {
    if !is_goal_record(relative) {
        return;
    }
    if let Err(error) = ActiveGoalIndex::record_goal(live_root, destination) {
        eprintln!(
            "refine: active Goal index was not updated for synchronized {}: {error}",
            relative.display()
        );
    }
}

fn forget_synchronized_goal(live_root: &std::path::Path, relative: &std::path::Path) {
    if !is_goal_record(relative) {
        return;
    }
    // The record is already gone, so identity comes from its sharded path
    // rather than from the file.
    let Some(goal_id) = goal_id_from_record_path(relative) else {
        return;
    };
    if let Err(error) = ActiveGoalIndex::forget_goal(live_root, &goal_id) {
        eprintln!(
            "refine: active Goal index still lists synchronized-away Goal {goal_id}: {error}"
        );
    }
}

fn is_goal_record(relative: &std::path::Path) -> bool {
    relative.file_name().and_then(|name| name.to_str()) == Some("goal.json")
}

/// `goals/<shard>/<rest>/goal.json` identifies Goal `<shard><rest>`.
fn goal_id_from_record_path(relative: &std::path::Path) -> Option<String> {
    let mut components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    components.pop()?;
    let rest = components.pop()?;
    let shard = components.pop()?;
    if components.last().map(String::as_str) != Some("goals") {
        return None;
    }
    Some(format!("{shard}{rest}"))
}

pub(super) fn copy_state_file(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> RefineResult<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create Refine state directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let parent = destination
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let temp = parent.join(format!(
        ".refine-sync-{}-{}",
        std::process::id(),
        STATE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::copy(source, &temp) {
        let _ = fs::remove_file(&temp);
        return Err(RefineError::Io(format!(
            "failed to copy Refine state {} to {}: {error}",
            source.display(),
            temp.display()
        )));
    }
    fs::rename(&temp, destination).map_err(|error| {
        let _ = fs::remove_file(&temp);
        RefineError::Io(format!(
            "failed to commit synchronized Refine state {}: {error}",
            destination.display()
        ))
    })
}

pub(super) fn state_commit_summary(status: &str) -> String {
    let mut goals = BTreeSet::new();
    let mut features = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut other = 0usize;
    for line in status.lines() {
        let path = line.get(3..).unwrap_or("").trim().replace('\\', "/");
        if let Some(record) = state_record_key(&path, ".refine/goals/") {
            goals.insert(record);
        } else if let Some(record) = state_record_key(&path, ".refine/features/") {
            features.insert(record);
        } else if let Some(record) = state_record_key(&path, ".refine/nodes/") {
            nodes.insert(record);
        } else {
            other += 1;
        }
    }
    let mut parts = Vec::new();
    if !goals.is_empty() {
        parts.push(format!("{} goal{}", goals.len(), plural(goals.len())));
    }
    if !features.is_empty() {
        parts.push(format!(
            "{} feature{}",
            features.len(),
            plural(features.len())
        ));
    }
    if !nodes.is_empty() {
        parts.push(format!("{} node{}", nodes.len(), plural(nodes.len())));
    }
    if other > 0 || parts.is_empty() {
        parts.push(format!("{other} other file{}", plural(other)));
    }
    format!("Sync Refine state: {}", parts.join(", "))
}

pub(super) fn state_record_key(path: &str, prefix: &str) -> Option<String> {
    let relative = path.strip_prefix(prefix)?;
    std::path::Path::new(relative)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .or_else(|| Some(relative.to_string()))
}

pub(super) fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
