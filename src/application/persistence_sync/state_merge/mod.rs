//! Deterministic structural three-way merge for `refine/state` files.
//!
//! The driver is a merge driver, not a judge: it merges only what it can
//! prove disjoint from the real merge base and returns `None` for anything
//! contested. One-sided member changes merge; both-changed-same-member stays
//! conflicted; Rounds and every other identity-free ordered array are atomic;
//! a Goal's top-level `updated` timestamp takes the later value once every
//! other member merges. `nodes.json` merges as a record union keyed by
//! canonical node id. No ownership arbitration and no schema-validity veto
//! live here — judgment belongs to resolution, not to the driver.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::model::fleet::valid_node_id;
use crate::model::node::NodeRegistry;

/// Merge one state file from its real three-way operands. `None` means the
/// path stays conflicted and must be resolved above the driver.
pub fn merge_state_file(
    relative: &Path,
    base: &[u8],
    local: &[u8],
    remote: &[u8],
) -> Option<Vec<u8>> {
    if relative == Path::new("nodes.json") {
        return merge_node_registry(base, local, remote);
    }
    if relative
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("json")
    {
        return None;
    }
    let base = serde_json::from_slice::<serde_json::Value>(base).ok()?;
    let local = serde_json::from_slice::<serde_json::Value>(local).ok()?;
    let remote = serde_json::from_slice::<serde_json::Value>(remote).ok()?;
    let depth = if is_goal_record(relative) {
        let base_id = base.get("id")?.as_str()?;
        if base_id.is_empty()
            || local.get("id").and_then(serde_json::Value::as_str) != Some(base_id)
            || remote.get("id").and_then(serde_json::Value::as_str) != Some(base_id)
        {
            return None;
        }
        GoalDepth::Root
    } else {
        GoalDepth::Deep
    };
    let merged = merge_json_value(&base, &local, &remote, None, depth)?;
    encode_json(&merged)
}

