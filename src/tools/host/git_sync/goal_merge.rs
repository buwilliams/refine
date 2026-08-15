use super::*;
use crate::model::goal::Goal;

/// Merge independently changed members in one Goal record without inventing
/// identity for ordered workflow arrays.
///
/// Goal persistence rewrites the complete JSON document even for a single
/// mutation. This policy admits ordinary object-member changes and keyed Note
/// changes, but treats Rounds and every other identity-free array as atomic.
/// The result must still deserialize as a Goal and satisfy the workflow-family
/// fence below before synchronization may call the path resolved.
pub(super) fn merge_goal_record(base: &[u8], local: &[u8], remote: &[u8]) -> Option<Vec<u8>> {
    let base = serde_json::from_slice::<serde_json::Value>(base).ok()?;
    let local = serde_json::from_slice::<serde_json::Value>(local).ok()?;
    let remote = serde_json::from_slice::<serde_json::Value>(remote).ok()?;
    let base_id = base.get("id")?.as_str()?;
    if base_id.is_empty()
        || local.get("id").and_then(serde_json::Value::as_str) != Some(base_id)
        || remote.get("id").and_then(serde_json::Value::as_str) != Some(base_id)
    {
        return None;
    }
    reject_cross_node_round_authority(&base, &local, &remote)?;
    let (local, remote) = resolve_queued_reassignment_start_race(&base, local, remote)?;
    let merged = merge_json_value(&base, &local, &remote, None)?;
    validate_merged_goal(&merged, base_id)?;
    let mut encoded = serde_json::to_vec_pretty(&merged).ok()?;
    encoded.push(b'\n');
    Some(encoded)
}

fn reject_cross_node_round_authority(
    base: &serde_json::Value,
    local: &serde_json::Value,
    remote: &serde_json::Value,
) -> Option<()> {
    let local_rounds_changed = local.get("rounds") != base.get("rounds");
    let remote_rounds_changed = remote.get("rounds") != base.get("rounds");
    let authority_fields = ["status", "node_id", "branch_name", "rounds"];
    let local_authority_changed = authority_fields
        .iter()
        .any(|field| local.get(*field) != base.get(*field));
    let remote_authority_changed = authority_fields
        .iter()
        .any(|field| remote.get(*field) != base.get(*field));
    if (local_rounds_changed && remote_authority_changed)
        || (remote_rounds_changed && local_authority_changed)
    {
        return None;
    }
    Some(())
}

/// Resolve the one lifecycle race with an unambiguous authority rule: if one
/// node starts a queued Goal while another concurrently requests reassignment,
/// the start on the previously authoritative node wins.
fn resolve_queued_reassignment_start_race(
    base: &serde_json::Value,
    mut local: serde_json::Value,
    mut remote: serde_json::Value,
) -> Option<(serde_json::Value, serde_json::Value)> {
    let base_status = base.get("status")?.as_str()?;
    if !matches!(base_status, "backlog" | "todo") {
        return Some((local, remote));
    }
    let base_node = base
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("default");
    let is_start = |side: &serde_json::Value| {
        matches!(
            side.get("status").and_then(serde_json::Value::as_str),
            Some("in-progress" | "ready-merge" | "build" | "qa")
        ) && side
            .get("node_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default")
            == base_node
    };
    let is_reassignment = |side: &serde_json::Value| {
        side.get("status").and_then(serde_json::Value::as_str) == Some(base_status)
            && side
                .get("node_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("default")
                != base_node
    };
    if is_start(&local) && is_reassignment(&remote) {
        let remote = remote.as_object_mut()?;
        remote.insert("status".to_string(), local.get("status")?.clone());
        remote.insert("node_id".to_string(), serde_json::json!(base_node));
    } else if is_start(&remote) && is_reassignment(&local) {
        let local = local.as_object_mut()?;
        local.insert("status".to_string(), remote.get("status")?.clone());
        local.insert("node_id".to_string(), serde_json::json!(base_node));
    }
    Some((local, remote))
}

fn merge_json_value(
    base: &serde_json::Value,
    local: &serde_json::Value,
    remote: &serde_json::Value,
    field: Option<&str>,
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
    if field == Some("updated") {
        return later_timestamp(local, remote);
    }
    if field == Some("notes") {
        return merge_keyed_notes(base, local, remote);
    }
    let (base, local, remote) = (base.as_object()?, local.as_object()?, remote.as_object()?);
    let keys = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut merged = serde_json::Map::new();
    for key in keys {
        if let Some(value) =
            merge_json_member(base.get(&key), local.get(&key), remote.get(&key), &key)?
        {
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
    Some(Some(merge_json_value(base?, local?, remote?, Some(field))?))
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
        if let Some(note) =
            merge_json_member(base.get(&id), local.get(&id), remote.get(&id), "note")?
        {
            merged.push(note);
        }
    }
    Some(serde_json::Value::Array(merged))
}

fn validate_merged_goal(value: &serde_json::Value, expected_id: &str) -> Option<()> {
    let goal = serde_json::from_value::<Goal>(value.clone()).ok()?;
    if goal.id != expected_id
        || goal.id.trim().is_empty()
        || goal.name.trim().is_empty()
        || (goal.feature_order.is_some() && goal.feature_id.is_none())
        || goal.rounds.iter().any(|round| {
            round.reporter.trim().is_empty()
                || round.prompt.trim().is_empty()
                || round.implementation_report.is_some()
                    != round.implementation_reported_at.is_some()
        })
    {
        return None;
    }
    let unique_notes = goal
        .notes
        .iter()
        .map(|note| note.id.as_str())
        .collect::<BTreeSet<_>>();
    (unique_notes.len() == goal.notes.len()).then_some(())
}

fn later_timestamp(
    local: &serde_json::Value,
    remote: &serde_json::Value,
) -> Option<serde_json::Value> {
    let local_text = local.as_str()?;
    let remote_text = remote.as_str()?;
    let local_time = chrono::DateTime::parse_from_rfc3339(local_text).ok()?;
    let remote_time = chrono::DateTime::parse_from_rfc3339(remote_text).ok()?;
    Some(if local_time >= remote_time {
        local.clone()
    } else {
        remote.clone()
    })
}
