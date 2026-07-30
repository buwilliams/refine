use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

use super::helpers::{RESIDENT_SEARCH_TEXT_LIMIT, goal_searchable_parts};

/// Cached full text for one Goal record, valid while the record has not moved.
struct CachedText {
    size: u64,
    modified_unix_ns: Option<i128>,
    lowercased: String,
}

static DEEP_TEXT_CACHE: OnceLock<Mutex<BTreeMap<PathBuf, CachedText>>> = OnceLock::new();
/// Entries kept before the cache is dropped wholesale. It is a pure cache, so
/// discarding it costs only the next query and keeps a large project from
/// pinning every Goal it has ever searched.
const DEEP_TEXT_CACHE_CAPACITY: usize = 4_096;

/// Text a Goal can be matched on beyond what the projection keeps resident.
///
/// The resident tier is a prefix, so a query it cannot decide has to consult
/// the record itself rather than conclude the Goal does not match. This is what
/// lets the projection stop carrying every note body and Round prompt without
/// making older text unsearchable.
///
/// Only reached for Goals that already passed every cheap filter and that the
/// resident prefix did not settle, so a scoped query normally reads nothing.
pub(super) fn goal_matches_deep_text(
    refine_dir: Option<&Path>,
    json_path: &str,
    lowercased_query: &str,
) -> bool {
    let Some(refine_dir) = refine_dir else {
        // Snapshots built without a source directory — test fixtures and
        // detached projections — have nothing further to consult. Reporting no
        // match keeps them to the resident tier rather than inventing one.
        return false;
    };
    let path = refine_dir.join(json_path);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return false;
    };
    let size = metadata.len();
    let modified_unix_ns = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);

    let cache = DEEP_TEXT_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(cached) = cache.get(&path)
        && cached.size == size
        && cached.modified_unix_ns == modified_unix_ns
    {
        return cached.lowercased.contains(lowercased_query);
    }

    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let lowercased = goal_searchable_parts(object).join("\n").to_lowercase();
    let matched = lowercased.contains(lowercased_query);
    if let Ok(mut cache) = cache.lock() {
        if cache.len() >= DEEP_TEXT_CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(
            path,
            CachedText {
                size,
                modified_unix_ns,
                lowercased,
            },
        );
    }
    matched
}

/// Whether the resident prefix is complete for this Goal.
///
/// A prefix shorter than the budget is the whole text, so a miss against it is
/// a real miss and the record need not be opened. This is what keeps ordinary
/// Goals — the ones with a short description and no long Round history — off
/// the read path entirely.
pub(super) fn resident_text_is_complete(searchable_text: &str) -> bool {
    // Truncation only happens at the budget, so anything shorter is whole. Text
    // landing exactly on the budget is treated as possibly truncated, which
    // costs one needless read rather than a wrong answer.
    searchable_text.len() < RESIDENT_SEARCH_TEXT_LIMIT
}

/// Full text match for a Goal, resident tier first.
///
/// Exposed for callers outside the projection query layer that match Goals on
/// text and must see the same corpus, so the resident prefix does not silently
/// narrow what they can find.
pub fn goal_text_matches(
    refine_dir: Option<&Path>,
    json_path: &str,
    resident_text: &str,
    lowercased_query: &str,
) -> bool {
    if resident_text.to_lowercase().contains(lowercased_query) {
        return true;
    }
    if resident_text_is_complete(resident_text) {
        return false;
    }
    goal_matches_deep_text(refine_dir, json_path, lowercased_query)
}