/// Merge a state file both sides added independently — an empty-tree merge
/// base joining unrelated bootstrap histories. Only the node registry has an
/// identity-keyed empty form to merge from; everything else stays contested.
pub fn merge_added_state_file(relative: &Path, local: &[u8], remote: &[u8]) -> Option<Vec<u8>> {
    if relative == Path::new("nodes.json") {
        return merge_node_registry(br#"{"nodes":[]}"#, local, remote);
    }
    None
}

/// Where a value sits relative to a goal record's top level. The `updated`
/// last-writer-wins and `notes` keyed-merge rules apply only to direct
/// members of a goal record (`Member`), never to like-named nested members
/// or to other state files.
#[derive(Clone, Copy, Eq, PartialEq)]
enum GoalDepth {
    Root,
    Member,
    Deep,
}

impl GoalDepth {
    fn child(self) -> Self {
        match self {
            Self::Root => Self::Member,
            _ => Self::Deep,
        }
    }
}

fn is_goal_record(relative: &Path) -> bool {
    relative.file_name().and_then(|name| name.to_str()) == Some("goal.json")
}

fn encode_json(value: &serde_json::Value) -> Option<Vec<u8>> {
    let mut encoded = serde_json::to_vec_pretty(value).ok()?;
    encoded.push(b'\n');
    Some(encoded)
}

fn merge_json_value(
    base: &serde_json::Value,
    local: &serde_json::Value,
    remote: &serde_json::Value,
    field: Option<&str>,
    depth: GoalDepth,
) -> Option<serde_json::Value> {
    if local == remote {
        return Some(local.clone());
    }
    if local == base {
        return Some(remote.clone());
    }
    if remote == base {
        return Some(local.clone());
    }
    if depth == GoalDepth::Member && field == Some("updated") {
        return later_timestamp(local, remote);
    }
    if depth == GoalDepth::Member && field == Some("notes") {
        return merge_keyed_notes(base, local, remote);
    }
    let (Some(base), Some(local_object), Some(remote_object)) =
        (base.as_object(), local.as_object(), remote.as_object())
    else {
        // Arrays without stable identity (Rounds included) and scalars are
        // atomic: a two-sided change is contested by construction.
        return None;
    };
    let keys = base
        .keys()
        .chain(local_object.keys())
        .chain(remote_object.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut merged = serde_json::Map::new();
    for key in keys {
        if let Some(value) = merge_json_member(
            base.get(&key),
            local_object.get(&key),
            remote_object.get(&key),
            &key,
            depth.child(),
        )? {
            merged.insert(key, value);
        }
    }
    Some(serde_json::Value::Object(merged))
}

fn merge_json_member(
    base: Option<&serde_json::Value>,
    local: Option<&serde_json::Value>,
    remote: Option<&serde_json::Value>,
    field: &str,
    depth: GoalDepth,
) -> Option<Option<serde_json::Value>> {
    if local == remote {
        return Some(local.cloned());
    }
    if local == base {
        return Some(remote.cloned());
    }
    if remote == base {
        return Some(local.cloned());
    }
    match (base, local, remote) {
        (Some(base), Some(local), Some(remote)) => {
            merge_json_value(base, local, remote, Some(field), depth).map(Some)
        }
        // A contested addition or removal has no three-way form to merge.
        _ => None,
    }
}

fn merge_keyed_notes(
    base: &serde_json::Value,
    local: &serde_json::Value,
    remote: &serde_json::Value,
) -> Option<serde_json::Value> {
    let keyed = |value: &serde_json::Value| {
        let mut notes = BTreeMap::new();
        for note in value.as_array()? {
            let id = note.get("id")?.as_str()?;
            if id.is_empty() || notes.insert(id.to_string(), note.clone()).is_some() {
                return None;
            }
        }
        Some(notes)
    };
    let base = keyed(base)?;
    let local = keyed(local)?;
    let remote = keyed(remote)?;
    let ids = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut merged = Vec::new();
    for id in ids {
        if let Some(note) = merge_json_member(
            base.get(&id),
            local.get(&id),
            remote.get(&id),
            "note",
            GoalDepth::Deep,
        )? {
            merged.push(note);
        }
    }
    Some(serde_json::Value::Array(merged))
}

fn later_timestamp(
    local: &serde_json::Value,
    remote: &serde_json::Value,
) -> Option<serde_json::Value> {
    let local_time = chrono::DateTime::parse_from_rfc3339(local.as_str()?).ok()?;
    let remote_time = chrono::DateTime::parse_from_rfc3339(remote.as_str()?).ok()?;
    Some(if local_time >= remote_time {
        local.clone()
    } else {
        remote.clone()
    })
}

#[derive(Clone)]
struct RegistryNode {
    value: serde_json::Value,
    updated_at: chrono::DateTime<chrono::FixedOffset>,
}

/// Merge a shared node registry by stable node id.
///
/// A missing record is not a deletion signal: independently observed nodes are
/// retained. When both sides changed one record, its parseable `updated_at`
/// establishes last-writer-wins authority. Equal-time disagreement and any
/// registry whose ids or timestamps are ambiguous remain ordinary sync
/// conflicts.
fn merge_node_registry(base: &[u8], local: &[u8], remote: &[u8]) -> Option<Vec<u8>> {
    let base = validated_nodes(base)?;
    let local = validated_nodes(local)?;
    let remote = validated_nodes(remote)?;
    let ids = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut nodes = Vec::with_capacity(ids.len());
    for id in ids {
        nodes.push(merge_node(base.get(&id), local.get(&id), remote.get(&id))?);
    }
    let merged = serde_json::json!({ "nodes": nodes });
    serde_json::from_value::<NodeRegistry>(merged.clone()).ok()?;
    encode_json(&merged)
}

fn validated_nodes(bytes: &[u8]) -> Option<BTreeMap<String, RegistryNode>> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    let registry = serde_json::from_value::<NodeRegistry>(value.clone()).ok()?;
    let raw_nodes = value.get("nodes")?.as_array()?;
    if raw_nodes.len() != registry.nodes.len() {
        return None;
    }
    let mut nodes = BTreeMap::new();
    for (node, value) in registry.nodes.into_iter().zip(raw_nodes) {
        let updated_at = chrono::DateTime::parse_from_rfc3339(&node.updated_at).ok()?;
        if !canonical_node_id(&node.id)
            || value.get("id").and_then(serde_json::Value::as_str) != Some(node.id.as_str())
            || nodes
                .insert(
                    node.id,
                    RegistryNode {
                        value: value.clone(),
                        updated_at,
                    },
                )
                .is_some()
        {
            return None;
        }
    }
    Some(nodes)
}

fn canonical_node_id(id: &str) -> bool {
    id == id.trim() && valid_node_id(id)
}

fn merge_node(
    base: Option<&RegistryNode>,
    local: Option<&RegistryNode>,
    remote: Option<&RegistryNode>,
) -> Option<serde_json::Value> {
    match (base, local, remote) {
        (_, Some(local), Some(remote)) if local.value == remote.value => Some(local.value.clone()),
        (Some(base), Some(local), Some(remote)) if local.value == base.value => {
            Some(remote.value.clone())
        }
        (Some(base), Some(local), Some(remote)) if remote.value == base.value => {
            Some(local.value.clone())
        }
        (_, Some(local), Some(remote)) => later_node(local, remote),
        (_, Some(local), None) => Some(local.value.clone()),
        (_, None, Some(remote)) => Some(remote.value.clone()),
        (Some(base), None, None) => Some(base.value.clone()),
        (None, None, None) => None,
    }
}

fn later_node(local: &RegistryNode, remote: &RegistryNode) -> Option<serde_json::Value> {
    match local.updated_at.cmp(&remote.updated_at) {
        std::cmp::Ordering::Greater => Some(local.value.clone()),
        std::cmp::Ordering::Less => Some(remote.value.clone()),
        std::cmp::Ordering::Equal => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn goal(status: &str, title: &str, updated: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "GOALA",
            "name": "GOALA",
            "title": title,
            "status": status,
            "node_id": "node-a",
            "created": "2026-08-03T18:00:00Z",
            "updated": updated,
            "notes": [],
            "rounds": []
        })
    }

    fn goal_path() -> PathBuf {
        PathBuf::from("goals/GO/ALA/goal.json")
    }

    fn merge_goal(
        base: &serde_json::Value,
        local: &serde_json::Value,
        remote: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        merge_state_file(
            &goal_path(),
            &serde_json::to_vec(base).unwrap(),
            &serde_json::to_vec(local).unwrap(),
            &serde_json::to_vec(remote).unwrap(),
        )
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn disjoint_member_changes_merge_and_updated_takes_the_later_value() {
        let base = goal("todo", "Base", "2026-08-03T18:20:00Z");
        let local = goal("done", "Base", "2026-08-03T18:22:00Z");
        let remote = goal("todo", "Clarified", "2026-08-03T18:21:00Z");

        let merged = merge_goal(&base, &local, &remote).unwrap();
        assert_eq!(merged["status"], "done");
        assert_eq!(merged["title"], "Clarified");
        assert_eq!(merged["updated"], "2026-08-03T18:22:00Z");
    }

    #[test]
    fn both_changed_same_member_stays_conflicted() {
        let base = goal("todo", "Base", "2026-08-03T18:20:00Z");
        let local = goal("done", "Base", "2026-08-03T18:21:00Z");
        let remote = goal("cancelled", "Base", "2026-08-03T18:22:00Z");
        assert!(merge_goal(&base, &local, &remote).is_none());
    }

    #[test]
    fn rounds_are_atomic_one_sided_merges_two_sided_conflicts() {
        let base = goal("todo", "Base", "2026-08-03T18:20:00Z");
        let round = |prompt: &str| {
            serde_json::json!([{
                "reporter": "A",
                "prompt": prompt,
                "created": "2026-08-03T18:21:00Z",
                "updated": "2026-08-03T18:21:00Z"
            }])
        };
        let mut local = base.clone();
        local["rounds"] = round("Implement");
        let mut remote = base.clone();
        remote["title"] = serde_json::json!("Clarified");

        let merged = merge_goal(&base, &local, &remote).unwrap();
        assert_eq!(merged["rounds"].as_array().unwrap().len(), 1);
        assert_eq!(merged["title"], "Clarified");

        let mut contested = base.clone();
        contested["rounds"] = round("Different");
        assert!(merge_goal(&base, &local, &contested).is_none());
    }

    #[test]
    fn notes_merge_by_stable_id_and_contested_notes_conflict() {
        let base = goal("todo", "Base", "2026-08-03T18:20:00Z");
        let note = |id: &str, body: &str| {
            serde_json::json!({
                "id": id,
                "author": "A",
                "body": body,
                "created": "2026-08-03T18:21:00Z",
                "updated": "2026-08-03T18:21:00Z"
            })
        };
        let mut local = base.clone();
        local["notes"] = serde_json::json!([note("NOTE-A", "local")]);
        let mut remote = base.clone();
        remote["notes"] = serde_json::json!([note("NOTE-B", "remote")]);

        let merged = merge_goal(&base, &local, &remote).unwrap();
        let ids = merged["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value["id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["NOTE-A", "NOTE-B"]);

        let mut contested = base.clone();
        contested["notes"] = serde_json::json!([note("NOTE-A", "remote")]);
        assert!(merge_goal(&base, &local, &contested).is_none());
    }

    #[test]
    fn updated_last_writer_wins_applies_only_to_goal_top_level_members() {
        let mut base = goal("todo", "Base", "2026-08-03T18:20:00Z");
        base["context"] = serde_json::json!({ "updated": "2026-08-03T18:20:00Z" });
        let mut local = base.clone();
        local["context"] = serde_json::json!({ "updated": "2026-08-03T18:21:00Z" });
        let mut remote = base.clone();
        remote["context"] = serde_json::json!({ "updated": "2026-08-03T18:22:00Z" });
        assert!(
            merge_goal(&base, &local, &remote).is_none(),
            "a nested both-changed `updated` member is contested, not last-writer-wins"
        );

        let record =
            |updated: &str| serde_json::to_vec(&serde_json::json!({ "updated": updated })).unwrap();
        assert!(
            merge_state_file(
                Path::new("shared.json"),
                &record("2026-08-03T18:20:00Z"),
                &record("2026-08-03T18:21:00Z"),
                &record("2026-08-03T18:22:00Z"),
            )
            .is_none(),
            "non-goal state files get no `updated` last-writer-wins rule"
        );
    }

    #[test]
    fn independently_added_node_registries_union_and_other_additions_conflict() {
        let node = |id: &str| {
            serde_json::json!({
                "id": id,
                "display_name": id,
                "created_at": "2026-08-17T08:00:00Z",
                "updated_at": "2026-08-17T08:00:00Z",
                "health": { "status": "unknown", "checked_at": "2026-08-17T08:00:00Z", "details": null }
            })
        };
        let registry = |nodes: Vec<serde_json::Value>| {
            serde_json::to_vec(&serde_json::json!({ "nodes": nodes })).unwrap()
        };
        let merged = merge_added_state_file(
            Path::new("nodes.json"),
            &registry(vec![node("node-a")]),
            &registry(vec![node("node-b")]),
        )
        .unwrap();
        let merged: serde_json::Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(merged["nodes"][0]["id"], "node-a");
        assert_eq!(merged["nodes"][1]["id"], "node-b");

        assert!(
            merge_added_state_file(&goal_path(), b"{\"id\":\"A\"}", b"{\"id\":\"B\"}").is_none()
        );
    }

    #[test]
    fn goal_identity_mismatch_and_non_json_stay_conflicted() {
        let base = goal("todo", "Base", "2026-08-03T18:20:00Z");
        let mut other = base.clone();
        other["id"] = serde_json::json!("GOALB");
        assert!(merge_goal(&base, &other, &base.clone()).is_none());
        assert!(
            merge_state_file(Path::new("report.md"), b"base\n", b"local\n", b"remote\n").is_none()
        );
    }

    mod nodes {
        use super::*;

        fn node(id: &str, updated_at: &str, status: &str) -> serde_json::Value {
            serde_json::json!({
                "id": id,
                "display_name": id,
                "created_at": "2026-08-17T08:00:00Z",
                "updated_at": updated_at,
                "health": {
                    "status": status,
                    "checked_at": updated_at,
                    "details": null
                }
            })
        }

        fn registry(nodes: Vec<serde_json::Value>) -> Vec<u8> {
            serde_json::to_vec(&serde_json::json!({ "nodes": nodes })).unwrap()
        }

        fn merged_nodes(
            base: Vec<serde_json::Value>,
            local: Vec<serde_json::Value>,
            remote: Vec<serde_json::Value>,
        ) -> Vec<crate::model::node::Node> {
            let merged = merge_state_file(
                Path::new("nodes.json"),
                &registry(base),
                &registry(local),
                &registry(remote),
            )
            .unwrap();
            serde_json::from_slice::<NodeRegistry>(&merged)
                .unwrap()
                .nodes
        }

        #[test]
        fn disjoint_heartbeat_updates_merge_by_node_id() {
            let node_a = node("node-a", "2026-08-17T08:00:00Z", "unknown");
            let node_b = node("node-b", "2026-08-17T08:00:00Z", "unknown");
            let local_a = node("node-a", "2026-08-17T08:01:00Z", "healthy");
            let remote_b = node("node-b", "2026-08-17T08:02:00Z", "healthy");

            let merged = merged_nodes(
                vec![node_a.clone(), node_b.clone()],
                vec![local_a, node_b],
                vec![node_a, remote_b],
            );

            assert_eq!(merged[0].updated_at, "2026-08-17T08:01:00Z");
            assert_eq!(merged[1].updated_at, "2026-08-17T08:02:00Z");
        }

        #[test]
        fn registry_merge_retains_record_union_and_one_sided_changes() {
            let base_a = node("node-a", "2026-08-17T08:00:00Z", "unknown");
            let local_a = node("node-a", "2026-08-17T08:01:00Z", "healthy");
            let merged = merged_nodes(
                vec![base_a.clone()],
                vec![local_a, node("node-b", "2026-08-17T08:01:00Z", "healthy")],
                vec![base_a, node("node-c", "2026-08-17T08:01:00Z", "healthy")],
            );

            assert_eq!(
                merged
                    .iter()
                    .map(|node| node.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["node-a", "node-b", "node-c"]
            );
            assert_eq!(merged[0].updated_at, "2026-08-17T08:01:00Z");
        }

        #[test]
        fn missing_records_do_not_infer_node_deletion() {
            let base_a = node("node-a", "2026-08-17T08:00:00Z", "unknown");
            let merged = merged_nodes(vec![base_a.clone()], vec![], vec![base_a]);

            assert_eq!(merged.len(), 1);
            assert_eq!(merged[0].id, "node-a");
        }

        #[test]
        fn concurrent_record_change_uses_later_timestamp() {
            let base = node("node-a", "2026-08-17T08:00:00Z", "unknown");
            let merged = merged_nodes(
                vec![base.clone()],
                vec![node("node-a", "2026-08-17T08:03:00Z", "local")],
                vec![node("node-a", "2026-08-17T08:02:00Z", "remote")],
            );

            assert_eq!(merged[0].health.as_ref().unwrap().status, "local");
        }

        #[test]
        fn registry_merge_preserves_unmodeled_fields_on_the_selected_record() {
            let base = node("node-a", "2026-08-17T08:00:00Z", "unknown");
            let mut local = node("node-a", "2026-08-17T08:01:00Z", "healthy");
            local["future_capability"] = serde_json::json!({ "enabled": true });

            let merged = merge_state_file(
                Path::new("nodes.json"),
                &registry(vec![base.clone()]),
                &registry(vec![local]),
                &registry(vec![base]),
            )
            .unwrap();
            let merged: serde_json::Value = serde_json::from_slice(&merged).unwrap();

            assert_eq!(
                merged["nodes"][0]["future_capability"],
                serde_json::json!({ "enabled": true })
            );
        }

        #[test]
        fn equal_timestamp_disagreement_is_ambiguous() {
            let base = registry(vec![node("node-a", "2026-08-17T08:00:00Z", "unknown")]);
            let local = registry(vec![node("node-a", "2026-08-17T08:01:00Z", "local")]);
            let remote = registry(vec![node("node-a", "2026-08-17T08:01:00Z", "remote")]);

            assert!(merge_state_file(Path::new("nodes.json"), &base, &local, &remote).is_none());
        }

        #[test]
        fn malformed_duplicate_noncanonical_or_uncomparable_registries_are_rejected() {
            let valid = registry(vec![node("node-a", "2026-08-17T08:00:00Z", "unknown")]);
            let invalid = [
                b"not json".to_vec(),
                registry(vec![
                    node("node-a", "2026-08-17T08:01:00Z", "one"),
                    node("node-a", "2026-08-17T08:02:00Z", "two"),
                ]),
                registry(vec![node(" Node-A ", "2026-08-17T08:01:00Z", "bad")]),
                registry(vec![node("-node-a", "2026-08-17T08:01:00Z", "bad")]),
                registry(vec![node("_node-a", "2026-08-17T08:01:00Z", "bad")]),
                registry(vec![node("node-a", "later", "bad")]),
            ];

            for local in invalid {
                assert!(
                    merge_state_file(Path::new("nodes.json"), &valid, &local, &valid).is_none()
                );
            }
        }
    }
}
