use super::*;
use crate::model::fleet::valid_node_id;
use crate::model::node::NodeRegistry;

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
pub(super) fn merge_node_registry(base: &[u8], local: &[u8], remote: &[u8]) -> Option<Vec<u8>> {
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
    let mut encoded = serde_json::to_vec_pretty(&merged).ok()?;
    encoded.push(b'\n');
    Some(encoded)
}

/// Reconcile two validated registries when a recorded three-way baseline can
/// no longer be reconstructed. This authority is deliberately narrower than
/// the normal merge: every unequal shared record needs a strictly later,
/// comparable `updated_at`; absent records are retained as a union.
pub(super) fn merge_node_registry_without_base(local: &[u8], remote: &[u8]) -> Option<Vec<u8>> {
    let local = validated_nodes(local)?;
    let remote = validated_nodes(remote)?;
    let ids = local
        .keys()
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut nodes = Vec::with_capacity(ids.len());
    for id in ids {
        let merged = match (local.get(&id), remote.get(&id)) {
            (Some(local), Some(remote)) if local.value == remote.value => local.value.clone(),
            (Some(local), Some(remote)) => later_node(local, remote)?,
            (Some(local), None) => local.value.clone(),
            (None, Some(remote)) => remote.value.clone(),
            (None, None) => unreachable!(),
        };
        nodes.push(merged);
    }
    let merged = serde_json::json!({ "nodes": nodes });
    serde_json::from_value::<NodeRegistry>(merged.clone()).ok()?;
    let mut encoded = serde_json::to_vec_pretty(&merged).ok()?;
    encoded.push(b'\n');
    Some(encoded)
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
        let merged =
            merge_node_registry(&registry(base), &registry(local), &registry(remote)).unwrap();
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

        let merged = merge_node_registry(
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

        assert!(merge_node_registry(&base, &local, &remote).is_none());
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
            assert!(merge_node_registry(&valid, &local, &valid).is_none());
        }
    }

    #[test]
    fn baseline_less_merge_keeps_union_and_uses_only_strictly_later_records() {
        let local = registry(vec![
            node("node-a", "2026-08-17T08:03:00Z", "local"),
            node("node-b", "2026-08-17T08:00:00Z", "local-only"),
        ]);
        let remote = registry(vec![
            node("node-a", "2026-08-17T08:02:00Z", "remote"),
            node("node-c", "2026-08-17T08:00:00Z", "remote-only"),
        ]);

        let merged: NodeRegistry =
            serde_json::from_slice(&merge_node_registry_without_base(&local, &remote).unwrap())
                .unwrap();

        assert_eq!(
            merged
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["node-a", "node-b", "node-c"]
        );
        assert_eq!(merged.nodes[0].health.as_ref().unwrap().status, "local");
    }

    #[test]
    fn baseline_less_merge_rejects_equal_time_disagreement_and_malformed_input() {
        let local = registry(vec![node("node-a", "2026-08-17T08:01:00Z", "local")]);
        let remote = registry(vec![node("node-a", "2026-08-17T08:01:00Z", "remote")]);
        assert!(merge_node_registry_without_base(&local, &remote).is_none());
        assert!(merge_node_registry_without_base(b"not json", &remote).is_none());
    }
}
