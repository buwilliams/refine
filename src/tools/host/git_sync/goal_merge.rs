use super::*;
use crate::model::goal::Goal;

/// Which side of a three-way Goal merge belongs to this node's live state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GoalMergeSide {
    Local,
    Remote,
}

/// How a Goal record reconciled.
pub(super) enum GoalRecordMerge {
    /// Every changed member was provably compatible.
    Merged(Vec<u8>),
    /// Contested members were settled by the record's baseline owner.
    OwnershipResolved { bytes: Vec<u8>, owner: String },
}

/// Merge one Goal record so that asynchronous nodes always converge.
///
/// Goal persistence rewrites the complete JSON document even for a single
/// mutation, and nodes mutate constantly — including while offline. Ordinary
/// object-member changes and keyed Note changes merge three-way; Rounds and
/// every other identity-free array stay atomic. When both sides changed the
/// same member and compatibility cannot be proven, the record's owner at the
/// baseline — the last state both sides agreed on, so neither contested edit
/// can vote for itself — is authoritative for the contested members, and the
/// other side's compatible edits still merge. A stale local understanding is
/// not a wrong one: staleness alone never discards work only the owning node
/// could have produced.
///
/// `None` remains possible only for records that cannot be arbitrated at all:
/// unparseable JSON, identity mismatches, or results that fail Goal schema
/// validation even from the owner's own side. Those fall through to the
/// fail-closed conflict report and its recovery path.
pub(super) fn merge_goal_record(
    base: &[u8],
    local: &[u8],
    remote: &[u8],
    local_node_id: &str,
) -> Option<GoalRecordMerge> {
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
    let owner = base
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("default")
        .to_string();
    let (local, remote) = resolve_queued_reassignment_start_race(&base, local, remote)?;

    if round_authority_is_uncontested(&base, &local, &remote)
        && let Some(merged) = merge_json_value(&base, &local, &remote, None, None)
        && validate_merged_goal(&merged, base_id).is_some()
    {
        return Some(GoalRecordMerge::Merged(encode_goal(&merged)?));
    }

    let owner_side = if owner == local_node_id {
        GoalMergeSide::Local
    } else {
        GoalMergeSide::Remote
    };
    let (mut local, mut remote) = (local, remote);
    couple_round_authority_to_owner(&base, owner_side, &mut local, &mut remote);
    if let Some(merged) = merge_json_value(&base, &local, &remote, None, Some(owner_side))
        && validate_merged_goal(&merged, base_id).is_some()
    {
        return Some(GoalRecordMerge::OwnershipResolved {
            bytes: encode_goal(&merged)?,
            owner,
        });
    }
    // Member mixing produced an invalid record (for example schema drift
    // between node versions): the owner's whole record is the last coherent
    // authority.
    let whole = match owner_side {
        GoalMergeSide::Local => &local,
        GoalMergeSide::Remote => &remote,
    };
    validate_merged_goal(whole, base_id)?;
    Some(GoalRecordMerge::OwnershipResolved {
        bytes: encode_goal(whole)?,
        owner,
    })
}

fn encode_goal(value: &serde_json::Value) -> Option<Vec<u8>> {
    let mut encoded = serde_json::to_vec_pretty(value).ok()?;
    encoded.push(b'\n');
    Some(encoded)
}

const ROUND_AUTHORITY_FIELDS: [&str; 4] = ["status", "node_id", "branch_name", "rounds"];

/// Rounds and workflow authority (status, assignment, branch) are one
/// semantic unit: Round evidence is only coherent under the authority that
/// produced it. A pure member merge is safe only while no side moves Rounds
/// against the other side's authority change.
fn round_authority_is_uncontested(
    base: &serde_json::Value,
    local: &serde_json::Value,
    remote: &serde_json::Value,
) -> bool {
    let local_rounds_changed = local.get("rounds") != base.get("rounds");
    let remote_rounds_changed = remote.get("rounds") != base.get("rounds");
    let local_authority_changed = ROUND_AUTHORITY_FIELDS
        .iter()
        .any(|field| local.get(*field) != base.get(*field));
    let remote_authority_changed = ROUND_AUTHORITY_FIELDS
        .iter()
        .any(|field| remote.get(*field) != base.get(*field));
    !(local_rounds_changed && remote_authority_changed)
        && !(remote_rounds_changed && local_authority_changed)
}

/// When Rounds race authority cross-node, the owner's coupled members win as
/// one unit: overwrite the losing side's authority set with the owner's so
/// the member merge cannot split Round evidence from the authority that
/// produced it. Non-authority members still merge normally.
fn couple_round_authority_to_owner(
    base: &serde_json::Value,
    owner_side: GoalMergeSide,
    local: &mut serde_json::Value,
    remote: &mut serde_json::Value,
) {
    if round_authority_is_uncontested(base, local, remote) {
        return;
    }
    let (winner, loser) = match owner_side {
        GoalMergeSide::Local => (local.clone(), remote),
        GoalMergeSide::Remote => (remote.clone(), local),
    };
    let Some(loser) = loser.as_object_mut() else {
        return;
    };
    for field in ROUND_AUTHORITY_FIELDS {
        match winner.get(field) {
            Some(value) => {
                loser.insert(field.to_string(), value.clone());
            }
            None => {
                loser.remove(field);
            }
        }
    }
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

fn arbitrated<'a>(
    arbiter: Option<GoalMergeSide>,
    local: &'a serde_json::Value,
    remote: &'a serde_json::Value,
) -> Option<serde_json::Value> {
    arbiter.map(|side| match side {
        GoalMergeSide::Local => local.clone(),
        GoalMergeSide::Remote => remote.clone(),
    })
}

fn merge_json_value(
    base: &serde_json::Value,
    local: &serde_json::Value,
    remote: &serde_json::Value,
    field: Option<&str>,
    arbiter: Option<GoalMergeSide>,
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
        return later_timestamp(local, remote).or_else(|| arbitrated(arbiter, local, remote));
    }
    if field == Some("notes") {
        return merge_keyed_notes(base, local, remote, arbiter)
            .or_else(|| arbitrated(arbiter, local, remote));
    }
    let (Some(base), Some(local_object), Some(remote_object)) =
        (base.as_object(), local.as_object(), remote.as_object())
    else {
        return arbitrated(arbiter, local, remote);
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
            arbiter,
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
    arbiter: Option<GoalMergeSide>,
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
            merge_json_value(base, local, remote, Some(field), arbiter).map(Some)
        }
        // A contested addition or removal has no three-way form to merge;
        // only an owner arbiter can settle it.
        (_, local, remote) => arbiter.map(|side| match side {
            GoalMergeSide::Local => local.cloned(),
            GoalMergeSide::Remote => remote.cloned(),
        }),
    }
}

fn merge_keyed_notes(
    base: &serde_json::Value,
    local: &serde_json::Value,
    remote: &serde_json::Value,
    arbiter: Option<GoalMergeSide>,
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
            arbiter,
        )? {
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
